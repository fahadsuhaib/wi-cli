# usage: python dev/pty_markdown.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# Rendered markdown preview through a real pty: theme colours, lists/quotes/code/tables laid out for the
# pane width, the M source toggle, re-layout on resize, find and mouse copy. Dark and light themes.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, signal, pyte
S, mode = sys.argv[1], sys.argv[2]
remote = mode == "remote"
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
W = S + "/mdws"
clipfile = S + "/md_clip.txt"
BOXCFG = open(S + "/test.toml").read()

SAMPLE = """---
title: Demo
tags: [a, b]
---

# Main Title

## Second Level

### Third Level

Some *emphasis*, **strong**, `inline code`, and a [link](https://example.com/page).

- first item
  - nested item
- [x] finished task
- [ ] open task
- A very long list item that keeps going and going so that it has to wrap onto several rows in a narrow pane, showing the hanging indent.

1. one
2. two

> A quote that is long enough to wrap around when the pane is narrow, with a bar on every row.

```rust
fn main() { println!("hi"); }
```

| Name | Qty |
|:-----|----:|
| apple | 5 |
| kiwi | 1234 |

---

![logo](logo.png)

<div>raw html</div>
"""

def sh_write():
    if remote:
        subprocess.run(["docker", "exec", "-i", "-u", "dev", "-w", "/home/dev", "devbox-test", "bash", "-c", "rm -rf md; mkdir -p md && cat > md/rich.md"], input=SAMPLE.encode(), check=True)
    else:
        os.makedirs(W + "/md", exist_ok=True)
        open(W + "/md/rich.md", "w").write(SAMPLE)
shutil.rmtree(W, ignore_errors=True); os.makedirs(W)
sh_write()

def config(theme, side):
    path = f"{S}/md_{theme}_{side}.toml"
    open(path, "w").write(BOXCFG + f'\n[defaults]\ntheme = "{theme}"\npreview_side = "{side}"\n')
    return path

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, cfg, cols, rows):
        self.cols, self.rows_n = cols, rows
        env = {k: v for k, v in os.environ.items() if k != "COLORFGBG"}
        env.update(WICLI_CONFIG=cfg, TERM="xterm-256color", COLORTERM="truecolor", WICLI_CLIPBOARD="file:" + clipfile)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            if not remote: os.chdir(W)
            os.execve(WI, ["wi", "test"] if remote else ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.scr = pyte.Screen(cols, rows); self.st = pyte.ByteStream(self.scr)
        self.pump(3.2 if remote else 2.2)
    def pump(self, t, until=None):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return False
            if until and until(): return True
        return until is None
    def resize(self, cols, rows):
        self.cols, self.rows_n = cols, rows
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        os.kill(self.pid, signal.SIGWINCH)
        self.scr.resize(rows, cols)
        self.pump(1.0)
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)) for y in range(self.scr.lines)]
    def text(self): return "\n".join(l.rstrip() for l in self.rows())
    def region(self, x0, x1):
        return [l[x0:x1].rstrip() for l in self.rows()[1:-1]]
    def status(self): return self.rows()[self.rows_n - 1]
    def send(self, b, t=0.6): os.write(self.fd, b); self.pump(t)
    def mouse(self, code, x, y, up=False): os.write(self.fd, f"\x1b[<{code};{x + 1};{y + 1}{'m' if up else 'M'}".encode())
    def drag(self, x1, y1, x2, y2, t=0.8):
        self.mouse(0, x1, y1); self.pump(0.05)
        steps = max(abs(x2 - x1), abs(y2 - y1), 1)
        for i in range(1, steps + 1):
            self.mouse(32, x1 + (x2 - x1) * i // steps, y1 + (y2 - y1) * i // steps)
        self.pump(0.05); self.mouse(0, x2, y2, up=True); self.pump(t)
    def find(self, pat, x0=0, x1=None):
        for y, l in enumerate(self.rows()):
            m = re.search(pat, l[x0:x1])
            if m: return m.start() + x0, y
        return None
    def cell(self, x, y): return self.scr.buffer[y][x]
    def open_md(self):
        self.send(MENU + b"p", 0.5); self.send(b"md/rich.md\r", 1.5)
    def quit(self):
        self.send(MENU + (b"e" if remote else b"q"), 1.5)

def block(rows, start, end):
    """Rows from the first one matching `start` up to (not including) the first later one matching `end`."""
    i = next((k for k, l in enumerate(rows) if re.search(start, l)), None)
    if i is None: return []
    j = next((k for k in range(i + 1, len(rows)) if re.search(end, rows[k])), len(rows))
    return rows[i:j]

# ---------------------------------------------------------------- full-tab preview, dark theme
r = Run(config("dark", "tab"), 100, 70)
r.open_md()
t = r.text()
rows = r.rows()
check("heading text is shown", "Main Title" in t and "Second Level" in t and "Third Level" in t)
check("no # heading markers", not re.search(r"^\s*#", t, re.M) and "# Main" not in t)
check("no [x] / [ ] task markup, boxes instead", "[x]" not in t and "[ ]" not in t and "☑ finished task" in t and "☐ open task" in t)
check("front matter is a metadata block, not a rule and a fake heading", "┌─ metadata" in t and not re.search(r"^\s*-{3}\s*$", t, re.M))
check("front matter lines sit inside the border", re.search(r"│ title: Demo\s+│", t) is not None)
check("H1 has a full rule under it", re.search(r"═{40,}", t) is not None)
x, y = r.find(r"Main Title")
c = r.cell(x, y)
check("H1 is bold in the theme accent colour", c.fg == "d69cf5" and c.bold)  # md_h1 of the dark theme
x2, y2 = r.find(r"Second Level")
check("H2 uses its own colour", r.cell(x2, y2).fg == "7dcfe0")
x3, y3 = r.find(r"Third Level")
check("H3 uses a third colour", r.cell(x3, y3).fg == "7aa2f7")
fx, fy = r.find(r"fn main")
check("code block sits on the code background panel", r.cell(fx, fy).bg == "2b303b" and r.cell(fx + 30, fy).bg == "2b303b" and r.cell(2, fy).bg == "2b303b")
check("language label above the code", re.search(r"^\s*rust\s*$", rows[fy - 1]) is not None)
ix, iy = r.find(r"inline code")
check("inline code on a chip", r.cell(ix, iy).bg == "2b303b")
lx, ly = r.find(r"link \(https://example.com/page\)")
check("link in the link colour, underlined, url shown dim", r.cell(lx, ly).fg == "6cb6ff" and r.cell(lx, ly).underscore
      and r.cell(lx + 8, ly).fg == "8b93a1")
check("image placeholder and html text", "[image: logo]" in t and "<div>raw html</div>" in t)
check("bullets and nesting", re.search(r"^ • first item", t, re.M) and re.search(r"^   ◦ nested item", t, re.M))
check("ordered list", re.search(r"^ 1\. one", t, re.M) and re.search(r"^ 2\. two", t, re.M))
check("table has box borders", all(ch in t for ch in "┌┬┐├┼┤└┴┘") and re.search(r"│ Name\s+│\s+Qty │", t) is not None)
m5 = re.search(r"│(\s+)5 │", t); m1 = re.search(r"│ ?1234 │", t)
check("table numbers are right aligned", m5 is not None and m1 is not None)
five = r.find(r"│\s+5 │"); k1234 = r.find(r"│ 1234 │")
check("5 and 1234 end in the same column", five and k1234 and rows[five[1]].index("5 │") + 1 == rows[k1234[1]].index("1234 │") + 4)
check("horizontal rule", re.search(r"^ ─{60,}\s*$", t, re.M) is not None)
check("status hint offers M source", "M source" in r.status() and "W wrap" not in r.status())

# source toggle
r.send(b"m", 0.8)
t = r.text()
check("M switches to the source: line numbers and raw markers", re.search(r"^\s*\d+\s+# Main Title", t, re.M) is not None and "[x] finished task" in t and "M preview" in r.status())
r.send(b"r", 1.0)
check("the source view survives R (reload)", "# Main Title" in r.text() and "M preview" in r.status())
r.send(b"M", 0.8)
t = r.text()
check("M switches back to the rendered view", "# Main Title" not in t and "Main Title" in t and "M source" in r.status())
r.send(b"r", 1.0)
check("the rendered view survives R", "# Main Title" not in r.text() and "☑ finished task" in r.text())

# find in the rendered view
r.send(b"f", 0.5); r.send(b"hanging\r", 0.8)
fx, fy = r.find(r"hanging")
check("F finds text in the rendered view and lights it", fx is not None and r.cell(fx, fy).bg in ("8fdc7f", "f2cc60") and "match 1 of 1" in r.status())

# mouse selection copies plain text
if os.path.exists(clipfile): os.remove(clipfile)
sx, sy = r.find(r"Some emphasis")
ex = r.find(r"inline code,")[0] + len("inline code,") - 1
r.drag(sx, sy, ex, sy)
clip = open(clipfile).read() if os.path.exists(clipfile) else ""
check("selecting rendered text copies plain text, not markup", clip.strip() == "Some emphasis, strong, inline code," and "*" not in clip and "`" not in clip)

# resize re-wraps
def long_rows(r):
    return len(block(r.region(0, r.cols), r"A very long|very long list", r"^\s*$"))
before = long_rows(r)
before_wide = before
r.resize(50, 70)
after = len(block(r.region(0, 50), r"A very long|very long list", r"^\s*$"))
check(f"resizing the terminal re-wraps the document ({before_wide} -> {after} rows for the long item)", after > before_wide)
rows50 = block(r.region(0, 50), r"A very long|very long list", r"^\s*$")
check("hanging indent after the resize", len(rows50) > 2 and all(l.startswith("   ") and not l.startswith("    ") for l in rows50[1:]))
check("no rendered row is wider than the window", all(len(l) <= 50 for l in r.rows()))
r.resize(100, 70)
check("and back again", long_rows(r) == before_wide)
r.send(b"\x1b", 0.6)
r.quit()
p, _ = os.waitpid(r.pid, os.WNOHANG)
check("quit exits (tab run)", p != 0)

# ---------------------------------------------------------------- side pane at 120 columns (59-wide pane)
r = Run(config("dark", "right"), 120, 70)
r.open_md()
side = r.region(61, 120)
st = "\n".join(side)
check("rendered markdown in the side pane", "Main Title" in st and "☑ finished task" in st and "# Main" not in st)
check("side pane is 59 wide and nothing spills", all(len(l) <= 59 for l in side))
item = block(side, r"A very long", r"^\s*$")
check(f"long list item wraps with a hanging indent ({len(item)} rows)", len(item) >= 3 and item[0].startswith(" • A very long") and all(l.startswith("   ") and not l.lstrip().startswith("•") and not l.startswith("    ") for l in item[1:]))
quote = block(side, r"▌ A quote", r"^\s*$")
check(f"quote rows each start with the bar ({len(quote)} rows)", len(quote) >= 2 and all(l.startswith(" ▌ ") for l in quote))
tab = block(side, r"┌─+┬", r"└")
check("table fits the pane", len(tab) > 3 and all(len(l) <= 59 for l in tab))
r.send(b"m", 0.8)
check("M in the side pane shows the source", re.search(r"\d+\s+# Main Title", "\n".join(r.region(61, 120))) is not None)
r.send(b"m", 0.5)
r.send(b"\x1b", 0.4)
r.quit()
p, _ = os.waitpid(r.pid, os.WNOHANG)
check("quit exits (side run)", p != 0)

# ---------------------------------------------------------------- light theme
r = Run(config("light", "tab"), 100, 70)
r.open_md()
t = r.text()
check("light theme renders the same text", "Main Title" in t and "☑ finished task" in t and "# Main" not in t)
x, y = r.find(r"Main Title")
check("light H1 colour", r.cell(x, y).fg == "7a3fd0")
fx, fy = r.find(r"fn main")
page_bg = r.cell(99, y).bg
check("light code panel background differs from the page", r.cell(fx, fy).bg == "eceff3" and r.cell(fx, fy).bg != page_bg)
check("light code text is readable (dark on light)", r.cell(fx, fy).fg == "57606a")
r.send(b"\x1b", 0.4)
r.quit()
p, _ = os.waitpid(r.pid, os.WNOHANG)
check("quit exits (light run)", p != 0)
sys.exit(1 if fails else 0)
