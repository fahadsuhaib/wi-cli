# usage: python dev/pty_scroll.py <scratch-dir-with-test.toml> local   (needs pyte)
# Checks that typing and pasting while scrolled back go to the tab (view jumps to the live
# bottom), and that scroll mode never gets stuck (no history, program on the alternate screen).
import os, pty, sys, time, select, struct, fcntl, termios, pyte
S, mode = sys.argv[1], sys.argv[2]
MENU = b"\x00"
PS, PE = b"\x1b[200~", b"\x1b[201~"
WHEEL_UP, WHEEL_DN = b"\x1b[<64;10;10M", b"\x1b[<65;10;10M"
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color")
pid, fd = pty.fork()
if pid == 0:
    os.makedirs(S + "/ws", exist_ok=True); os.chdir(S + "/ws")
    os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), ["wi"], env)
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
def lines(): return [l.strip() for l in text().split("\n")]
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
def live(): return "scroll:" not in text() and "scrollback" not in text()
pump(2.5)

# (a) wheel up with almost no history must not leave a stuck scroll mode
os.write(fd, WHEEL_UP); pump(0.5)
os.write(fd, b"echo NOSTUCK$((1+1))\n"); pump(1)
check("no history: wheel up then typing works", "NOSTUCK2" in lines() and live())

os.write(fd, b"clear; for i in $(seq 1 200); do echo LINE$i; done\n"); pump(2)

# (b) wheel up, then type: the first key is not lost, the view goes live
os.write(fd, WHEEL_UP * 3); pump(0.5)
check("wheel up enters scroll", "scrollback" in text() and "LINE200" not in lines())
os.write(fd, b"echo WHEELTYPED$((2+3))\n"); pump(1)
check("typing after wheel: delivered whole", "WHEELTYPED5" in lines())
check("typing after wheel: back live", live() and any(l.startswith("LINE") for l in lines()))

# (c) menu Scroll, PgUp, then a printable key / Enter / Backspace / Ctrl-letter
os.write(fd, MENU + b"s"); pump(0.5); os.write(fd, b"\x1b[5~"); pump(0.3)
os.write(fd, b"echo KEYTYPEX\x7fD\n"); pump(1)
check("scroll mode: typed text (with Backspace) reaches shell", "KEYTYPED" in lines() and live())
os.write(fd, MENU + b"s"); pump(0.5); os.write(fd, b"\x1b[5~"); pump(0.3)
os.write(fd, b"\x03"); pump(0.5)
check("scroll mode: Ctrl-C leaves scroll", live())

# (d) arrows still scroll; Esc exits without sending
os.write(fd, MENU + b"s"); pump(0.5); os.write(fd, b"\x1b[A\x1b[A"); pump(0.3)
check("arrows scroll in scroll mode", "scrollback 2/" in text())
os.write(fd, b"\x1b"); pump(0.6)
check("Esc exits scroll", live())

# (e) paste while scrolled back
os.write(fd, WHEEL_UP * 3); pump(0.4)
os.write(fd, PS + b"echo PASTED$((4+4))\n" + PE); pump(1)
check("paste while scrolled: in the prompt, view live", "echo PASTED$((4+4))" in text() and live())
os.write(fd, b"\r"); pump(1)
check("paste while scrolled: runs", "PASTED8" in lines())

# (f) a program that switches to the alternate screen while scrolled back
os.write(fd, b"(sleep 2; printf '\\033[?1049hALTSCREEN'; sleep 3; printf '\\033[?1049l') &\n"); pump(0.5)
os.write(fd, WHEEL_UP * 3); pump(0.5)
check("scrolled before alt screen", "scrollback" in text())
pump(2.5)
check("alt screen: scroll mode left", "ALTSCREEN" in text() and live())
pump(3)
os.write(fd, b"echo AFTERALT$((1+1))\n"); pump(1)
check("after alt screen: typing works", "AFTERALT2" in lines())

os.write(fd, MENU + b"q"); pump(1.5)
sys.exit(1 if fails else 0)
