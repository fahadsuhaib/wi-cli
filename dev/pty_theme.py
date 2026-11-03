# usage: python dev/pty_theme.py <scratch-dir> local   (needs pyte)
# Themes: dark / light / auto (OSC 11 answered by this harness), the menu cycle auto -> dark -> light -> auto
# and persistence in state.toml (auto saves nothing), ANSI remap, 256-colour fallback, and theme colours in the previews.
# Every scenario gets its own config directory so state.toml never leaks between them.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, pyte
from pyte.graphics import FG_BG_256
S = sys.argv[1]
MENU = b"\x00"
HERE = os.path.dirname(os.path.abspath(__file__))
WI = os.path.join(HERE, "..", "target/debug/wi")
W = S + "/themews"
LIGHT_REPLY = b"\x1b]11;rgb:ffff/ffff/ffff\x07"
DARK_REPLY = b"\x1b]11;rgb:1c1c/1e1e/2626\x07"

# Theme colours come straight from src/theme.rs so the test cannot drift from the real values.
src = open(os.path.join(HERE, "..", "src/theme.rs")).read()
def theme_colors(name):
    blk = src[src.index("pub const " + name):]
    blk = blk[:blk.index("\n};")]
    d = {k: v for k, v in re.findall(r"(\w+): c\(0x([0-9a-fA-F]{6})\)", blk.split("ansi:")[0])}
    d["ansi"] = re.findall(r"c\(0x([0-9a-fA-F]{6})\)", blk.split("ansi:")[1])
    return d
T = {"dark": theme_colors("DARK"), "light": theme_colors("LIGHT")}

def quantize(h):
    r, g, b = int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)
    lv = [0, 95, 135, 175, 215, 255]
    li = lambda v: min(range(6), key=lambda i: abs(lv[i] - v))
    ri, gi, bi = li(r), li(g), li(b)
    cube = (lv[ri], lv[gi], lv[bi]); ci = 16 + 36 * ri + 6 * gi + bi
    gr = max(0, min(23, ((r + g + b) // 3 - 8 + 5) // 10)); gv = 8 + 10 * gr
    d = lambda a, c: sum((x - y) ** 2 for x, y in zip(a, c))
    return 232 + gr if d((r, g, b), (gv,) * 3) < d((r, g, b), cube) else ci

shutil.rmtree(W, ignore_errors=True); os.makedirs(W)
subprocess.run(["bash", "-c", r'''
set -e
printf '# Mainhead\n\nsome text\n' > a.md
printf '{"keyname": "strval", "num": 12}\n' > b.json
printf 'idcol,namecol\n1,x\n2,y\n' > c.csv
git init -q -b testbr
git config user.email t@example.com; git config user.name t
seq 1 8 | sed 's/^/orig /' > g1.txt
git add g1.txt && git commit -q -m init
seq 1 8 | sed 's/^/orig /' | sed 's/^orig 4$/CHANGED 4/' > g1.txt
'''], check=True, cwd=W)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    """One wi in a pty. `answer` is written back when wi asks for the terminal background."""
    def __init__(self, tag, cfg_text="", answer=None, colorterm="truecolor", env_extra=None, fresh=True, state_text=None):
        self.cfgdir = S + "/theme_" + tag
        if fresh: shutil.rmtree(self.cfgdir, ignore_errors=True)
        os.makedirs(self.cfgdir, exist_ok=True)
        open(self.cfgdir + "/config.toml", "w").write(cfg_text)
        if state_text is not None: open(self.cfgdir + "/state.toml", "w").write(state_text)
        env = {k: v for k, v in os.environ.items() if k not in ("COLORFGBG", "COLORTERM")}
        env.update(WICLI_CONFIG=self.cfgdir + "/config.toml", TERM="xterm-256color", **(env_extra or {}))
        if colorterm: env["COLORTERM"] = colorterm
        self.answer, self.asked, self.raw = answer, False, bytearray()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(W)
            os.execve(WI, ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 100, 0, 0))
        self.scr = pyte.Screen(100, 50); self.st = pyte.ByteStream(self.scr)
        self.pump(2.0)
    def pump(self, t, until=None):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if r:
                try: data = os.read(self.fd, 65536)
                except OSError: return False
                self.raw += data
                if not self.asked and b"\x1b]11;?" in self.raw:
                    self.asked = True
                    if self.answer: os.write(self.fd, self.answer)
                self.st.feed(data)
            if until and until(): return True
        return until is None
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)) for y in range(self.scr.lines)]
    def text(self): return "\n".join(l.rstrip() for l in self.rows())
    def send(self, b, t=0.6): os.write(self.fd, b); self.pump(t)
    def cell(self, x, y): return self.scr.buffer[y][x]
    def find(self, pat, last=False):
        hits = [(m.start(), y) for y, l in enumerate(self.rows()) for m in [re.search(pat, l)] if m]
        return (hits[-1] if last else hits[0]) if hits else None
    def header_bg(self): return self.cell(50, 0).bg
    def status_bg(self): return self.cell(50, 49).bg
    def quit(self):
        self.send(MENU + b"q", 0.8)
        try: os.kill(self.pid, 9)
        except OSError: pass
        try: os.waitpid(self.pid, 0)
        except OSError: pass

# ------------------------------------------------------------ explicit dark and light
r = Run("dark", '[defaults]\ntheme = "dark"\n')
check("dark: header background is the dark bar colour", r.header_bg() == T["dark"]["bar_bg"])
check("dark: status bar background is the dark status colour", r.status_bg() == T["dark"]["status_bg"])
check("dark: no OSC 11 query is sent for an explicit theme", not r.asked)
r.quit()
r = Run("light", '[defaults]\ntheme = "light"\n')
check("light: header background is the light bar colour", r.header_bg() == T["light"]["bar_bg"])
check("light: status bar background is the light status colour", r.status_bg() == T["light"]["status_bg"])
check("dark and light bars really differ", T["dark"]["bar_bg"] != T["light"]["bar_bg"])
r.quit()

# ------------------------------------------------------------ auto: OSC 11
r = Run("autolight", answer=LIGHT_REPLY)
check("auto: wi asks the terminal for its background", r.asked)
check("auto: a light answer picks the light theme", r.header_bg() == T["light"]["bar_bg"])
r.quit()
r = Run("autodark", answer=DARK_REPLY)
check("auto: a dark answer picks the dark theme", r.asked and r.header_bg() == T["dark"]["bar_bg"])
r.quit()
t0 = time.time()
r = Run("autonone", answer=None)
check("auto: no answer falls back to dark", r.asked and r.header_bg() == T["dark"]["bar_bg"])
r.quit()
r = Run("fgbg", answer=None, env_extra={"COLORFGBG": "0;15"})
check("auto: no answer but COLORFGBG 0;15 picks light", r.header_bg() == T["light"]["bar_bg"])
r.quit()
r = Run("fgbgdark", answer=None, env_extra={"COLORFGBG": "15;0"})
check("auto: COLORFGBG 15;0 picks dark", r.header_bg() == T["dark"]["bar_bg"])
r.quit()

# ------------------------------------------------------------ menu cycle and persistence
def saved(r):
    p = r.cfgdir + "/state.toml"
    return open(p).read() if os.path.exists(p) else ""
r = Run("toggle", '[defaults]\ntheme = "auto"\n', answer=DARK_REPLY)
check("cycle: starts dark (terminal answered dark)", r.header_bg() == T["dark"]["bar_bg"])
r.send(MENU, 0.6)
check("cycle: the menu shows the mode and what auto picked", "Theme: auto (dark)" in r.text())
r.send(b"y", 0.8)
check("cycle: first Y goes auto -> dark", r.header_bg() == T["dark"]["bar_bg"] and 'theme = "dark"' in saved(r))
r.send(MENU, 0.6)
check("cycle: the label now reads Theme: dark", "Theme: dark" in r.text() and "auto (" not in r.text())
r.send(b"y", 0.8)
check("cycle: second Y goes dark -> light live", r.header_bg() == T["light"]["bar_bg"] and r.status_bg() == T["light"]["status_bg"])
check("cycle: light is saved in state.toml", 'theme = "light"' in saved(r))
r.send(MENU + b"y", 0.8)
check("cycle: third Y goes light -> auto and re-detects at once (terminal said dark)", r.header_bg() == T["dark"]["bar_bg"])
check("cycle: auto saves nothing (the override is cleared)", "theme" not in saved(r))
check("cycle: the status line says what auto did", "theme: auto -> dark" in r.text())
r.send(MENU, 0.6)
check("cycle: label back to auto (dark)", "Theme: auto (dark)" in r.text())
r.send(b"\x1b", 0.4)
r.send(MENU + b"y", 0.6); r.send(MENU + b"y", 0.8)   # dark, then light: saved again
r.quit()
r = Run("toggle", '[defaults]\ntheme = "auto"\n', answer=DARK_REPLY, fresh=False)
check("cycle: a restart with theme = auto restores the saved light", r.header_bg() == T["light"]["bar_bg"])
r.quit()
r = Run("toggle", '[defaults]\ntheme = "dark"\n', fresh=False)
check("cycle: an explicit config theme beats the saved choice", r.header_bg() == T["dark"]["bar_bg"])
r.quit()

# The reported bug: a light terminal stuck on a saved dark. Cycling back to auto fixes it live,
# and the next start detects the terminal again.
r = Run("stuck", '[defaults]\ntheme = "auto"\n', answer=LIGHT_REPLY, state_text='theme = "dark"\n')
check("stuck: the saved dark wins at start", r.header_bg() == T["dark"]["bar_bg"])
r.send(MENU + b"y", 0.8)
check("stuck: Y goes dark -> light", r.header_bg() == T["light"]["bar_bg"])
r.send(MENU + b"y", 0.8)
check("stuck: Y again goes to auto, which is light on this terminal", r.header_bg() == T["light"]["bar_bg"] and "theme: auto -> light" in r.text())
check("stuck: the saved override is gone", "theme" not in saved(r))
r.quit()
r = Run("stuck", '[defaults]\ntheme = "auto"\n', answer=LIGHT_REPLY, fresh=False)
check("stuck: the next start asks the terminal and picks light", r.asked and r.header_bg() == T["light"]["bar_bg"])
r.quit()
r = Run("stuckfgbg", '[defaults]\ntheme = "auto"\n', answer=None, env_extra={"COLORFGBG": "0;15"}, state_text='theme = "dark"\n')
r.send(MENU + b"y", 0.8); r.send(MENU + b"y", 0.8)
check("auto with a silent terminal falls back to COLORFGBG live", r.header_bg() == T["light"]["bar_bg"] and "COLORFGBG" in r.text())
r.quit()

# ------------------------------------------------------------ ANSI remap and untouched colours
PRINT = b"printf '\\033[31mRE''DTXT\\033[0m \\033[38;5;196mIDX''196\\033[0m \\033[38;2;10;20;30mTRU''ECOL\\033[0m plain''TXT\\n'\n"
for th in ("dark", "light"):
    r = Run("ansi_" + th, f'[defaults]\ntheme = "{th}"\n')
    r.send(PRINT, 1.0)
    red, idx, tru, pl = (r.find(p, last=True) for p in ("REDTXT", "IDX196", "TRUECOL", "plainTXT"))
    check(f"{th}: ANSI red in the shell becomes the theme red", red and r.cell(*red).fg == T[th]["ansi"][1])
    check(f"{th}: 256-colour text is untouched", idx and r.cell(*idx).fg == FG_BG_256[196])
    check(f"{th}: truecolor text is untouched", tru and r.cell(*tru).fg == "0a141e")
    check(f"{th}: default colours stay the terminal's (not painted)", pl and r.cell(*pl).fg == "default" and r.cell(*pl).bg == "default")
    r.quit()
r = Run("noremap", '[defaults]\ntheme = "dark"\nansi_remap = false\n')
r.send(PRINT, 1.0)
red, idx = r.find("REDTXT", last=True), r.find("IDX196", last=True)
ref = pyte.Screen(10, 2); pyte.ByteStream(ref).feed(b"\x1b[38;5;1mX")
check("ansi_remap = false keeps the terminal's own red (index 1)", red and r.cell(*red).fg == ref.buffer[0][0].fg and r.cell(*red).fg != T["dark"]["ansi"][1])
check("ansi_remap = false: 256-colour text still untouched", idx and r.cell(*idx).fg == FG_BG_256[196])
r.quit()

# ------------------------------------------------------------ 256-colour fallback
for ct in (None, "256color"):
    r = Run("c256_" + str(ct), '[defaults]\ntheme = "dark"\n', colorterm=ct)
    want = FG_BG_256[quantize(T["dark"]["bar_bg"])]
    check(f"COLORTERM={ct}: bars use a 256-colour index (nearest to the theme colour)", r.header_bg() == want)
    check(f"COLORTERM={ct}: output has no 24-bit colour escapes", not re.search(rb"[34]8;2;", bytes(r.raw)))
    r.quit()
r = Run("c24", '[defaults]\ntheme = "dark"\n', colorterm="24bit")
check("COLORTERM=24bit uses RGB", b"48;2;46;52;64" in bytes(r.raw))
r.quit()

# ------------------------------------------------------------ previews use theme colours
def preview_checks(th):
    r = Run("pv_" + th, f'[defaults]\ntheme = "{th}"\npreview_side = "tab"\n')
    t = T[th]
    r.send(MENU + b"p", 0.5); r.send(b"a.md\r", 1.0)
    p = r.find("Mainhead")
    check(f"{th}: markdown heading uses the h1 colour", p and r.cell(*p).fg == t["md_h1"])
    check(f"{th}: preview body is painted with the theme background", r.cell(60, 10).bg == t["bg"])
    r.send(b"\x1b", 0.5)
    r.send(MENU + b"p", 0.5); r.send(b"b.json\r", 1.0)
    p = r.find("keyname"); q = r.find("strval"); n = r.find(r"12")
    check(f"{th}: json key uses the key colour", p and r.cell(*p).fg == t["json_key"])
    check(f"{th}: json string uses the string colour", q and r.cell(*q).fg == t["json_string"])
    r.send(b"\x1b", 0.5)
    r.send(MENU + b"p", 0.5); r.send(b"c.csv\r", 1.0)
    p = r.find("idcol")
    check(f"{th}: csv header uses the table header colour", p and r.cell(*p).fg == t["table_header"])
    r.send(b"\x1b", 0.5)
    r.send(MENU + b"g", 1.5)
    add, rem = r.find(r"\+CHANGED"), r.find(r"-orig 4")
    check(f"{th}: diff added line uses the add colour", add and r.cell(*add).fg == t["diff_add"])
    check(f"{th}: diff removed line uses the remove colour", rem and r.cell(*rem).fg == t["diff_remove"])
    r.quit()
preview_checks("dark")
preview_checks("light")

print()
print("FAILED: " + ", ".join(fails) if fails else "all passed")
sys.exit(1 if fails else 0)
