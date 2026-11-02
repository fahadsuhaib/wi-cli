# usage: python dev/pty_remote.py <scratch-dir> remote   (needs pyte; box "test" in <scratch-dir>/test.toml)
# remote: yes
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, pyte
S, mode = sys.argv[1], sys.argv[2]
if mode != "remote":
    sys.exit(0)
MENU = b"\x00"
BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
def box(cmd):
    return subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "sh", "-c", cmd], capture_output=True, text=True).stdout
def sessions(): return box("tmux -L wicli list-sessions -F '#S' 2>/dev/null")

class Run:
    def __init__(self, args, env):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execve(BIN, ["wi"] + args, env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 90, 0, 0))
        self.scr = pyte.Screen(90, 24); self.st = pyte.ByteStream(self.scr)
        self.seen = ""   # every screen snapshot, to look for tmux text at any moment
        self.pump(3)
    def pump(self, t):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return
                self.seen += self.text() + "\n"
    def text(self): return "\n".join(l.rstrip() for l in self.scr.display)
    def send(self, b, t=1.0):
        os.write(self.fd, b); self.pump(t)
    def exited(self):
        p, _ = os.waitpid(self.pid, os.WNOHANG); return p != 0

env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color")
box("tmux -L wicli kill-server 2>/dev/null; pkill -f DETACHSLEEP")
screens = ""

# (a) two tabs with markers, detach
r = Run(["test"], env)
r.send(b"echo DETACH_MARK_$((40+2))\n")
r.send(b"(exec -a DETACHSLEEP sleep 300) &\n")
r.send(MENU + b"n", 2)
r.send(b"echo SECOND_MARK_$((50+5))\n")
r.send(MENU + b"1", 1)
check("tab 1 shows marker before detach", "DETACH_MARK_42" in r.text())
screens += r.seen
r.send(MENU + b"q", 1.5)
check("Detach exits wi", r.exited())
s = sessions().split()
check("both tmux sessions survive detach", s == ["wicli-1", "wicli-2"])
check("sleep process still alive", "DETACHSLEEP" in box("ps -eo args | grep [D]ETACHSLEEP"))

# (b) reattach
r = Run(["test"], env)
check("tab strip shows two tabs on reattach", "1 " in r.text().split("\n")[1] and "2 " in r.text().split("\n")[1])
check("tab 1 restored with earlier output", "DETACH_MARK_42" in r.text())
r.send(MENU + b"2", 1)
check("tab 2 restored with earlier output", "SECOND_MARK_55" in r.text())
screens += r.seen
# close tab with the menu ends its session
r.send(MENU + b"c", 1.5)
check("Close tab ends that tmux session", sessions().split() == ["wicli-1"])
# new tab gets reused number and a fresh shell
r.send(MENU + b"n", 2)
check("new tab after close uses a fresh session", sorted(sessions().split()) == ["wicli-1", "wicli-2"])

# (c) end and quit
r.send(MENU + b"e", 2)
check("End sessions and quit exits wi", r.exited())
check("no wicli sessions remain", sessions().strip() == "")
screens += r.seen

# (d) no tmux UI visible anywhere
import re
bad = [l for l in screens.split("\n") if re.search(r"\[wicli-\d\]|\bwicli-\d\b.*\d+:\w+|tmux", l)]
check("no tmux status bar or text on screen", not bad)
if bad: print("  saw:", bad[:3])

# (e) alias from an ssh-config style file
cfg = S + "/ssh_config"
import re as _re
ident = _re.search(r'identity\s*=\s*"([^"]+)"', open(S + "/test.toml").read()).group(1)
open(cfg, "w").write("Host aliasbox\n  HostName 127.0.0.1\n  User dev\n  Port 2222\n  IdentityFile %s\n" % ident)
open(S + "/empty.toml", "w").write("")
# known_hosts: reuse the box's file location through ~/.ssh is not ours; use HOME override
home = S + "/home"; os.makedirs(home + "/.ssh", exist_ok=True)
env2 = dict(env, WICLI_CONFIG=S + "/empty.toml", WICLI_SSH_CONFIG=cfg, HOME=home)
r = Run(["aliasbox"], env2)
r.send(b"echo ALIAS_$((6*7))\n")
check("ssh config alias connects", "ALIAS_42" in r.text())
r.send(MENU + b"e", 2)
check("alias session ended", sessions().strip() == "")
# proxyjump rejected
open(S + "/ssh_config2", "w").write("Host pj\n  HostName x\n  ProxyJump y\n")
out = subprocess.run([BIN, "pj"], env=dict(env2, WICLI_SSH_CONFIG=S + "/ssh_config2"), capture_output=True, text=True, stdin=subprocess.DEVNULL).stderr
check("ProxyJump gives a clear wi: error", out.startswith("wi:") and "ProxyJump" in out)

# (f) plain shells with use_tmux = false
base = open(S + "/test.toml").read()
open(S + "/notmux.toml", "w").write(base + "use_tmux = false\n")
r = Run(["test"], dict(env, WICLI_CONFIG=S + "/notmux.toml"))
r.send(b"echo PLAIN_$((3+4))\n")
check("plain shell works without tmux", "PLAIN_7" in r.text())
check("status bar says plain shells", "no tmux" in r.text())
check("no tmux session created", sessions().strip() == "")
r.send(MENU + b"q", 1.5)
check("Quit exits in plain mode", r.exited())
box("tmux -L wicli kill-server 2>/dev/null")
sys.exit(1 if fails else 0)
