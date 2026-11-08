# usage: python dev/pty_mermaid.py <scratch-dir> local   (needs pyte)
# Mermaid blocks in the markdown preview: a flowchart is drawn as a bordered box-drawing diagram titled
# "mermaid · flowchart", a broken block shows its source plus the dim "mermaid: not rendered" note, D toggles
# all diagrams to source and back (also after R), a narrow pane shows source, M source still works.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, signal, pyte
S, mode = os.path.realpath(sys.argv[1]), sys.argv[2]
remote = False
if mode != "local":
    print("local only"); sys.exit(0)
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
W = S + "/mmws"
clipfile = S + "/mm_clip.txt"
BOXCFG = open(S + "/test.toml").read()

SAMPLE = """# Plan

```mermaid
flowchart LR
  A[Build] --> B{Tests pass?}
  B -- yes --> C[Deploy]
  B -- no --> D[Notify]
```

Middle paragraph.

```mermaid
flowchart LR
  A[Start --> B
  ]]] nonsense
  B -->> C{
```

```mermaid
bogus thing
```

end of file
"""
shutil.rmtree(W, ignore_errors=True); os.makedirs(W + "/md")
open(W + "/md/diag.md", "w").write(SAMPLE)

def config(side):
    path = f"{S}/mm_{side}.toml"
    open(path, "w").write(BOXCFG + f'\n[defaults]\ntheme = "dark"\npreview_side = "{side}"\n')
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
        self.send(MENU + b"p", 0.5); self.send(b"md/diag.md\r", 1.5)
    def quit(self):
        self.send(MENU + (b"e" if remote else b"q"), 1.5)

def block(rows, start, end):
    """Rows from the first one matching `start` up to (not including) the first later one matching `end`."""
    i = next((k for k, l in enumerate(rows) if re.search(start, l)), None)
    if i is None: return []
    j = next((k for k in range(i + 1, len(rows)) if re.search(end, rows[k])), len(rows))
    return rows[i:j]


def rows_in(r): return r.rows()

r = Run(config("tab"), 100, 60)
r.open_md()
t = r.text()
check("flowchart title row", "╭─ mermaid · flowchart" in t)
check("box drawing characters of the diagram", all(ch in t for ch in "┌┐└┘│─▸") and "Build" in t and "Tests pass?" in t and "Deploy" in t)
check("diagram closes with a bottom border", "╰──" in t)
check("source of the flowchart is not shown while drawn", "flowchart LR" not in t.split("Middle paragraph")[0])
check("broken block falls back to source plus note", "]]] nonsense" in t and "mermaid: not rendered" in t)
check("unknown type note names it", "mermaid: not rendered (unknown diagram type 'bogus')" in t)
x, y = r.find(r"mermaid: not rendered")
check("note is dim", r.cell(x, y).fg == "8b93a1")
tx, ty = r.find(r"╭─ mermaid")
check("border is dim", r.cell(tx, ty).fg == "8b93a1")
check("status hint offers D", "D diagrams off" in r.status())
check("no row wider than the window", all(len(l) <= 100 for l in r.rows()))

r.send(b"d", 0.8)
t = r.text()
check("D shows every mermaid block as source", "flowchart LR" in t and "A[Build] --> B{Tests pass?}" in t and "╭─ mermaid" not in t)
check("no note in source view", "not rendered" not in t and "D diagrams on" in r.status())
r.send(b"r", 1.0)
check("source choice survives R", "flowchart LR" in r.text() and "╭─ mermaid" not in r.text())
r.send(b"D", 0.8)
check("D draws the diagrams again", "╭─ mermaid · flowchart" in r.text() and "A[Build]" not in r.text())
r.send(b"m", 0.8)
check("M still shows the raw file", re.search(r"^\s*\d+\s+# Plan", r.text(), re.M) is not None)
r.send(b"d", 0.5)
check("D does nothing in the raw source view", re.search(r"^\s*\d+\s+# Plan", r.text(), re.M) is not None)
r.send(b"m", 0.8)
check("M returns to the rendered view", "╭─ mermaid" in r.text())
r.resize(60, 60)
t = r.text()
check("narrower pane re-lays the diagram out (still drawn at 60)", "╭─ mermaid" in t and all(len(l) <= 60 for l in r.rows()))
r.resize(36, 60)
check("under 40 columns shows source and the note", "A[Build]" in r.text() and "not rendered" in r.text() and "40" in r.text())
r.resize(100, 60)
check("and back to the diagram", "╭─ mermaid · flowchart" in r.text())
r.send(b"\x1b", 0.6)
r.quit()
p, _ = os.waitpid(r.pid, os.WNOHANG)
check("quit exits", p != 0)
sys.exit(1 if fails else 0)
