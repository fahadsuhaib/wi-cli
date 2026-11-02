# usage: python dev/pty_compat.py <scratch-dir-with-test.toml> local|remote   (needs pyte)
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
def text():
    # pyte's scr.display crashes on wide-char continuation cells; render by hand.
    return "\n".join("".join(scr.buffer[y][x].data for x in range(scr.columns)).rstrip() for y in range(scr.lines))
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
pump(2.5)
def send(cmd, t=1.2): os.write(fd, cmd.encode() + b"\n"); pump(t)
send("bash --norc")
send("printf '\\033[c'; read -s -t 3 -d c r; echo DA1GOT${r#*[}END")
check("DA1 reply reaches shell", "DA1GOT?62;END" in text())
send("printf '\\033[6n'; IFS=';' read -s -t 3 -d R -a pos; echo POS${pos[0]#*[}_${pos[1]}_END")
import re
m = re.search(r"POS(\d+)_(\d+)_END", text())
check("cursor position reply reaches shell", bool(m) and int(m.group(1)) > 1 and int(m.group(2)) == 1)
send("printf '\\033[5n'; read -s -t 3 -d n r; echo DSRGOT${r#*[}END")
check("DSR status reply reaches shell", "DSRGOT0END" in text())
send("printf '\\033[>q'; read -r -s -t 3 -d '\\\\' r; echo XTV${r:4:5}END")
check("XTVERSION reply reaches shell", "XTVwicliEND" in text())
send("clear"); 
send("printf '\\r\\xe6\\x97\\xa5\\xe6\\x9c\\xac\\xe8\\xaa\\x9e|\\n'", 1)
rows = [y for y in range(24) if scr.buffer[y][0].data == "日" and scr.buffer[y][6].data == "|"]
check("wide chars aligned (pipe at column 6)", bool(rows))
if not rows: print(text())
send("printf '\\033[38;2;255;100;0mTRUE\\033[0m \\033[38;5;200mIDX\\033[0m done\\n'", 1)
check("true colour / 256 colour render", "TRUE IDX done" in text())
send("printf '\\r\\xf0\\x9f\\x8e\\x89|\\n'", 1)
rows = [y for y in range(24) if scr.buffer[y][0].data == "\U0001F389" and scr.buffer[y][2].data == "|"]
check("emoji aligned (pipe at column 2)", bool(rows))
send("exit", 0.8)
send("echo BEFOREVIM")
send("vim -u NONE -c 'q'", 1.5)
send("echo AFTERVIM$((3+4))", 1)
check("vim opens and quits cleanly, primary screen restored", "AFTERVIM7" in text() and "BEFOREVIM" in text())
send("printf '\\033]0;mytitle\\007'", 0.5)
os.write(fd, MENU + b"n"); pump(1.5)
check("tab strip shows OSC title", "1 mytitle" in text() and "2 shell 2" in text())
send("printf '\\033]2;second\\033\\\\'", 0.8)
check("OSC title via ST on tab 2", "2 second" in text())
if os.system("command -v claude >/dev/null") == 0:
    send("claude --version", 4)
    print("claude --version screen tail:", [l for l in text().split("\n") if "Claude Code" in l][-1:])
os.write(fd, MENU + b"q"); pump(1.5)
p, _ = os.waitpid(pid, os.WNOHANG)
check("quit exits", p != 0)
sys.exit(1 if fails else 0)
