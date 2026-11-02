# usage: python dev/pty_select.py <scratch-dir-with-test.toml> local   (needs pyte)
# Selection and copy. WICLI_CLIPBOARD=file:... keeps the tests off the real clipboard.
import os, pty, sys, time, select, struct, fcntl, termios, pyte
S = sys.argv[1]
MENU = b"\x00"
clipfile = S + "/clip.txt"
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color", WICLI_CLIPBOARD="file:" + clipfile)
os.makedirs(S + "/ws/pv", exist_ok=True)
open(S + "/ws/pv/live.txt", "w").write("first line\nsecond line\n")
pid, fd = pty.fork()
if pid == 0:
    os.chdir(S + "/ws")
    os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), ["wi"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
scr = pyte.Screen(100, 30); st = pyte.ByteStream(scr)
def pump(t):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: st.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in scr.display)
def find_all(s):
    out = []
    for y, l in enumerate(scr.display):
        x = l.find(s)
        while x >= 0:
            out.append((x, y)); x = l.find(s, x + 1)
    return out
def send(code, x, y):   # x, y 0-based screen cell
    os.write(fd, f"\x1b[<{code};{x + 1};{y + 1}M".encode())
def release(x, y): os.write(fd, f"\x1b[<0;{x + 1};{y + 1}m".encode())
def drag(x1, y1, x2, y2):
    send(0, x1, y1); pump(0.05)
    steps = max(abs(x2 - x1), abs(y2 - y1), 1)
    for i in range(1, steps + 1):
        send(32, x1 + (x2 - x1) * i // steps, y1 + (y2 - y1) * i // steps)
    pump(0.05); release(x2, y2); pump(0.5)
def click(x, y): send(0, x, y); release(x, y); pump(0.05)
def clip():
    return open(clipfile).read() if os.path.exists(clipfile) else None
def reverse_cells(y): return sum(1 for x in range(100) if scr.buffer[y][x].reverse)
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
if os.path.exists(clipfile): os.remove(clipfile)

pump(2.5)
os.write(fd, b"echo HELLO_SELECT_WORLD\n"); pump(0.6)
hits = find_all("HELLO_SELECT_WORLD")
check("test line is on screen twice (command and output)", len(hits) == 2)
x0, y0 = hits[1]
drag(x0, y0, x0 + len("HELLO_SELECT_WORLD") - 1, y0)
check("drag copies exactly the selected text", clip() == "HELLO_SELECT_WORLD")
check("status bar says what was copied", "copied 18 characters" in text())
check("the selection is highlighted", reverse_cells(y0) == 18)
drag(x0 + 6, y0, x0 + 11, y0)
check("a partial drag copies just that part", clip() == "SELECT")

# typing clears the highlight
os.write(fd, b"x"); pump(0.4)
check("typing clears the highlight", all(reverse_cells(y) == 0 for y in range(1, 29)))
os.write(fd, b"\x15"); pump(0.2)   # Ctrl-U: clear the typed x

# multi-line selection
os.write(fd, b"clear; printf 'LINE_ONE\\nLINE_TWO\\n'\n"); pump(0.6)
a = find_all("LINE_ONE")[-1]; b = find_all("LINE_TWO")[-1]
drag(a[0], a[1], b[0] + 7, b[1])
check("multi-line drag keeps the line break", clip() == "LINE_ONE\nLINE_TWO")

# a plain click copies nothing and leaves no highlight
os.remove(clipfile)
click(5, 10); pump(0.3)
check("a click without dragging copies nothing", clip() is None)
check("and leaves no highlight", all(reverse_cells(y) == 0 for y in range(1, 29)))

# double-click selects the word under the pointer, triple-click the line
os.write(fd, b"clear; echo path/to/file.rs:12 and more\n"); pump(0.6)
h = find_all("path/to/file.rs:12")[-1]
click(h[0] + 3, h[1]); click(h[0] + 3, h[1]); pump(0.5)
check("double-click selects the word (a whole path)", clip() == "path/to/file.rs:12")
click(h[0] + 3, h[1]); click(h[0] + 3, h[1]); click(h[0] + 3, h[1]); pump(0.5)
check("triple-click selects the line", clip() == "path/to/file.rs:12 and more")

# preview: copied text has no line numbers
os.write(fd, MENU + b"p"); pump(0.4)
os.write(fd, b"pv/live.txt\r"); pump(1.0)
f = find_all("first line")[0]
drag(f[0], f[1], f[0] + 9, f[1])
check("preview selection copies the text without the line number", clip() == "first line")
os.write(fd, b"\x1b"); pump(0.4)

# an app that asked for the mouse still gets the events, not a selection
os.remove(clipfile)
os.write(fd, b"clear; printf '\\033[?1000h\\033[?1006h'; cat -v\n"); pump(0.8)
drag(10, 8, 20, 8)
check("apps that use the mouse get the drag, wi does not select", clip() is None)
os.write(fd, b"\x03"); pump(0.3)

os.write(fd, MENU + b"q"); pump(1.5)
p, _ = os.waitpid(pid, os.WNOHANG)
check("quit exits", p != 0)
sys.exit(1 if fails else 0)
