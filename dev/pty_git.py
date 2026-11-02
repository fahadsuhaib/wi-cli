# usage: python dev/pty_git.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# Remote mode turns the box's home directory (the login directory) into a throwaway git repo for
# the test and removes it again afterwards.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, pyte
S, mode = sys.argv[1], sys.argv[2]
MENU = b"\x00"  # Ctrl-Space
remote = mode == "remote"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
G = S + "/gitfix"          # local fixtures live here, never in the repo
REPO, NOGIT = G + "/repo", G + "/nogit"

def sh(script, cwd=None):
    if remote:
        subprocess.run(["docker", "exec", "-u", "dev", "-w", "/home/dev", "devbox-test", "bash", "-c", script], check=True)
    else:
        subprocess.run(["bash", "-c", script], check=True, cwd=cwd or REPO)

CLEAN = "rm -rf .git a.txt b.txt c.txt"
FIXTURE = r'''
set -e
rm -rf .git a.txt b.txt c.txt
git init -q -b testbr
git config user.email t@example.com; git config user.name t
printf '/*\n!/a.txt\n!/b.txt\n!/c.txt\n' > .git/info/exclude
seq 1 40 | sed 's/^/line /' > a.txt
seq 1 40 | sed 's/^/bee /' > b.txt
git add a.txt b.txt && git commit -q -m init
seq 1 40 | sed 's/^/line /' | sed 's/^line \([0-9]*[02468]\)$/LINE \1/' > a.txt
seq 1 40 | sed 's/^/bee /' | sed 's/^bee \([0-9]*[02468]\)$/BEE \1/' > b.txt
git add b.txt
printf 'hello\n' > c.txt
'''
os.makedirs(REPO, exist_ok=True); shutil.rmtree(NOGIT, ignore_errors=True); os.makedirs(NOGIT)
shutil.rmtree(REPO); os.makedirs(REPO)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

HEX = {"cyan": "00cdcd", "green": "00cd00", "red": "cd0000"}  # pyte reports 256-colour SGR 36/32/31 as hex
def is_col(c, name): return c.fg in (name, HEX[name])

class Session:
    def __init__(self, cwd, env_extra=None):
        env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color", **(env_extra or {}))
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            if not remote: os.chdir(cwd)
            os.execve(WI, ["wi", "test"] if remote else ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
        self.scr = pyte.Screen(100, 30); self.st = pyte.ByteStream(self.scr)
        self.pump(3.0)
    def pump(self, t, until=None):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return False
            if until and until(): return True
        return until is None
    def text(self): return "\n".join(l.rstrip() for l in self.scr.display)
    def send(self, b, wait=0.5): os.write(self.fd, b); self.pump(wait)
    def row(self, prefix):
        for i, l in enumerate(self.scr.display):
            if l.startswith(prefix): return i
        return None
    def cell(self, prefix):
        r = self.row(prefix)
        return None if r is None else self.scr.buffer[r][0]
    def quit(self):
        self.send(MENU + b"q", 1.5)
        try: os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError: pass

# ---- not a repo: status-bar message, no tab, no branch in the header
if remote: sh("rm -rf .git")
s = Session(NOGIT)
check("no branch in header outside a repo", "(" not in s.scr.display[0])
s.send(MENU + b"g", 1.5)
bar = s.scr.display[-1]
check("non-repo: status bar says not a git repository", "not a git repository" in bar)
check("non-repo: no tab opened (no tab strip)", "1 shell" not in s.scr.display[1] and " 2 git" not in s.scr.display[1])
check("non-repo: no diff view", "Branch" not in s.text())
s.quit()

if not remote:
    s = Session(NOGIT, {"PATH": "/nonexistent"})
    s.send(MENU + b"g", 1.5)
    check("git missing: status bar says so", "not installed" in s.scr.display[-1])
    s.quit()

# ---- a repo with a modified, a staged and an untracked file
sh(FIXTURE)
s = Session(REPO)
check("header shows the branch", "(testbr)" in s.scr.display[0])
s.send(MENU + b"g", 2.0)
t = s.text()
check("git tab opened", "git" in s.scr.display[1])
check("shows branch", "Branch  testbr" in t)
check("status: unstaged modified file", " M a.txt" in t)
check("status: staged file", "M  b.txt" in t)
check("status: untracked file", "?? c.txt" in t)
check("diff header present", s.row("diff --git a/a.txt b/a.txt") is not None)
c = s.cell("diff --git a/a.txt")
check("diff --git line is bold", c is not None and c.bold)
c = s.cell("@@ ")
check("hunk line is cyan", c is not None and is_col(c, "cyan"))
c = s.cell("+LINE 2")
check("added line is green", c is not None and is_col(c, "green"))
c = s.cell("-line 2")
check("removed line is red", c is not None and is_col(c, "red"))
c = s.cell(" line 1")
check("context line is uncoloured", c is not None and c.fg == "default")
check("hint mentions N/P", "N/P" in s.scr.display[-1])

# N / P jump between files (a.txt first, then b.txt)
s.send(b"n", 0.5)
check("N jumps to the first file", s.scr.display[2].startswith("diff --git a/a.txt"))
s.send(b"N", 0.5)
check("N jumps to the next file", s.scr.display[2].startswith("diff --git a/b.txt"))
s.send(b"p", 0.5)
check("P jumps back", s.scr.display[2].startswith("diff --git a/a.txt"))

# R reloads after a further edit, keeping the position
check("edit not visible before reload", "FIRSTCHANGE" not in s.text())
sh("sed '1s/.*/FIRSTCHANGE/' a.txt > a.tmp && mv a.tmp a.txt")
s.send(b"r", 1.5)
check("R reloads and shows the new edit", "+FIRSTCHANGE" in s.text())
s.send(b"\x1b[F", 0.5)
check("End shows the untracked list", "Untracked files (1)" in s.text() and "  c.txt" in s.text())
s.send(b"\x1b[H", 0.4)

# reopening from the menu reuses the tab
s.send(b"\x1b", 0.6)
check("Esc returns to the shell", "Branch" not in s.text())
s.send(MENU + b"g", 1.5)
check("menu Git again reuses the one git tab", s.scr.display[1].count("git") == 1 and "Branch  testbr" in s.text())
s.send(b"\x1b", 0.4)
s.quit()

if remote: sh(CLEAN)
sys.exit(1 if fails else 0)
