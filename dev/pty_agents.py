# usage: python dev/pty_agents.py <scratch-dir-with-test.toml> local   (needs pyte)
# Uses fake agent commands (echo) so no real agent is started.
import os, pty, sys, time, select, struct, fcntl, termios, shutil, pyte
S = sys.argv[1]
MENU = b"\x00"
cfgfile = S + "/agents.toml"
statefile = S + "/state.toml"   # sits next to the config file
open(cfgfile, "w").write('[agents.claude]\ncommand = "echo FAKE_CLAUDE_STARTED"\ncommands = ["echo SECOND_CMD"]\n')
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
check("asks on first open", "Start an agent in agentws" in r.text() and "Claude" in r.text())
check("lists several commands", all(x in r.text() for x in ("echo FAKE_CLAUDE_STARTED", "echo SECOND_CMD", "codex", "Other command", "Not now", "Never here")))
r.send(b"\r", 1.5)
check("Enter starts it in its own tab", "FAKE_CLAUDE_STARTED" in r.text() and "claude" in r.scr.display[1])
check("choice is remembered", 'agent = "claude"' in state() and "FAKE_CLAUDE_STARTED" in state())
r.quit()

# 2. second time: starts without asking
r = Run(proj)
check("starts remembered agent without asking", "Start an agent in" not in r.text() and "FAKE_CLAUDE_STARTED" in r.text())
check("says what it ran and how to change it", "started: echo FAKE_CLAUDE_STARTED" in r.text() and "then A, changes it" in r.text())
r.quit()

# 2b. Down+Enter picks the second command; it is remembered and recents show next time
os.remove(statefile)
r = Run(proj)
r.send(b"\x1b[B\r", 1.5)
check("Down+Enter starts the second command", "SECOND_CMD" in r.text() and "FAKE_CLAUDE_STARTED" not in r.text().replace("echo FAKE_CLAUDE_STARTED", ""))
check("second command remembered", "echo SECOND_CMD" in state())
r.quit()
r = Run(proj)
check("remembered one starts without asking", "Start an agent in" not in r.text())
r.send(MENU + b"a", 0.8)
check("menu: remembered command is first and marked", "last used here" in r.text())
lines = r.text().split("\n")
rows = [l for l in lines if "Claude  echo" in l]
check("menu: remembered row listed before the config one", len(rows) >= 2 and "SECOND_CMD" in rows[0] and "last used here" in rows[0])
r.send(b"\x1b", 0.5)
r.quit()

# 3. other command: visible field, real cursor follows the text
os.remove(statefile)
r = Run(proj)
r.send(b"o", 0.6)
check("field has a label and a border", "Command:" in r.text() and "\u250c" in r.text() and "Esc back to the list" in r.text())
y = r.scr.cursor.y
row = r.scr.display[y]
check("cursor sits inside the field", "\u2502" in row and not r.scr.cursor.hidden)
x0 = r.scr.cursor.x
r.send(b"echo EDITED_CMD", 0.5)
check("typed text shows in the field", "echo EDITED_CMD" in r.scr.display[y])
check("cursor moved with the text", r.scr.cursor.x == x0 + 15 and r.scr.cursor.y == y)
r.send(b"\x1b[D\x1b[D", 0.3)
check("left arrow moves the cursor", r.scr.cursor.x == x0 + 13)
r.send(b"\x1b[F\x7f", 0.3)
check("end then backspace", r.scr.cursor.x == x0 + 14 and "echo EDITED_CM" in r.scr.display[y] and "EDITED_CMD" not in r.scr.display[y])
r.send(b"D", 0.3)
r.send(b"\x1b", 0.5)
check("Esc in the field goes back to the list", "Start an agent in" in r.text() and "Command:" not in r.text())
r.send(b"o", 0.4)
r.send(b"\r", 0.5)
check("empty command refused", "Type a command first" in r.text())
r.send(b"echo EDITED_CMD\r", 1.5)
check("edited command runs", "EDITED_CMD" in r.text() and "Start an agent in" not in r.text())
check("edited command saved", "echo EDITED_CMD" in state())
r.quit()
r = Run(proj)
check("edited command is what starts next time", "EDITED_CMD" in r.text() and "Start an agent in" not in r.text())
r.send(MENU + b"a", 0.8)
check("recent commands appear in the list", "echo SECOND_CMD" in r.text() and "echo EDITED_CMD" in r.text())
r.send(b"\x1b", 0.5)
r.quit()

# 4. never here
os.remove(statefile)
r = Run(proj)
r.send(b"x", 1.0)
check("never: no agent tab", "FAKE_CLAUDE_STARTED" not in r.text() and "claude" not in r.scr.display[1])
check("never is remembered", 'agent = "none"' in state())
r.quit()
r = Run(proj)
check("never: does not ask again", "Start an agent in" not in r.text())
r.send(MENU + b"a", 0.8)
check("menu 'Start an agent' still works", "Start an agent in" in r.text())
r.send(b"\r", 1.5)
check("and starts it", "FAKE_CLAUDE_STARTED" in r.text())
r.quit()

# 5. not now: asks again next time, nothing saved
os.remove(statefile)
r = Run(proj)
r.send(b"n", 0.8)
check("not now: closes the prompt, no agent", "Start an agent in" not in r.text() and "FAKE_CLAUDE_STARTED" not in r.text())
check("not now: nothing saved", state() == "")
r.quit()
r = Run(proj)
check("not now: asks again next time", "Start an agent in" in r.text())
r.send(b"\x1b", 0.8)
check("Esc also means not now", "Start an agent in" not in r.text())
r.quit()

# 6. workspace without markers: no prompt
r = Run(plain)
check("plain folder: no prompt", "Start an agent in" not in r.text())
r.send(MENU + b"a", 0.8)
check("plain folder: menu offers the agents anyway", "Start an agent in" in r.text() and "Codex" in r.text())
r.send(b"\x1b", 0.5)
r.quit()
sys.exit(1 if fails else 0)
