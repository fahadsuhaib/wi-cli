# usage: python dev/pty_agents.py <scratch-dir-with-test.toml> local   (needs pyte)
# Uses fake agent commands (echo) so no real agent is started.
import os, pty, sys, time, select, struct, fcntl, termios, shutil, pyte
S = sys.argv[1]
MENU = b"\x00"
cfgfile = S + "/agents.toml"
statefile = S + "/state.toml"   # sits next to the config file
open(cfgfile, "w").write('[agents.claude]\ncommand = "echo FAKE_CLAUDE_STARTED"\n')
proj = S + "/agentws"; plain = S + "/plainws"
for d in (proj, plain):
    shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
open(proj + "/CLAUDE.md", "w").write("# project\n")
env = dict(os.environ, WICLI_CONFIG=cfgfile, TERM="xterm-256color")
BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")

class Run:
    def __init__(self, cwd):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd); os.execve(BIN, ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
        self.scr = pyte.Screen(100, 30); self.st = pyte.ByteStream(self.scr)
        self.pump(2.2)
    def pump(self, t):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return
    def text(self): return "\n".join(l.rstrip() for l in self.scr.display)
    def send(self, b, t=0.8): os.write(self.fd, b); self.pump(t)
    def quit(self):
        self.send(MENU + b"q", 1.2)
        try: os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError: pass

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
def state(): return open(statefile).read() if os.path.exists(statefile) else ""
if os.path.exists(statefile): os.remove(statefile)

# 1. first time in an agent workspace: asks, with the configured command prefilled
r = Run(proj)
check("asks on first open", "start an agent" in r.text() and "Claude Code" in r.text())
check("prefills the configured command", "echo FAKE_CLAUDE_STARTED" in r.text())
r.send(b"\r", 1.5)
check("Enter starts it in its own tab", "FAKE_CLAUDE_STARTED" in r.text() and "claude" in r.scr.display[1])
check("choice is remembered", 'agent = "claude"' in state() and "FAKE_CLAUDE_STARTED" in state())
r.quit()

# 2. second time: starts without asking
r = Run(proj)
check("starts remembered agent without asking", "start an agent" not in r.text() and "FAKE_CLAUDE_STARTED" in r.text())
check("says what it ran and how to change it", "started: echo FAKE_CLAUDE_STARTED" in r.text() and "then A, changes it" in r.text())
r.quit()

# 3. edit the command: change and remember it
os.remove(statefile)
r = Run(proj)
r.send(b"e", 0.5)
r.send(b"\x15echo EDITED_CMD", 0.5)
check("editing shows the text being typed", "echo EDITED_CMD" in r.text())
r.send(b"\r", 1.5)
check("edited command runs", "EDITED_CMD" in r.text())
r.quit()
r = Run(proj)
check("edited command is what starts next time", "EDITED_CMD" in r.text() and "start an agent" not in r.text())
r.quit()

# 4. never here
os.remove(statefile)
r = Run(proj)
r.send(b"x", 1.0)
check("never: no agent tab", "FAKE_CLAUDE_STARTED" not in r.text() and "claude" not in r.scr.display[1])
check("never is remembered", 'agent = "none"' in state())
r.quit()
r = Run(proj)
check("never: does not ask again", "start an agent" not in r.text())
r.send(MENU + b"a", 0.8)
check("menu 'Start an agent' still works", "start an agent" in r.text())
r.send(b"\r", 1.5)
check("and starts it", "FAKE_CLAUDE_STARTED" in r.text())
r.quit()

# 5. not now: asks again next time, nothing saved
os.remove(statefile)
r = Run(proj)
r.send(b"n", 0.8)
check("not now: closes the prompt, no agent", "start an agent" not in r.text() and "FAKE_CLAUDE_STARTED" not in r.text())
check("not now: nothing saved", state() == "")
r.quit()
r = Run(proj)
check("not now: asks again next time", "start an agent" in r.text())
r.send(b"\x1b", 0.8)
check("Esc also means not now", "start an agent" not in r.text())
r.quit()

# 6. workspace without markers: no prompt
r = Run(plain)
check("plain folder: no prompt", "start an agent" not in r.text())
r.send(MENU + b"a", 0.8)
check("plain folder: menu offers the agents anyway", "start an agent" in r.text() and "Codex" in r.text())
r.send(b"\x1b", 0.5)
r.quit()
sys.exit(1 if fails else 0)
