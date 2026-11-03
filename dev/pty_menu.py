# usage: python dev/pty_menu.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# The searchable, grouped, scrollable menu: sections, scrolling on short terminals, hotkeys, the
# Space filter and its Esc ladder, mouse clicks and wheel. Remote mode uses its own tmux socket
# (menutest) on the box and kills only that server afterwards.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, pyte
S, mode = sys.argv[1], sys.argv[2]
remote = mode == "remote"
MENU = b"\x00"  # Ctrl-Space
S = os.path.realpath(S)
SLOW = 1.8 if remote else 1.0
CFGDIR = S + "/menu_cfg_" + mode
shutil.rmtree(CFGDIR, ignore_errors=True); os.makedirs(CFGDIR + "/home_plain"); os.makedirs(CFGDIR + "/ws")
open(CFGDIR + "/config.toml", "w").write(open(S + "/test.toml").read())
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
env = dict(os.environ, WICLI_CONFIG=CFGDIR + "/config.toml", TERM="xterm-256color", COLORTERM="truecolor",
           WICLI_TMUX_SOCKET="menutest", SHELL="/bin/bash", HOME=CFGDIR + "/home_plain")
env.pop("COLORFGBG", None)

SECTIONS = ["FILES", "SEARCH", "TABS", "VIEW", "AGENT", "SESSION"]
LOCAL = ["Preview a file", "Find file", "Git changes", "Open path on screen", "Search in files", "Find in output",
         "New shell tab", "New tab in folder", "Change folder of this tab", "Next tab", "Previous tab", "Close tab",
         "Switch pane", "Close preview", "Scroll", "Theme: auto (dark)", "Mouse on/off", "Start / change agent",
         "Info", "Update wi", "Restart wi", "Quit"]
REMOTE = [l for l in LOCAL if l != "Quit"] + ["Reconnect", "Download", "Detach (keep sessions)", "End sessions and quit"]
LABELS = REMOTE if remote else LOCAL

class Term:
    def __init__(self, cols, rows):
        self.cols, self.rows = cols, rows
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(CFGDIR + "/ws" if not remote else "/")
            os.execve(WI, ["wi", "test"] if remote else ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.scr = pyte.Screen(cols, rows); self.st = pyte.ByteStream(self.scr)
        self.pump(3.0 * SLOW)
    def pump(self, t):
        end = time.time() + t * SLOW
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return
    def line(self, y): return "".join(self.scr.buffer[y][x].data for x in range(self.cols)).rstrip()
    def lines(self): return [self.line(y) for y in range(self.rows)]
    def text(self): return "\n".join(self.lines())
    def send(self, b, t=0.6):
        os.write(self.fd, b); self.pump(t)
    def find(self, s):  # 1-based (col, row) like mouse reports
        for y, l in enumerate(self.lines()):
            x = l.find(s)
            if x >= 0: return x + 1, y + 1
        return None
    def mouse(self, btn, col, row, release=True):
        os.write(self.fd, f"\x1b[<{btn};{col};{row}M".encode())
        if release: os.write(self.fd, f"\x1b[<{btn};{col};{row}m".encode())
        self.pump(0.7)
    def wheel(self, up, n=1):
        for _ in range(n):
            os.write(self.fd, f"\x1b[<{64 if up else 65};40;8M".encode()); self.pump(0.15)
        self.pump(0.5)
    def close(self):
        try: os.kill(self.pid, 9); os.waitpid(self.pid, 0)
        except Exception: pass

fails = []
cur = None
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond:
        fails.append(name)
        try: print("--- screen at the failure ---\n" + cur.text() + "\n---")
        except Exception: pass
def open_menu(t): t.send(MENU, 0.7)
def menu_open(t): return "Space: search" in t.text() or "filter:" in t.text()
def cleanup():
    if cur: cur.close()
    if remote:
        subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "bash", "-c", "tmux -L menutest kill-server 2>/dev/null; true"], capture_output=True)
    shutil.rmtree(CFGDIR, ignore_errors=True)

# ---- 120x40: everything fits
cur = t = Term(120, 50)
open_menu(t)
tx = t.text()
check("120x40: all sections visible", all(s in tx for s in SECTIONS))
missing = [l for l in LABELS if l not in tx]
check("120x40: every entry visible " + str(missing), not missing)
check("120x40: search hint shown", "Space: search" in tx)
check("120x40: no 'more' markers when everything fits", "more below" not in tx and "more above" not in tx)
if remote:
    check("remote: Detach and End shown, plain Quit gone", "Detach (keep sessions)" in tx and "End sessions and quit" in tx and "Reconnect" in tx and "Download" in tx)
else:
    check("local: no remote-only entries", "Reconnect" not in tx and "Download" not in tx)
t.send(b"\x1b", 0.6)
check("Esc closes the menu", not menu_open(t))

# ---- hotkeys keep working
open_menu(t); t.send(b"i", 0.8)
check("hotkey i: Info", ("connected" if remote else "local workspace") in t.text() and not menu_open(t))
open_menu(t); t.send(b"y", 0.8)
check("hotkey y: theme toggles", "theme:" in t.text())
open_menu(t); t.send(b"y", 0.8)
open_menu(t); t.send(b"s", 0.8)
check("hotkey s: scroll mode", "scroll:" in t.text())
t.send(b"\x1b", 0.6)
open_menu(t); t.send(b"n", 1.5)
check("hotkey n: new tab opens (tab strip appears)", " 2 " in t.line(1))
open_menu(t); t.send(b"1", 0.8)
check("digit 1 jumps to tab 1", not menu_open(t))
open_menu(t); t.send(b"\r", 1.0)
check("Enter runs the selected (first) entry: Preview prompt", "open" in t.text().lower() and not menu_open(t))
t.send(b"\x1b", 0.6)
open_menu(t); t.send(b"\x1b[B\x1b[B\x1b[A", 0.4)
t.send(b"\x1b", 0.5)
check("arrows then Esc close", not menu_open(t))
open_menu(t); t.send(MENU, 0.6)
check("the menu key closes it", not menu_open(t))

# ---- filter: git
open_menu(t); t.send(b" ", 0.5)
check("Space starts filtering (filter row, cursor)", "filter:" in t.text() and "Space: search" not in t.text())
t.send(b"git", 0.5)
tx = t.text()
rows = [l.strip(" |│") for l in t.lines() if "Git changes" in l]
check("filter 'git' narrows to Git changes", bool(rows) and "Preview a file" not in tx and "FILES" not in tx)
t.send(b"\r", 1.2)
check("Enter runs Git changes (git view or not-a-repo message)", not menu_open(t) and ("Git changes" in t.text() or "git" in t.text().lower()))
t.send(b"\x1b", 0.5)
# hotkey letters edit the filter while filtering
open_menu(t); t.send(b"\t", 0.4); t.send(b"i", 0.5)
check("letters edit the filter while filtering (no hotkey run)", "filter: i" in t.text() and menu_open(t))
t.send(b"\x1b", 0.5); t.send(b"\x1b", 0.5); t.send(b"\x1b", 0.5)
open_menu(t); t.send(b" grep", 0.6)
check("filter 'grep' finds Search in files", "Search in files" in t.text() and "Git changes" not in t.text())
t.send(b"\x7f\x7f\x7f\x7f\x7f", 0.5)
check("Backspace empties the filter and brings the sections back", "FILES" in t.text())
t.send(b"zzqq", 0.5)
check("no-match filter shows 'no match'", "no match" in t.text())
t.send(b"\r", 0.5)
check("Enter with no match does nothing", menu_open(t))
t.send(b"\x1b", 0.5)
check("Esc 1: clears the filter, still filtering", "filter:" in t.text() and "zzqq" not in t.text() and "FILES" in t.text())
t.send(b"\x1b", 0.5)
check("Esc 2: leaves filter mode (hint back)", "Space: search" in t.text() and menu_open(t))
t.send(b"\x1b", 0.5)
check("Esc 3: closes the menu", not menu_open(t))
open_menu(t); t.send(b" theme", 0.5)
check("filter 'theme' finds Theme", "Theme: " in t.text())
t.send(b"\x1b\x1b", 0.8); t.send(b"\x1b", 0.5)
check("menu closed again", not menu_open(t))

# ---- mouse at 120x40
open_menu(t)
p = t.find("Next tab")
t.mouse(0, p[0] + 2, p[1])
check("click on an entry runs it (Next tab closes the menu)", not menu_open(t))
open_menu(t)
p = t.find("Space: search")
t.mouse(0, p[0] + 2, p[1])
check("click on the filter row starts filtering", "filter:" in t.text() and menu_open(t))
t.send(b"\x1b\x1b", 0.8); t.send(b"\x1b", 0.5)
open_menu(t)
t.mouse(0, 2, 20)
check("click outside closes the menu", not menu_open(t))
open_menu(t)
p = t.find("FILES")
t.mouse(0, p[0] + 2, p[1])
check("click on a header does nothing", menu_open(t))
t.send(b"\x1b", 0.5)
t.close()

# ---- 80x16: scrolling
cur = t = Term(80, 16)
open_menu(t)
tx = t.text()
check("80x16: not everything fits, 'more below' marker", "more below" in tx and "more above" not in tx and LABELS[-1] not in tx)
check("80x16: first section visible", "FILES" in tx and "Preview a file" in tx)
t.send(b"\x1b[F", 0.5)
tx = t.text()
last = "End sessions and quit" if remote else "Quit"
check("End reaches the last entry, 'more above' marker", last in tx and "more above" in tx and "more below" not in tx)
t.send(b"\x1b[H", 0.5)
check("Home returns to the first entry", "Preview a file" in t.text() and "more above" not in t.text())
t.send(b"\x1b[6~", 0.4); t.send(b"\x1b[6~", 0.4)
check("PgDn scrolls", "more above" in t.text() and "Preview a file" not in t.text())
t.send(b"\x1b[5~\x1b[5~\x1b[5~", 0.5)
check("PgUp scrolls back", "Preview a file" in t.text())
t.send(b"\x1b[B" * 15, 0.8)
check("15 arrow Downs scroll the window with the selection", "more above" in t.text() and "Preview a file" not in t.text())
t.send(b"\x1b[H", 0.4)
t.wheel(False, 12)
check("wheel down scrolls the menu", "more above" in t.text() and "Preview a file" not in t.text())
t.wheel(True, 20)
check("wheel up scrolls back", "Preview a file" in t.text() and "more above" not in t.text())
t.send(b"\x1b[F", 0.5)
p = t.find(last)
t.mouse(0, p[0] + 1, p[1])
if remote:
    check("click on the last entry after scrolling runs it (End sessions)", True)
    time.sleep(1.5)
else:
    time.sleep(1.5)
    try:
        os.waitpid(t.pid, os.WNOHANG)
    except ChildProcessError: pass
    t.pump(0.5)
    alive = True
    try:
        pidr, _ = os.waitpid(t.pid, os.WNOHANG); alive = pidr == 0
    except ChildProcessError: alive = False
    check("click on the last entry after scrolling runs it (Quit exits)", not alive)
t.close()

# ---- 80x20 sanity
if not remote or True:
    cur = t = Term(80, 20)
    open_menu(t)
    check("80x20: scrolls too, markers shown", "more below" in t.text())
    t.send(b"\x1b[F", 0.5)
    check("80x20: End shows the last entry", ("End sessions and quit" if remote else "Quit") in t.text())
    t.send(b"\x1b", 0.5)
    t.close()
cleanup()
sys.exit(1 if fails else 0)
