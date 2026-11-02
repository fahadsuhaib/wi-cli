# usage: python dev/pty_robust.py <scratch-dir-with-test.toml>   (needs pyte)
import os, pty, sys, time, select, struct, fcntl, termios, signal, subprocess, pyte
S = sys.argv[1]
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
LEAVE = b"\x1b[?1049l"; ENTER = b"\x1b[?1049h"
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

def spawn(args, cfg=None, cwd=None):
    env = dict(os.environ, WICLI_CONFIG=cfg or S + "/test.toml", TERM="xterm-256color")
    pid, fd = pty.fork()
    if pid == 0:
        os.makedirs(S + "/ws", exist_ok=True); os.chdir(cwd or S + "/ws")
        os.execve(WI, ["wi"] + args, env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 90, 0, 0))
    return pid, fd

def pump(fd, t, acc):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                d = os.read(fd, 65536)
                if not d: return
                acc.extend(d)
            except OSError: return

def alive(pid):
    try: return os.waitpid(pid, os.WNOHANG) == (0, 0)
    except ChildProcessError: return False

def wait_exit(pid, fd, acc, t=5):
    end = time.time() + t
    while time.time() < end:
        pump(fd, 0.2, acc)
        try:
            p, st = os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            return None
        if p: return st
    return "timeout"

def pgone(pid):
    try: os.kill(pid, 0); return False
    except ProcessLookupError: return True

def children(pid):
    out = subprocess.run(["pgrep", "-P", str(pid)], capture_output=True, text=True).stdout.split()
    return [int(x) for x in out]

for name, sig in (("SIGTERM", signal.SIGTERM), ("SIGHUP", signal.SIGHUP)):
    pid, fd = spawn([]); acc = bytearray()
    pump(fd, 1.0, acc)
    kids = children(pid)
    for _ in range(40):
        if kids and b"\x1b[?1049h" in acc: break
        pump(fd, 0.25, acc); kids = children(pid)
    pump(fd, 0.5, acc)
    os.write(fd, b"echo READY\n"); pump(fd, 0.5, acc)
    os.kill(pid, sig)
    st = wait_exit(pid, fd, acc)
    time.sleep(0.3)
    check(f"{name}: wi exits", st != "timeout")
    check(f"{name}: output ends with leave-alt-screen", bytes(acc).rstrip().endswith(LEAVE))
    check(f"{name}: shell child gone", bool(kids) and all(pgone(k) for k in kids))

# (c) unreachable box
cfg = S + "/bad.toml"
open(cfg, "w").write('[[box]]\nname="bad"\nhost="127.0.0.1"\nport=1\nuser="dev"\nidentity="%s/../dev/id_test"\n' % os.path.dirname(os.path.abspath(__file__)))
pid, fd = spawn(["bad"], cfg); acc = bytearray()
st = wait_exit(pid, fd, acc, 15)
out = bytes(acc).decode(errors="replace")
lines = [l for l in out.replace("\r", "").split("\n") if l.strip()]
print("   output:", lines)
check("bad box: exit status 1", st not in ("timeout", None) and os.WIFEXITED(st) and os.WEXITSTATUS(st) == 1)
check("bad box: single wi: line", len(lines) == 1 and lines[0].startswith("wi: "))
check("bad box: no alt screen", ENTER not in acc and LEAVE not in acc)

# (d) invalid prefix_key
cfg = S + "/badkey.toml"
open(cfg, "w").write('[defaults]\nprefix_key = "banana"\n')
pid, fd = spawn([], cfg); acc = bytearray()
st = wait_exit(pid, fd, acc, 10)
out = bytes(acc).decode(errors="replace")
lines = [l for l in out.replace("\r", "").split("\n") if l.strip()]
print("   output:", lines)
check("bad prefix_key: exit status 1", st not in ("timeout", None) and os.WIFEXITED(st) and os.WEXITSTATUS(st) == 1)
check("bad prefix_key: clear wi: line", len(lines) == 1 and lines[0].startswith("wi: ") and "prefix_key" in lines[0])
check("bad prefix_key: no alt screen", ENTER not in acc and LEAVE not in acc)

# (e) Ctrl-C in shell
pid, fd = spawn([]); acc = bytearray()
pump(fd, 2.0, acc)
os.write(fd, b"sleep 30; echo AFTER$((1+1))\n"); pump(fd, 0.7, acc)
os.write(fd, b"\x03"); pump(fd, 0.7, acc)
os.write(fd, b"echo ALIVE$((2+3))\n"); pump(fd, 1.0, acc)
check("ctrl-c interrupts sleep, wi keeps running", alive(pid) and b"ALIVE5" in acc and b"AFTER2" not in acc)
os.write(fd, b"\x00q"); wait_exit(pid, fd, acc)
sys.exit(1 if fails else 0)
