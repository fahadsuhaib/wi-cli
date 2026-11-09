# usage: python dev/pty_dictate.py <scratch-dir-with-test.toml> local   (needs pyte)
# Hands-free dictation with a FAKE engine (no microphone, no model): a script that speaks the
# `wi-audio listen` event lines from a scenario file and waits on marker files, so every phrase
# arrives exactly when the test says. Checks: the menu entry and the status indicator, N phrases
# typed with no Enter (plain and bracketed-paste panes), typing wins over an arriving phrase, the
# next phrase follows a tab switch, a phrase is held while the menu is open, idle auto-off with a
# shortened timeout, friendly errors, a missing engine, a click on the indicator turns it off, and
# the child process is gone when the mode is off.
import json, os, pty, re, sys, time, select, struct, fcntl, termios, shutil, signal, pyte
S = os.path.realpath(sys.argv[1])
MENU = b"\x00"
CFGDIR = S + "/dictate_cfg"
shutil.rmtree(CFGDIR, ignore_errors=True); os.makedirs(CFGDIR)
WS = S + "/dictate_ws"
shutil.rmtree(WS, ignore_errors=True); os.makedirs(WS)
WI = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
FAKE = CFGDIR + "/fake_engine.py"
open(FAKE, "w").write(r'''
import sys, os, time, json, threading
sc = json.load(open(os.environ["FAKE_SCENARIO"]))
open(os.environ["FAKE_PIDFILE"], "w").write(str(os.getpid()))
def watch():
    sys.stdin.read()          # EOF: wi turned the mode off or went away
    os._exit(0)
threading.Thread(target=watch, daemon=True).start()
open(os.environ["FAKE_ARGS"], "w").write(" ".join(sys.argv[1:]))
for step in sc:
    if step[0] == "sleep": time.sleep(step[1])
    elif step[0] == "wait":
        while not os.path.exists(step[1]): time.sleep(0.02)
    elif step[0] == "emit": print(json.dumps(step[1]), flush=True)
    elif step[0] == "exit": os._exit(step[1])
while True: time.sleep(1)
''')
# A program that turns bracketed paste on and prints every byte it receives as hex.
open(CFGDIR + "/bp.pl", "w").write(r'''
system("stty raw -echo"); $| = 1; print "\e[?2004h";
while (sysread(STDIN, $b, 256)) { last if $b eq "q"; print "KE:" . unpack("H*", $b) . "\r\n"; }
system("stty sane");
''')
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

def go(name): return CFGDIR + "/go_" + name
def ev(**kw): return ["emit", dict(kw)]
def state(s): return ev(ev="state", s=s)
def final(t): return ev(ev="final", text=t, ms=50)

class Run:
    def __init__(self, scenario, command=None, idle=None, tag="a"):
        for f in os.listdir(CFGDIR):
            if f.startswith("go_"): os.remove(CFGDIR + "/" + f)
        json.dump(scenario, open(CFGDIR + "/scenario.json", "w"))
        self.pidfile = CFGDIR + "/pid_" + tag
        self.argsfile = CFGDIR + "/args_" + tag
        for f in (self.pidfile, self.argsfile):
            if os.path.exists(f): os.remove(f)
        cmd = command or f"{sys.executable} -I {FAKE}"
        cfg = CFGDIR + "/config.toml"
        open(cfg, "w").write(open(S + "/test.toml").read() + f'\n[defaults]\nagent_start = "never"\n[transcribe]\ncommand = "{cmd}"\n')
        if os.path.exists(CFGDIR + "/state.toml"): os.remove(CFGDIR + "/state.toml")
        env = dict(os.environ, WICLI_CONFIG=cfg, TERM="xterm-256color", WICLI_TMUX_SOCKET="dictatetest",
                   FAKE_SCENARIO=CFGDIR + "/scenario.json", FAKE_PIDFILE=self.pidfile, FAKE_ARGS=self.argsfile)
        if idle is not None: env["WICLI_LISTEN_IDLE_SECS"] = str(idle)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(WS); os.execve(WI, ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
        self.scr = pyte.Screen(110, 30); self.st = pyte.ByteStream(self.scr)
        self.pump(3.0)
    def pump(self, t):
        end = time.time() + t
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if r:
                try: self.st.feed(os.read(self.fd, 65536))
                except OSError: return
    def rows(self):
        return ["".join((self.scr.buffer[y][x].data or " ") for x in range(self.scr.columns)).rstrip() for y in range(self.scr.lines)]
    def text(self): return "\n".join(self.rows())
    def status(self): return self.rows()[29]
    def send(self, b, t=0.5):
        os.write(self.fd, b.encode() if isinstance(b, str) else b); self.pump(t)
    def dictation(self): self.send(MENU + b" dictate\r", 1.0)
    def release(self, name):
        open(go(name), "w").close(); self.pump(0.7)
    def child_pid(self):
        try: return int(open(self.pidfile).read())
        except Exception: return None
    def child_alive(self):
        p = self.child_pid()
        if p is None: return False
        try: os.kill(p, 0); return True
        except OSError: return False
    def wait_until(self, fn, t=6.0):
        end = time.time() + t
        while time.time() < end:
            self.pump(0.2)
            if fn(): return True
        return fn()
    def quit(self):
        self.send(MENU + b"q", 1.0)
        try: os.kill(self.pid, signal.SIGKILL)
        except OSError: pass
        try: os.waitpid(self.pid, 0)
        except ChildProcessError: pass

# 1. Three phrases, plain pane (cat): typed one after another, never with Enter.
r = Run([state("loading"), state("listening"), ["wait", go("hear")], state("hearing"), ["wait", go("p1")],
         state("transcribing"), final("open the explorer"), state("listening"), ["wait", go("p2")],
         final("run the tests"), ["wait", go("p3")], final("then commit")], tag="one")
r.send(b"cat\n", 0.6)
check("dictation is off at the start: no indicator", "●" not in r.status())
r.dictation()
check("the menu entry (search 'dictate') turns it on and starts the engine", r.child_alive())
check("the status bar shows the listening indicator", "● listening" in r.status())
args = open(r.argsfile).read() if os.path.exists(r.argsfile) else ""
check("the engine gets `listen` and the tuning options", args.startswith("listen") and "--end-silence-ms 700" in args and "--min-speech-ms 250" in args)
r.release("hear")
check("hearing is shown while speech is detected", "● hearing" in r.status())
r.release("p1"); r.pump(0.8)
check("the first phrase is typed into the focused pane", "open the explorer" in r.text())
r.release("p2"); r.release("p3")
line = next((l for l in r.rows() if "open the explorer" in l), "")
check("phrases build one line, space separated", "open the explorer run the tests then commit" in line)
check("no Enter was pressed: each phrase appears once (cat would echo it again)", r.text().count("open the explorer") == 1 and r.text().count("then commit") == 1)
check("the indicator is back to listening", "● listening" in r.status())
col = r.status().find("●")
r.send(f"\x1b[<0;{col + 3};30M\x1b[<0;{col + 3};30m".encode(), 0.8)
check("the indicator is gone after the click", "●" not in r.status())
check("the engine process is gone (mic released)", r.wait_until(lambda: not r.child_alive(), 4))
check("the status line says dictation is off", "dictation off" in r.status())
r.quit()

# 2. Bracketed paste and no Enter, seen byte by byte.
r = Run([state("listening"), ["wait", go("p1")], final("fix the login bug")], tag="two")
r.send(f"perl {CFGDIR}/bp.pl\n", 0.8)
r.dictation()
r.release("p1")
hexes = [l.strip()[3:] for l in r.rows() if l.strip().startswith("KE:")]
joined = "".join(hexes)
want = "1b5b3230307e" + "fix the login bug".encode().hex() + "1b5b3230317e"
check("the phrase arrives inside bracketed-paste markers (the pane asked for them)", want in joined)
check("nothing else arrived: no carriage return or newline (no Enter)", joined == want and len(hexes) == 1)
r.quit()

# 3. Typing wins: a phrase that finishes while the user types waits for a pause.
r = Run([state("listening"), ["wait", go("p1")], final("after the typing")], tag="three")
r.send(b"cat\n", 0.6)
r.dictation()
open(go("p1"), "w").close()
for ch in "typedwords":
    r.send(ch.encode(), 0.1)
r.pump(1.2)
line = next((l for l in r.rows() if "typedwords" in l), "")
check("typed characters stay together, the phrase follows after the pause", "typedwords after the typing" in line)
r.quit()

# 4. Focus follows the user: switch tab and the next phrase goes to the new tab.
r = Run([state("listening"), ["wait", go("p1")], final("goes to second"), ["wait", go("p2")], final("goes to first")], tag="four")
r.send(b"cat\n", 0.6)
r.dictation()
r.send(MENU + b"n", 1.5)          # new shell tab (it becomes the active one)
r.send(b"cat\n", 0.6)
r.release("p1"); r.pump(0.5)
check("the phrase goes to the tab in front (the second)", "goes to second" in r.text())
r.send(MENU + b"b", 1.0)          # previous tab
r.release("p2"); r.pump(0.5)
check("after switching back, the next phrase goes to the first tab", "goes to first" in r.text() and "goes to second" not in r.text())
r.quit()

# 5. A phrase is held while the menu is open and pasted when the pane is ready again.
r = Run([state("listening"), ["wait", go("p1")], final("held phrase here")], tag="five")
r.send(b"cat\n", 0.6)
r.dictation()
r.send(MENU, 0.6)
r.release("p1")
check("while the menu is open the phrase is shown as held", 'held: "held phrase here"' in r.status())
check("and not typed yet", "held phrase here" not in "\n".join(r.rows()[:28]))
r.send(b"\x1b", 1.0)
check("closing the menu types it", "held phrase here" in "\n".join(r.rows()[:28]) and "held:" not in r.status())
r.quit()

# 6. Silence only: nothing is typed, and the mode ends by itself after the (shortened) idle time.
r = Run([state("loading"), state("listening")], idle=2, tag="six")
r.send(b"cat\n", 0.6)
r.dictation()
check("silence: on and listening", "● listening" in r.status())
check("silence: the engine was asked to stop on its own a little later than wi does", "--idle-secs 12" in open(r.argsfile).read())
check("silence: idle auto-off ends the mode with a note", r.wait_until(lambda: "dictation off: 2 s of silence" in r.status(), 6))
check("silence: indicator gone, engine process gone", "●" not in r.status() and r.wait_until(lambda: not r.child_alive(), 4))
check("silence: nothing was typed", "silence" not in "\n".join(r.rows()[:28]))
r.quit()

# 7. Errors are one friendly line.
r = Run([state("loading"), ev(ev="error", kind="no_mic", msg="no microphone found"), ["exit", 2]], tag="seven")
r.dictation()
r.pump(1.0)
check("no microphone: a friendly line, mode off", "dictation: no microphone found" in r.status() and "●" not in r.status())
r.quit()
r = Run([state("loading"), ev(ev="error", kind="mic_denied", msg="microphone access denied: allow your terminal app in System Settings > Privacy & Security > Microphone, then try again"), ["exit", 2]], tag="eight")
r.dictation(); r.pump(1.0)
check("mic denied: names the Settings path", "System Settings > Privacy & Security > Microphone" in r.status())
r.quit()
r = Run([state("listening"), ev(ev="error", kind="mic_silent", msg="ignored, wi words it itself"), ["wait", go("p1")], final("still works")], tag="nine")
r.send(b"cat\n", 0.6); r.dictation(); r.pump(0.8)
check("a non-fatal error shows its line and keeps listening", "no sound from the mic" in r.status() and r.child_alive())
r.release("p1")
check("and later phrases still arrive", "still works" in r.text())
r.quit()
r = Run([], command="/nonexistent/wi-audio", tag="ten")
r.dictation(); r.pump(0.5)
check("missing engine: one line with where to get it and what to run", "no wi-audio found" in r.status() and "wi-audio models install" in r.status())
r.quit()
r = Run([state("listening"), ["exit", 1]], tag="eleven")
r.dictation(); r.pump(1.0)
check("an engine that exits on its own ends the mode with a note", "dictation stopped" in r.status() and "●" not in r.status())
r.quit()

# 8. Menu toggles it off again; quitting wi leaves no engine behind.
r = Run([state("listening")], tag="twelve")
r.dictation()
check("on again", r.child_alive())
r.dictation()
check("the same menu entry turns it off, the engine ends", r.wait_until(lambda: not r.child_alive(), 4) and "dictation off" in r.status())
r.dictation()
check("it can be turned on a second time", r.wait_until(r.child_alive, 4))
r.quit()
check("quitting wi leaves no engine process", r.wait_until(lambda: not r.child_alive(), 4))

print("\n%d failed" % len(fails) if fails else "\nall passed")
sys.exit(1 if fails else 0)
