# usage: python dev/pty_panes.py <scratch-dir> local   (needs pyte)
# remote: no
# Pinned panes: pin the active tab right/left from the menu, swap with the title-bar button, unsplit,
# keep output flowing in the pinned pane, mouse (focus, wheel, selection), remembered layout.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, pyte
S, mode = sys.argv[1], sys.argv[2]
remote = False
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
W = S + "/pnw"
clipfile = S + "/panes_clip.txt"
shutil.rmtree(W, ignore_errors=True); os.makedirs(W)
cfg = S + "/panes.toml"
open(cfg, "w").write("")
state = S + "/state.toml"
if os.path.exists(state): os.remove(state)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, cfg, cols=120, rows=30):
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
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)) for y in range(self.scr.lines)]
    def text(self): return "\n".join(l.rstrip() for l in self.rows())
    def region(self, x0, x1, y0=1, y1=29):   # text of columns [x0, x1) of rows [y0, y1)
        return "\n".join(l[x0:x1].rstrip() for l in self.rows()[y0:y1])
    def left(self): return self.region(0, 60)
    def right(self): return self.region(61, 120)
    def status(self): return self.rows()[self.rows_n - 1]
    def send(self, b, t=0.6): os.write(self.fd, b); self.pump(t)
    def mouse(self, code, x, y, up=False): os.write(self.fd, f"\x1b[<{code};{x + 1};{y + 1}{'m' if up else 'M'}".encode())
    def click(self, x, y, code=0, t=0.8):
        self.mouse(code, x, y); self.mouse(code & ~32, x, y, up=True); self.pump(t)
    def drag(self, x1, y1, x2, y2, t=0.6):
        self.mouse(0, x1, y1); self.pump(0.05)
        steps = max(abs(x2 - x1), abs(y2 - y1), 1)
        for i in range(1, steps + 1):
            self.mouse(32, x1 + (x2 - x1) * i // steps, y1 + (y2 - y1) * i // steps)
        self.pump(0.05); self.mouse(0, x2, y2, up=True); self.pump(t)
    def wheel(self, up, x, y, n=1):
        for _ in range(n): self.mouse(64 if up else 65, x, y)
        self.pump(0.6)
    def find(self, pat, x0=0, x1=None):
        for y, l in enumerate(self.rows()):
            m = re.search(pat, l[x0:x1])
            if m: return m.start() + x0, y
        return None
    def cols_now(self):
        """Run tput cols in the shell and read the answer."""
        tag = f"C{int(time.time() * 1000) % 100000}"
        self.send(f"echo {tag}=$(tput cols)\n".encode(), 0.8)
        hits = re.findall(tag + r"=(\d+)", self.text())
        return int(hits[-1]) if hits else None
    def lines_from(self):
        m = re.search(r"lines (\d+)-", self.status())
        return int(m.group(1)) if m else None
    def open_file(self, path, t=1.2):
        self.send(MENU + b"p", 0.5); self.send(path.encode() + b"\r", t)
    def quit(self):
        self.send(MENU + (b"e" if remote else b"q"), 1.5)


def clip():
    return open(clipfile).read() if os.path.exists(clipfile) else None
def noclip():
    if os.path.exists(clipfile): os.remove(clipfile)

def run_menu(r, text, t=1.0):
    r.send(MENU + b" ", 0.4); r.send(text.encode(), 0.4); r.send(b"\r", t)

def ticks(r, x0, x1):
    nums = [int(n) for n in re.findall(r"T(\d+)", r.region(x0, x1))]
    return max(nums) if nums else 0

r = Run(S + "/panes.toml")
check("one tab: no panes yet, full width", r.cols_now() == 120)
r.send(b"echo PIN_MARK; i=0; while sleep 0.4; do i=$((i+1)); echo -n \" T$i\"; done &\n", 1.0)
run_menu(r, "pin right", 1.5)
check("Pin right with a single tab opens a second shell and pins the first on the right", "PIN_MARK" in r.right() and "PIN_MARK" not in r.left())
check("the tab strip appears with two tabs", "1 " in r.rows()[1] and "2 " in r.rows()[1])
check("the divider separates the panes", all(r.rows()[y][60] in "│▶◀" for y in range(2, 28)))
check("the pinned pane shows swap and close buttons", "⇄" in r.rows()[2][100:120] and "✕" in r.rows()[2][100:120])
check("the active (left) shell is 60 columns wide", r.cols_now() == 60)
t1 = ticks(r, 61, 120); r.pump(2.0); t2 = ticks(r, 61, 120)
check("the pinned tab keeps running and updating while it is not focused", t2 > t1 >= 0 and t2 > 0)
r.send(b"echo LEFT_ONE\n", 0.6)
check("typing goes to the focused (left) pane only", "LEFT_ONE" in r.left() and "LEFT_ONE" not in r.right())

# focus the pinned pane by click; type into it
r.click(90, 12)
r.send(b"echo RIGHT_TYPED\n", 0.8)
check("clicking the pinned pane gives it the keyboard", "RIGHT_TYPED" in r.right() and "RIGHT_TYPED" not in r.left())
r.send(b"kill %1\n", 0.6)   # stop the ticker so it cannot garble later output
w = r.cols_now()
check("the pinned shell is sized to its pane (59 columns)", w == 59)
r.click(10, 12)
r.send(b"echo LEFT_TWO\n", 0.8)
check("clicking the left pane moves the keyboard back", "LEFT_TWO" in r.left() and "LEFT_TWO" not in r.right())
r.send(MENU + b"w", 0.6)
r.send(b"echo VIA_W\n", 0.8)
check("menu W (switch pane) moves the keyboard to the pinned pane", "VIA_W" in r.right() and "VIA_W" not in r.left())
r.send(MENU + b"w", 0.6)

# wheel and selection in the pinned pane
r.click(90, 12)
r.send(b"clear; seq 1 300\n", 1.0)
r.wheel(True, 90, 12, 2)
check("wheel over the pinned pane scrolls it", "scrollback" in r.text())
r.send(b"\x1b", 0.5)
r.send(b"clear; echo SELMARK_PINNED\n", 0.8)
hits = [y for y, l in enumerate(r.rows()) if l[61:].startswith("SELMARK_PINNED")]
noclip()
if hits:
    r.drag(61, hits[0], 61 + len("SELMARK_PINNED") - 1, hits[0])
check("dragging in the pinned pane copies its text", clip() == "SELMARK_PINNED")
r.click(10, 12)

# clickable path in the pinned pane
open(W + "/note.txt", "w").write("NOTE_BODY\n")
r.click(90, 12)
r.send(b"echo see note.txt\n", 0.8)
pos = r.find(r"note\.txt", 61)
if pos: r.click(pos[0] + 2, pos[1], code=16)
check("Ctrl+click on a path in the pinned pane opens a preview", "NOTE_BODY" in r.text())
r.send(b"\x1b", 0.8)

# swap with the title-bar button
r.pump(0.5)
r.click(115, 2)
check("the swap button moves the pinned pane to the left", "VIA_W" not in r.right() and ("SELMARK_PINNED" in r.left() or "see note.txt" in r.left()))
check("...and the buttons are now at the pinned pane's top corner on the left", "⇄" in r.rows()[2][0:60])
run_menu(r, "swap pane", 0.8)
check("menu Swap pane sides moves it back to the right", "⇄" in r.rows()[2][61:120])

# remembered layout
check("state.toml remembers the layout", os.path.exists(state) and "side = \"right\"" in open(state).read() and "kind = \"shell\"" in open(state).read())

# unsplit by the close button: the tab stays
r.click(118, 2)
check("the X button unsplits (full width again)", r.cols_now() == 120)
check("...and the pinned tab is still a tab", "1 " in r.rows()[1] and "2 " in r.rows()[1])
run_menu(r, "pin left", 1.2)
check("Pin left puts the active tab's pane on the left", r.cols_now() is not None and "⇄" in r.rows()[2][0:60])
run_menu(r, "unsplit", 1.0)
check("menu Unsplit goes back to one pane", r.cols_now() == 120)
r.send(MENU + b"w", 0.6)
check("Switch pane with a single pane is a harmless message", "no preview pane" in r.status())
run_menu(r, "pin right", 1.2)
r.quit()

# restored on the next start
r = Run(S + "/panes.toml")
check("the pinned pane is restored on the next start", "⇄" in r.rows()[2][61:120] and r.cols_now() == 60)
r.quit()

# narrow window: one pane, nothing breaks
r = Run(S + "/panes.toml", cols=80)
r.send(b"echo NARROW_OK\n", 0.6)
check("narrow terminal: remembered pin is not shown, shell fills the window", "NARROW_OK" in r.text() and r.cols_now() == 80)
run_menu(r, "pin right", 1.0)
check("pinning in a narrow window says so", "too narrow" in r.status())
r.quit()
sys.exit(1 if fails else 0)
