# usage: python dev/pty_links.py <scratch-dir> local   (needs pyte)
# Clickable paths and links. WICLI_OPEN_LOG makes wi record "open <path>" / "url <url>" instead of
# launching apps; WICLI_CACHE_DIR keeps downloads inside the scratch dir.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, re, shutil, pyte
S, mode = sys.argv[1], sys.argv[2]
MENU = b"\x00"
openlog = S + "/open.log"; cache = S + "/cache"
# These tests are about preview tabs and links: keep previews as separate tabs (preview_side = "tab");
# dev/pty_split.py covers the side-by-side pane.
cfg = S + "/links_tab.toml"
open(cfg, "w").write('[defaults]\npreview_side = "tab"\n')
env = {k: v for k, v in os.environ.items() if k != "COLORFGBG"}
env.update(WICLI_CONFIG=cfg, TERM="xterm-256color", COLORTERM="truecolor", WICLI_OPEN_LOG=openlog, WICLI_CACHE_DIR=cache)

def build_fixtures():
    script = r'''
set -e
mkdir -p src sub deep/er/nest x/dup y/dup
printf 'LEAF_BODY\n' > deep/er/nest/leaf_unique.md
printf 'X_BODY\n' > x/dup/m_dup.txt
printf 'Y_BODY\n' > y/dup/m_dup.txt
for i in $(seq 1 20); do echo "fn line_$i() {}"; done > src/main.rs
printf 'one\ntwo\nthree_inner\nfour\n' > sub/inner.rs
printf '\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01' > pic.png
(printf '%%PDF-1.4\n\0\0\0\0'; head -c 2000000 /dev/urandom) > doc.pdf
printf 'MZ\0\0\0\0binary' > blob.bin
printf '\0\0\0\0app' > tool.app
'''
    shutil.rmtree(S + "/lws", ignore_errors=True); os.makedirs(S + "/lws")
    subprocess.run(["bash", "-c", script], check=True, cwd=S + "/lws")
build_fixtures()
for p in (openlog,):
    if os.path.exists(p): os.remove(p)
shutil.rmtree(cache, ignore_errors=True)

class Run:
    def __init__(self, config=cfg):
        e = dict(env, WICLI_CONFIG=config)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(S + "/lws")
            os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), ["wi"], e)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
        self.scr = pyte.Screen(110, 30); self.st = pyte.ByteStream(self.scr)
        self.pump(2.2)
    def pump(self, t, until=None):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return False
            if until and until(): return True
        return until is None
    def rows(self):
        # pyte's own .display fails on empty cells next to wide characters
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)) for y in range(self.scr.lines)]
    def text(self): return "\n".join(l.rstrip() for l in self.rows())
    def last(self, s):
        hits = [(l.find(s), y) for y, l in enumerate(self.rows()) if s in l]
        return (hits[-1][0], hits[-1][1]) if hits else None
    def send(self, b, t=0.6): os.write(self.fd, b); self.pump(t)
    def click(self, x, y, code=0, t=0.9):   # 0-based cell; code carries button + modifiers (ctrl=16, alt=8)
        os.write(self.fd, f"\x1b[<{code};{x + 1};{y + 1}M\x1b[<{code & ~32};{x + 1};{y + 1}m".encode()); self.pump(t)
    def quit(self):
        self.send(MENU + b"q", 1.5)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
def log(): return open(openlog).read() if os.path.exists(openlog) else ""
def strip(r): return r.rows()[1]

r = Run()
hello = "src/main.rs:5:3"

# ---- text files open in a preview, at the right line
r.send(f"echo see {hello} now\n".encode(), 0.8)
x, y = r.last(hello)
r.click(x + 3, y, code=0)   # plain click: selects nothing, opens nothing
check("a plain click does not open anything", "line_5()" not in r.text() and "preview" not in strip(r))
r.click(x + 3, y, code=16)
check("Ctrl+click opens the file in a preview tab", "main.rs" in strip(r) and "line_5" in r.text())
# the target line is marked (background colour on that row)
ry = [i for i, l in enumerate(r.rows()) if "line_5()" in l][0]
check("the target line is highlighted", r.scr.buffer[ry][12].bg != "default")
check("lines above it are visible for context", "line_1()" in r.text() or "line_3()" in r.text())
r.send(b"\x1b", 0.5)

# ---- Alt+click and right-click also work
r.send(b"echo go sub/inner.rs:3\n", 0.8)
x, y = r.last("sub/inner.rs:3")
r.click(x + 2, y, code=8)
check("Alt+click opens too", "inner.rs" in strip(r) and "three_inner" in r.text())
r.send(b"\x1b", 0.5)
r.click(x + 2, y, code=2)
check("right-click opens too", "three_inner" in r.text())
r.send(b"\x1b", 0.5)

# ---- relative paths without OSC 7: the OS cwd, then one bounded search, then a picker
r.send(b"clear; cd sub; echo go inner.rs:3\n", 0.8)
x, y = r.last("inner.rs:3")
r.click(x + 2, y, code=2)
check("after cd (no OSC 7) a relative path resolves against the shell's real folder", "three_inner" in r.text())
r.send(b"\x1b", 0.5)
r.send(b"clear; echo see nest/leaf_unique.md\n", 0.8)
x, y = r.last("nest/leaf_unique.md")
r.click(x + 2, y, code=2)
check("a path found nowhere near the shell is searched for: a unique match opens", "LEAF_BODY" in r.text())
r.send(b"\x1b", 0.5)
r.send(b"clear; echo see dup/m_dup.txt\n", 0.8)
x, y = r.last("dup/m_dup.txt")
r.click(x + 2, y, code=2)
check("several matches raise a picker and open nothing", "Several places match" in r.text() and "x/dup/m_dup.txt" in r.text() and "y/dup/m_dup.txt" in r.text() and "X_BODY" not in r.text())
r.send(b"\x1b", 0.5)
check("Esc cancels the picker", "Several places match" not in r.text() and "X_BODY" not in r.text())
r.click(x + 2, y, code=2)
r.send(b"\x1b[B", 0.3); r.send(b"\r", 0.9)
check("arrows and Enter choose the second match", "Y_BODY" in r.text() and "X_BODY" not in r.text())
r.send(b"\x1b", 0.5)
r.send(b"cd ..\n", 0.4)

# ---- the "Preview a file" prompt is a real line editor: paste, move, edit mid-text
PS, PE = b"\x1b[200~", b"\x1b[201~"
r.send(MENU + b"p", 0.6)
r.send(PS + b"sub/inne.rs" + b"\n" + PE, 0.6)
check("pasted text shows in the prompt (newline stripped)", "inne.rs" in r.text() and "open file:" in r.text())
x_end = r.scr.cursor.x
r.send(b"\x1b[D\x1b[D\x1b[D", 0.5)
check("Left moves the real cursor", r.scr.cursor.x == x_end - 3)
r.send(b"r", 0.4)
check("typing inserts at the cursor", "sub/inner.rs" in r.text() and r.scr.cursor.x == x_end - 2)
r.send(b"\x1b[3~", 0.4)
check("Delete removes the character under the cursor", "sub/innerrs" in r.text())
r.send(b".", 0.3)
r.send(b"\x1b[F", 0.3)
check("End goes to the end", r.scr.cursor.x == x_end + 1)
r.send(b"\x1b[H", 0.3)
check("Home goes to the start of the text", r.scr.cursor.x == x_end + 1 - len("sub/inner.rs"))
r.send(b"\r", 1.0)
check("Enter opens the edited path", "three_inner" in r.text())
r.send(b"\x1b", 0.5)

# ---- relative paths resolve against the shell's own directory
r.send(b"cd sub; echo rel inner.rs:2\n", 0.9)
x, y = r.last("inner.rs:2")
r.click(x + 2, y, code=16)
check("a relative path resolves against the tab's working directory", "three_inner" in r.text())
r.send(b"\x1b", 0.5)
r.send(b"cd ..; clear\n", 0.6)

# ---- missing file
r.send(b"echo ghost nothere/missing.rs\n", 0.8)
x, y = r.last("nothere/missing.rs")
r.click(x + 3, y, code=16)
check("a missing file says so", "no such file" in r.text())

# ---- URLs: confirm first
r.send(b"echo link https://example.com/x?y=1 end\n", 0.8)
x, y = r.last("https://example.com")
r.click(x + 4, y, code=16)
check("a URL asks before opening, showing the whole address", "Open link in your browser" in r.text() and "https://example.com/x?y=1" in r.text())
r.send(b"n", 0.5)
check("N cancels and nothing is opened", log() == "" and "cancelled" in r.text())
r.click(x + 4, y, code=16)
r.send(b"y", 0.6)
check("Y opens it through the OS", "url https://example.com/x?y=1" in log())

# ---- native files: confirm, never programs
pdf = "doc.pdf"
r.send(f"echo file {pdf} end\n".encode(), 0.8)
x, y = r.last(pdf)
r.click(x + 2, y, code=16)
check("a PDF asks before opening, showing path and size", "Open in the default app" in r.text() and "doc.pdf" in r.text() and "Size:" in r.text())
r.send(b"\x1b", 0.5)
check("Esc cancels: nothing opened, nothing downloaded", "open " not in log() and not os.path.exists(cache))
r.click(x + 2, y, code=16)
r.send(b"\r", 1.0)
check("Enter hands it to the OS", f"open {S}/lws/doc.pdf" in log().replace(os.path.realpath(S), S) or "doc.pdf" in log())

# programs and unknown binaries are refused
blob = "./blob.bin"
r.send(f"echo x {blob} y\n".encode(), 0.8)
x, y = r.last(blob)
n_before = len(log().splitlines())
r.click(x + 4, y, code=16)
check("an unknown binary is refused, no dialog", "never opened" in r.text() or "not opened" in r.text())
check("and nothing was opened", len(log().splitlines()) == n_before)
r.send(b"echo x ./tool.app y\n", 0.8)
x, y = r.last("./tool.app")
r.click(x + 4, y, code=16)
check("an app bundle is refused", "never opened" in r.text())

# ---- keyboard route: labels on every path and link
r.send(b"clear\n", 0.5)
r.send(f"echo A {hello} B https://x.dev/y C {pdf}\n".encode(), 0.8)
r.send(MENU + b"o", 0.8)
t = r.text()
check("the menu entry labels the paths and links on screen", "labels" not in t and "type a label" in t)
lab = [r.scr.buffer[y][x].data for y in range(2, 29) for x in range(0, 110) if r.scr.buffer[y][x].bg == "f2cc60"]  # hint_bg of the dark theme
check("labels are drawn over the links (home-row letters)", len(lab) >= 3 and set(lab) <= set("asdfghjkl "))
r.send(b"a", 1.0)
check("pressing a label opens that link", "main.rs" in strip(r) and "line_5" in r.text())
r.send(b"\x1b", 0.5)
r.send(MENU + b"o", 0.6)
r.send(b"\x1b", 0.5)
check("Esc leaves label mode", "type a label" not in r.text())
r.quit()

# ---- confirm_open = never skips the question (for people who want that)
nc = S + "/noconfirm.toml"
open(nc, "w").write('[defaults]\nconfirm_open = "never"\npreview_side = "tab"\n')
os.remove(openlog)
r = Run(nc)
r.send(b"echo q doc.pdf end\n", 0.8)
x, y = r.last("doc.pdf")
r.click(x + 2, y, code=16)
check("confirm_open = never opens straight away", "Open in the default app" not in r.text() and "doc.pdf" in log())
r.quit()
sys.exit(1 if fails else 0)
