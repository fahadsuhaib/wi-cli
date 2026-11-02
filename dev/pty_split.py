# usage: python dev/pty_split.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# Side-by-side preview pane in a 120x30 terminal. WICLI_CLIPBOARD=file:... keeps the tests off the
# real clipboard. Remote mode uses ~ on the test box (files under split/, and a throwaway git repo in
# the home directory for the Git view, removed again at the end).
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, pyte
S, mode = sys.argv[1], sys.argv[2]
remote = mode == "remote"
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
W = S + "/sws"
clipfile = S + "/split_clip.txt"
BOXCFG = open(S + "/test.toml").read()

def sh(script):
    if remote:
        subprocess.run(["docker", "exec", "-u", "dev", "-w", "/home/dev", "devbox-test", "bash", "-c", script], check=True)
    else:
        subprocess.run(["bash", "-c", script], check=True, cwd=W)

FILES = r'''
set -e
rm -rf split; mkdir -p split
for i in $(seq 1 60); do echo "alpha_line_$i"; done > split/a.txt
printf 'BRAVO_TEXT\nref a.txt:7\n' > split/b.txt
'''
GIT = r'''
set -e
rm -rf .git g1.txt
git init -q -b testbr
git config user.email t@example.com; git config user.name t
printf '/*\n!/g1.txt\n' > .git/info/exclude
seq 1 30 | sed 's/^/orig /' > g1.txt
git add g1.txt && git commit -q -m init
seq 1 30 | sed 's/^/orig /' | sed 's/^orig \([0-9]*[02468]\)$/CHANGED \1/' > g1.txt
'''
shutil.rmtree(W, ignore_errors=True); os.makedirs(W)
sh(FILES)

def config(side=None, pct=None):
    extra = ""
    if side or pct:
        extra = "\n[defaults]\n" + (f'preview_side = "{side}"\n' if side else "") + (f"preview_width_pct = {pct}\n" if pct else "")
    path = S + "/split_" + (side or "default") + ".toml"
    open(path, "w").write(BOXCFG + extra)
    return path

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

# ---------------------------------------------------------------- default: preview on the right
r = Run(config())
r.send(b"echo SHELL_MARK_ONE\n", 0.6)
w0 = r.cols_now()
check("before opening a preview the shell is full width", w0 == 120)
r.open_file("split/a.txt")
check("shell on the left and preview on the right at the same time", "SHELL_MARK_ONE" in r.left() and "alpha_line_1" in r.right() and "alpha_line_1" not in r.left())
check("the preview is not in a separate tab (no tab strip)", "a.txt" not in r.rows()[1] or "alpha_line" not in r.rows()[1])
check("the divider separates the panes", all(r.rows()[y][60] in "│▶◀" for y in range(1, 29)))
check("preview has the keyboard right after opening (status shows preview keys)", "Esc to the shell" in r.status() and r.lines_from() == 1)
r.send(b"\x1b", 0.6)
check("Esc returns focus to the shell without closing the preview", "Esc to the shell" not in r.status() and "alpha_line_1" in r.right())
w1 = r.cols_now()
check("shell PTY width equals the left pane width during the preview", w1 == 60)
check("text typed after Esc reached the shell", "SHELL_MARK_ONE" in r.left())

# focus by menu; typing goes only to the focused pane
noclip()
r.send(MENU + b"w", 0.6)
check("menu 'Switch pane' moves the keyboard to the preview", "Esc to the shell" in r.status())
r.send(b"echo TYPED_AWAY\n", 0.6)
check("typing while the preview is focused does not reach the shell", "TYPED_AWAY" not in r.text())
before = r.lines_from()
r.send(b"\x1b[B\x1b[B\x1b[B", 0.6)
check("...and the preview scrolls instead", before == 1 and r.lines_from() == 4)
r.send(MENU + b"w", 0.6)
check("Switch pane again returns to the shell", "Esc to the shell" not in r.status())
r.send(b"echo SECOND_MARK\n", 0.6)
check("typing with the shell focused goes to the shell (preview untouched)", "SECOND_MARK" in r.left() and r.lines_from() is None and "alpha_line_4" in r.right())

# click focus
r.click(90, 10)
check("clicking in the preview focuses it", "Esc to the shell" in r.status())
r.click(10, 10)
check("clicking in the shell focuses it", "Esc to the shell" not in r.status())

# selection in each pane
r.send(b"clear; echo SELMARK_LEFT_TEXT\n", 0.8)
hits = [y for y, l in enumerate(r.rows()) if l[:60].startswith("SELMARK_LEFT_TEXT")]
noclip()
if hits:
    r.drag(0, hits[0], len("SELMARK_LEFT_TEXT") - 1, hits[0])
check("dragging in the shell pane copies the shell text", clip() == "SELMARK_LEFT_TEXT")
pos = r.find(r"alpha_line_5(?!\d)", 61)
noclip()
if pos:
    r.drag(pos[0], pos[1], pos[0] + len("alpha_line_5") - 1, pos[1])
check("dragging in the preview pane copies the preview text (no line numbers)", clip() == "alpha_line_5")
check("selection highlights in the preview", pos is not None and r.scr.buffer[pos[1]][pos[0]].bg == "3d59a1")  # sel_bg of the dark theme

# wheel
r.click(10, 20)
r.send(b"clear; seq 1 300\n", 1.0)
r.wheel(True, 10, 10, 2)
# (under tmux the box repaints on the alternate screen, so wi has no scrollback of its own to browse)
if not remote: check("wheel in the shell pane scrolls the shell scrollback", "scrollback" in r.text())
r.wheel(False, 10, 10, 60)
f0 = r.lines_from()
r.wheel(False, 90, 10, 3)
check("wheel in the preview pane scrolls the preview", "scrollback" not in r.text() and "alpha_line_" in r.right())
r.send(b"clear\n", 0.5)

# links: from the shell into the side pane, at the right line; opening another file replaces it
r.send(b"echo see split/a.txt:40 now\n", 0.8)
pos = r.find(r"split/a\.txt:40", 0, 60)
r.click(pos[0] + 3, pos[1], code=16)
check("Ctrl+click on a path in the shell opens it in the side pane at that line", "alpha_line_40" in r.right() and "alpha_line_1 " not in r.right())
check("...and the shell is still on the left", "see split/a.txt:40" in r.left())
r.click(10, 20)   # opening a preview moved the keyboard to it; click back into the shell
r.send(b"echo go split/b.txt\n", 0.8)
pos = r.find(r"split/b\.txt", 0, 60)
r.click(pos[0] + 3, pos[1], code=16)
check("opening another file replaces the side preview", "BRAVO_TEXT" in r.right() and "alpha_line" not in r.right())
pos = r.find(r"a\.txt:7", 61)
if pos: r.click(pos[0] + 2, pos[1], code=16)
check("Ctrl+click on a path inside the preview pane opens it in the same pane", "alpha_line_7" in r.right() and "BRAVO_TEXT" not in r.right())

# labelled hints cover both panes
r.click(10, 20)
r.send(b"clear; echo HINT split/b.txt\n", 0.8)
pos = r.find(r"split/b\.txt", 0, 60)
r.click(pos[0] + 3, pos[1], code=16)   # b.txt, which mentions a.txt:7, now fills the preview
r.click(10, 20)
r.send(MENU + b"o", 0.8)
lab = [(x, y) for y in range(1, 29) for x in range(120) if r.scr.buffer[y][x].bg == "f2cc60"]  # hint_bg of the dark theme
check("labels are drawn in both panes", any(x < 60 for x, y in lab) and any(x > 60 for x, y in lab))
r.send(b"\x1b", 0.6)

# divider drag
r.drag(60, 10, 70, 10)
w2 = r.cols_now()
check("dragging the divider resizes the shell (tput cols follows)", w2 is not None and abs(w2 - 70) <= 1)
check("...and the preview takes the rest", "BRAVO_TEXT" in r.region(w2 + 1, 120) and "BRAVO_TEXT" not in r.region(0, w2))
r.drag(w2, 10, 30, 10)
w3 = r.cols_now()
check("dragging the divider far left stops at the 60-column shell minimum", w3 == 60)

# close
r.send(MENU + b"v", 0.8)
w4 = r.cols_now()
check("closing the preview restores full width", w4 == 120)
check("...and removes the pane", "alpha_line" not in r.text() and "BRAVO" not in r.text())
r.send(MENU + b"w", 0.6)
check("Switch pane with no preview is a harmless message", "no preview pane" in r.status())
r.quit()

# ---------------------------------------------------------------- left
r = Run(config("left"))
r.send(b"echo SHELL_MARK_LEFT\n", 0.6)
r.open_file("split/a.txt")
check("preview_side = left puts the preview on the left and the shell on the right", "alpha_line_1" in r.left() and "SHELL_MARK_LEFT" in r.region(60, 120) and "SHELL_MARK_LEFT" not in r.region(0, 59))
r.send(b"\x1b", 0.6)
check("left layout: shell PTY width equals the right pane width", r.cols_now() == 60)
# a mouse-aware app in the right-hand shell pane gets pane-relative coordinates
r.send(b"clear; printf '\\033[?1000h\\033[?1006h'; cat -v\n", 0.8)
r.click(70, 8)
check("a mouse-aware app in the shell pane gets coordinates relative to the pane", "^[[<0;11;8M" in r.text())
r.send(b"\x03", 0.3)
r.send(b"printf '\\033[?1000l\\033[?1006l'; clear\n", 0.5)
r.send(b"echo SELMARK_RIGHT_SHELL\n", 0.8)
hits = [y for y, l in enumerate(r.rows()) if l[60:].startswith("SELMARK_RIGHT_SHELL")]
noclip()
if hits:
    r.drag(60, hits[0], 60 + len("SELMARK_RIGHT_SHELL") - 1, hits[0])
check("selection works in a right-hand shell pane", clip() == "SELMARK_RIGHT_SHELL")
pos = r.find(r"alpha_line_2(?!\d)", 0, 59)
noclip()
if pos:
    r.drag(pos[0], pos[1], pos[0] + len("alpha_line_2") - 1, pos[1])
check("selection works in a left-hand preview pane", clip() == "alpha_line_2")
r.send(MENU + b"v", 0.8)
check("closing restores full width", r.cols_now() == 120)
r.quit()

# ---------------------------------------------------------------- tab (the old behaviour)
r = Run(config("tab"))
r.send(b"echo SHELL_MARK_TAB\n", 0.6)
r.open_file("split/a.txt")
check("preview_side = tab keeps the separate preview tab", "a.txt" in r.rows()[1] and "alpha_line_1" in r.text() and "SHELL_MARK_TAB" not in r.text())
r.send(b"\x1b", 0.6)
check("Esc goes back to the shell tab at full width", "SHELL_MARK_TAB" in r.text() and r.cols_now() == 120)
r.quit()

# ---------------------------------------------------------------- narrow terminal falls back to a tab
r = Run(config(), cols=80)
r.send(b"echo SHELL_MARK_NARROW\n", 0.6)
r.open_file("split/a.txt")
check("an 80-column terminal opens a preview tab instead of a pane", "a.txt" in r.rows()[1] and "alpha_line_1" in r.text() and "SHELL_MARK_NARROW" not in r.text())
check("the fallback says so in one word", "narrow" in r.status())
r.send(b"\x1b", 0.6)
check("the shell keeps the full 80 columns", r.cols_now() == 80)
r.quit()

# ---------------------------------------------------------------- terminal resize while split
r = Run(config())
r.open_file("split/a.txt")
r.send(b"\x1b", 0.4)
fcntl.ioctl(r.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 160, 0, 0))
os.kill(r.pid, 28)   # SIGWINCH
r.scr.resize(30, 160); r.cols = 160
r.pump(1.0)
c160 = r.cols_now()
check("terminal resize re-sizes the shell to the pane", c160 == 160 - 1 - 80 or (c160 is not None and 79 <= c160 <= 81))
r.quit()

# ---------------------------------------------------------------- Git changes in the side pane
sh(GIT)
try:
    r = Run(config())
    r.send(b"echo SHELL_MARK_GIT\n", 0.6)
    r.send(MENU + b"g", 1.5)
    check("Git changes opens in the side pane beside the shell", "SHELL_MARK_GIT" in r.left() and "g1.txt" in r.right())
    r.send(b"\x1b", 0.5)
    check("and the shell is full of life at 60 columns", r.cols_now() == 60)
    r.quit()
finally:
    sh("rm -rf .git g1.txt")
if remote:
    sh("rm -rf split")
sys.exit(1 if fails else 0)
