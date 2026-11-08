# usage: python dev/pty_gitdiff.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# The git diff viewer in the file explorer: a changed file opens to its diff in the detail pane (unified,
# line numbers, theme-tinted add/remove rows, header with +added -removed and hunk n/m), staged/unstaged
# split (two letters in the mark column, Tab switches), N/P hunks, D toggles diff/content, untracked and
# binary files, and the auto-refresh of an open diff after the file is edited. Remote mode builds the repo
# under /home/dev/gdtest on the box (own tmux socket) and removes it afterwards.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, pyte
S, mode = os.path.realpath(sys.argv[1]), sys.argv[2]
remote = mode == "remote"
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
X = "/home/dev/gdtest" if remote else S + "/gdtest"
P = X + "/proj"
H = S + "/gd_home_" + mode
CFG = S + "/gitdiff_" + mode + ".toml"
open(CFG, "w").write(open(S + "/test.toml").read() + "\n[defaults]\npreview_refresh_debounce_ms = 400\n")
shutil.rmtree(H, ignore_errors=True); os.makedirs(H)

def sh(script):
    if remote:
        return subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "bash", "-c", script], capture_output=True, text=True)
    return subprocess.run(["bash", "-c", script], capture_output=True, text=True)

FIXTURE = r'''
set -e
rm -rf "$X"; mkdir -p "$X/proj"
cd "$X/proj"
git init -q -b main
git config user.email t@example.com; git config user.name t
seq 1 40 | sed 's/^/line /' > a.txt
seq 1 40 | sed 's/^/bee /' > b.txt
seq 1 10 | sed 's/^/clean /' > clean.txt
printf 'one\n' > "sp ace.txt"
printf '\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01' > pic.bin
git add -A >/dev/null; git commit -q -m init
sed -e '2s/.*/LINE two/' -e '30s/.*/LINE thirty/' a.txt > a.tmp && mv a.tmp a.txt
sed -e '5s/.*/BEE five/' b.txt > b.tmp && mv b.tmp b.txt
git add b.txt
sed -e '35s/.*/BEE thirty-five/' b.txt > b.tmp && mv b.tmp b.txt
printf 'hello\nworld\n' > c.txt
printf 'new\n' > e.txt
git add e.txt
printf 'new\nmore\n' > e.txt
printf 'two\n' > "sp ace.txt"
printf '\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x02\0' > pic.bin
'''.replace("$X", X)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

SEL = "34487a"
HEX = {"cyan": "7dcfe0", "green": "8fdc7f", "red": "f07178"}  # diff_hunk, diff_add, diff_remove (dark theme)

class Run:
    def __init__(self, cols=120, rows=18):
        self.cols, self.rows_n = cols, rows
        env = {k: v for k, v in os.environ.items() if k != "COLORFGBG"}
        env.update(WICLI_CONFIG=CFG, TERM="xterm-256color", COLORTERM="truecolor", WICLI_TMUX_SOCKET="gdtest", SHELL="/bin/bash", HOME=H)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(P if not remote else "/")
            os.execve(WI, ["wi", "test"] if remote else ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.scr = pyte.Screen(cols, rows); self.st = pyte.ByteStream(self.scr)
        self.pump(3.5 if remote else 2.2)
        if remote:
            self.send(MENU + b"f", 0.6)
            self.send(b"\x15" + P.encode() + b"\r", 2.5)
    def pump(self, t, until=None):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return False
            if until and until(): return True
        return until is None
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)) for y in range(self.scr.lines)]
    def text(self): return "\n".join(l.rstrip() for l in self.rows())
    @property
    def div(self):
        for y in range(4, 8):
            xs = [x for x in range(20, self.cols - 20) if self.scr.buffer[y][x].data == "│"]
            if xs: return xs[0]
        return 48
    def left(self): return "\n".join(l[: self.div].rstrip() for l in self.rows()[3:])
    def right(self): return "\n".join(l[self.div + 1:].rstrip() for l in self.rows()[3:])
    def send(self, b, t=0.5): os.write(self.fd, b); self.pump(t)
    def wait(self, pat, t=8.0, side=None):
        get = {None: self.text, "r": self.right}[side]
        return self.pump(t, until=lambda: re.search(pat, get()) is not None)
    def bg(self, x, y): return self.scr.buffer[y][x].bg
    def fg(self, x, y): return self.scr.buffer[y][x].fg
    def sel_name(self):
        for y in range(4, self.rows_n - 2):
            if self.bg(10, y) == SEL: return self.rows()[y][: self.div]
        return None
    def goto(self, name, key=b"\x1b[B", max_steps=30):
        for _ in range(max_steps):
            s = self.sel_name()
            if s and re.search(r"(^|\s)" + re.escape(name) + r"(\s|$)", s): self.pump(0.6); return True
            self.send(key, 0.15)
        return False
    def right_row(self, pat):
        for y, l in enumerate(self.rows()):
            m = re.search(pat, l[self.div + 1:])
            if m: return y, self.div + 1 + m.start()
        return None
    def hint(self): return self.rows()[1]
    def status(self): return self.rows()[2]
    def quit(self):
        self.send(MENU + (b"e" if remote else b"q"), 1.5)

def main():
    sh("rm -rf " + X)
    r_ = sh(FIXTURE)
    if r_.returncode: print(r_.stderr); sys.exit(2)
    r = Run()
    r.send(MENU + b"x", 0.6)
    check("menu x opens the explorer", r.wait(r"explorer ·", 6) and r.wait(r"\d+ items", 8))
    r.pump(1.0)
    left = r.left()
    # ---- marks: staged / unstaged split
    check("mark column: unstaged-only file shows M", re.search(r"a\.txt\s.*\sM\s*$", [l for l in left.split("\n") if "a.txt" in l][0]) is not None)
    check("mark column: staged and unstaged both M shows a single M", re.search(r"b\.txt.*\sM\s*$", [l for l in left.split("\n") if "b.txt" in l][0]) is not None)
    check("mark column: added then modified shows AM", re.search(r"e\.txt.*\sAM\s*$", [l for l in left.split("\n") if "e.txt" in l][0]) is not None)
    check("mark column: untracked shows ?", re.search(r"c\.txt.*\s\?\s*$", [l for l in left.split("\n") if "c.txt" in l][0]) is not None)

    # ---- a changed file opens to its diff by itself
    check("select a.txt", r.goto("a.txt"))
    check("detail pane shows the diff header with counts and hunk position", r.wait(r"a\.txt · diff unstaged · \+2 -2 · hunk 1/2", 6, "r"))
    rt = r.right()
    check("hunk header row", "@@ -1,5 +1,5 @@" in rt and "@@ -27,7 +27,7 @@" in rt)
    check("removed and added rows with line numbers", re.search(r"\b2\s+- line 2\b", rt) is not None and re.search(r"\b2\s+\+ LINE two\b", rt) is not None)
    check("context row carries both numbers", re.search(r"\b1\s+1\s+line 1\b", rt) is not None)
    y, x = r.right_row(r"\+ LINE two")
    check("added row: green sign", r.fg(x, y) == HEX["green"])
    check("added row: tinted background (not the canvas)", r.bg(x + 4, y) not in ("1c1f26", "default"))
    y2, x2 = r.right_row(r"- line 2\b")
    check("removed row: red sign and a different tint than added", r.fg(x2, y2) == HEX["red"] and r.bg(x2 + 4, y2) != r.bg(x + 4, y))
    y3, x3 = r.right_row(r"@@ -1,5")
    check("hunk row: cyan", r.fg(x3, y3) == HEX["cyan"])
    y4, x4 = r.right_row(r"\b1\s+1\s+line 1\b")
    check("context row keeps the canvas background", r.bg(x4 + 12, y4) == "1c1f26")

    # ---- hunk navigation (detail focused with Enter)
    r.send(b"\r", 0.5)
    check("Enter focuses the detail and the hint shows the diff keys", "N/P" in r.hint() and "hunk" in r.hint())
    r.send(b"n", 0.5)
    check("N goes to hunk 2/2", "hunk 2/2" in r.right())
    check("second hunk header is near the top of the pane", r.right().split("\n")[2].lstrip().startswith("@@ -27,7") or r.right().split("\n")[3].lstrip().startswith("@@ -27,7") or "@@ -27,7" in "\n".join(r.right().split("\n")[:6]))
    r.send(b"p", 0.5)
    check("P goes back to hunk 1/2", "hunk 1/2" in r.right() and "@@ -1,5" in "\n".join(r.right().split("\n")[:6]))
    r.send(b"\x1b[B" * 3, 0.5)
    check("Down scrolls the diff", "@@ -1,5" not in "\n".join(r.right().split("\n")[:3]))
    r.send(b"\x1b", 0.5)
    check("Esc gives the keys back to the tree", "explorer ·" in r.rows()[0] and "Enter" in r.hint())

    # ---- D toggles diff and content
    r.send(b"d", 1.0)
    check("D shows the file content (line numbers, no diff header)", r.wait(r"\b1\s+line 1\b", 5, "r") and "diff unstaged" not in r.right())
    r.send(b"D", 1.0)
    check("D again shows the diff", r.wait(r"diff unstaged", 5, "r"))

    # ---- staged and unstaged both: Tab switches
    check("select b.txt", r.goto("b.txt"))
    check("b.txt opens to unstaged first and offers Tab", r.wait(r"b\.txt · diff unstaged · \+1 -1 · hunk 1/1 · Tab: staged", 6, "r") and "BEE thirty-five" in r.right())
    r.send(b"\r", 0.4)
    r.send(b"\t", 0.8)
    check("Tab shows the staged side", "diff staged" in r.right() and "BEE five" in r.right() and "thirty-five" not in r.right())
    r.send(b"\t", 0.8)
    check("Tab again returns to unstaged", "diff unstaged" in r.right() and "thirty-five" in r.right())
    r.send(b"\x1b", 0.4)

    # ---- untracked file: whole file as additions
    check("select c.txt", r.goto("c.txt"))
    check("untracked file shows as a new file with additions", r.wait(r"c\.txt · diff unstaged · \+2 -0", 6, "r") and "new file" in r.right() and "+ hello" in r.right())

    # ---- added in the index, modified after
    check("select e.txt", r.goto("e.txt"))
    check("e.txt has both sides", r.wait(r"e\.txt · diff unstaged · \+1 -0 · hunk 1/1 · Tab: staged", 6, "r"))

    # ---- a name with a space, binary file, clean file
    check("select sp ace.txt", r.goto("sp ace.txt"))
    check("path with a space diffs fine", r.wait(r"sp ace\.txt · diff unstaged · \+1 -1", 6, "r") and "+ two" in r.right())
    check("select pic.bin", r.goto("pic.bin"))
    check("binary file says it is not shown", r.wait(r"binary file differs", 6, "r"))
    check("select clean.txt", r.goto("clean.txt"))
    check("unchanged file shows its content, not a diff", r.wait(r"clean 10", 6, "r") and "diff " not in r.right().split("\n")[0])
    r.send(b"d", 0.6)
    check("D on an unchanged file says there are no changes", "no git changes" in r.status() or "no git changes" in r.text())

    # ---- auto refresh of an open diff
    check("select a.txt again", r.goto("a.txt"))
    check("a.txt diff on screen", r.wait(r"hunk 1/2", 6, "r"))
    r.send(b"\r", 0.4)
    r.send(b"n", 0.5)
    check("on hunk 2 before the edit", "hunk 2/2" in r.right())
    before = r.right()
    sh(f"cd {P} && sed -e '12s/.*/LINE twelve/' a.txt > a.tmp && mv a.tmp a.txt")
    check("the diff refreshes by itself with the new change (3 hunks)", r.wait(r"\+3 -3 · hunk \d/3", 14, "r") and "LINE twelve" in "".join(r.rows()) or r.wait(r"hunk \d/3", 6, "r"))
    r.pump(0.8)
    if os.environ.get("GD_DUMP"): print(r.text())
    check("scroll kept on the same hunk after the refresh (still the thirty hunk at the top)", "@@ -27" in "\n".join(r.right().split("\n")[:8]) and "hunk 3/3" in r.right())
    r.send(b"\x1b", 0.4)
    # staging with git add shows after R (index change has no file stat)
    sh(f"cd {P} && git add a.txt")
    r.send(b"r", 2.0)
    r.pump(0.8)
    if os.environ.get("GD_DUMP"): print(r.text())
    check("R reloads the marks and diff after staging", r.wait(r"a\.txt · diff staged · \+3 -3", 6, "r"))
    r.send(b"q", 0.8)
    check("Q leaves the explorer", "explorer ·" not in r.text())
    r.quit()
    sh("rm -rf " + X)
    print("FAILED: " + ", ".join(fails) if fails else "all passed")
    sys.exit(1 if fails else 0)

main()
