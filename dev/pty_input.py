# usage: python dev/pty_input.py <scratch-dir-with-test.toml> local   (needs pyte)
# Checks scrollback mode and bracketed paste handling.
import os, pty, sys, time, select, struct, fcntl, termios, pyte
S, mode = sys.argv[1], sys.argv[2]
MENU = b"\x00"  # Ctrl-Space
PS, PE = b"\x1b[200~", b"\x1b[201~"
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color")
args = ["wi"] if mode == "local" else ["wi", "test"]
pid, fd = pty.fork()
if pid == 0:
    if mode == "local":
        os.makedirs(S + "/ws", exist_ok=True); os.chdir(S + "/ws")
    os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), args, env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 90, 0, 0))
scr = pyte.Screen(90, 24); st = pyte.ByteStream(scr)
raw = bytearray()
def pump(t):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                d = os.read(fd, 65536); raw.extend(d); st.feed(d)
            except OSError: return
def text(): return "\n".join(l.rstrip() for l in scr.display)
def lines(): return [l.strip() for l in text().split("\n")]
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
pump(2.5)
check("host bracketed paste enabled", b"\x1b[?2004h" in raw)

# (a) scrollback
os.write(fd, b"clear; for i in $(seq 1 200); do echo LINE$i; done\n"); pump(2)
check("live view shows LINE200", "LINE200" in lines())
os.write(fd, MENU + b"s"); pump(0.8)
check("scroll mode hint shown", "scrollback 0/" in text())
os.write(fd, b"\x1b[5~"); pump(0.5)
t = text()
nums = [int(l[4:]) for l in lines() if l.startswith("LINE") and l[4:].isdigit()]
check("PgUp shows earlier line, LINE200 gone", bool(nums) and max(nums) < 200 and max(nums) > 150)
check("indicator moved", "scrollback 0/" not in t and "scrollback " in t)
os.write(fd, b"\x1b[H"); pump(0.5)
nums = [int(l[4:]) for l in lines() if l.startswith("LINE") and l[4:].isdigit()]
check("Home reaches top of history", bool(nums) and min(nums) < 30)
os.write(fd, b"\x1b[F"); pump(0.5)
check("End returns to bottom", "LINE200" in lines())
os.write(fd, b"\x1b[A\x1b[B"); pump(0.3)
os.write(fd, b"\x1b[5~"); pump(0.3)
os.write(fd, b"\x1b"); pump(0.8)
check("Esc returns to live view", "LINE200" in lines() and "scrollback" not in text())
os.write(fd, b"echo AFTER$((1+1))\n"); pump(1)
check("keys reach shell after Esc", "AFTER2" in lines())

# scroll keys are not forwarded; output while scrolled doesn't yank
os.write(fd, MENU + b"s"); pump(0.5); os.write(fd, b"\x1b[5~"); pump(0.3)
check("scroll keys not echoed to shell", "[5~" not in text())
os.write(fd, b"\x1b"); pump(0.5)

# (b) paste into a plain program: no markers, no menu
os.write(fd, b"cat -v\n"); pump(1)
os.write(fd, PS + b"first line\nsecond\x00line\nthird" + PE); pump(1)
t = text()
check("plain app: paste text arrives", "first line" in t and "third" in t)
check("plain app: no markers", "200~" not in t and "201~" not in t)
check("paste with 0x00 did not open menu", "arrows + Enter" not in t and "\u250c" not in t)
check("0x00 forwarded as data", "second^@line" in t)
# paste split across writes, menu byte inside
# gaps stay under wi's 50 ms hold-back for a partial marker
os.write(fd, PS[:3]); time.sleep(0.02); os.write(fd, PS[3:] + b"A\x00B"); pump(0.3); os.write(fd, PE[:2]); time.sleep(0.02); os.write(fd, PE[2:]); pump(0.8)
t = text()
check("split markers: no menu, no markers", "arrows + Enter" not in t and "200~" not in t and "201~" not in t and "A^@B" in t)
os.write(fd, b"\x03"); pump(0.5)

# bracketed-paste-aware shell: paste with newline is not executed until Enter
import shutil
sh = "zsh -f -i" if shutil.which("zsh") else "bash --norc -i"  # (bash 5.1+ readline also brackets paste)
# macOS bash 3.2 has no bracketed paste; zsh ZLE does (same as bash 5 readline)
os.write(fd, sh.encode() + b"\n"); pump(1.5)
os.write(fd, b"clear\n"); pump(0.5)
os.write(fd, PS + b"echo PAST$((1+2))ED\necho SEC$((2+2))OND" + PE); pump(1)
check("multi-line paste not executed before Enter", "PAST3ED" not in lines() and "SEC4OND" not in lines())
check("pasted text is in the editor", "echo PAST$((1+2))ED" in text())
os.write(fd, b"\r"); pump(1.2)
check("Enter executes pasted lines", "PAST3ED" in lines() and "SEC4OND" in lines())
check("menu not opened by paste", "arrows + Enter" not in text())
os.write(fd, b"\x00\x00"); pump(0.2)  # open/close menu
os.write(fd, b"exit\n"); pump(0.8)
os.write(fd, MENU + b"q"); pump(1.5)
p, _ = os.waitpid(pid, os.WNOHANG)
check("quit exits", p != 0)
sys.exit(1 if fails else 0)
