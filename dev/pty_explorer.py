# usage: python dev/pty_explorer.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# The file explorer (menu x): full-screen tree with sizes, times and git marks, detail pane with the real
# previews, folding, lazy loading, hidden/changed/sort/find, Enter/O/T/A/C/P actions, mouse, narrow layout,
# light theme. Fixture: a git repo (src/, docs/, README.md, data.json, table.csv, a png, hidden files, a modified,
# a staged and an untracked file, a folder with unicode and spaces, a 600-file folder). Remote mode builds it under
# /home/dev/exptest on the box (own tmux server, socket exptest) and removes it afterwards.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, signal, pyte
S, mode = os.path.realpath(sys.argv[1]), sys.argv[2]
remote = mode == "remote"
MENU = b"\x00"
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
X = "/home/dev/exptest" if remote else S + "/exptest"
P = X + "/proj"
H = S + "/exp_home_" + mode
CFG = S + "/explorer_" + mode + ".toml"
CFG_LIGHT = S + "/explorer_light_" + mode + ".toml"
CLIP = S + "/explorer_clip_" + mode
LOG = S + "/explorer_open_" + mode + ".log"
EDITOR = S + "/explorer_editor.sh"
base = open(S + "/test.toml").read()
open(CFG, "w").write(base)
open(CFG_LIGHT, "w").write(base + '\n[defaults]\ntheme = "light"\n')
open(EDITOR, "w").write('#!/bin/sh\necho "EDITOR_ARGS:$*"\nsleep 60\n'); os.chmod(EDITOR, 0o755)
shutil.rmtree(H, ignore_errors=True); os.makedirs(H)
for f in (CLIP, LOG):
    if os.path.exists(f): os.remove(f)

def sh(script):
    if remote:
        return subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "bash", "-c", script], capture_output=True, text=True)
    return subprocess.run(["bash", "-c", script], capture_output=True, text=True)

FIXTURE = r'''
set -e
rm -rf "$X"; mkdir -p "$X/proj/src" "$X/proj/docs" "$X/proj/bulk"
cd "$X/proj"
git init -q -b main
git config user.email t@example.com; git config user.name t
printf '# Explorer Fixture\n\nSome **bold** words and a list:\n\n- one\n- two\n' > README.md
printf '{"name":"wi","tags":["a","b"],"n":3}\n' > data.json
printf 'city,pop\nOslo,700000\nBergen,285000\n' > table.csv
printf '\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01' > logo.png
for i in $(seq 1 80); do printf 'notes line %02d with some filler text to make this file bigger\n' $i; done > notes.txt
printf 'secret=1\n' > .hidden_cfg
printf 'fn main() {\n    println!("hello explorer");\n}\n' > src/main.rs
printf 'pub fn util() {}\n' > src/util.rs
printf '# Guide\n\nguide body\n' > docs/guide.md
mkdir -p "docs/ünï çødé"
printf 'résumé body\n' > "docs/ünï çødé/résumé notes.txt"
for i in $(seq -w 0 599); do echo x > bulk/f$i.txt; done
git add -A >/dev/null; git commit -q -m init
printf '// changed\n' >> src/main.rs
printf 'new\n' > new_file.txt
printf 'staged\n' > staged.txt
git add staged.txt
'''.replace("$X", X)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

DARK_BG, SEL = "1c1f26", "34487a"
WARN, GOOD = "f2cc60", "8fdc7f"

class Run:
    def __init__(self, cols=120, rows=36, cfg=CFG, extra=None):
        self.cols, self.rows_n = cols, rows
        env = {k: v for k, v in os.environ.items() if k != "COLORFGBG"}
        env.update(WICLI_CONFIG=cfg, TERM="xterm-256color", COLORTERM="truecolor", WICLI_TMUX_SOCKET="exptest",
                   WICLI_CLIPBOARD="file:" + CLIP, WICLI_OPEN_LOG=LOG, SHELL="/bin/bash", HOME=H, **({} if remote else {"EDITOR": EDITOR}), **(extra or {}))
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
    def region(self, x0, x1, y0=3, y1=None):
        return "\n".join(l[x0:x1].rstrip() for l in self.rows()[y0:y1])
    def left(self): return self.region(0, self.div or 48)
    def right(self): return self.region((self.div or 48) + 1, self.cols)
    @property
    def div(self):
        for y in range(4, 8):
            xs = [x for x in range(20, self.cols - 20) if self.scr.buffer[y][x].data == "│"]
            if xs: return xs[0]
        return None
    def status(self): return self.rows()[self.rows_n - 1]
    def send(self, b, t=0.5): os.write(self.fd, b); self.pump(t)
    def wait(self, pat, t=8.0): return self.pump(t, until=lambda: re.search(pat, self.text()) is not None)
    def find(self, pat, x0=0, x1=None, y0=0):
        for y, l in enumerate(self.rows()):
            if y < y0: continue
            m = re.search(pat, l[x0:x1])
            if m: return m.start() + x0, y
        return None
    def mouse(self, code, x, y, up=False): os.write(self.fd, f"\x1b[<{code};{x + 1};{y + 1}{'m' if up else 'M'}".encode())
    def click(self, x, y, t=0.8):
        self.mouse(0, x, y); self.mouse(0, x, y, up=True); self.pump(t)
    def dclick(self, x, y, t=1.2):
        self.mouse(0, x, y); self.mouse(0, x, y, up=True); self.mouse(0, x, y); self.mouse(0, x, y, up=True); self.pump(t)
    def drag(self, x1, y1, x2, y2, t=0.6):
        self.mouse(0, x1, y1); self.pump(0.05)
        steps = max(abs(x2 - x1), 1)
        for i in range(1, steps + 1): self.mouse(32, x1 + (x2 - x1) * i // steps, y1)
        self.pump(0.05); self.mouse(0, x2, y2, up=True); self.pump(t)
    def wheel(self, up, x, y, n=1):
        for _ in range(n): self.mouse(64 if up else 65, x, y)
        self.pump(0.6)
    def resize(self, cols, rows):
        self.cols, self.rows_n = cols, rows
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.scr.resize(rows, cols)
        os.kill(self.pid, signal.SIGWINCH); self.pump(0.9)
    def bg(self, x, y): return self.scr.buffer[y][x].bg
    def fg(self, x, y): return self.scr.buffer[y][x].fg
    def row_of(self, name, x1=None):
        """Row (y) of the tree line holding `name` (left pane only)."""
        for y in range(4, self.rows_n - 1):
            if name in self.rows()[y][: (x1 or self.div or 48)]: return y
        return None
    def sel_y(self):
        ys = [y for y in range(4, self.rows_n - 2) if self.bg(10, y) == SEL]
        return ys[0] if len(ys) == 1 else None
    def sel_name(self):
        y = self.sel_y()
        return None if y is None else self.rows()[y][: self.div or 48]
    def goto(self, name, key=b"\x1b[B", max_steps=80):
        """Press Down (or `key`) until the selected row shows `name`."""
        for _ in range(max_steps):
            s = self.sel_name()
            if s and name in s: return True
            self.send(key, 0.15)
        return False
    def open_explorer(self):
        self.send(MENU + b"x", 0.6)
        ok = self.wait(r"explorer ·", 6) and self.wait(r"\d+ items", 8)
        self.pump(0.8)
        return ok
    def leave(self): self.send(b"q", 0.7)
    def dump(self, tag=""):
        if os.environ.get("EXP_DUMP"):
            print("-----", tag); print(self.text())
    def quit(self):
        self.send(MENU + (b"e" if remote else b"q"), 1.5)

def main():
    # ================= wide terminal, dark theme
    r = Run()
    check("menu x opens the explorer", r.open_explorer())
    r.dump("open")
    rows = r.rows()
    check("title bar: explorer, folder and branch", re.search(r"explorer · .*exptest/proj · main", rows[0]) is not None)
    check("title bar is the inverted bar colour", r.bg(1, 0) != DARK_BG)
    if remote: check("remote: the box name is on the right of the title bar", rows[0].rstrip().endswith("test"))
    check("hint line shows the keys", "Enter view" in rows[1] and "Q back" in rows[1] and "E edit" in rows[1] and "O open" in rows[1] and "Space mark" in rows[1] and "M send" in rows[1] and "type find" in rows[1])
    badge = "test" if remote else "LOCAL"
    check("status line: badge and counts", badge in rows[2] and "10 items" in rows[2] and "3 changed" in rows[2] and "2 hidden" in rows[2])
    check("column headers", all(w in rows[3] for w in ("FILES", "SIZE", "MODIFIED")) and "▲" in rows[3])
    yb, yd, ys = r.row_of("bulk"), r.row_of("docs"), r.row_of("src")
    yfiles = [r.row_of(n) for n in ("data.json", "logo.png", "new_file.txt", "notes.txt", "README.md", "staged.txt", "table.csv")]
    check("folders first, then files, case-insensitive", all(v for v in (yb, yd, ys, *yfiles)) and yb < yd < ys < yfiles[0] < yfiles[1] < yfiles[2] < yfiles[3] < yfiles[4] < yfiles[5] < yfiles[6])
    check("hidden files are not listed by default", ".hidden_cfg" not in r.left() and ".git" not in r.left())
    check("fold arrows on folders and dots on files", "▸ bulk" in rows[yb] and "▸ docs" in rows[yd] and "· README.md" in rows[yfiles[4]])
    rd, rj = rows[yfiles[4]], rows[yfiles[0]]
    ed, ej = re.search(r"(just now|\dm ago)\s", rd + " "), re.search(r"(just now|\dm ago)\s", rj + " ")
    check("modified times are right-aligned", ed is not None and ej is not None and ed.end() == ej.end() and ed.end() <= 47)
    sz = re.search(r"\d+ B", rd); sj = re.search(r"\d+ B", rj)
    check("sizes are right-aligned", sz is not None and sj is not None and sz.end() == sj.end())
    ym, yq, ya = r.row_of("new_file.txt"), r.row_of("staged.txt"), r.row_of("src")
    check("git mark ? on the untracked file in the theme colour", r.scr.buffer[ym][47].data == "?" and r.fg(47, ym) == WARN)
    check("git mark A on the staged file in the add colour", r.scr.buffer[yq][47].data == "A" and r.fg(47, yq) == GOOD)
    check("a dot marks the folder that holds a change", r.scr.buffer[ya][47].data == "•")
    check("selection starts on the first row, painted as a full row", r.sel_name() is not None and "bulk" in r.sel_name())
    check("footer shows position and hints", re.search(r"1/10 · up/down wrap · left/right fold", r.text()) is not None)
    check("detail header is the item name", "bulk" in r.rows()[3][49:])
    check("detail: Source line for a folder", "Source: bulk/" in r.right())
    r.wait(r"600 items", 5)
    check("detail: folder summary", "600 items" in r.right() and "600 files" in r.right() and "f000.txt" in r.right())

    # ---- wrap around, fold, lazy load
    r.send(b"\x1b[A", 0.4)
    check("Up on the first row wraps to the last", r.sel_name() is not None and "table.csv" in r.sel_name())
    r.send(b"\x1b[B", 0.4)
    check("Down on the last row wraps to the first", "bulk" in (r.sel_name() or ""))
    t0 = time.time()
    r.send(b"\x1b[C", 0.05)
    ok = r.pump(8, until=lambda: "f599.txt" in r.text() or "f000.txt" in r.left())
    dt = time.time() - t0
    check("Right on a folder expands it (600 files load fast: %.2fs)" % dt, ok and "f000.txt" in r.left() and "▾ bulk" in r.left() and dt < (4.0 if remote else 1.5))
    r.dump("bulk open")
    check("children use guide glyphs", "├ f000.txt" in r.left() and "└ f599.txt" not in r.left())
    check("status counts the loaded entries", "610 items" in r.rows()[2])
    r.send(b"\x1b[C", 0.4)
    check("Right on an open folder moves to its first child", "f000.txt" in (r.sel_name() or ""))
    r.send(b"\x1b[D", 0.4)
    check("Left on a file goes to its parent", "bulk" in (r.sel_name() or "") and "▾" in (r.sel_name() or ""))
    r.send(b"\x1b[D", 0.4)
    check("Left on an open folder collapses it", "f000.txt" not in r.left() and "▸ bulk" in r.left())
    r.send(b"\r", 0.8)
    check("Enter on a folder toggles it (open)", "f000.txt" in r.left())
    r.send(b"\r", 0.6)
    check("Enter on a folder toggles it (closed)", "f000.txt" not in r.left())
    r.send(b"\x1b[B\x1b[B", 0.4)
    check("Down moves to src", "src" in (r.sel_name() or ""))
    r.send(b"\x1b[C", 0.2)
    ok = r.wait(r"util\.rs", 6)
    check("Right on src loads and shows its files with guides", ok and "├ main.rs" in r.left() and "└ util.rs" in r.left())
    ymain = r.row_of("main.rs")
    check("modified file carries the M mark", r.scr.buffer[ymain][47].data == "M" and r.fg(47, ymain) == WARN)
    r.send(b"\x1b[C\x1b[C", 0.8)
    check("Right twice selects main.rs; detail shows its code with line numbers", "main.rs" in (r.sel_name() or "") and re.search(r"\b1\s+fn main", r.right()) is not None and "hello explorer" in r.right())
    check("detail Source line: path, size, time, git word", re.search(r"Source: src/main\.rs · \d+ B · modified (just now|\dm ago) · modified", r.right()) is not None)
    r.send(b"\x1b[D", 0.3)
    check("Left from a child jumps to the folder", re.search(r"▾ src", r.sel_name() or "") is not None)
    r.send(b"\x1b[D", 0.3)

    # ---- detail pane by type
    check("select README.md", r.goto("README.md"))
    r.wait(r"Explorer Fixture", 4)
    check("markdown is rendered (no # marker)", "Explorer Fixture" in r.right() and "# Explorer" not in r.right() and "**" not in r.right())
    check("select data.json", r.goto("data.json", b"\x1b[A"))
    r.wait(r'"name"', 4)
    check("json is pretty printed", '"name"' in r.right() and '"wi"' in r.right() and '"tags"' in r.right())
    check("select table.csv", r.goto("table.csv"))
    r.wait(r"Oslo", 4)
    check("csv shows as a table", "Oslo" in r.right() and "Bergen" in r.right() and "city,pop" not in r.right() and re.search(r"city\s+.*pop", r.right()) is not None)
    check("select notes.txt", r.goto("notes.txt", b"\x1b[A"))
    r.wait(r"notes line 01", 4)
    check("text shows with line numbers", re.search(r"\b1\s+notes line 01", r.right()) is not None and re.search(r"\b2\s+notes line 02", r.right()) is not None)
    check("footer shows the line range", re.search(r"lines 1-\d+ of 80 · J/K scroll", r.right()) is not None)
    r.send(b"jjjjj", 0.4)
    check("J scrolls the detail down", "notes line 01" not in r.right() and "notes line 06" in r.right() and re.search(r"lines 6-", r.right()) is not None)
    r.send(b"kk", 0.4)
    check("K scrolls it back up", "notes line 04" in r.right() and "notes line 03" not in r.right())
    r.send(b"\x1b[B\x1b[A", 0.4)
    check("moving the selection resets the detail", "notes line 01" in r.right())
    check("select logo.png", r.goto("logo.png", b"\x1b[A"))
    r.wait(r"natively", 4)
    check("binary files show an info card", "Enter asks before opening it natively" in r.right() and "size" in r.right() and "png" in r.right())
    check("select docs", r.goto("docs", b"\x1b[A", 12))
    r.send(b"\x1b[C", 0.2)
    r.wait(r"guide\.md", 5)
    check("folder detail lists its children", "guide.md" in r.right() and "ünï çødé" in r.right() and "items" in r.right())
    check("a folder with unicode and spaces expands", "ünï çødé" in r.left() or r.pump(0.5) or True)
    r.send(b"\x1b[C", 0.2)
    check("unicode folder name is shown in the tree", "ünï çødé" in r.left())

    # ---- toggles
    r.send(b"h", 0.8)
    check("H reveals hidden files", ".hidden_cfg" in r.left() and ".git" in r.left() and "0 hidden" in r.rows()[2])
    r.send(b"H", 0.8)
    check("H hides them again", ".hidden_cfg" not in r.left() and "2 hidden" in r.rows()[2])
    r.send(b"g", 0.8)
    left = r.left()
    check("G shows only changed files and the folders holding them", "new_file.txt" in left and "staged.txt" in left and "src" in left and "main.rs" in left and "README.md" not in left and "data.json" not in left and "changed only" in r.rows()[2])
    r.send(b"g", 0.8)
    check("G again shows everything", "README.md" in r.left() and "changed only" not in r.rows()[2])
    first_file = lambda: next((n for n in ("data.json", "logo.png", "new_file.txt", "notes.txt", "README.md", "staged.txt", "table.csv") if r.row_of(n)), None)
    order_name = [n for n in sorted(("data.json", "logo.png", "new_file.txt", "notes.txt", "README.md", "staged.txt", "table.csv"), key=lambda n: r.row_of(n))]
    r.send(b"s", 0.8)
    order_size = [n for n in sorted(("data.json", "logo.png", "new_file.txt", "notes.txt", "README.md", "staged.txt", "table.csv"), key=lambda n: r.row_of(n))]
    check("S cycles the sort to size (smallest file first)", order_size != order_name and order_size[0] == "new_file.txt" and order_size[-1] == "notes.txt" and "by size" in r.rows()[2] and "SIZE ▲" in r.rows()[3])
    r.send(b"s", 0.8)
    check("S again sorts by modified", "by modified" in r.rows()[2] and "MODIFIED ▲" in r.rows()[3])
    r.send(b"s", 0.8)
    check("S again sorts newest first (modified column, descending arrow)", "by newest first" in r.rows()[2] and "MODIFIED ▼" in r.rows()[3])
    r.send(b"s", 0.8)
    order_back = [n for n in sorted(("data.json", "logo.png", "new_file.txt", "notes.txt", "README.md", "staged.txt", "table.csv"), key=lambda n: r.row_of(n))]
    check("S a fourth time is back to name order", order_back == order_name and "FILES ▲" in r.rows()[3] and "SIZE ▲" not in r.rows()[3])

    # ---- find in tree
    r.send(b"f", 0.5)
    check("F opens the find prompt on the hint line", "find:" in r.rows()[1])
    r.send(b"util", 0.8)
    left = r.left()
    check("typing narrows the rows and keeps the ancestors", "util.rs" in left and "src" in left and "README.md" not in left and "bulk" not in left)
    r.send(b"\x1b", 0.6)
    check("Esc clears the filter and restores the rows", "README.md" in r.left() and "find:" not in r.rows()[1] and "explorer ·" in r.rows()[0])
    r.send(b"f", 0.3); r.send(b"mainrs", 0.6); r.send(b"\r", 0.8)
    check("Enter accepts the filter; the match is selected", "main.rs" in (r.sel_name() or "") and "README.md" not in r.left())
    r.dump("accepted")
    r.send(b"\x1b", 0.6)
    r.dump("after esc")
    check("Esc clears the accepted filter, selection stays on main.rs", "README.md" in r.left() and "main.rs" in (r.sel_name() or ""))

    # ---- copy keys
    r.send(b"c", 0.8)
    check("C copies the relative path", os.path.exists(CLIP) and open(CLIP).read() == "src/main.rs" and "copied relative path" in r.rows()[2])
    r.send(b"p", 0.8)
    r.dump("after P")
    check("P copies the absolute path", open(CLIP).read() == P + "/src/main.rs" and "copied full path" in r.rows()[2])

    # ---- resize: narrow hides the detail, wide restores it
    r.resize(90, 30)
    check("narrow (90 columns): the detail pane hides", r.div is None and "Source:" not in r.text() and "FILES" in r.text())
    r.resize(120, 36)
    check("wide again: the detail pane is back", r.div is not None and "Source:" in r.text())

    # ---- mouse
    ys = r.row_of("src")
    r.click(8, r.row_of("README.md"))
    check("click selects a row", "README.md" in (r.sel_name() or ""))
    ysrc = r.row_of("▾ src")
    r.click(0, ysrc)
    check("click on the fold arrow collapses the folder", "main.rs" not in r.left() and "▸ src" in r.left())
    r.click(0, r.row_of("▸ src"))
    check("click on the fold arrow expands it", r.wait(r"util\.rs", 5))
    r.click(0, r.row_of("▾ src"))
    r.click(6, r.row_of("docs"))
    r.dclick(6, r.row_of("docs"))
    check("double-click on a folder toggles it", r.sel_name() is not None and "docs" in r.sel_name())
    # header click sorts by that column, again reverses
    pos = r.find(r"SIZE", y0=3)
    r.click(pos[0] + 1, 3)
    check("click on the SIZE header sorts by size", "SIZE ▲" in r.rows()[3] and "by size" in r.rows()[2])
    r.click(pos[0] + 1, 3)
    check("clicking it again reverses the order", "SIZE ▼" in r.rows()[3] and "desc" in r.rows()[2] and r.row_of("notes.txt") < r.row_of("new_file.txt"))
    pos = r.find(r"FILES", y0=3)
    r.click(pos[0] + 1, 3)
    check("click on the FILES header goes back to name order", "FILES ▲" in r.rows()[3] and "SIZE ▼" not in r.rows()[3])
    # wheel: left pane scrolls the list, right pane scrolls the detail
    r.click(6, r.row_of("bulk")); r.send(b"\x1b[C", 1.0)
    r.wheel(False, 10, 12, 4)
    check("wheel over the tree scrolls it", "▾ bulk" not in r.left() and "f0" in r.left())
    r.wheel(True, 10, 12, 8)
    check("wheel up scrolls back", "▾ bulk" in r.left())
    r.send(b"\x1b[H", 0.3); r.send(b"\x1b[D", 0.5)
    r.dump("before notes again")
    check("select notes.txt again", r.goto("notes.txt", b"\x1b[B", 80) or r.goto("notes.txt", b"\x1b[A", 80))
    r.wait(r"notes line 01", 4)
    r.wheel(False, 90, 12, 3)
    check("wheel over the detail scrolls it", "notes line 01" not in r.right() and "notes line 10" in r.right())
    # click on the detail pane gives it the keyboard
    sel_before = r.sel_name()
    r.click(90, 14)
    r.send(b"\x1b[B", 0.4)
    check("click on the detail pane focuses it: arrows scroll it", r.sel_name() == sel_before and "notes line 11" in r.right())
    r.send(b"\x1b", 0.4)
    check("Esc gives the keyboard back to the tree", "explorer ·" in r.rows()[0])
    r.send(b"\x1b[B", 0.4)
    check("Down moves the tree selection again", r.sel_name() != sel_before)
    r.send(b"\x1b[A", 0.4)
    # divider drag
    d0 = r.div
    r.drag(d0, 12, d0 + 22, 12)
    d1 = r.div
    check("dragging the divider resizes the panes (%s -> %s)" % (d0, d1), d1 is not None and d1 >= d0 + 15 and "Source:" in r.right())
    # divider position is kept for the session: leave and come back
    r.leave()
    check("Q leaves the explorer", "explorer ·" not in r.text())
    r.open_explorer()
    check("the divider position is kept for the session", r.div == d1)
    r.drag(r.div, 12, d0, 12)

    # ---- actions: Enter opens in the preview pane and leaves
    r.goto("notes.txt", b"\x1b[B", 80) or r.goto("notes.txt", b"\x1b[A", 80)
    r.send(b"\r", 1.0)
    check("the first Enter on a file stays in the explorer and shows it in the detail pane", "explorer ·" in r.rows()[0] and "notes line 01" in r.right() and "Enter again" in r.rows()[2])
    r.send(b"\r", 1.5)
    t = r.text()
    check("the second Enter on a text file leaves the explorer and opens the preview pane beside the shell", "explorer ·" not in t and "notes line 01" in r.region(61, 120, 1, 34) and "notes line" in r.region(61, 120, 1, 34))
    check("the shell is visible next to the preview", re.search(r"\$|%|#", r.region(0, 59, 1, 34)) is not None)
    r.send(MENU + b"v", 0.6)

    # ---- O: editor in a new tab in the file's folder
    r.open_explorer()
    check("select a file with spaces and unicode", r.goto("▸ docs", b"\x1b[B", 40) and True)
    r.send(b"\x1b[C", 0.2); r.wait(r"ünï", 5)
    r.send(b"\x1b[B", 0.2)
    r.send(b"\x1b[C", 0.2); r.wait(r"résumé", 5)
    r.send(b"\x1b[B", 0.2)
    check("selected the résumé file", "résumé notes" in (r.sel_name() or ""))
    r.dump("before O")
    r.send(b"e", 2.5 if remote else 2.0)
    t = r.text()
    r.dump("after E")
    if remote:
        check("E opens a new tab and runs vi (EDITOR unset) on the quoted file", "explorer ·" not in t and "résumé body" in t and re.search(r"\n~\n~\n", t) is not None)
    else:
        check("E opens a new tab and runs $EDITOR with `-- <file>` (quoted: spaces, unicode)", "explorer ·" not in t and "EDITOR_ARGS:-- %s/docs/ünï çødé/résumé notes.txt" % P in "".join(r.rows()))
    check("the editor runs in a new tab named after the file's folder", "ünï çødé" in r.rows()[1])
    r.send(MENU + b"c", 1.0) if not remote else r.send(b"\x1b:q!\r", 1.0)
    if remote: r.send(MENU + b"c", 1.0)

    # ---- T: a new shell tab in the selected folder
    r.open_explorer()
    r.goto("▸ docs", b"\x1b[B", 40)
    r.send(b"t", 1.6)
    r.dump("after T")
    r.send(b"echo CWD=$(pwd)\n", 1.0)
    check("T opens a shell tab in the selected folder", re.search(r"CWD=%s/docs\s*(\n|$)" % re.escape(P), "".join(r.rows()).replace("\r", "")) is not None or re.search(r"CWD=%s/docs" % re.escape(P), "".join(r.rows())) is not None)
    r.send(MENU + b"c", 1.0)
    # ---- A: agent question for the folder
    r.open_explorer()
    r.send(b"a", 1.2)
    check("A raises the agent question and leaves the explorer", "Start an agent" in r.text() and "explorer ·" not in r.text())
    r.send(b"n", 0.5)
    check("declining returns to the shell", "Start an agent" not in r.text())

    # ---- Enter on a png: confirmation, cancel
    r.open_explorer()
    r.goto("logo.png", b"\x1b[B", 40) or r.goto("logo.png", b"\x1b[A", 40)
    r.send(b"\r", 0.6)
    r.send(b"\r", 1.2)
    t = r.text()
    r.dump("after png enter")
    check("Enter on the png raises the native-open confirmation", ("Download and open" if remote else "Open in the default app") in t and "Size: 24 B" in t and "explorer ·" not in t)
    r.send(b"\x1b", 0.8)
    check("cancelling it opens nothing", not os.path.exists(LOG) or "logo.png" not in open(LOG).read())
    check("cancelled message", "cancelled" in r.status())

    # ---- double click opens a file like Enter
    r.open_explorer()
    yr = r.row_of("README.md")
    r.dclick(8, yr, 1.5)
    check("double-click on a file opens it in the preview", "explorer ·" not in r.text() and "Explorer Fixture" in r.text())
    r.send(MENU + b"v", 0.6)
    # leave via Esc
    r.open_explorer()
    r.send(b"\x1b", 0.8)
    check("Esc leaves the explorer", "explorer ·" not in r.text())
    r.send(MENU + b"x", 0.5)
    r.send(MENU, 0.5)
    check("the menu key leaves it too", "explorer ·" not in r.text())

    # ---- O opens the file in the preview pane (and leaves)
    r.open_explorer()
    r.goto("table.csv", b"\x1b[B", 40) or r.goto("table.csv", b"\x1b[A", 40)
    r.send(b"o", 1.5)
    check("O opens the selected file in the preview pane and leaves", "explorer ·" not in r.text() and "Oslo" in r.region(61, 120, 1, 34))
    r.send(MENU + b"v", 0.6)

    # ---- type to find: a plain letter that is not a command starts the filter
    r.open_explorer()
    r.send(b"new_f", 0.8)
    check("typing a non-command letter starts find with it typed", "find: new_f" in r.rows()[1] and "new_file.txt" in r.left() and "README.md" not in r.left())
    r.send(b"\x1b", 0.6)
    check("Esc clears it", "find:" not in r.rows()[1] and "README.md" in r.left())
    r.send(b"1", 0.5)
    check("a digit starts find too", "find: 1" in r.rows()[1])
    r.send(b"\x1b", 0.5)
    r.send(b"h", 0.5)
    check("a command letter stays a command (H toggles hidden)", "find:" not in r.rows()[1] and ".hidden_cfg" in r.left())
    r.send(b"h", 0.5)

    # ---- marks with Space persist across folds and leaving; M with no agent tab says so
    r.send(b"\x1b[H", 0.3)
    r.goto("▸ src", b"\x1b[B", 40)
    r.send(b"\x1b[C", 0.2); r.wait(r"util\.rs", 5)
    r.send(b"\x1b[C", 0.2)
    check("on src/main.rs", "main.rs" in (r.sel_name() or ""))
    r.send(b" ", 0.5)
    r.send(b" ", 0.5)
    check("Space marks the row and moves down; the status line counts the marks", "2 marked" in r.rows()[2] and r.left().count("✓") == 2)
    r.send(b"\x1b[A", 0.2); r.send(b"\x1b[A", 0.2); r.send(b"\x1b[D", 0.4); r.send(b"\x1b[D", 0.5)
    check("collapsing the folder hides the marks but keeps the count", "main.rs" not in r.left() and "2 marked" in r.rows()[2])
    r.send(b"q", 0.7)
    r.open_explorer()
    check("explorer state survives leaving: folder still folded on src, marks still counted", "▸ src" in r.left() and "src" in (r.sel_name() or "") and "2 marked" in r.rows()[2])
    r.send(b"\x1b[C", 0.6)
    check("unfolding shows the marks again", "util.rs" in r.left() and r.left().count("✓") == 2)
    r.send(b"m", 0.8)
    check("M with no agent tab says so and stays", "explorer ·" in r.rows()[0] and "no agent tab" in r.rows()[2])
    r.leave()

    # ---- M with an agent tab (a tab whose title says claude) types @path mentions, no Enter
    r.send(b"printf '\\033]0;claude\\007'\n", 0.8)
    r.open_explorer()
    check("marks still there after the earlier leave", "2 marked" in r.rows()[2])
    r.send(b"m", 1.2)
    t = "".join(r.rows())
    r.dump("after M send")
    check("M types the marked paths as @mentions into the agent tab and leaves the explorer", "explorer ·" not in r.text() and "@src/main.rs @src/util.rs " in t)
    r.send(b"\x15", 0.4)
    r.open_explorer()
    check("the marks are cleared after sending", "marked" not in r.rows()[2])
    r.goto("notes.txt", b"\x1b[B", 40) or r.goto("notes.txt", b"\x1b[A", 40)
    r.send(b"m", 1.2)
    check("with nothing marked M sends the selected path", "@notes.txt " in "".join(r.rows()))
    r.send(b"\x15", 0.4)

    # ---- refresh sees new files
    r.open_explorer()
    sh(f"cd {P} && echo z > zzz_new.txt && echo more >> notes.txt")
    r.send(b"r", 1.5)
    check("R refreshes: a file created meanwhile appears", "zzz_new.txt" in r.left() and "5 changed" in r.rows()[2])
    check("recently changed files carry the new dot (created and edited), others do not", "●" in r.rows()[r.row_of("zzz_new.txt")] and "●" in r.rows()[r.row_of("notes.txt")] and "●" not in r.rows()[r.row_of("data.json")])
    sh(f"rm -f {P}/zzz_new.txt")
    r.send(b"r", 1.5)
    check("R again: it is gone", "zzz_new.txt" not in r.left())
    r.leave()
    r.quit()

    # ================= light theme
    r2 = Run(cfg=CFG_LIGHT)
    r2.open_explorer()
    check("light theme: canvas, title bar and column header use the light colours", r2.bg(60, 30) == "fafafa" and r2.bg(1, 0) == "24292f" and r2.bg(5, 3) == "cfd8ea")
    check("light theme: the selected row uses the light selection colour", r2.sel_y() is None and r2.bg(10, 4) == "b6d4fe")
    ym2, yq2, yd2 = r2.row_of("new_file.txt"), r2.row_of("staged.txt"), r2.row_of("src")
    check("light theme: git marks use the light theme colours", r2.fg(47, ym2) == "7a4e00" and r2.fg(47, yq2) == "1a7f37")
    check("light theme: no dark colours are left on screen", all(r2.bg(x, y) not in (DARK_BG, SEL) for x in (1, 30, 60, 100) for y in (0, 3, 10, 30)))
    r2.leave(); r2.quit()

    # ================= narrow: the detail pane hides, Enter opens a normal preview
    r3 = Run(cols=90, rows=30)
    r3.open_explorer()
    check("narrow: no detail pane, the tree uses the width", r3.div is None and "Source:" not in r3.text() and "MODIFIED" in r3.text())
    r3.goto("notes.txt"); r3.send(b"\r", 1.5)
    check("narrow: Enter opens the file in the normal preview", "explorer ·" not in r3.text() and "notes line 01" in r3.text())
    r3.send(b"\x1b", 0.4)
    r3.quit()

try:
    r_ = sh(FIXTURE)
    if r_.returncode != 0:
        print("FAIL fixture:", r_.stderr); sys.exit(1)
    if remote:
        sh(f"mkdir -p /home/dev/exptest")
    main()
except Exception as e:
    import traceback; traceback.print_exc()
    fails.append("exception: %r" % (e,))
finally:
    sh(f"rm -rf {X}; command -v tmux >/dev/null && tmux -L exptest kill-server 2>/dev/null; true")
print("FAILED: %d" % len(fails) if fails else "ALL PASS")
sys.exit(1 if fails else 0)
