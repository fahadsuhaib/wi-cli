# usage: python dev/pty_tabdir.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# Per-tab base folders: new tab in a folder, recent folders, folder completion, change folder
# (only at an idle prompt), labels, header, git per folder, agent choice per folder.
# Uses fake agent commands (echo) so no real agent is started. Remote mode builds its fixtures
# under /home/dev/tabdir-test on the box and removes them (and its own tmux server, socket tabdirtest) afterwards.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, pyte
S, mode = sys.argv[1], sys.argv[2]
remote = mode == "remote"
MENU = b"\x00"  # Ctrl-Space
S = os.path.realpath(S)
COLS, ROWS = (240 if remote else 150), 30  # remote labels carry the hostname title, so many tabs need width
SLOW = 1.8 if remote else 1.0
CFGDIR = S + "/tabdir_cfg_" + mode
STATE = CFGDIR + "/state.toml"
shutil.rmtree(CFGDIR, ignore_errors=True); os.makedirs(CFGDIR)
base_cfg = open(S + "/test.toml").read()
open(CFGDIR + "/config.toml", "w").write(base_cfg + '\n[agents.claude]\ncommand = "echo FAKE_CLAUDE_STARTED"\n')
X = "/home/dev/tabdir-test" if remote else S + "/tabdir"
START = "/home/dev" if remote else X + "/ws"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
# Own tmux server on the box, so this test can run next to other remote tests.
env = dict(os.environ, WICLI_CONFIG=CFGDIR + "/config.toml", TERM="xterm-256color", WICLI_TMUX_SOCKET="tabdirtest")
# A plain shell with an empty home: default prompts (Ubuntu's .bashrc, zsh themes) change the title
# and can be slow to start, which these checks do not want.
os.makedirs(CFGDIR + "/home_plain", exist_ok=True)
env.update(SHELL="/bin/bash", HOME=CFGDIR + "/home_plain")

def sh(script, check=True):
    if remote:
        return subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "bash", "-c", script], capture_output=True, text=True)
    return subprocess.run(["bash", "-c", script], capture_output=True, text=True)

FIXTURE = f'''
set -e
rm -rf "{X}"
mkdir -p "{X}/ws" "{X}/alpha" "{X}/beta" "{X}/proj" "{X}/plain" "{X}/comp/apple" "{X}/comp/avocado" "{X}/comp/banana"
touch "{X}/comp/afile.txt"
echo "# project" > "{X}/proj/CLAUDE.md"
for r in alpha beta; do
  cd "{X}/$r"
  git init -q -b br-$r
  echo one > $r-file.txt
  git add $r-file.txt
  git -c user.email=t@e -c user.name=t commit -q -m init
  echo two >> $r-file.txt
done
'''
r = sh(FIXTURE)
if r.returncode != 0:
    print("FAIL fixture setup:", r.stderr); sys.exit(1)

pid, fd = pty.fork()
if pid == 0:
    os.chdir(START if not remote else "/")
    os.execve(WI, ["wi", "test"] if remote else ["wi"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
scr = pyte.Screen(COLS, ROWS); st = pyte.ByteStream(scr)

def pump(t):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try: st.feed(os.read(fd, 65536))
            except OSError: return
def line(y): return "".join(scr.buffer[y][x].data for x in range(COLS)).rstrip()
def lines(): return [line(y) for y in range(ROWS)]
def text(): return "\n".join(lines())
def send(b, t=0.8):
    try:
        os.write(fd, b)
    except OSError:
        import traceback; print("FAIL wi exited unexpectedly at line %d; last screen:\n" % traceback.extract_stack()[-2].lineno + text()); cleanup(); sys.exit(1)
    pump(t * SLOW)
def type_path(p, t=0.5): send(b"\x15" + p.encode(), t)

def cleanup():
    if remote:
        sh(f"rm -rf '{X}'; tmux -L tabdirtest kill-server 2>/dev/null; true")
    else:
        shutil.rmtree(X, ignore_errors=True)
    sh("pkill -x -f 'sleep 31'; true")
    shutil.rmtree(CFGDIR, ignore_errors=True)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond:
        fails.append(name)
        try: print("--- screen at the failure ---\n" + text() + "\n---")
        except Exception: pass
def state(): return open(STATE).read() if os.path.exists(STATE) else ""
def where():
    send(b"\x15clear\r", 0.6)
    send(b"echo WHERE=$(pwd)\r", 0.8)
    for l in lines():
        if "WHERE=/" in l and "echo" not in l:
            return l.split("WHERE=", 1)[1].strip()
    return None
def new_tab_in(path, t=1.5):
    send(MENU + b"f", 0.8)
    type_path(path)
    send(b"\r", t)

pump(3.0 * SLOW)
# 1. first tab starts in the workspace folder
w = where()
check("first tab starts in the workspace folder", w == START)
check("one tab: no tab strip", "shell 1" not in line(1))

# 2. new tab in another folder
send(MENU + b"f", 0.8)
check("folder prompt shows, starting in the tab's folder",
      "new tab in folder:" in text() and (START + "/") in line(ROWS - 2))
check("no recent folders yet", START + "/" not in "\n".join(lines()[:ROWS - 3]))
type_path(X + "/alpha")
send(b"\r", 1.5)
w = where()
check("new tab in another folder starts there (pwd)", w == X + "/alpha")
check("tab label shows the folder name", "alpha" in line(1))
check("header shows the active tab's folder", "alpha" in line(0) and "tabdir" in line(0))
check("header shows the branch of that folder", "br-alpha" in line(0))

new_tab_in(X + "/beta")
w = where()
check("second folder tab starts there", w == X + "/beta")
check("labels show both folders", "alpha" in line(1) and "beta" in line(1))
check("header follows to the other folder and branch", "beta" in line(0) and "br-beta" in line(0) and "br-alpha" not in line(0))

# 3. each tab keeps its own folder
send(MENU + b"2", 0.8)
check("tab 2 still reports its own pwd", where() == X + "/alpha")
check("header switched back to tab 2's folder", "alpha" in line(0) and "br-alpha" in line(0))
send(MENU + b"3", 0.8)
check("tab 3 still reports its own pwd", where() == X + "/beta")
send(MENU + b"1", 0.8)
check("tab 1 still in the workspace folder", where() == START)

# 4. new shell tab (N) starts in the active tab's folder
send(MENU + b"2", 0.8)
send(MENU + b"n", 1.5)
check("New shell tab starts in the active tab's folder", where() == X + "/alpha")
send(MENU + b"c", 1.2)   # close it again

# 5. git view follows the tab's folder
send(MENU + b"2", 0.8)
send(MENU + b"g", 1.5)
t = text()
check("git view in tab 2 shows alpha's repository", "alpha-file.txt" in t and "beta-file.txt" not in t)
send(b"\x1b", 1.0)
send(MENU + b"3", 0.8)
send(MENU + b"g", 1.5)
t = text()
check("git view in tab 3 shows beta's repository", "beta-file.txt" in t and "alpha-file.txt" not in t)
send(b"\x1b", 1.0)

# 6. folder completion: directories only
send(MENU + b"f", 0.8)
type_path(X + "/comp/a")
send(b"\t", 1.0)
t = text()
check("Tab lists matching folders", "apple/" in t and "avocado/" in t)
check("completion never offers files", "afile.txt" not in t)
send(b"p\t", 1.0)
check("Tab completes a unique folder with a slash", (X + "/comp/apple/") in line(ROWS - 2))
send(b"\x1b", 0.8)
check("Esc closes the prompt", "new tab in folder:" not in text())
send(MENU + b"f", 0.8)
type_path(X + "/comp/")
send(b"\t", 1.0)
t = text()
check("banana listed, hidden and file entries are not", "banana/" in t and "afile.txt" not in t)
send(b"\x1b", 0.8)

# 7. recent folders: listed, selectable with arrows and with the mouse
send(MENU + b"f", 0.8)
ls = lines()
ib = next((i for i, l in enumerate(ls) if l.strip() == X + "/beta"), None)
ia = next((i for i, l in enumerate(ls) if l.strip() == X + "/alpha"), None)
check("recent folders are listed, most recent first", ib is not None and ia is not None and ib < ia)
send(b"\x1b[B\x1b[B", 0.6)
send(b"\r", 1.5)
check("arrows + Enter open the selected recent folder", where() == X + "/alpha")
send(MENU + b"f", 0.8)
ls = lines()
ib = next((i for i, l in enumerate(ls) if l.strip() == X + "/beta"), None)
if ib is not None:
    send(f"\x1b[<0;12;{ib + 1}M\x1b[<0;12;{ib + 1}m".encode(), 1.5)
check("clicking a recent folder opens a tab there", where() == X + "/beta")
check("recent folders are stored in the state file", "recent" in state() and (X + "/beta") in state())

# 8. change folder at an idle prompt
send(MENU + b"h", 0.8)
check("change folder prompt shows at an idle prompt", "change folder to:" in text())
type_path(X + "/plain")
send(b"\r", 1.5)
check("change folder moves the shell (pwd)", where() == X + "/plain")
check("label and header follow the change", "plain" in line(0) and "plain" in line(1))

# 9. refused while a program runs
send(b"\x15clear\r", 0.6)
send(b"sleep 31\r", 0.8)
send(MENU + b"h", 1.0)
t = text()
check("busy: change is refused with a message naming the program", "sleep is running" in t and "new tab in that folder" in t)
check("busy: no folder prompt", "change folder to:" not in t)
check("busy: nothing was typed into the program", "cd --" not in t)
pg = sh("pgrep -x -f 'sleep 31'")
check("busy: the program is still running", pg.returncode == 0)
send(b"\x03", 0.8)
send(b"echo AFTER_$((1+2))\r", 0.8)
t = text()
check("busy: the prompt got no stray input afterwards", "AFTER_3" in t and "cd --" not in t and "sleep 31" in t)
check("busy: folder unchanged", where() == X + "/plain")

# 10. agent workspaces per folder
new_tab_in(X + "/proj", 1.5)
t = text()
check("folder with CLAUDE.md asks which agent", "start an agent" in t.lower() and "echo FAKE_CLAUDE_STARTED" in t)
send(b"\r", 1.5)
t = text()
check("choice starts the agent in the new tab", "FAKE_CLAUDE_STARTED" in t and "claude" in line(1))
check("choice is remembered per folder", 'agent = "claude"' in state() and (X + "/proj") in state())
new_tab_in(X + "/plain", 1.5)
check("folder without markers does not ask", "start an agent" not in text().lower())
new_tab_in(X + "/proj", 2.0)
t = text()
check("remembered folder starts the agent without asking", "start an agent" not in t.lower() and "FAKE_CLAUDE_STARTED" in t)

# quit
send(MENU + (b"e" if remote else b"q"), 1.5)
try:
    p, _ = os.waitpid(pid, os.WNOHANG)
except ChildProcessError:
    p = pid
check("quit exits", p != 0)

cleanup()
sys.exit(1 if fails else 0)
