# usage: python dev/bench.py <scratch-dir-with-test.toml> [--remote]
# Measures wi's own RSS/CPU (ps on the wi pid only; child shells are separate processes).
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, re
S = sys.argv[1]
REMOTE = "--remote" in sys.argv
HERE = os.path.dirname(os.path.abspath(__file__))
WI = os.path.join(HERE, "..", "target/release/wi")
MENU = b"\x00"
BUDGET = dict(rss=50.0, idle=1.0, cpu=30.0, fps=30.0)
TOL = 1.2

def ps(pid, field):
    return subprocess.run(["ps", "-o", field + "=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
def rss_mb(pid):
    v = ps(pid, "rss"); return int(v) / 1024 if v else 0.0
def cputime(pid):  # [dd-][hh:]mm:ss.ss -> seconds
    v = ps(pid, "time")
    if not v: return 0.0
    d = 0
    if "-" in v: d, v = v.split("-"); d = int(d)
    parts = [float(x) for x in v.split(":")]
    s = 0.0
    for p in parts: s = s * 60 + p
    return s + d * 86400

class Run:
    def __init__(self, remote):
        env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color")
        args = ["wi", "test"] if remote else ["wi"]
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            if not remote:
                os.makedirs(S + "/ws", exist_ok=True); os.chdir(S + "/ws")
            os.execve(WI, args, env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 90, 0, 0))
        self.tail = b""
    def pump(self, t, stop=None, sample=None, bursts=None):
        end = time.time() + t; last = 0; nxt = 0
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            now = time.time()
            if sample and now >= nxt: sample(); nxt = now + 0.25
            if r:
                try: d = os.read(self.fd, 65536)
                except OSError: return False
                if bursts is not None and now - last > 0.010: bursts.append(now)
                last = now
                self.tail = (self.tail + d)[-4096:]
                if stop and stop in self.tail: return True
        return False
    def kill(self):
        try: os.kill(self.pid, 9); os.waitpid(self.pid, 0)
        except Exception: pass

def scenario(remote):
    res = {}
    r = Run(remote)
    r.pump(3.0 if remote else 2.0)
    for _ in range(9):
        os.write(r.fd, MENU + b"n"); r.pump(0.8 if remote else 0.4)
    r.pump(2.0)
    # (a) idle, 10 tabs
    c0, t0 = cputime(r.pid), time.time()
    rss = []
    r.pump(20, sample=lambda: rss.append(rss_mb(r.pid)))
    res["idle"] = 100 * (cputime(r.pid) - c0) / (time.time() - t0)
    res["idle_rss"] = max(rss + [rss_mb(r.pid)])
    # (b) 100 MB output in the current tab
    cmd = b"yes abcdefghij | head -c 100000000; echo BENCH$((40+2))DONE\n"
    c0, t0 = cputime(r.pid), time.time()
    peak = [0.0]; bursts = []
    def smp(): peak[0] = max(peak[0], rss_mb(r.pid))
    os.write(r.fd, cmd)
    ok = r.pump(180, stop=b"BENCH42DONE", sample=smp, bursts=bursts)
    wall = time.time() - t0; cpu = cputime(r.pid) - c0
    res["ok"] = ok
    res["cpu"] = 100 * cpu / wall
    res["peak_rss"] = max(peak[0], res["idle_rss"])
    res["fps"] = len(bursts) / wall
    res["wall"] = wall
    r.kill()
    return res

rows = []; fail = False
def row(name, val, budget, unit):
    global fail
    ok = val <= budget * TOL
    fail |= not ok
    rows.append((name, f"{val:.2f} {unit}", f"< {budget:g} {unit} (+20%)", "PASS" if ok else "FAIL"))

for remote in ([False, True] if REMOTE else [False]):
    tag = "remote" if remote else "local"
    try:
        res = scenario(remote)
    except Exception as e:
        print(f"{tag}: scenario error {e!r}"); fail = True; continue
    if not res["ok"]:
        print(f"{tag}: WARNING 100 MB command did not finish within 180 s"); fail = True
    row(f"{tag} RSS, 10 tabs idle (max of 20 s)", res["idle_rss"], BUDGET["rss"], "MB")
    row(f"{tag} idle CPU, 10 tabs, 20 s", res["idle"], BUDGET["idle"], "%")
    row(f"{tag} CPU during 100 MB output ({res['wall']:.1f} s wall)", res["cpu"], BUDGET["cpu"], "%")
    row(f"{tag} peak RSS during 100 MB output", res["peak_rss"], BUDGET["rss"], "MB")
    row(f"{tag} redraw rate (approx: write bursts/s at pty)", res["fps"], BUDGET["fps"], "fps")
w = max(len(r[0]) for r in rows)
print(f"{'metric'.ljust(w)}  {'measured':>12}  {'budget':<18} result")
for n, v, b, s in rows: print(f"{n.ljust(w)}  {v:>12}  {b:<18} {s}")
print("note: fps is approximate (reads separated by >10 ms counted as one burst).")
sys.exit(1 if fail else 0)
