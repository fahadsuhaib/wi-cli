# usage: python dev/pty_newline.py <scratch-dir> local   (needs pyte)
# Alt+Enter is a newline in agent tabs in every key-protocol state (standard ESC CR / ESC LF,
# kitty CSI u, modifyOtherKeys), never a submit; the one-time "New line: Alt+Enter" hint and the
# Info wording. A perl "key echo" program (hex of each input chunk) is the fake agent.
import os, pty, sys, time, select, struct, fcntl, termios, shutil, pyte
S = os.path.realpath(sys.argv[1])
MENU = b"\x00"
CFGDIR = S + "/newline_cfg"
shutil.rmtree(CFGDIR, ignore_errors=True); os.makedirs(CFGDIR)
WS = S + "/newline_ws"
shutil.rmtree(WS, ignore_errors=True); os.makedirs(WS)
open(WS + "/CLAUDE.md", "w").write("# project\n")
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
open(CFGDIR + "/ke.pl", "w").write(r'''
system("stty raw -echo"); $| = 1;
while (sysread(STDIN, $b, 256)) { last if $b eq "q"; print "KE:" . unpack("H*", $b) . "\r\n"; }
system("stty sane");
''')
KE = f"perl {CFGDIR}/ke.pl"
HINT = "Alt+Enter (Shift+Enter isn't"
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, extra="", reply=False, keep_state=False):
        cfg = CFGDIR + "/config.toml"
        open(cfg, "w").write(open(S + "/test.toml").read() + f'\n[defaults]\nagent_start = "always"\n{extra}\n[agents.claude]\ncommand = "{KE}"\n')
        if not keep_state and os.path.exists(CFGDIR + "/state.toml"): os.remove(CFGDIR + "/state.toml")
        env = dict(os.environ, WICLI_CONFIG=cfg, TERM="xterm-256color", WICLI_TMUX_SOCKET="newlinetest")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(WS); os.execve(WI, ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
        self.scr = pyte.Screen(110, 30); self.st = pyte.ByteStream(self.scr)
        self.pump(0.4)
        if reply: os.write(self.fd, b"\x1b[?1u")  # a kitty terminal answers the flags query
        self.seen = []
        self.pump(2.6, True)
    def pump(self, t, snap=False):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: d = os.read(self.fd, 65536)
                except OSError: return
                self.st.feed(d)
                if snap: self.seen.append(self.text())
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)).rstrip() for y in range(self.scr.lines)]
    def text(self): return "\n".join(self.rows())
    def hint_seen(self): return any(HINT in t for t in self.seen)
    def ke(self):
        k = [l.strip()[3:] for l in self.rows() if l.strip().startswith("KE:")]
        return k[-1] if k else None
    def send(self, b, t=0.7): os.write(self.fd, b); self.pump(t, True)
    def key(self, b):
        self.send(b"x", 0.5)
        self.send(b, 0.6)
        return self.ke()
    def quit(self):
        self.send(MENU + b"q", 1.5)
        try: os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError: pass

# 1. every Alt+Enter encoding, every shift_enter setting: ESC CR, never a bare CR
for setting in ("auto", "enter", "newline"):
    r = Run(f'shift_enter = "{setting}"')
    check(f"[{setting}] agent tab runs", r.key(b"a") == "61")
    check(f"[{setting}] standard terminal Alt+Enter (ESC CR) -> ESC CR", r.key(b"\x1b\r") == "1b0d")
    check(f"[{setting}] kitty Alt+Enter -> ESC CR", r.key(b"\x1b[13;3u") == "1b0d")
    check(f"[{setting}] kitty Alt+Enter with event type -> ESC CR", r.key(b"\x1b[13;3:1u") == "1b0d")
    check(f"[{setting}] modifyOtherKeys Alt+Enter -> ESC CR", r.key(b"\x1b[27;3;13~") == "1b0d")
    check(f"[{setting}] ESC LF reaches the agent whole, not as Enter", r.key(b"\x1b\n") == "1b0a")
    check(f"[{setting}] plain Enter still submits (CR)", r.key(b"\r") == "0d")
    r.quit()

# 2. hint: terminal without enhanced keys sees it once; Shift+Enter is LF where reported
r = Run()
check("hint shown in a standard terminal", r.hint_seen())
check("Info wording, standard", (r.send(MENU + b"i", 0.8), "New line: Alt+Enter (Shift+Enter looks like Enter in this terminal)" in r.text())[1])
r.quit()

# 3. terminal that answers the kitty query: no hint, Info says Shift or Alt, Shift+Enter is LF
r = Run(reply=True)
check("no hint when the terminal speaks the kitty protocol", not r.hint_seen())
check("Shift+Enter is LF in an agent tab", r.key(b"\x1b[13;2u") == "0a")
r.send(MENU + b"i", 0.8)
check("Info wording, enhanced", "New line: Shift+Enter or Alt+Enter" in r.text())
r.quit()

# 4. hint is remembered away once Alt+Enter was used (state.toml), and shown once per session
r = Run()
check("hint shown on a fresh state", r.hint_seen())
r.key(b"\x1b\r")
r.quit()
check("newline_hint_seen saved", "newline_hint_seen = true" in open(CFGDIR + "/state.toml").read())
r = Run(keep_state=True)
check("no hint on the next session", not r.hint_seen())
r.quit()

shutil.rmtree(CFGDIR, ignore_errors=True); shutil.rmtree(WS, ignore_errors=True)
sys.exit(1 if fails else 0)
