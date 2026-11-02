# usage: python dev/pty_quiet.py <scratch-dir-with-test.toml> local   (needs pyte)
# Background tab markers: `*` while a background tab prints, `!` once it goes quiet.
import os, pty, sys, time, select, struct, fcntl, termios, re, pyte
S = sys.argv[1]
MENU = b"\x00"
cfg = S + "/quiet.toml"
open(cfg, "w").write("[defaults]\nquiet_secs = 1\n")
env = dict(os.environ, WICLI_CONFIG=cfg, TERM="xterm-256color")
pid, fd = pty.fork()
if pid == 0:
    os.makedirs(S + "/ws", exist_ok=True); os.chdir(S + "/ws")
    os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), ["wi"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 90, 0, 0))
scr = pyte.Screen(90, 24); st = pyte.ByteStream(scr)
def pump(t):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: st.feed(os.read(fd, 65536))
            except OSError: return
def strip(): return scr.display[1]
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
pump(2.5)
os.write(fd, b"sleep 2; echo BGDONE\n"); pump(0.3)
os.write(fd, MENU + b"n"); pump(1.0)
check("two tabs, no marker yet", re.search(r"\b1 \S.*\b2 \S", strip()) and "!" not in strip() and "*" not in strip())
pump(1.6)   # tab 1 prints BGDONE at ~2s, quiet_secs = 1 -> `*` around 2.0s, `!` around 3.0s
s1 = strip()
check("background output shows * while streaming", "*" in s1 or "!" in s1)
pump(2.0)
check("tab 1 shows ! after going quiet", "!" in strip() and "*" not in strip())
os.write(fd, MENU + b"1"); pump(0.8)
check("marker clears when you open the tab", "!" not in strip() and "*" not in strip())
check("tab 1 content is intact", "BGDONE" in "\n".join(scr.display))
os.write(fd, MENU + b"q"); pump(1.5)
p, _ = os.waitpid(pid, os.WNOHANG)
check("quit exits", p != 0)
sys.exit(1 if fails else 0)
