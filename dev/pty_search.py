# usage: python dev/pty_search.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# Search: text in files (rg or the grep fallback), fuzzy find file, find in shell output, find in a
# preview. Runs the whole thing twice when the tool allows: once normally and once with
# WICLI_NO_RG=1 (forces the grep fallback). Local mode uses a throwaway git repo under the scratch
# dir (the macOS box has no rg, so it exercises grep; Linux with rg exercises both). Remote mode
# builds ~/srchtest/repo on the test box, opens a tab in it, and removes it and its own tmux server
# (socket searchtest) afterwards. Install ripgrep there for the rg pass:
#   docker exec -u root devbox-test apt-get install -y ripgrep
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, pyte
S, mode = os.path.realpath(sys.argv[1]), sys.argv[2]
remote = mode == "remote"
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
COLS, ROWS = 120, 30
X = "/home/dev/srchtest" if remote else S + "/srchtest"
R = X + "/repo"
CFG = S + "/search_" + mode + ".toml"
open(CFG, "w").write(open(S + "/test.toml").read())

def sh(script):
    if remote:
        return subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "bash", "-c", script], capture_output=True, text=True)
    return subprocess.run(["bash", "-c", script], capture_output=True, text=True)

def box_has_rg():
    if remote:
        return sh("command -v rg").returncode == 0
    return shutil.which("rg") is not None

FIXTURE = r'''
set -e
rm -rf "$X"; mkdir -p "$X/repo/src" "$X/repo/docs" "$X/repo/ignored" "$X/repo/node_modules/dep"
cd "$X/repo"
git init -q -b main
git config user.email t@example.com; git config user.name t
cat > src/main.rs <<'EOF'
fn main() {
    println!("hello");
    let tok = "ZQXTOKEN_alpha";
}
EOF
cat > src/lib.rs <<'EOF'
// library
pub fn a() {}
pub fn b() {}
pub fn c() {}
// ZQXTOKEN_beta here
pub fn d() {}
let s = "it's $HOME `x` a b";
let w = "say \"hi\" ; echo pwned";
EOF
printf 'notes\nZQXTOKEN_gamma\nrun: rm -rf /tmp/x\n' > docs/notes.md
printf 'ZQXTOKEN_ignored\n' > ignored/skip.txt
printf 'ZQXTOKEN_dep\n' > node_modules/dep/x.js
printf 'ignored/\n' > .gitignore
seq 1 2600 | sed 's/^/bulkline /' > bulk.txt
for i in $(seq 1 100); do if [ $((i % 30)) = 10 ]; then echo "line $i has a needle here, Needle too"; else echo "line $i plain"; fi; done > findme.txt
git add -A >/dev/null; git commit -q -m init
'''.replace("$X", X)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, extra_env=None):
        env = dict(os.environ, WICLI_CONFIG=CFG, TERM="xterm-256color", WICLI_TMUX_SOCKET="searchtest", **(extra_env or {}))
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(R if not remote else "/")
            os.execve(WI, ["wi", "test"] if remote else ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        self.scr = pyte.Screen(COLS, ROWS); self.st = pyte.ByteStream(self.scr)
        self.pump(3.5 if remote else 2.2)
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
    def region(self, x0, x1, y0=1, y1=29): return "\n".join(l[x0:x1].rstrip() for l in self.rows()[y0:y1])
    def right(self): return self.region(61, 120)
    def status(self): return self.rows()[ROWS - 1]
    def send(self, b, t=0.5): os.write(self.fd, b); self.pump(t)
    def wait(self, pat, t=8.0):
        return self.pump(t, until=lambda: re.search(pat, self.text()) is not None)
    def waitstatus(self, pat, t=6.0):
        return self.pump(t, until=lambda: re.search(pat, self.status()) is not None)
    def find(self, pat, x0=0, x1=None, y0=0):
        for y, l in enumerate(self.rows()):
            if y < y0: continue
            m = re.search(pat, l[x0:x1])
            if m: return m.start() + x0, y
        return None
    def mouse(self, code, x, y, up=False): os.write(self.fd, f"\x1b[<{code};{x + 1};{y + 1}{'m' if up else 'M'}".encode())
    def click(self, x, y, t=0.9):
        self.mouse(0, x, y); self.mouse(0, x, y, up=True); self.pump(t)
    def wheel(self, up, x, y, n=1):
        for _ in range(n): self.mouse(64 if up else 65, x, y)
        self.pump(0.6)
    def bg(self, x, y): return self.scr.buffer[y][x].bg
    def quit(self):
        self.send(MENU + (b"e" if remote else b"q"), 1.5)

LIT = ("brown", "yellow", "cdcd00")   # matches
CUR = ("green", "00cd00")              # the current match

def listrow(r, i):
    """Text of candidate row i of the finder (the row after its title line; a tab strip shifts it)."""
    y = next(y for y, l in enumerate(r.rows()) if "find file:" in l)
    return r.rows()[y + 1 + i].strip()

def search(r, query, opts=b"", t=12.0):
    r.send(MENU + b"l", 0.5)
    if "search in files:" not in r.text():
        r.send(b"s", 0.5)   # the last results came back: S starts a new search
    r.send(b"\x15", 0.2)
    r.send(opts + query.encode(), 0.4)
    r.send(b"\r", 0.2)
    return r.wait(r"(\d[\d,]* match(es)? in \d+ files?|no matches|search failed)", t)

def run_pass(norg):
    tag = "[grep fallback] " if norg else ("[rg] " if box_has_rg() else "[grep] ")
    r = Run({"WICLI_NO_RG": "1"} if norg else None)
    tool = "grep" if (norg or not box_has_rg()) else "rg"
    if remote:
        r.send(MENU + b"f", 0.6)
        r.send(b"\x15" + R.encode() + b"\r", 2.5)
        check(tag + "remote: new tab opened in the repo folder", r.find(r"repo") is not None)

    # ---- search in files: prompt, chips, results
    r.send(MENU + b"l", 0.6)
    check(tag + "menu l opens the search prompt with option chips", "search in files:" in r.text() and "case: smart" in r.text() and "whole word: off" in r.text() and "regex: off" in r.text())
    r.send(b"\t", 0.3)
    check(tag + "Tab cycles the case chip", "case: match case" in r.text())
    r.send(b"\t\t", 0.3)
    check(tag + "Tab cycles back to smart", "case: smart" in r.text())
    r.send(b"\x1b[A", 0.3)
    check(tag + "Up toggles whole word", "whole word: on" in r.text())
    pos = r.find(r"regex: off")
    r.click(pos[0] + 2, pos[1], t=0.4)
    check(tag + "clicking a chip toggles it", "regex: on" in r.text())
    r.send(b"\x1b", 0.5)
    check(tag + "Esc closes the prompt", "search in files:" not in r.text())

    ok = search(r, "ZQXTOKEN")
    n = 3 if tool == "rg" else 4
    t = r.text()
    check(tag + f"results list the files and the count line ({n} matches)", ok and f"{n} matches in {n} files" in t)
    check(tag + "file headers and line numbers are listed", "src/main.rs" in t and "docs/notes.md" in t and "src/lib.rs" in t and re.search(r"\b3\s+.*ZQXTOKEN_alpha", t) and re.search(r"\b5\s+.*ZQXTOKEN_beta", t) and re.search(r"\b2\s+.*ZQXTOKEN_gamma", t))
    check(tag + "results show which tool ran", f"[{tool}]" in t)
    check(tag + ".gitignore" + (" is respected (rg)" if tool == "rg" else " is not (grep fallback)"), ("ignored/skip.txt" in t) == (tool != "rg"))
    check(tag + "node_modules is skipped", "node_modules" not in t and "ZQXTOKEN_dep" not in t)
    pos = r.find(r"ZQXTOKEN_alpha", y0=2)
    hl = pos is not None and r.bg(pos[0] + 2, pos[1]) in LIT
    check(tag + "the matched text is highlighted", hl)
    # arrows move the selection (the selected row is drawn reversed)
    sel_rows = lambda: [y for y in range(1, 29) if r.scr.buffer[y][2].reverse]
    s0 = sel_rows()
    r.send(b"\x1b[B", 0.4)
    s1 = sel_rows()
    check(tag + "Down moves the selection", len(s0) == 1 and len(s1) == 1 and s1[0] > s0[0])
    r.send(b"\x1b[A", 0.3)
    r.send(b"\x1b", 0.6)
    check(tag + "Esc closes the results", "match" not in r.text() or "matches in" not in r.text())
    r.send(MENU + b"l", 0.6)
    check(tag + "menu l again shows the last results without searching", f"{n} matches in {n} files" in r.text())
    r.send(b"\x1b", 0.5)

    # ---- Enter opens the preview at the line, marked
    search(r, "ZQXTOKEN_alpha")
    check(tag + "a unique token gives one match", "1 match in 1 file" in r.text() and "src/main.rs" in r.text())
    r.send(b"\r", 1.5)
    right = r.right()
    check(tag + "Enter opens the preview beside the shell", "ZQXTOKEN_alpha" in right)
    pos = r.find(r"ZQXTOKEN_alpha", 61)
    check(tag + "the matching line is marked", pos is not None and r.bg(pos[0] + 2, pos[1]) not in ("default",))
    r.send(b"\x1b", 0.6)
    r.send(MENU + b"l", 0.6)
    check(tag + "the results stay available after opening", "1 match in 1 file" in r.text())
    r.send(b"\x1b", 0.5)
    r.send(MENU + b"v", 0.6)

    # ---- click a result
    search(r, "ZQXTOKEN")
    pos = r.find(r"ZQXTOKEN_beta", y0=2)
    r.click(pos[0] + 12, pos[1], t=1.5)
    check(tag + "clicking a result opens that file at its line", "ZQXTOKEN_beta" in r.right() and "ZQXTOKEN_alpha" not in r.right() and "pub fn c" in r.right())
    r.send(MENU + b"v", 0.6)

    # ---- the cap
    search(r, "bulkline", t=20.0)
    check(tag + "more than 2,000 matches stops at 2,000 with a visible notice", "2,000 matches in 1 file (stopped at 2,000 matches" in r.text())
    r.send(b"\x1b[6~", 0.3)
    r.send(b"\x1b[F", 0.4)
    check(tag + "End scrolls to the last result", re.search(r"\b2000\b.*bulkline", r.text()) is not None or re.search(r"\b\d{4}\b.*bulkline", r.text()) is not None)
    r.send(b"\x1b[H", 0.4)
    r.wheel(False, 40, 10, 5)
    check(tag + "the wheel scrolls the list", re.search(r"bulkline", r.text()) is not None)
    r.send(b"\x1b", 0.5)

    # ---- queries are literal
    for q, pat in [("it's $HOME `x` a b", r"it's \$HOME `x` a b"), ('say \\"hi\\" ; echo pwned', r'say \\"hi\\" ; echo pwned'), ("-rf", r"rm -rf /tmp/x"), ("$HOME", r"\$HOME")]:
        search(r, q)
        t = r.text()
        check(tag + f"literal query {q!r}", re.search(r"\b1 match(es)? in 1 file", t) is not None and re.search(pat, t) is not None)
        r.send(b"\x1b", 0.4)
    check(tag + "nothing was executed by a query", sh(f"test ! -e {R}/pwned && test ! -e pwned").returncode == 0)
    # options: case, regex, word
    search(r, "ZQXTOKEN_ALPHA")
    check(tag + "smart case: a lowercase-insensitive query still finds mixed case (all caps = exact)", "no matches" in r.text())
    r.send(b"\x1b", 0.4)
    search(r, "zqxtoken_alpha")
    check(tag + "smart case: all-lowercase query ignores case", "1 match in 1 file" in r.text())
    r.send(b"S", 0.6)
    check(tag + "S in the results opens the prompt again with the last query", "search in files:" in r.text() and "zqxtoken_alpha" in r.text())
    r.send(b"\x15", 0.2)
    r.send(b"\x1b[B", 0.2)   # regex on
    r.send(b"ZQXTOKEN_(alpha|beta)\r", 0.2)
    r.wait(r"matches in", 8)
    check(tag + "regex mode", "2 matches in 2 files" in r.text())
    r.send(b"R", 0.5)
    r.wait(r"matches in", 8)
    check(tag + "R runs the search again", "2 matches in 2 files" in r.text())
    r.send(b"\x1b", 0.5)
    search(r, "pub fn", opts=b"\x1b[A")
    r.send(b"\x1b", 0.4)
    # no match / invalid regex
    search(r, "qqqqnotfound")
    check(tag + "no matches is reported", "no matches" in r.text())
    r.send(b"\x1b", 0.4)

    # ---- find file
    r.send(MENU + b"k", 0.5)
    r.wait(r"find file: \d+ of \d+ files", 10)
    check(tag + "Find file lists tracked and untracked files but not gitignored ones", "find file:" in r.text() and "src/main.rs" in r.text() and "ignored/skip.txt" not in r.text() and "node_modules/dep/x.js" in r.text())
    before = re.search(r"find file: (\d+) of", r.text())
    r.send(b"mnrs", 0.8)
    t = r.text()
    after = re.search(r"find file: (\d+) of", t)
    check(tag + "typing narrows the list live", before and after and int(after.group(1)) < int(before.group(1)))
    first = listrow(r, 0)
    check(tag + "mnrs finds src/main.rs first", first.startswith("src/main.rs"))
    r.send(b"\r", 1.5)
    check(tag + "Enter opens it in a preview", "ZQXTOKEN_alpha" in r.right())
    r.send(MENU + b"v", 0.6)
    r.send(MENU + b"k", 0.5)
    r.wait(r"find file: \d+ of", 10)
    r.send(b"rs", 0.6)
    rows = [listrow(r, 0), listrow(r, 1)]
    r.send(b"\x1b[B", 0.4)
    r.send(b"\r", 1.5)
    want = "ZQXTOKEN_alpha" if rows[1].startswith("src/main.rs") else "ZQXTOKEN_beta" if rows[1].startswith("src/lib.rs") else None
    check(tag + "arrows then Enter open the second candidate " + repr(rows[1][:14]), want is not None and want in r.right())
    r.send(MENU + b"v", 0.6)
    r.send(MENU + b"k", 0.5)
    r.wait(r"find file: \d+ of", 10)
    r.send(b"notes", 0.6)
    pos = r.find(r"docs/notes\.md")
    r.click(pos[0] + 3, pos[1], t=1.5)
    check(tag + "clicking a candidate opens it", "ZQXTOKEN_gamma" in r.right())
    r.send(MENU + b"v", 0.6)
    r.send(MENU + b"k", 0.5)
    r.wait(r"find file: \d+ of", 10)
    r.send(b"\x1b", 0.5)
    check(tag + "Esc closes the finder", "find file:" not in r.text())

    # ---- find in output
    r.send(b'clear; for i in $(seq 1 300); do echo "OUTLINE_$i"; done\n', 2.0)
    r.send(MENU + b"u", 0.5)
    check(tag + "Find in output prompts", "find in output:" in r.text())
    r.send((b"OUTLINE_2" if remote else b"OUTLINE_1") + b"\r", 0.8)   # under tmux only the screen is searchable (lines 2xx)
    st = r.status()
    m = re.search(r"match 1 of (\d+)", st)
    total = int(m.group(1)) if m else 0
    check(tag + "the newest match comes first: " + st.strip()[:40], m is not None and total >= 10)
    lit = [(x, y) for y in range(1, 29) for x in range(COLS) if r.bg(x, y) in CUR]
    check(tag + "the current match is highlighted", len(lit) >= len("OUTLINE_1"))
    r.send(b"n", 0.5)
    check(tag + "N steps to the next (older) match", re.search(r"match 2 of %d" % total, r.status()) is not None)
    r.send(b"b", 0.5)
    check(tag + "B steps back", re.search(r"match 1 of %d" % total, r.status()) is not None)
    r.send(b"\x1b", 0.6)
    check(tag + "Esc leaves find mode", "match 1 of" not in r.status() and "N older" not in r.status())
    r.send(MENU + b"u", 0.5)
    early = "OUTLINE_42" if not remote else "OUTLINE_29"
    r.send(early.encode() + b"\r", 0.8)
    check(tag + "a number printed early is found (%s)" % early, re.search(r"match \d+ of \d+", r.status()) is not None)
    if remote:
        check(tag + "the matches on screen are highlighted", any(r.bg(x, y) in LIT + CUR for y in range(1, 29) for x in range(COLS)))
    else:
        pos = r.find(early + r"(?!\d)")
        check(tag + "the view scrolled back to it and it is highlighted", pos is not None and r.bg(pos[0] + 2, pos[1]) in LIT + CUR)
    if not remote:
        check(tag + "the match is far above the live screen", re.search(r"OUTLINE_300", r.text()) is None)
    r.send(b"\x1b", 0.6)
    check(tag + "back at the live view", "OUTLINE_300" in r.text() or "OUTLINE_299" in r.text() or remote)
    r.send(MENU + b"u", 0.5)
    r.send(b"nothing_like_this\r", 0.6)
    check(tag + "no match in output is reported", "no match" in r.status() or "no match" in r.text())

    # ---- find in a preview
    r.send(b"clear\n", 0.5)
    r.send(MENU + b"p", 0.5)
    r.send(b"findme.txt\r", 1.5)
    check(tag + "preview open", "line 1 plain" in r.right())
    r.send(b"f", 0.5)
    check(tag + "F opens the find prompt in a preview", "find in preview:" in r.text())
    r.send(b"needle\r", 0.8)
    check(tag + "the first match is reported", "match 1 of 8" in r.status())
    lit = [(x, y) for y in range(1, 29) for x in range(61, COLS) if r.bg(x, y) in LIT + CUR]
    check(tag + "matches are highlighted in the preview", len(lit) >= 8)
    r.send(b"n", 0.5)
    check(tag + "N steps to the next match", "match 2 of 8" in r.status())
    r.send(b"b", 0.5)
    r.send(b"b", 0.5)
    check(tag + "B steps back (wraps around)", "match 8 of 8" in r.status())
    r.send(b"w\n", 0.5)
    check(tag + "Enter also steps, with wrap on", re.search(r"match 1 of 8", r.status()) is not None)
    r.send(b"\x1b", 0.5)
    r.send(MENU + b"v", 0.6)
    r.quit()

try:
    shutil.rmtree(X, ignore_errors=True)
    if remote:
        sh(f"rm -rf {X}")
    r_ = sh(FIXTURE)
    if r_.returncode != 0:
        print("FAIL fixture:", r_.stderr); sys.exit(1)
    run_pass(False)
    if box_has_rg():
        run_pass(True)
finally:
    sh(f"rm -rf {X}; command -v tmux >/dev/null && tmux -L searchtest kill-server 2>/dev/null; true")
print("FAILED: %d" % len(fails) if fails else "ALL PASS")
sys.exit(1 if fails else 0)
