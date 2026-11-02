# usage: python dev/pty_repaint.py <scratch-dir-with-test.toml> local   (needs pyte)
# After a pause in input the next key repaints the whole screen once (a terminal may have
# discarded the screen while its tab was hidden); fast typing never does.
import os, pty, sys, time, select, struct, fcntl, termios, pyte
S = sys.argv[1]
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color", COLORTERM="truecolor")
env.pop("COLORFGBG", None)
os.makedirs(S + "/ws", exist_ok=True)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(S + "/ws")
    os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), ["wi"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
scr = pyte.Screen(100, 30); st = pyte.ByteStream(scr)
CLEAR = b"\x1b[2J"
def pump(t):
    """Read for t seconds; return the raw bytes seen."""
    raw = b""; end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: d = os.read(fd, 65536)
            except OSError: return raw
            raw += d; st.feed(d)
    return raw
def rows(): return ["".join((scr.buffer[y][x].data or " ") for x in range(100)).rstrip() for y in range(30)]
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

pump(2.5)
os.write(fd, b"echo REPAINT_MARK\n"); pump(0.6)
# fast typing: no full repaints
fast = b""
for ch in b"abc":
    os.write(fd, bytes([ch])); fast += pump(0.15)
check("fast typing does not repaint the whole screen", CLEAR not in fast)
os.write(fd, b"\x15"); pump(0.3)   # Ctrl-U clears the typed text
# a pause, then one key
pump(2.0)
after = b""
os.write(fd, b"x"); after += pump(0.6)
check("the first key after a pause repaints the whole screen", CLEAR in after)
check("and the content is all there afterwards", any("REPAINT_MARK" in r for r in rows()))
# the next keys are fast again
more = b""
for ch in b"yz":
    os.write(fd, bytes([ch])); more += pump(0.15)
check("typing right after that does not repaint again", CLEAR not in more)
# a mouse click after a pause also repaints
os.write(fd, b"\x15"); pump(0.3)
pump(2.0)
os.write(fd, b"\x1b[<0;10;10M\x1b[<0;10;10m"); click = pump(0.6)
check("a click after a pause repaints too", CLEAR in click)
os.write(fd, b"\x00q"); pump(1.0)
sys.exit(1 if fails else 0)
