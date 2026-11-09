# usage: python dev/pty_dictate.py <scratch-dir-with-test.toml> local   (needs pyte)
# Hands-free dictation with a FAKE engine (no microphone, no model): a script that speaks the
# `wi-audio listen` event lines from a scenario file and waits on marker files, so every phrase
# arrives exactly when the test says. Checks: the menu entry and the status indicator, N phrases
# typed with no Enter (plain and bracketed-paste panes), typing wins over an arriving phrase, the
# next phrase follows a tab switch, a phrase is held while the menu is open, idle auto-off with a
# shortened timeout, friendly errors, a missing helper (guided install popup), the model download,
# the microphone test, the options screen, the mic button in the top bar (click on/off, tabs
# unaffected, narrow width), and the child process is gone when the mode is off.
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
sub = sys.argv[1] if len(sys.argv) > 1 else ""
flag = os.environ.get("FAKE_MODEL_FLAG", "")
def out(**kw): print(json.dumps(kw), flush=True)
if sub == "status":
    sp = os.path.exists(flag)
    best = os.path.exists(flag + ".best") or not os.environ.get("FAKE_OLD_MODEL")
    out(ev="status", protocol=1, version="0.0.0", speech=sp, speech_bytes=295793568 if sp else 0, speech_total=295793568, vad=True, models_dir="x", best="medium", best_present=bool(sp and best))
    sys.exit(0)
if sub == "fetch-models":
    for pct in (10, 40, 70):
        out(ev="progress", what="small", done=295793568 * pct // 100, total=295793568); time.sleep(0.7)
    if os.environ.get("FAKE_FETCH_FAIL"):
        out(ev="error", kind="download", msg="the download failed: check your connection"); sys.exit(2)
    open(flag, "w").write("x"); open(flag + ".best", "w").write("x")
    open(os.environ.get("FAKE_FETCH_LOG", "/dev/null"), "a").write(" ".join(sys.argv[1:]) + "\n")
    out(ev="done"); sys.exit(0)
if sub == "probe":
    for i in range(12):
        out(ev="level", v=0.2 + 0.6 * (i % 3) / 2); time.sleep(0.1)
    if os.environ.get("FAKE_PROBE_ERR"):
        out(ev="error", kind="mic_silent", msg="x"); sys.exit(2)
    out(ev="heard", text="hello from the test", peak=0.5); sys.exit(0)
if sub == "models":
    if os.path.exists(flag): os.remove(flag)
    sys.exit(0)
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
SERVE = CFGDIR + "/serve"
os.makedirs(SERVE)
PKG = CFGDIR + "/pkg/wi-audio-macos-arm64"
os.makedirs(PKG)
open(PKG + "/wi-audio", "w").write('#!/bin/sh\nif [ "$1" = "--version" ]; then echo "wi-audio 0.0.0"; exit 0; fi\nexec %s -I %s "$@"\n' % (sys.executable, FAKE))
os.chmod(PKG + "/wi-audio", 0o755)
import subprocess, hashlib
subprocess.run(["tar", "-czf", SERVE + "/wi-audio-macos-arm64.tar.gz", "-C", CFGDIR + "/pkg", "wi-audio-macos-arm64"], check=True)
open(SERVE + "/SHA256SUMS", "w").write("%s  wi-audio-macos-arm64.tar.gz\n" % hashlib.sha256(open(SERVE + "/wi-audio-macos-arm64.tar.gz", "rb").read()).hexdigest())
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)

def go(name): return CFGDIR + "/go_" + name
def ev(**kw): return ["emit", dict(kw)]
def state(s): return ev(ev="state", s=s)
def final(t): return ev(ev="final", text=t, ms=50)

class Run:
    def __init__(self, scenario, command=None, idle=None, tag="a", model=True, mic_ok=True, declined=(), missing=False, serve=False, extra_env=None, cols=110, auto_send=False):
        for f in os.listdir(CFGDIR):
            if f.startswith("go_"): os.remove(CFGDIR + "/" + f)
        json.dump(scenario, open(CFGDIR + "/scenario.json", "w"))
        self.pidfile = CFGDIR + "/pid_" + tag
        self.argsfile = CFGDIR + "/args_" + tag
        for f in (self.pidfile, self.argsfile):
            if os.path.exists(f): os.remove(f)
        cmd = command or f"{sys.executable} -I {FAKE}"
        self.flag = CFGDIR + "/model_" + tag
        if os.path.exists(self.flag): os.remove(self.flag)
        if os.path.exists(self.flag + ".best"): os.remove(self.flag + ".best")
        if model: open(self.flag, "w").write("x")
        wi = WI
        self.wibin = CFGDIR + "/wibin_" + tag
        self.vbin = CFGDIR + "/vbin_" + tag
        shutil.rmtree(self.wibin, ignore_errors=True); shutil.rmtree(self.vbin, ignore_errors=True)
        if missing:
            # the real lookup: next to this copy of wi, then the fallback folder, then PATH (empty here)
            os.makedirs(self.wibin); wi = self.wibin + "/wi"; shutil.copy(WI, wi)
            cmd = "wi-audio"
        cfg = CFGDIR + "/config.toml"
        open(cfg, "w").write(open(S + "/test.toml").read() + '\n[defaults]\nagent_start = "never"\n')
        self.state = CFGDIR + "/state.toml"
        if os.path.exists(self.state): os.remove(self.state)
        st = ("voice_mic_ok = true\n" if mic_ok else "") + ("voice_declined = [%s]\n" % ", ".join('"%s"' % d for d in declined) if declined else "")
        if auto_send: st += "voice_auto_send = true\n"
        if st: open(self.state, "w").write(st)
        env = dict(os.environ, WICLI_CONFIG=cfg, TERM="xterm-256color", WICLI_TMUX_SOCKET="dictatetest",
                   FAKE_SCENARIO=CFGDIR + "/scenario.json", FAKE_PIDFILE=self.pidfile, FAKE_ARGS=self.argsfile, FAKE_MODEL_FLAG=self.flag,
                   WICLI_VOICE_BIN=self.vbin, WICLI_DICTATION_LOG=CFGDIR + "/dictation_default.log", PATH="/usr/bin:/bin")
        if not missing: env["WICLI_VOICE_COMMAND"] = cmd
        if serve: env["WICLI_AUDIO_URL"] = "file://" + SERVE
        if extra_env: env.update(extra_env)
        if idle is not None: env["WICLI_LISTEN_IDLE_SECS"] = str(idle)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(WS); os.execve(wi, ["wi"], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, cols, 0, 0))
        self.scr = pyte.Screen(cols, 30); self.st = pyte.ByteStream(self.scr)
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
    def top(self): return self.rows()[0]
    def click(self, col, row=1):
        self.send(f"\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m".encode(), 0.8)
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
    def state_text(self):
        try: return open(self.state).read()
        except Exception: return ""
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
check("dictation is off at the start: a dim mic button at the right end of the top bar", "●" not in r.top() and r.top().endswith("○ mic"))
r.dictation()
check("the menu entry (search 'dictate') turns it on and starts the engine", r.child_alive())
check("the top bar shows the listening indicator", "● listening" in r.top())
args = open(r.argsfile).read() if os.path.exists(r.argsfile) else ""
check("the engine gets `listen` and the tuning options", args.startswith("listen") and "--end-silence-ms 700" in args and "--min-speech-ms 250" in args)
r.release("hear")
check("hearing is shown while speech is detected", "● hearing" in r.top())
r.release("p1"); r.pump(0.8)
check("the first phrase is typed into the focused pane", "open the explorer" in r.text())
r.release("p2"); r.release("p3")
line = next((l for l in r.rows() if "open the explorer" in l), "")
check("phrases build one line, space separated", "open the explorer run the tests then commit" in line)
check("no Enter was pressed: each phrase appears once (cat would echo it again)", r.text().count("open the explorer") == 1 and r.text().count("then commit") == 1)
check("the indicator is back to listening", "● listening" in r.top())
col = r.top().find("●")
r.click(col + 2)
check("a click on the live indicator turns it off", "●" not in r.top() and r.top().endswith("○ mic"))
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
check("silence: on and listening", "● listening" in r.top())
check("silence: the engine was asked to stop on its own a little later than wi does", "--idle-secs 12" in open(r.argsfile).read())
check("silence: idle auto-off ends the mode with a note", r.wait_until(lambda: "dictation off: 2 s of silence" in r.status(), 6))
check("silence: indicator gone, engine process gone", "●" not in r.top() and r.wait_until(lambda: not r.child_alive(), 4))
check("silence: nothing was typed", "silence" not in "\n".join(r.rows()[:28]))
r.quit()

# 7. Errors are one friendly line.
r = Run([state("loading"), ev(ev="error", kind="no_mic", msg="no microphone found"), ["exit", 2]], tag="seven")
r.dictation()
r.pump(1.0)
check("no microphone: a friendly line, mode off", "dictation: no microphone found" in r.status() and "●" not in r.top())
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
r.dictation(); r.pump(0.8)
check("a configured command that cannot run: a friendly dialog, no instruction to run anything by hand", "does not answer" in r.text())
r.send(b"\r", 0.5)
r.quit()
r = Run([state("listening"), ["exit", 1]], tag="eleven")
r.dictation(); r.pump(1.0)
check("an engine that exits on its own ends the mode with a note", "dictation stopped" in r.status() and "●" not in r.top())
r.quit()

# 8. The one menu entry: on -> a small "Dictation is on" screen (Enter turns off, O opens options).
r = Run([state("listening")], tag="twelve")
r.dictation()
check("on again", r.child_alive())
r.dictation()
check("the entry while on shows the small screen with its keys", "Dictation is on" in r.text() and "Enter: turn off" in r.status() and "O: options" in r.status())
r.send(b"\x1b", 0.5)
check("Esc closes it and leaves dictation on", r.child_alive() and "Dictation is on" not in r.text())
r.dictation(); r.send(b"\r", 0.8)
check("Enter turns it off, the engine ends", r.wait_until(lambda: not r.child_alive(), 4) and "dictation off" in r.status())
r.dictation()
check("it can be turned on a second time", r.wait_until(r.child_alive, 4))
r.quit()
check("quitting wi leaves no engine process", r.wait_until(lambda: not r.child_alive(), 4))

# 8b. The mic button: click off -> on, on -> off; tabs unaffected; narrow width.
r = Run([state("listening")], tag="btn")
check("button: off shows a dim mic at the right end", r.top().endswith("○ mic"))
col = r.top().find("○ mic")
r.click(col + 2)
check("button: a click turns dictation on (engine started, live indicator)", r.wait_until(r.child_alive, 5) and "● listening" in r.top())
check("button: the folder text in the top bar is still there", "wicli" in r.top())
col = r.top().find("●")
r.click(col + 2)
check("button: a click while on turns it off", r.wait_until(lambda: not r.child_alive(), 4) and r.top().endswith("○ mic"))
r.send(b"echo AAA$((1+1))\n", 0.6)
r.send(MENU + b"n", 1.5)
r.send(b"echo BBB$((1+1))\n", 0.8)
check("tabs: two tabs open and the mic button is still at the right end of the top bar", "BBB2" in r.text() and r.top().endswith("○ mic"))
m = re.search(r"(?:^|\s)1 \S", r.rows()[1])
check("tabs: the strip is on the next row", m is not None)
r.click((m.start() + 3) if m else 1, row=2)
check("tabs: a click on tab 1 still switches to it", "AAA2" in r.text() and "BBB2" not in r.text())
r.quit()
r = Run([state("listening")], tag="narrow", cols=50)
check("narrow: under 60 columns the button is just a dot", r.top().endswith("○") and "mic" not in r.top())
r.click(len(r.top()))
check("narrow: the dot is clickable", r.wait_until(r.child_alive, 5) and r.top().endswith("●"))
r.quit()
r = Run([state("listening")], tag="long", cols=70)
r.send(MENU + b"n", 1.5)
r.dictation()
check("crowded: the top bar keeps the button with a long folder name", r.top().rstrip().endswith("listening") or "● listening" in r.top())
r.quit()

# 9. Helper missing: a guided popup naming file, size and host; declining is remembered, no nagging.
r = Run([state("listening")], tag="m1", missing=True)
r.dictation()
txt = r.text()
check("helper missing: the popup names the file, size and host", "Voice setup: the helper" in txt and "wi-audio-macos-arm64.tar.gz" in txt and "about 2 MB" in txt and "github.com/fahadsuhaib/wi-cli" in txt)
check("the popup never tells the user to run a command", "wi-audio models" not in txt and "run wi-audio" not in txt)
check("the key hints are shown", " Enter " in txt and "yes" in txt and " Esc " in txt and "not now" in txt)
check("the popup says what it is, why, and that audio stays on the Mac", "Install the small voice helper" in txt and "SHA256" in txt and "never leaves this Mac" in txt)
r.send(b"n", 0.8)
check("declined: the popup closes and says where to do it later", "Voice setup: the helper" not in "\n".join(r.rows()[:28]) and "choose Dictation again" in r.status())
check("declined: remembered in state.toml", '"helper"' in r.state_text())
check("declined: no engine was started", not r.child_alive())
r.dictation(); r.pump(0.5)
check("asking again does not nag: no popup, the same one-line pointer", "Voice setup: the helper" not in "\n".join(r.rows()[:28]) and "choose Dictation again" in r.status())
r.quit()

# 10. Accept the install from a local fixture URL: placed next to (a copy of) the wi binary, then
#     the flow goes on to the microphone test and turns dictation on.
r = Run([state("listening")], tag="m2", missing=True, serve=True, mic_ok=False)
r.dictation()
r.send(b"y", 0.3)
check("installing: the helper lands next to the wi binary", r.wait_until(lambda: os.path.exists(r.wibin + "/wi-audio"), 15))
check("installed helper is executable and nothing else was left beside wi", os.access(r.wibin + "/wi-audio", os.X_OK) and sorted(os.listdir(r.wibin)) == ["wi", "wi-audio"])
check("the flow continues to the microphone test", r.wait_until(lambda: "Voice setup: the microphone" in r.text(), 10))
check("the microphone popup mentions the macOS permission", "Choose Allow" in r.text())
r.send(b"y", 0.5)
check("the test shows a level meter while it records", r.wait_until(lambda: "Speak now" in r.text() and any(c in r.text() for c in "▂▅▇"), 6))
check("then what was heard", r.wait_until(lambda: "heard: hello from the test" in r.text(), 8))
r.send(b"\r", 1.2)
check("Enter turns dictation on with the installed helper", r.wait_until(r.child_alive, 5) and "● listening" in r.top())
check("the passed test is remembered", "voice_mic_ok = true" in r.state_text())
r.dictation(); r.pump(0.5)
check("while on, the entry shows the small screen", "Dictation is on" in r.text())
r.send(b"o", 0.5)
check("options: at most two actions and Esc", "T: test the microphone" in r.text() and "R: remove voice" in r.text() and "frees 300 MB" in r.text())
r.send(b"r", 0.5)
check("remove asks first", "Remove voice?" in r.text())
r.send(b"y", 1.5)
check("remove voice deletes the helper next to wi and stops dictation", r.wait_until(lambda: not os.path.exists(r.wibin + "/wi-audio"), 5) and not r.child_alive() and not os.path.exists(r.flag))
r.quit()

# 10b. A wrong checksum installs nothing and says so.
shutil.copy(SERVE + "/SHA256SUMS", SERVE + "/SHA256SUMS.good")
open(SERVE + "/SHA256SUMS", "w").write("%s  wi-audio-macos-arm64.tar.gz\n" % ("0" * 64))
r = Run([state("listening")], tag="m3", missing=True, serve=True)
r.dictation(); r.send(b"y", 0.3)
check("bad checksum: a dialog explains and nothing is installed", r.wait_until(lambda: "Could not install the voice helper" in r.text() and "checksum" in r.text(), 12) and not os.path.exists(r.wibin + "/wi-audio"))
r.send(b"\r", 0.5)
shutil.copy(SERVE + "/SHA256SUMS.good", SERVE + "/SHA256SUMS")
r.quit()

# 11. Model missing: popup with file, size and host; accept shows a progress bar; then the mic step.
r = Run([state("listening")], tag="m4", model=False, mic_ok=False)
r.dictation()
txt = r.text()
check("model missing: popup names file, size and host", "Download the speech model (295.8 MB)" in txt and "moonshine-streaming-medium-Q8_0.gguf" in txt and "huggingface.co" in txt)
r.send(b"y", 1.0)
check("the download shows a progress bar and percent", r.wait_until(lambda: "[#" in r.text() and "%" in r.text() and "MB of 295.8 MB" in r.text(), 6))
check("when the download ends the flow continues to the microphone test", r.wait_until(lambda: "Voice setup: the microphone" in r.text(), 10))
r.send(b"n", 1.0)
check("skipping the test still turns dictation on and is remembered", r.wait_until(r.child_alive, 5) and '"mic"' in r.state_text())
r.quit()
r = Run([state("listening")], tag="m5", model=False)
r.dictation(); r.send(b"\x1b", 0.8)
check("model declined: no engine, remembered, pointer to the settings", not r.child_alive() and '"model"' in r.state_text() and "choose Dictation again" in r.status())
r.dictation(); r.pump(0.4)
check("model declined: not asked again", "Download the speech model" not in r.text())
r.quit()
r = Run([state("listening")], tag="m6", model=False, extra_env={"FAKE_FETCH_FAIL": "1"})
r.dictation(); r.send(b"y", 0.5)
check("a failed download is a friendly dialog", r.wait_until(lambda: "Could not download the speech model" in r.text() and "check your connection" in r.text(), 8))
r.send(b"\r", 0.5)
r.quit()

# 12. Microphone test failure names the Settings path; settings popup actions on a working install.
r = Run([state("listening")], tag="m7", mic_ok=False, extra_env={"FAKE_PROBE_ERR": "1"})
r.dictation(); r.send(b"y", 0.5)
check("mic test failure: names System Settings > Privacy & Security > Microphone", r.wait_until(lambda: "Microphone test" in r.text() and "System Settings" in r.text() and "Privacy & Security" in r.text(), 8))
r.send(b"\r", 0.5)
check("mic test failure: dictation was not turned on", not r.child_alive())
r.quit()
r = Run([state("listening")], tag="m8")
r.dictation()
r.dictation(); r.send(b"o", 0.5)
check("options: Test the microphone and Remove voice, nothing else", "T: test the microphone" in r.text() and "R: remove voice" in r.text() and "T" in r.text() and "Esc" in r.text())
r.send(b"r", 0.4); r.send(b"n", 0.6)
check("keeping returns to the options", "Dictation options" in r.text() and os.path.exists(r.flag))
r.send(b"t", 0.5)
check("T stops dictation and tests the microphone with a level meter", r.wait_until(lambda: "Microphone test" in r.text() or "Speak now" in r.text(), 4) and not r.child_alive())
check("then shows what it heard", r.wait_until(lambda: "heard: hello from the test" in r.text(), 8))
r.send(b"\r", 1.2)
check("Enter turns dictation back on", r.wait_until(r.child_alive, 5) and "● listening" in r.top())
r.quit()

# 14. Automatic Enter after a quiet (opt-in): pressed once, in the pane that got the text.
def keys(r): return "".join(l.strip()[3:] for l in r.rows() if l.strip().startswith("KE:"))
paste = "1b5b3230307e" + "fix the login bug".encode().hex() + "1b5b3230317e"
r = Run([state("listening"), ["wait", go("p1")], final("fix the login bug")], tag="as1", auto_send=True, extra_env={"WICLI_VOICE_AUTOSEND_S": "3"})
r.send(f"perl {CFGDIR}/bp.pl\n", 0.8)
r.dictation()
r.release("p1")
check("auto Enter: not pressed right after the phrase", keys(r) == paste)
r.pump(3.2)
check("auto Enter: pressed once after the quiet (a single carriage return after the paste)", keys(r) == paste + "0d")
r.quit()
r = Run([state("listening"), ["wait", go("p1")], final("fix the login bug")], tag="as2", extra_env={"WICLI_VOICE_AUTOSEND_S": "3"})
r.send(f"perl {CFGDIR}/bp.pl\n", 0.8)
r.dictation()
r.release("p1"); r.pump(4.0)
check("auto Enter is off by default: no Enter even after the quiet", keys(r) == paste)
r.quit()
r = Run([state("listening"), ["wait", go("p1")], final("fix the login bug")], tag="as3", auto_send=True, extra_env={"WICLI_VOICE_AUTOSEND_S": "3"})
r.send(f"perl {CFGDIR}/bp.pl\n", 0.8)
r.dictation()
r.release("p1")
r.send(b"z", 0.3)
r.pump(4.0)
check("auto Enter: typing after the phrase cancels it (the user is editing)", "0d" not in keys(r)[len(paste):])
r.quit()
r = Run([state("listening")], tag="as4")
r.dictation(); r.dictation(); r.send(b"o", 0.5)
check("options: auto Enter is listed as off", "automatic Enter is OFF" in r.text() and "A: auto Enter" in r.text())
r.send(b"a", 0.5)
check("options: A turns it on and remembers it", "automatic Enter is ON" in r.text() and "voice_auto_send = true" in r.state_text())
r.send(b"a", 0.5)
check("options: A again turns it off", "automatic Enter is OFF" in r.text() and "voice_auto_send" not in r.state_text())
r.quit()

# 15. A better speech model: offered once when only an older one is installed; D downloads it again.
FETCHLOG = CFGDIR + "/fetch_log"
if os.path.exists(FETCHLOG): os.remove(FETCHLOG)
r = Run([state("listening")], tag="up1", extra_env={"FAKE_OLD_MODEL": "1", "FAKE_FETCH_LOG": FETCHLOG})
r.dictation()
txt = r.text()
check("older model: a better one is offered with its size and host", "Voice update: a better speech model" in txt and "295.8 MB" in txt and "huggingface.co" in txt and "fewer mistakes" in txt)
check("the offer shows the key hints", " Enter " in txt and "download" in txt and " Esc " in txt and "keep the current one" in txt)
r.send(b"\x1b", 0.8)
check("declined: dictation still starts with the current model", r.wait_until(r.child_alive, 5))
check("declined: remembered for that model only", '"upgrade:medium"' in r.state_text())
r.quit()
r = Run([state("listening")], tag="up3", declined=["upgrade:medium"], extra_env={"FAKE_OLD_MODEL": "1", "FAKE_FETCH_LOG": FETCHLOG})
r.dictation()
check("a declined upgrade is not offered again: dictation just starts", r.wait_until(r.child_alive, 5) and "Voice update" not in r.text())
r.quit()
r = Run([state("listening")], tag="up4", extra_env={"FAKE_OLD_MODEL": "1", "FAKE_FETCH_LOG": FETCHLOG})
r.dictation(); r.send(b"\r", 0.5)
check("accepting downloads with a progress bar", r.wait_until(lambda: "[#" in r.text() and "MB of 295.8 MB" in r.text(), 6))
check("after the download dictation turns on", r.wait_until(r.child_alive, 12) and "● listening" in r.top())
check("an accepted upgrade is a normal download (no force)", open(FETCHLOG).read().strip() == "fetch-models")
r.quit()
r = Run([state("listening")], tag="up5", extra_env={"FAKE_FETCH_LOG": FETCHLOG})
r.dictation(); r.dictation(); r.send(b"o", 0.5)
check("options list the download-again action", "D: download the speech model again" in r.text() and "D: download again" in r.text())
r.send(b"d", 0.5)
check("D downloads the speech model again with a progress bar", r.wait_until(lambda: "[#" in r.text() and "MB of 295.8 MB" in r.text(), 6) and not r.child_alive())
check("D asks the helper to force it", r.wait_until(lambda: "fetch-models --force" in open(FETCHLOG).read(), 12))
r.quit()

# 16. Spoken controls: "scratch that" removes the last phrase; "new line" needs an agent prompt; a log is kept.
LOG = CFGDIR + "/dictation.log"
if os.path.exists(LOG): os.remove(LOG)
r = Run([state("listening"), ["wait", go("p1")], final("fix the login bug"), ["wait", go("p2")], final("Scratch that.")], tag="sc1", extra_env={"WICLI_DICTATION_LOG": LOG})
r.send(f"perl {CFGDIR}/bp.pl\n", 0.8)
r.dictation()
r.release("p1"); r.pump(0.3)
r.release("p2"); r.pump(0.8)
check("scratch that: the last phrase is removed with one backspace per character, nothing else sent", keys(r) == paste + "7f" * len("fix the login bug"))
check("scratch that: the status line says so", "scratched the last phrase" in r.status())
r.quit()
r = Run([state("listening"), ["wait", go("p1")], final("fix the login bug"), ["wait", go("p2")], final("new line")], tag="sc2", extra_env={"WICLI_DICTATION_LOG": LOG})
r.send(f"perl {CFGDIR}/bp.pl\n", 0.8)
r.dictation()
r.release("p1"); r.pump(0.3)
r.release("p2"); r.pump(0.8)
check("new line in a plain shell is refused (it would run the command)", keys(r) == paste and "new line works in an agent prompt" in r.status())
r.quit()
logtxt = open(LOG).read() if os.path.exists(LOG) else ""
check("the log records the phrases by length only, never the words", "final chars=17" in logtxt and "scratch chars=17" in logtxt and "login" not in logtxt)

print("\n%d failed" % len(fails) if fails else "\nall passed")
sys.exit(1 if fails else 0)
