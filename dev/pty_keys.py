# usage: python dev/pty_keys.py <scratch-dir> local|remote   (needs pyte)
# remote: yes
# Key-protocol layer: Shift+Enter and friends reported with kitty CSI u / modifyOtherKeys are
# translated to what the program in the tab expects. A perl "key echo" program prints each chunk
# of input as hex. Uses a temp config and fake agent commands only. Local: the agent tab is a
# CLAUDE.md workspace; remote: an OSC title containing "claude" marks the tab as an agent.
# Remote mode uses its own tmux server (socket keystest) and removes it afterwards.
import os, pty, sys, time, select, struct, fcntl, termios, shutil, subprocess, pyte
S, mode = sys.argv[1], sys.argv[2]
remote = mode == "remote"
S = os.path.realpath(S)
MENU = b"\x00"
PS, PE = b"\x1b[200~", b"\x1b[201~"
CFGDIR = S + "/keys_cfg_" + mode
shutil.rmtree(CFGDIR, ignore_errors=True); os.makedirs(CFGDIR)
WS = S + "/keys_ws"
shutil.rmtree(WS, ignore_errors=True); os.makedirs(WS)
open(WS + "/CLAUDE.md", "w").write("# project\n")
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
KE_SRC = r'''
system("stty raw -echo"); $| = 1;
while (sysread(STDIN, $b, 256)) {
  last if $b eq "q";
  if ($b eq "p") { print "\e[>1u"; }
  elsif ($b eq "o") { print "\e[<u"; }
  elsif ($b eq "Q") { print "\e[?u"; }
  print "KE:" . unpack("H*", $b) . "\r\n";
}
system("stty sane");
'''
RDIR = "/home/dev/keys-test"
if remote:
    subprocess.run(["docker", "exec", "-i", "-u", "dev", "devbox-test", "sh", "-c", f"mkdir -p {RDIR} && cat > {RDIR}/ke.pl"], input=KE_SRC, text=True, check=True)
    KE = f"perl {RDIR}/ke.pl"
else:
    open(CFGDIR + "/ke.pl", "w").write(KE_SRC)
    KE = f"perl {CFGDIR}/ke.pl"

def cleanup():
    if remote:
        subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "sh", "-c", f"rm -rf {RDIR}; tmux -L keystest kill-server 2>/dev/null; true"], capture_output=True)
    shutil.rmtree(CFGDIR, ignore_errors=True); shutil.rmtree(WS, ignore_errors=True)

fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

class Run:
    def __init__(self, extra=""):
        cfg = CFGDIR + "/config.toml"
        base = open(S + "/test.toml").read()
        open(cfg, "w").write(base + f'\n[defaults]\nagent_start = "always"\n{extra}\n[agents.claude]\ncommand = "{KE}"\n')
        if os.path.exists(CFGDIR + "/state.toml"): os.remove(CFGDIR + "/state.toml")
        env = dict(os.environ, WICLI_CONFIG=cfg, TERM="xterm-256color", WICLI_TMUX_SOCKET="keystest")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(WS)
            os.execve(WI, ["wi", "test"] if remote else ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
        self.scr = pyte.Screen(110, 30); self.st = pyte.ByteStream(self.scr)
        self.raw = bytearray()
        self.pump(3.0 if remote else 2.2)
    def pump(self, t):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try:
                    d = os.read(self.fd, 65536); self.raw.extend(d); self.st.feed(d)
                except OSError: return
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)).rstrip() for y in range(self.scr.lines)]
    def text(self): return "\n".join(self.rows())
    def ke(self):
        k = [l.strip()[3:] for l in self.rows() if l.strip().startswith("KE:")]
        return k[-1] if k else None
    def send(self, b, t=0.7): os.write(self.fd, b); self.pump(t)
    def key(self, b, t=0.7):
        """Send b, return the hex the key-echo program saw (None if nothing new)."""
        before = [l for l in self.rows() if l.strip().startswith("KE:")]
        self.send(b, t)
        after = [l for l in self.rows() if l.strip().startswith("KE:")]
        return self.ke() if after != before or len(after) else None
    def quit(self):
        self.send(MENU + (b"e" if remote else b"q"), 1.5)
        try: os.waitpid(self.pid, os.WNOHANG)
        except ChildProcessError: pass

def start_agent_tab(r):
    """Make the first tab a key-echo agent tab and return once KE runs."""
    if remote:
        r.send(("printf '\\033]0;claude-fake\\007'; " + KE + "\r").encode(), 1.2)
    # local: started by wi itself (CLAUDE.md workspace, agent_start = always)
    r.send(b"x", 0.7)
    return r.ke() == "78"

# ---- 1. agent tab, auto mapping
r = Run()
check("agent tab runs the key-echo program", start_agent_tab(r))
check("Shift+Enter as CSI u reaches an agent tab as LF", r.key(b"\x1b[13;2u") == "0a")
check("Shift+Enter as modifyOtherKeys reaches an agent tab as LF", r.key(b"\x1b[27;2;13~") == "0a")
check("Ctrl+Enter behaves like Shift+Enter", r.key(b"\x1b[13;5u") == "0a")
check("plain Enter is CR", r.key(b"\r") == "0d")
check("plain Enter as CSI u is CR", r.key(b"\x1b[13u") == "0d")
check("Alt+Enter as CSI u is ESC CR", r.key(b"\x1b[13;3u") == "1b0d")
check("Alt+Enter legacy ESC CR untouched", r.key(b"\x1b\r") == "1b0d")
check("Esc as CSI 27 u reaches the program as 1b", r.key(b"\x1b[27u") == "1b")
check("a real lone Esc still reaches the program", r.key(b"\x1b", 0.9) == "1b")
check("Ctrl+C as CSI u arrives as 03", r.key(b"\x1b[99;5u") == "03")
check("Shift+Tab as CSI u arrives as CSI Z", r.key(b"\x1b[9;2u") == "1b5b5a")
check("Alt+x as CSI u arrives as ESC x", r.key(b"\x1b[120;3u") == "1b78")
check("arrow keys untouched", r.key(b"\x1b[A") == "1b5b41")
check("modified arrows untouched", r.key(b"\x1b[1;5C") == "1b5b313b3543")
check("unicode text untouched", r.key("é日🎉".encode()) == "c3a9e697a5f09f8e89")
check("shifted letter as CSI u arrives as the capital", r.key(b"\x1b[97:65;2u") == "41")
check("super combos are dropped", r.key(b"\x1b[97;9u\x1b[98u") == "62")
# (remote: tmux sits between wi and the program, rewrites pastes and consumes the program's own
# kitty requests, so those checks are local only)
if not remote:
    # bracketed paste content is never decoded
    r.send(PS + b"a\x1b[13;2ub" + PE, 0.9)
    check("paste containing literal CSI u is delivered as written", r.ke() == "611b5b31333b32756 2".replace(" ", ""))

# Ctrl+Space as CSI u opens the menu, in both encodings
r.send(b"\x1b[32;5u", 0.8)
check("Ctrl+Space as CSI u opens the menu", "New shell tab" in r.text() or "arrows + Enter" in r.text())
r.send(b"\x1b", 0.9)
check("Esc closes the menu", "arrows + Enter" not in r.text() and "New shell tab" not in r.text())
r.send(b"\x1b[27;5;32~", 0.8)
check("Ctrl+Space as modifyOtherKeys opens the menu", "New shell tab" in r.text() or "arrows + Enter" in r.text())
r.send(b"\x1b", 0.9)
if not remote:
    # the program turns the kitty protocol on itself: raw reports are forwarded; LF again after it pops
    r.send(b"Q", 0.8)
    check("query before the program enables anything answers flags 0", "KE:1b5b3f3075" in r.text())
    r.send(b"p", 0.8)
    check("program with kitty flags gets raw CSI u for Shift+Enter", r.key(b"\x1b[13;2u") == "1b5b31333b3275")
    check("it also gets raw Ctrl+C report", r.key(b"\x1b[99;5u") == "1b5b39393b3575")
    r.send(b"Q", 0.8)
    check("query answers the tab's flags", "KE:1b5b3f3175" in r.text())
    r.send(b"\x1b[32;5u", 0.8)
    check("Ctrl+Space still opens the menu with the protocol on in the tab", "New shell tab" in r.text() or "arrows + Enter" in r.text())
    r.send(b"\x1b", 0.9)
    r.send(b"o", 0.8)
    check("after the program pops, Shift+Enter is LF again", r.key(b"\x1b[13;2u") == "0a")

# plain (non-agent) tab
r.send(MENU + b"n", 1.5)
r.send((KE + "\r").encode(), 1.2)
r.send(b"x", 0.7)
check("plain tab runs the key-echo program", r.ke() == "78")
check("Shift+Enter in a plain tab is CR", r.key(b"\x1b[13;2u") == "0d")
check("modifyOtherKeys Shift+Enter in a plain tab is CR", r.key(b"\x1b[27;2;13~") == "0d")
check("Alt+Enter in a plain tab is ESC CR", r.key(b"\x1b[13;3u") == "1b0d")
r.send(b"q", 0.8)
# Ctrl+C as CSI u interrupts a running command in a shell tab
r.send(b"sleep 30\r", 0.8)
r.send(b"\x1b[99;5u", 0.8)
r.send(b"echo ALIVE$((1+1))\r", 1.0)
check("Ctrl+C as CSI u interrupts sleep in a shell tab", "ALIVE2" in r.text().replace("echo ALIVE$((1+1))", ""))
r.send(b"\x1b[27;5;99~sleep 30\r", 0.8)
r.send(b"\x1b[27;5;99~", 0.8)
r.send(b"echo BACK$((2+3))\r", 1.0)
check("Ctrl+C as modifyOtherKeys interrupts too", "BACK5" in r.text().replace("echo BACK$((2+3))", ""))
r.raw.clear()
r.quit()
raw = bytes(r.raw)
check("wi pops the key protocol on quit, before leaving the alternate screen",
      b"\x1b[<u\x1b[>4;0m" in raw and raw.rfind(b"\x1b[<u\x1b[>4;0m") < raw.rfind(b"\x1b[?1049l"))

# ---- 2. the host request itself
r = Run()
check("wi requests kitty flags and modifyOtherKeys on start", b"\x1b[>1u" in bytes(r.raw) and b"\x1b[>4;2m" in bytes(r.raw))
r.send(MENU + b"i", 0.8)
check("Info says standard keyboard before any report", "keys: standard" in r.text())
r.send(b"\x1b[13;2u", 0.5)
r.send(MENU + b"i", 0.8)
check("Info says enhanced keyboard after a report", "keys: enhanced keyboard active" in r.text())
r.quit()

# ---- 3. overrides
for setting, want in (("esc-cr", "1b0d"), ("newline", "0a"), ("enter", "0d")):
    r = Run(f'shift_enter = "{setting}"')
    ok = start_agent_tab(r)
    got = r.key(b"\x1b[13;2u")
    check(f'shift_enter = "{setting}" in an agent tab sends {want}', ok and got == want)
    if setting == "newline":
        r.send(MENU + b"n", 1.5); r.send((KE + "\r").encode(), 1.2); r.send(b"x", 0.7)
        check('shift_enter = "newline" also applies in a plain tab', r.key(b"\x1b[13;2u") == "0a")
        r.send(b"q", 0.5)
    r.quit()

# ---- 4. key_protocol = off: no request, nothing decoded
r = Run('key_protocol = "off"')
check("key_protocol = off sends no request", b"\x1b[>1u" not in bytes(r.raw) and b"\x1b[>4;2m" not in bytes(r.raw))
ok = start_agent_tab(r)
if not remote:
    check("key_protocol = off forwards reports untouched", ok and r.key(b"\x1b[13;2u") == "1b5b31333b3275")
r.raw.clear()
r.quit()
check("key_protocol = off sends no pop on quit", b"\x1b[<u" not in bytes(r.raw))

cleanup()
sys.exit(1 if fails else 0)
