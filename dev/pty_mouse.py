# usage: python dev/pty_mouse.py <scratch-dir-with-test.toml> local   (needs pyte)
import os, pty, sys, time, select, struct, fcntl, termios, re, pyte
S = sys.argv[1]
MENU = b"\x00"
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color")
os.makedirs(S + "/ws/pv", exist_ok=True)
open(S + "/ws/pv/big.log", "w").write(("x" * 79 + "\n") * 300)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(S + "/ws")
    os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), ["wi"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
scr = pyte.Screen(100, 30); st = pyte.ByteStream(scr)
raw = bytearray()
def pump(t):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try:
                d = os.read(fd, 65536); raw.extend(d); st.feed(d)
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in scr.display)
def click(col, row, btn=0):   # 1-based, like the terminal reports
    os.write(fd, f"\x1b[<{btn};{col};{row}M\x1b[<{btn};{col};{row}m".encode()); pump(0.5)
def wheel(up, col=10, row=10, n=1):
    for _ in range(n): os.write(fd, f"\x1b[<{64 if up else 65};{col};{row}M".encode())
    pump(0.6)
def find(s):
    for y, l in enumerate(scr.display):
        x = l.find(s)
        if x >= 0: return x + 1, y + 1
    return None
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

pump(2.5)
check("host terminal got mouse reporting enabled", b"\x1b[?1000h" in raw and b"\x1b[?1006h" in raw)
os.write(fd, b"echo FIRSTTAB\n"); pump(0.5)
os.write(fd, MENU + b"n"); pump(1.0)
os.write(fd, b"echo SECONDTAB\n"); pump(0.5)
check("on tab 2", "SECONDTAB" in text() and "FIRSTTAB" not in text())
click(3, 2)   # first label in the strip
check("clicking tab 1 in the strip switches to it", "FIRSTTAB" in text() and "SECONDTAB" not in text())
x = text().split("\n")[1].find("2 ") + 2
click(x, 2)
check("clicking tab 2 switches back", "SECONDTAB" in text())

# menu: click an entry
os.write(fd, MENU); pump(0.5)
pos = find("Previous tab")
check("menu is open", pos is not None)
click(pos[0] + 2, pos[1])
check("clicking a menu entry runs it (Previous tab -> tab 1)", "FIRSTTAB" in text() and find("Previous tab") is None)
os.write(fd, MENU); pump(0.5)
click(2, 20)   # outside the menu box
check("clicking outside the menu closes it", find("Previous tab") is None)

# wheel in the shell tab enters scrollback
os.write(fd, b"seq 1 300\n"); pump(1.0)
wheel(True, n=2)
check("wheel up scrolls back through output", "scrollback" in text())
wheel(False, n=60)
check("wheel down returns to the live view", "scrollback" not in text())

# an app that asked for the mouse receives events at its own coordinates
os.write(fd, b"clear; printf '\\033[?1000h\\033[?1006h'; cat -v\n"); pump(0.8)
click(10, 6)
check("mouse app gets the click (SGR, coordinates relative to its screen)", "^[[<0;10;4M" in text())
os.write(fd, b"\x03"); pump(0.3)
os.write(fd, b"printf '\\033[?1000l\\033[?1006l'\n"); pump(0.3)

# toggle mouse off from the menu
os.write(fd, MENU + b"m"); pump(0.6)
check("menu toggle turns mouse reporting off", b"\x1b[?1000l" in raw[-300:] and "mouse off" in text())
os.write(fd, MENU + b"m"); pump(0.6)
check("and back on", "mouse on" in text())

# previews scroll with the wheel
os.write(fd, MENU + b"p"); pump(0.4)
os.write(fd, b"pv/big.log\r"); pump(1.0)
check("preview open at the top", "lines 1-" in text())
wheel(False, n=3)
check("wheel down scrolls the preview", "lines 1-" not in text())
os.write(fd, b"\x1b"); pump(0.4)

os.write(fd, MENU + b"q"); pump(1.5)
p, _ = os.waitpid(pid, os.WNOHANG)
check("quit exits", p != 0)
sys.exit(1 if fails else 0)
