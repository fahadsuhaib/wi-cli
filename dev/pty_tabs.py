# usage: python dev/pty_tabs.py <scratch-dir-with-test.toml> local|remote   (needs pyte)
# remote: yes
import os, pty, sys, time, select, struct, fcntl, termios, pyte
S, mode = sys.argv[1], sys.argv[2]
MENU = b"\x00"  # Ctrl-Space
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color")
args = ["wi"] if mode == "local" else ["wi", "test"]
pid, fd = pty.fork()
if pid == 0:
    if mode == "local":
        os.makedirs(S + "/ws", exist_ok=True); os.chdir(S + "/ws")
    os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), args, env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 90, 0, 0))
scr = pyte.Screen(90, 24); st = pyte.ByteStream(scr)
def pump(t):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try: st.feed(os.read(fd, 65536))
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in scr.display)
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
pump(2.5)
os.write(fd, b"echo ONE$((1+1))\n"); pump(1)
check("tab 1 runs command", "ONE2" in text())
check("no tab strip with one tab", "shell 1" not in text().split("\n")[1])
os.write(fd, MENU + b"n"); pump(1.5)
import re
strip = text().split("\n")[1]
# titles may be set by the shell prompt (OSC), so only require two numbered entries
check("tab strip appears", re.search(r"\b1 \S.*\b2 \S", strip) is not None)
os.write(fd, b"echo TWO$((2+2))\n"); pump(1)
check("tab 2 runs command", "TWO4" in text())
check("tab 2 does not show tab 1 output", "ONE2" not in text())
os.write(fd, MENU + b"1"); pump(0.8)
check("jump to tab 1 by number", "ONE2" in text() and "TWO4" not in text())
os.write(fd, MENU + b"t"); pump(0.8)
check("next tab", "TWO4" in text())
os.write(fd, MENU + b"c"); pump(1)
check("close tab returns to tab 1", "ONE2" in text() and not re.search(r"\b1 \S.*\b2 \S", text().split("\n")[1]))
os.write(fd, MENU + b"q"); pump(1.5)
p, _ = os.waitpid(pid, os.WNOHANG)
check("quit exits", p != 0)
sys.exit(1 if fails else 0)
