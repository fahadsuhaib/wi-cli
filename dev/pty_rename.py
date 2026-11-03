# usage: python dev/pty_rename.py <scratch-dir-with-test.toml> local   (needs pyte)
# Rename a tab: menu action, single click on the active tab, double-click on another tab; prefilled
# name, cursor editing, paste, Esc cancels, empty name resets, long names, the name survives the
# program setting its own title, and an agent tab with a custom name still gets its new-line keys.
import os, pty, re, sys, time, select, struct, fcntl, termios, shutil, pyte
S = os.path.realpath(sys.argv[1])
MENU = b"\x00"
CFGDIR = S + "/rename_cfg"
shutil.rmtree(CFGDIR, ignore_errors=True); os.makedirs(CFGDIR)
WS = S + "/rename_ws"
shutil.rmtree(WS, ignore_errors=True); os.makedirs(WS)
AWS = S + "/rename_agent_ws"
shutil.rmtree(AWS, ignore_errors=True); os.makedirs(AWS)
open(AWS + "/CLAUDE.md", "w").write("# project\n")
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
open(CFGDIR + "/ke.pl", "w").write(r'''
system("stty raw -echo"); $| = 1;
while (sysread(STDIN, $b, 256)) { last if $b eq "q"; print "KE:" . unpack("H*", $b) . "\r\n"; }
system("stty sane");
''')
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, ws, agent=False, reply=False):
        cfg = CFGDIR + "/config.toml"
        extra = f'\n[defaults]\nagent_start = "always"\n[agents.claude]\ncommand = "perl {CFGDIR}/ke.pl"\n' if agent else '\n[defaults]\nagent_start = "never"\n'
        open(cfg, "w").write(open(S + "/test.toml").read() + extra)
        if os.path.exists(CFGDIR + "/state.toml"): os.remove(CFGDIR + "/state.toml")
        env = dict(os.environ, WICLI_CONFIG=cfg, TERM="xterm-256color", WICLI_TMUX_SOCKET="renametest")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(ws); os.execve(WI, ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
        self.scr = pyte.Screen(110, 30); self.st = pyte.ByteStream(self.scr)
        self.pump(0.4)
        if reply: os.write(self.fd, b"\x1b[?1u")
        self.pump(2.6)
    def pump(self, t):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)).rstrip() for y in range(self.scr.lines)]
    def text(self): return "\n".join(self.rows())
    def strip(self): return self.rows()[1]
    def send(self, b, t=0.6): os.write(self.fd, b); self.pump(t)
    def click(self, col, row=2, n=1):
        for _ in range(n):
            os.write(self.fd, f"\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m".encode()); self.pump(0.05 if n > 1 else 0.5)
        self.pump(0.5)
    def tab_col(self, n):
        m = re.search(rf"(?:^|\s){n} \S", self.strip())
        return None if not m else m.start() + 3  # 1-based column of the number
    def menu_rename(self): self.send(MENU + b" rename\r", 0.8)
    def prompt_line(self):
        return next((l for l in self.rows() if "rename tab:" in l), None)
    def ke(self):
        k = [l.strip()[3:] for l in self.rows() if l.strip().startswith("KE:")]
        return k[-1] if k else None
    def key(self, b):
        self.send(b"x", 0.5); self.send(b, 0.6)
        return self.ke()
    def quit(self):
        self.send(MENU + b"q", 1.5)
        try: os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError: pass
    def cursor_col(self): return self.scr.cursor.x

r = Run(WS)
r.send(b"echo ONE$((1+1))\n", 1)
r.menu_rename()
pl = r.prompt_line()
check("menu 'rename' opens the inline prompt", pl is not None)
check("prompt starts with the current name and has the real cursor at its end", pl is not None and r.cursor_col() == len(pl) and r.scr.cursor.y == 28 and not r.scr.cursor.hidden)
r.send(b"\x7f" * 40 + b"build\r", 0.8)
check("saved name is shown (Info)", (r.send(MENU + b"i", 0.8), "tab: build" in r.text())[1])
r.send(b"\x1b", 0.4)
r.send(MENU + b"n", 1.5)
r.send(b"echo TWO$((2+2))\n", 1)
st = r.strip()
check("strip shows the custom name and the automatic one", re.search(r"1 build", st) is not None and r.tab_col(2) is not None)

# Esc cancels (single click on the active tab 2 opens the prompt)
r.click(r.tab_col(2))
check("single click on the active tab opens the prompt, prefilled", r.prompt_line() is not None)
r.send(b"zz\x1b", 0.6)
check("Esc cancels: no change", r.prompt_line() is None and "zz" not in r.strip())

# single click on an inactive tab only switches
r.click(r.tab_col(1))
check("click on an inactive tab switches without a prompt", r.prompt_line() is None and "ONE2" in r.text())

# click the (now active) tab 1 -> prompt with "build"; empty name resets
r.click(r.tab_col(1))
pl = r.prompt_line()
check("prompt is prefilled with the custom name", pl is not None and pl.rstrip().endswith("build"))
r.send(b"\x7f" * 5 + b"\r", 0.8)
check("empty name goes back to the automatic title", "build" not in r.strip() and r.tab_col(1) is not None)

# double-click on an inactive tab (tab 2) opens the prompt for it
r.click(r.tab_col(2), n=2)
check("double-click on an inactive tab opens the prompt", r.prompt_line() is not None)
r.send(b"\x15", 0.1)
r.send(b"\x7f" * 40 + b"second\r", 0.8)
check("double-click rename applied to tab 2", re.search(r"2 second", r.strip()) is not None)

# the program setting its own title does not replace the name
r.send(b"printf '\\033]0;other\\007'\n", 1)
check("custom name survives the shell setting its own title", "2 second" in r.strip() and "other" not in r.strip())

# cursor editing: left, insert, home, delete
r.menu_rename()
r.send(b"\x1b[D\x1b[D" + b"X" + b"\x1b[H" + b"\x1b[3~", 0.6)
check("left/insert/home/delete edit in place", r.prompt_line() is not None and r.prompt_line().rstrip().endswith("ecoXnd"))
r.send(b"\x1b[F\x7f\r", 0.8)
check("end + backspace, Enter saves", "2 ecoXn" in r.strip())

# bracketed paste, and long names
r.menu_rename()
r.send(b"\x7f" * 40 + b"\x1b[200~" + b"p" * 60 + b"\x1b[201~", 0.6)
r.send(b"\r", 0.8)
check("paste works; long names are cut with an ellipsis in the strip", "…" in r.strip() and "ppppppppppp" in r.strip())
r.menu_rename()
check("the full (40 character) name is kept in the prompt", r.prompt_line() is not None and r.prompt_line().count("p") >= 40)
r.send(b"\x1b", 0.4)
r.quit()

# agent tab: a custom name does not change Shift+Enter / Alt+Enter handling
a = Run(AWS, agent=True, reply=True)
check("agent tab runs", a.key(b"a") == "61")
check("Shift+Enter is LF before renaming", a.key(b"\x1b[13;2u") == "0a")
a.menu_rename()
a.send(b"\x7f" * 40 + b"myagent\r", 0.8)
check("agent tab renamed (Info)", (a.send(MENU + b"i", 0.8), "tab: myagent" in a.text())[1])
a.send(b"\x1b", 0.4)
check("Shift+Enter is still LF after renaming", a.key(b"\x1b[13;2u") == "0a")
check("Alt+Enter is still ESC CR after renaming", a.key(b"\x1b\r") == "1b0d")
check("plain Enter still CR", a.key(b"\r") == "0d")
a.quit()
sys.exit(1 if fails else 0)
