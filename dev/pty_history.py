# usage: python dev/pty_history.py <scratch-dir-with-test.toml> local|remote   (needs pyte)
# remote: no (the history logic is the same on both; remote opens go through the same preview path)
# Recent files: H in a preview, the "Recent files" menu item, persistence across a restart, the full
# path in the preview header, and a deleted file in the list.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, pyte
S, mode = sys.argv[1], sys.argv[2]
if mode == "remote":
    print("SKIP remote (local only)"); sys.exit(0)
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
W = S + "/hws"
CFG = S + "/hcfg"
shutil.rmtree(W, ignore_errors=True); shutil.rmtree(CFG, ignore_errors=True)
os.makedirs(W + "/hist"); os.makedirs(CFG)
for n in "abc":
    open(f"{W}/hist/{n}.txt", "w").write(f"CONTENT_OF_{n.upper()}\n")
open(CFG + "/config.toml", "w").write("")
env0 = {k: v for k, v in os.environ.items() if k != "COLORFGBG"}
env0.update(WICLI_CONFIG=CFG + "/config.toml", TERM="xterm-256color", WICLI_TMUX_SOCKET=os.environ.get("WICLI_TMUX_SOCKET", "histtest"))

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, cols=120, rows=30):
        self.rows_n = rows
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(W)
            os.execve(WI, ["wi"], env0)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.scr = pyte.Screen(cols, rows); self.st = pyte.ByteStream(self.scr)
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
    def rows(self): return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)) for y in range(self.scr.lines)]
    def text(self): return "\n".join(l.rstrip() for l in self.rows())
    def right(self): return "\n".join(l[61:].rstrip() for l in self.rows()[1:29])
    def status(self): return self.rows()[self.rows_n - 1]
    def send(self, b, t=0.6): os.write(self.fd, b); self.pump(t)
    def open_file(self, path):
        self.send(MENU + b"p", 0.5); self.send(path.encode() + b"\r", 1.2)
    def quit(self):
        try: os.kill(self.pid, 9); os.waitpid(self.pid, 0)
        except Exception: pass
    def order(self):
        """Paths listed in the recent-files screen, top to bottom."""
        return [m.group(1) for l in self.rows() if (m := re.match(r"\s*(hist/\w+\.txt|/\S+)\s", l))]

r = Run()
for n in "abc":
    r.open_file(f"hist/{n}.txt")
check("preview shows the last file", "CONTENT_OF_C" in r.right())
hdr = r.rows()[1][61:]
check("the full path is always on the header row", "hist/c.txt" in hdr and hdr.lstrip().startswith("/"))
r.send(b"h", 0.8)
t = r.text()
check("H opens the recent files list", "recent files" in t and "newest first" in r.status())
check("newest first", r.order()[:3] == ["hist/c.txt", "hist/b.txt", "hist/a.txt"])
check("current file marked", re.search(r"hist/c\.txt\s+shown now", t) is not None)
check("shows how long ago", "just now" in t)
r.send(b"\x1b[B\x1b[B", 0.4)   # down, down: a.txt
r.send(b"\r", 1.0)
check("Enter shows the picked file in the same pane", "CONTENT_OF_A" in r.right() and "CONTENT_OF_C" not in r.right())
check("header follows", "hist/a.txt" in r.rows()[1][61:])
r.send(b"h", 0.8)   # the preview kept the keyboard
check("picked file moves to the top", r.order()[:3] == ["hist/a.txt", "hist/c.txt", "hist/b.txt"])
r.send(b"b", 0.5)
check("typing narrows the list", r.order() == ["hist/b.txt"])
r.send(b"\x1b", 0.5)
check("Esc closes the list", "recent files" not in r.text())
r.quit()

# persisted across a restart; the menu item works from the shell tab
st = open(CFG + "/state.toml").read() if os.path.exists(CFG + "/state.toml") else ""
check("state.toml has the history", "hist/a.txt" in st and "hist/b.txt" in st and "hist/c.txt" in st)
r = Run()
check("fresh start shows the shell", "CONTENT_OF" not in r.text())
r.send(MENU, 0.5); r.send(b" ", 0.4); r.send(b"recent", 0.5)
check("menu search finds Recent files", "Recent files" in r.text())
r.send(b"\r", 1.0)
check("history survived the restart", r.order()[:3] == ["hist/a.txt", "hist/c.txt", "hist/b.txt"])
r.send(b"\x1b[B\r", 1.0)   # c.txt
check("menu item opens the file in the preview pane", "CONTENT_OF_C" in r.right())
r.send(b"\x1b", 0.5)

# a deleted file stays listed, dimmed, and says so
os.remove(W + "/hist/b.txt")
r.send(MENU, 0.5); r.send(b" ", 0.4); r.send(b"recent", 0.5); r.send(b"\r", 1.0)
check("missing file is marked", re.search(r"hist/b\.txt\s+not found", r.text()) is not None)
r.send(b"b", 0.4)
r.send(b"\r", 0.8)
check("choosing it says file not found", "file not found" in r.status() and "recent files" in r.text())
r.send(b"\x1b", 0.4)
r.quit()
shutil.rmtree(W, ignore_errors=True)
sys.exit(1 if fails else 0)
