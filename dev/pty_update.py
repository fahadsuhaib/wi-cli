# usage: python dev/pty_update.py <scratch-dir> local   (needs pyte)
# "Update wi" and "Restart wi" from the menu, with no internet: WICLI_UPDATE_URL points at a file://
# folder holding a fake release (latest.json, the archive for this platform, SHA256SUMS). The binary
# under test is a copy of target/debug/wi, so the real one is never replaced.
# Covers: declining the first question (nothing happens), up to date, a real update (the replaced
# binary's --version), a checksum mismatch, an unwritable folder (prints the sudo command), offline,
# no curl/wget, an unsupported platform, and restarting with two tabs in different folders.
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, shutil, re, json, hashlib, platform, pyte
S = sys.argv[1]
MENU = b"\x00"
HERE = os.path.dirname(os.path.abspath(__file__))
REAL = os.path.realpath(os.path.join(HERE, "..", "target/debug/wi"))
W = os.path.realpath(S) + "/updatews"
shutil.rmtree(W, ignore_errors=True); os.makedirs(W)
CUR = subprocess.run([REAL, "--version"], capture_output=True, text=True).stdout.split()[1]
NEW = "9.9.9"
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

def asset_dir_name():
    if sys.platform == "darwin": return "macos-universal"
    return "linux-" + ("aarch64" if platform.machine() in ("arm64", "aarch64") else "x86_64")

def make_release(tag, version=NEW, bad_sum=False, name="rel"):
    d = W + "/" + name
    shutil.rmtree(d, ignore_errors=True); os.makedirs(d + "/stage/wi-" + asset_dir_name())
    # The "new binary" answers --version itself and otherwise runs the real wi (so a restart works).
    script = f'#!/bin/sh\nif [ "$1" = "--version" ]; then echo "wi {version}"; exit 0; fi\necho started >> "{W}/wrapper.log"\nexec "{REAL}" "$@"\n'
    p = d + "/stage/wi-" + asset_dir_name() + "/wi"
    open(p, "w").write(script); os.chmod(p, 0o755)
    asset = f"wi-{asset_dir_name()}.tar.gz"
    subprocess.run(["tar", "-czf", d + "/" + asset, "-C", d + "/stage", "wi-" + asset_dir_name()], check=True)
    h = hashlib.sha256(open(d + "/" + asset, "rb").read()).hexdigest()
    if bad_sum: h = "0" * 64
    open(d + "/SHA256SUMS", "w").write(f"{h}  {asset}\n")
    json.dump({"tag_name": tag, "assets": [{"name": asset, "size": os.path.getsize(d + "/" + asset)}]}, open(d + "/latest.json", "w"))
    return "file://" + d

def fresh_bin(name, writable=True):
    d = W + "/" + name
    shutil.rmtree(d, ignore_errors=True) if writable else subprocess.run(["chmod", "-R", "u+w", d], stderr=subprocess.DEVNULL)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d); shutil.copy(REAL, d + "/wi")
    if not writable: os.chmod(d, 0o555)
    return d + "/wi"

def version_of(exe):
    return subprocess.run([exe, "--version"], capture_output=True, text=True).stdout.strip()

class Run:
    def __init__(self, tag, exe, url=None, env_extra=None, cwd=None):
        self.cfgdir = W + "/cfg_" + tag
        shutil.rmtree(self.cfgdir, ignore_errors=True); os.makedirs(self.cfgdir)
        open(self.cfgdir + "/config.toml", "w").write('[defaults]\ntheme = "dark"\n')
        env = {k: v for k, v in os.environ.items() if not k.startswith("WICLI_UPDATE") and k != "WICLI_RESTORE"}
        env.update(WICLI_CONFIG=self.cfgdir + "/config.toml", TERM="xterm-256color", SHELL="/bin/sh", PS1="$ ")
        if url: env["WICLI_UPDATE_URL"] = url
        env.update(env_extra or {})
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd or W)
            os.execve(exe, ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 50, 100, 0, 0))
        self.scr = pyte.Screen(100, 50); self.st = pyte.ByteStream(self.scr)
        self.pump(1.5)
    def pump(self, t, until=None):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.02)
            if r:
                try: data = os.read(self.fd, 65536)
                except OSError: return False
                self.st.feed(data)
            if until and until(): return True
        return until is None
    def text(self): return "\n".join("".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)).rstrip() for y in range(self.scr.lines))
    def send(self, b, t=0.5): os.write(self.fd, b); self.pump(t)
    def wait(self, pat, t=15):
        return self.pump(t, lambda: re.search(pat, self.text()) is not None)
    def menu(self, query):
        self.send(MENU, 0.4); self.send(b" ", 0.3); self.send(query.encode(), 0.4); self.send(b"\r", 0.5)
    def quit(self):
        try: os.kill(self.pid, 9)
        except OSError: pass
        try: os.waitpid(self.pid, 0)
        except OSError: pass

def full_update(r, restart_answer=b"n"):
    r.menu("update")
    ok1 = r.wait(r"contacts github\.com", 3)
    r.send(b"\r", 0.3)
    ok2 = r.wait(rf"Update {re.escape(CUR)} -> {NEW}\?", 15)
    r.second = r.text()
    r.send(b"\r", 0.3)
    ok3 = r.wait(r"Restart wi now\?", 30) or r.wait(r"not writable|failed|mismatch", 1)
    return ok1, ok2, ok3

# ---------------------------------------------------------------- decline the first question
url = make_release("v" + NEW)
exe = fresh_bin("bin_decline")
r = Run("decline", exe, url)
r.menu("update")
check("first question says it will contact github.com and sends nothing yet", r.wait(r"contacts github\.com", 3) and "nothing is sent until you confirm" in r.text().replace("\n", " ").lower())
check("the question names the repository", "fahadsuhaib/wi-cli" in r.text())
r.send(b"n", 2.0)
t = r.text()
check("declining does nothing but say cancelled", "cancelled" in t and "Update " not in t.split("cancelled")[0][-0:] and "Could not reach" not in t and "up to date" not in t)
check("the binary is untouched after declining", version_of(exe) == f"wi {CUR}")
r.quit()

# ---------------------------------------------------------------- up to date
url = make_release("v" + CUR)
exe = fresh_bin("bin_uptodate")
r = Run("uptodate", exe, url)
r.menu("update"); r.send(b"\r", 0.3)
check("up to date: plain status line", r.wait(rf"wi {re.escape(CUR)} is up to date", 10))
check("up to date: no further prompt", "Update " not in r.text().replace("Update wi", "") and "Restart" not in r.text())
r.quit()
# an older release is not an update either
url = make_release("v0.0.1")
r = Run("older", exe, url)
r.menu("update"); r.send(b"\r", 0.3)
check("an older release counts as up to date", r.wait(r"is up to date", 10))
r.quit()

# ---------------------------------------------------------------- update succeeds
url = make_release("v" + NEW)
exe = fresh_bin("bin_ok")
r = Run("ok", exe, url)
ok = full_update(r)
check("update: both questions and the restart question appear", all(ok))
check("update: the second question says size and what it does", "MB" in r.second and "verifies SHA256" in r.second and "replaces this binary" in r.second)
r.send(b"n", 0.8)
check("update: declining the restart says to restart later", "Updated to 9.9.9 — restart wi when convenient" in r.text())
check("update: the binary was replaced (--version)", version_of(exe) == f"wi {NEW}")
check("update: the new binary is executable", os.access(exe, os.X_OK))
check("update: no leftover staging file next to the binary", [f for f in os.listdir(os.path.dirname(exe)) if f.startswith(".wi-update")] == [])
check("update: wi is still running with its tab", r.pump(0.3) and "$" in r.text())
r.quit()

# ---------------------------------------------------------------- checksum mismatch
url = make_release("v" + NEW, bad_sum=True)
exe = fresh_bin("bin_bad")
r = Run("bad", exe, url)
full_update(r)
check("mismatch: refused with a clear message", r.wait(r"checksum mismatch", 5))
check("mismatch: the binary is untouched", version_of(exe) == f"wi {CUR}")
r.quit()

# ---------------------------------------------------------------- unwritable folder
url = make_release("v" + NEW)
exe = fresh_bin("bin_ro", writable=False)
r = Run("ro", exe, url)
full_update(r)
check("unwritable: says so in the status line", r.wait(r"folder is not writable", 5))
check("unwritable: the sudo command is shown in a tab", r.wait(r"sudo install -m 755 \\", 5) and "wi-update-" in r.text())
check("unwritable: the binary is untouched", version_of(exe) == f"wi {CUR}")
r.quit()
subprocess.run(["chmod", "-R", "u+w", os.path.dirname(exe)])

# ---------------------------------------------------------------- failures: offline, no curl, platform
exe = fresh_bin("bin_fail")
r = Run("offline", exe, "file://" + W + "/does-not-exist")
r.menu("update"); r.send(b"\r", 0.3)
check("offline: Could not reach ... check your connection", r.wait(r"Could not reach .* — check your connection", 20))
check("offline: wi still responds", r.pump(0.3) and "$" in r.text())
r.quit()
empty = W + "/emptypath"; os.makedirs(empty, exist_ok=True)
r = Run("nocurl", exe, make_release("v" + NEW), env_extra={"PATH": empty})
r.menu("update"); r.send(b"\r", 0.3)
check("no curl/wget: short clear message", r.wait(r"needs curl or wget", 10))
r.quit()
r = Run("platform", exe, make_release("v" + NEW), env_extra={"WICLI_UPDATE_PLATFORM": "plan9/mips"})
r.menu("update"); r.send(b"\r", 0.3)
check("unsupported platform: short clear message", r.wait(r"no wi build for plan9/mips", 10))
r.quit()

# ---------------------------------------------------------------- restart with two tabs
A, B = W + "/folder_a", W + "/folder_b"
os.makedirs(A); os.makedirs(B)
def two_tabs(r):
    r.send(f"cd {A}\r".encode(), 0.6)
    r.send(MENU + b"n", 1.0)
    r.send(f"cd {B}\r".encode(), 0.6)
def pwd_ok(r, want):
    r.send(b"clear; echo MARK-$(basename $PWD)-END\r", 0.8)
    return r.wait(r"MARK-" + re.escape(os.path.basename(want)) + "-END", 3)

# plain "Restart wi" from the menu, no update
exe = fresh_bin("bin_restart")
r = Run("restart", exe, None)
two_tabs(r)
r.menu("restart")
check("restart: the question counts the tabs", r.wait(r"2 tabs will be restarted", 3))
r.send(b"n", 0.6)
check("restart: no keeps running", "restart wi when convenient" in r.text())
r.menu("restart"); r.send(b"\r", 0.3)
check("restart: yes brings the tabs back", r.wait(r"wi restarted: 2 tab\(s\) back", 15))
check("restart: the snapshot is consumed", "restart" not in (open(r.cfgdir + "/state.toml").read() if os.path.exists(r.cfgdir + "/state.toml") else ""))
check("restart: the active tab is back in its folder", pwd_ok(r, B))
r.send(MENU + b"b", 0.8)
check("restart: the other tab is back in its folder", pwd_ok(r, A))
r.quit()

# update and then restart into the new binary
url = make_release("v" + NEW)
exe = fresh_bin("bin_upd_restart")
log = W + "/wrapper.log"
if os.path.exists(log): os.remove(log)
r = Run("updrestart", exe, url)
two_tabs(r)
full_update(r)
check("update+restart: the question counts the tabs", r.wait(r"2 tabs will be restarted", 3))
r.send(b"\r", 0.3)
check("update+restart: the restarted wi reports its tabs", r.wait(r"wi restarted: 2 tab\(s\) back", 20))
check("update+restart: the new version is what started", os.path.exists(log) and version_of(exe) == f"wi {NEW}")
check("update+restart: active tab back in its folder", pwd_ok(r, B))
r.send(MENU + b"b", 0.8)
check("update+restart: other tab back in its folder", pwd_ok(r, A))
r.quit()

print()
print("FAILED: " + ", ".join(fails) if fails else "all passed")
sys.exit(1 if fails else 0)
