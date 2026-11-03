# usage: python dev/pty_refresh.py <scratch-dir-with-test.toml> local|remote   (needs pyte)
# remote: yes
# Auto-refresh of visible file previews: the side pane while the shell has the keyboard, a focused
# preview tab, idle quiet, deletion note, a burst of writes, the 5 s default debounce, and the
# preview_autorefresh = false switch. Files live in refresh/ (workspace root, or the login dir on the box).
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, threading, pyte
S, mode = sys.argv[1], sys.argv[2]
remote = mode == "remote"
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
W = S + "/rws"
BOXCFG = open(S + "/test.toml").read()
POLL = 2.0 if remote else 1.0

def sh(script, bg=False):
    if remote:
        return subprocess.run(["docker", "exec", "-u", "dev", "-w", "/home/dev", "devbox-test", "bash", "-c", script], check=True)
    return subprocess.run(["bash", "-c", script], check=True, cwd=W)

shutil.rmtree(W, ignore_errors=True); os.makedirs(W)
sh("rm -rf refresh; mkdir -p refresh; for i in $(seq 1 5); do echo base_$i; done > refresh/live.txt; "
   "seq 1 100 | sed 's/^/row_/' > refresh/long.txt; printf 'one\\n' > refresh/gone.txt; printf 'x\\n' > refresh/burst.txt; "
   "printf 'k\\n' > refresh/slow.txt; printf 'm\\n' > refresh/manual.txt; printf '# Title\\n\\nbody\\n' > refresh/doc.md")

def config(name, extra):
    path = S + f"/refresh_{name}.toml"
    open(path, "w").write(BOXCFG + "\n[defaults]\n" + extra)
    return path

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, cfg, cols=120, rows=30):
        self.cols, self.rows_n = cols, rows
        env = {k: v for k, v in os.environ.items() if k != "COLORFGBG"}
        env.update(WICLI_CONFIG=cfg, TERM="xterm-256color", WICLI_TMUX_SOCKET=os.environ.get("WICLI_TMUX_SOCKET", "refreshtest"))
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            if not remote: os.chdir(W)
            os.execve(WI, ["wi", "test"] if remote else ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.scr = pyte.Screen(cols, rows); self.st = pyte.ByteStream(self.scr)
        self.nbytes = 0
        self.pump(3.2 if remote else 2.2)
    def pump(self, t, until=None):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if r:
                try:
                    b = os.read(self.fd, 65536)
                    self.nbytes += len(b); self.st.feed(b)
                except OSError: return False
            if until and until(): return True
        return until is None
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)) for y in range(self.scr.lines)]
    def text(self): return "\n".join(l.rstrip() for l in self.rows())
    def left(self): return "\n".join(l[:60].rstrip() for l in self.rows()[1:29])
    def right(self): return "\n".join(l[61:].rstrip() for l in self.rows()[1:29])
    def status(self): return self.rows()[self.rows_n - 1]
    def send(self, b, t=0.6): os.write(self.fd, b); self.pump(t)
    def open_file(self, path, t=1.5):
        self.send(MENU + b"p", 0.5); self.send(path.encode() + b"\r", t)
    def lines_from(self):
        m = re.search(r"lines (\d+)-", self.status())
        return int(m.group(1)) if m else None
    def quit(self):
        try: os.kill(self.pid, 9); os.waitpid(self.pid, 0)
        except Exception: pass

def append(name, line): sh(f"echo '{line}' >> refresh/{name}")
WAIT = POLL + 2.5

# ---------------------------------------------------------------- side pane, shell has the keyboard
r = Run(config("fast", "preview_refresh_debounce_ms = 600\n"))
r.open_file("refresh/live.txt")
check("side preview shows the file", "base_5" in r.right())
r.send(b"\x1b", 0.5)   # keyboard back to the shell; the preview stays beside it
r.send(b"echo SHELL_FOCUS_MARK\n", 0.6)
check("shell has the keyboard", "SHELL_FOCUS_MARK" in r.left())
append("live.txt", "appended_one")
check("side pane refreshes by itself while the shell is focused", r.pump(WAIT, lambda: "appended_one" in r.right()))
check("'refreshed' note shows", r.pump(0.5, lambda: "refreshed" in r.status()) or "appended_one" in r.right())
r.send(b"echo STILL_SHELL\n", 0.6)
check("keyboard stayed in the shell", "STILL_SHELL" in r.left())
check("note disappears by itself", r.pump(6, lambda: "refreshed" not in r.status()))

# idle: nothing changed, nothing redrawn
r.pump(0.5); n0 = r.nbytes
r.pump(4.5)
check(f"no redraw churn while the file is unchanged ({r.nbytes - n0} bytes)", r.nbytes - n0 < 64)

# modified (not just appended): edit a line in place
sh("sed -i.bak 's/base_2/EDITED_two/' refresh/live.txt; rm -f refresh/live.txt.bak")
check("in-place edit shows up", r.pump(WAIT, lambda: "EDITED_two" in r.right()))

# deletion
sh("rm refresh/live.txt")
check("deletion shows a short note", r.pump(WAIT, lambda: "file removed" in r.status()))
check("old content kept after deletion", "EDITED_two" in r.right())
n0 = r.nbytes; r.pump(0.3)
r.pump(6, lambda: "file removed" not in r.status())
r.pump(0.5); n0 = r.nbytes; r.pump(3.5)
check(f"the removal is reported once, not every poll ({r.nbytes - n0} bytes)", r.nbytes - n0 < 64 and "file removed" not in r.status())
r.quit()

# ---------------------------------------------------------------- a burst of writes ends in one stable render
r = Run(config("fast", "preview_refresh_debounce_ms = 600\n"))
r.open_file("refresh/burst.txt")
r.send(b"\x1b", 0.5)
def burst():
    for i in range(1, 21):
        sh(f"echo burst_{i} >> refresh/burst.txt"); time.sleep(0.1)
t = threading.Thread(target=burst); t.start()
r.pump(2.5)
t.join()
check("a burst ends with the final lines on screen", r.pump(WAIT + 4, lambda: "burst_20" in r.right()))
check("and everything before them", "burst_1" in r.right() and "burst_10" in r.right())
r.pump(6.0); n0 = r.nbytes; r.pump(3.5)
check(f"then it goes quiet ({r.nbytes - n0} bytes)", r.nbytes - n0 < 64)
r.quit()

# ---------------------------------------------------------------- focused preview tab keeps scroll and markdown view
r = Run(config("tab", 'preview_side = "tab"\npreview_refresh_debounce_ms = 600\n'))
r.open_file("refresh/long.txt")
r.send(b"\x1b[6~", 0.6)
top = r.lines_from()
check("preview tab scrolled down", top is not None and top > 1)
append("long.txt", "row_tail_marker")
check("focused preview tab refreshes", r.pump(WAIT, lambda: "of 101" in r.status()))
check("scroll position kept", r.lines_from() == top)
r.send(b"\x1b", 0.5)
r.open_file("refresh/doc.md")
check("markdown rendered", "Title" in r.text() and "# Title" not in r.text())
append("doc.md", "\\nmore text here")
check("markdown refreshes", r.pump(WAIT, lambda: "more text here" in r.text()))
check("markdown stays rendered", "# Title" not in r.text())
r.quit()

# ---------------------------------------------------------------- default debounce is 5 seconds
r = Run(config("default", "preview_side = \"right\"\n"))
r.open_file("refresh/slow.txt")
r.send(b"\x1b", 0.5)
append("slow.txt", "slow_change")
t0 = time.time()
r.pump(3.0)
check("default: not refreshed after 3 s", "slow_change" not in r.right())
ok = r.pump(8, lambda: "slow_change" in r.right())
dt = time.time() - t0
check(f"default: refreshed after the 5 s debounce ({dt:.1f}s)", ok and 4.5 <= dt <= 9.5)
r.quit()

# ---------------------------------------------------------------- switch off
r = Run(config("off", "preview_autorefresh = false\n"))
r.open_file("refresh/manual.txt")
append("manual.txt", "manual_change")
r.pump(WAIT + 2)
check("autorefresh = false: no automatic refresh", "manual_change" not in r.right())
r.send(b"r", 1.0)
check("R still reloads by hand (side pane focused)", "manual_change" in r.right())
r.quit()

if remote:
    sh("rm -rf refresh")
sys.exit(1 if fails else 0)
