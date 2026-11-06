# usage: python dev/pty_restore.py <scratch-dir-with-test.toml> local   (needs pyte)
# Reopening the tabs after a normal quit: three tabs (one renamed, one in a subfolder), quit from the
# menu, start again in the same folder: same tabs, names, active tab and the status note. Also
# `wi --fresh` (one tab, snapshot forgotten), `restore_tabs = false`, an old snapshot (30 days), a
# missing folder, a lone plain shell (nothing saved), closing the last tab (nothing saved), and a
# second workspace that keeps its own entry.
import os, pty, re, sys, time, select, struct, fcntl, termios, shutil, pyte
S = os.path.realpath(sys.argv[1])
MENU = b"\x00"
CFGDIR = S + "/restore_cfg"
shutil.rmtree(CFGDIR, ignore_errors=True); os.makedirs(CFGDIR)
WS = os.path.realpath(S) + "/restore_ws"
shutil.rmtree(WS, ignore_errors=True); os.makedirs(WS + "/sub"); os.makedirs(WS + "/gone")
WS2 = S + "/restore_ws2"
shutil.rmtree(WS2, ignore_errors=True); os.makedirs(WS2)
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
STATE = CFGDIR + "/state.toml"
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, ws, args=(), restore=True):
        cfg = CFGDIR + "/config.toml"
        open(cfg, "w").write(open(S + "/test.toml").read() + f'\n[defaults]\nagent_start = "never"\nrestore_tabs = {"true" if restore else "false"}\n')
        env = dict(os.environ, WICLI_CONFIG=cfg, TERM="xterm-256color", WICLI_TMUX_SOCKET="restoretest", SHELL="/bin/sh", PS1="$ ")
        env.pop("WICLI_RESTORE_TABS", None)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(ws); os.execve(WI, ["wi", *args], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
        self.scr = pyte.Screen(110, 30); self.st = pyte.ByteStream(self.scr)
        self.pump(2.5)
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
    def rename(self, name): self.send(MENU + b" rename\r", 0.8); self.send(b"\x15" + b"\x7f" * 40 + name.encode() + b"\r", 0.8)
    def quit(self):
        self.send(MENU + b"q", 1.5)
        try: os.waitpid(self.pid, 0)
        except ChildProcessError: pass
    def ntabs(self): return len(re.findall(r"(?:^|\s)\d \S", self.strip()))

def nsess(): return len(re.findall(r'^\[sessions\."', state(), re.M))
def state(): return open(STATE).read() if os.path.exists(STATE) else ""

# 1. three tabs, one renamed, active = 2, then quit from the menu
r = Run(WS)
r.send(b"echo A$((1+1))\n", 0.8)
r.rename("build")
r.send(MENU + b"n", 1.5); r.send(b"cd sub\n", 0.8)
r.send(MENU + b"n", 1.5); r.send(b"cd gone\n", 0.8)
check("three tabs open", r.ntabs() == 3)
r.send(MENU + b"1", 0.8)
r.send(MENU + b"2", 0.8)
r.quit()
check("quit saved the session", "[sessions." in state())
check("snapshot holds no screen text", "A2" not in state())

# 2. a normal start restores it
r = Run(WS)
st = r.strip()
check("same tab count", r.ntabs() == 3)
check("custom name kept", "build" in st)
check("status note shown", "reopened 3 tabs from last session" in r.text() and "restore_tabs = false" in r.text())
r.send(b"pwd\n", 0.8)
r.send(MENU + b"3", 0.8)
r.send(b"pwd\n", 0.8)
check("tab folders kept (3rd tab is in gone)", "3 gone" in r.strip() and "restore_ws/gone" in r.rows()[0])
r.quit()

# 3. --fresh: one tab, snapshot forgotten
r = Run(WS, args=["--fresh"])
check("--fresh starts with one tab", r.ntabs() <= 1 and "reopened" not in r.text())
check("--fresh clears the snapshot", "[sessions." not in state())
r.quit()
r = Run(WS)
check("next start has one tab too", r.ntabs() <= 1 and "reopened" not in r.text())
r.quit()

# 4. restore_tabs = false neither saves nor restores
r = Run(WS, restore=False)
r.send(MENU + b"n", 1.5)
r.quit()
check("restore_tabs=false saves nothing", "[sessions." not in state())
r = Run(WS)
check("and a later start has one tab", r.ntabs() <= 1)
r.send(MENU + b"n", 1.5)
r.quit()
check("saved again with the default config", "[sessions." in state())

# 5. a folder that no longer exists is skipped
r = Run(WS)
check("restored two tabs", r.ntabs() == 2)
r.send(MENU + b"n", 1.5); r.send(b"cd gone\n", 0.8)
r.quit()
os.rmdir(WS + "/gone")
r = Run(WS)
check("tab with the missing folder skipped", r.ntabs() == 2 and "reopened 2 tabs" in r.text())
r.quit()

# 6. a snapshot older than 30 days is ignored
s = state()
s2 = re.sub(r"saved = \d+", "saved = 1000", s)
check("snapshot has a saved time", s != s2)
open(STATE, "w").write(s2)
r = Run(WS)
check("old snapshot ignored", r.ntabs() <= 1 and "reopened" not in r.text())
r.quit()

# 7. a lone plain shell saves nothing; another workspace keeps its own entry
r = Run(WS2)
r.send(MENU + b"n", 1.5); r.rename("two")
r.quit()
r = Run(WS)
r.send(MENU + b"n", 1.5)
r.quit()
n_before = nsess()
r = Run(WS)
check("two workspaces both remembered", n_before == 2)
r.quit()
r = Run(WS2)
check("other workspace restored its own tabs", "two" in r.strip() and r.ntabs() == 2)
r.quit()

# 8. closing the last tab forgets the session
r = Run(WS2)
r.send(b"exit\n", 1.0)
r.send(b"exit\n", 1.5)
try: os.waitpid(r.pid, os.WNOHANG)
except ChildProcessError: pass
r = Run(WS2)
check("closing every tab left nothing to restore", r.ntabs() <= 1 and "reopened" not in r.text())
r.quit()
check("a lone plain shell saves nothing", nsess() <= 1 and WS2 not in state())

print("FAILED: " + ", ".join(fails) if fails else "ALL PASS")
sys.exit(1 if fails else 0)
