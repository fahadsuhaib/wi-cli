# usage: python dev/pty_fetch.py <scratch-dir> remote   (box "test" in <scratch-dir>/test.toml; no pty needed, drives `wi fetch`)
# remote: yes
import hashlib, os, shutil, signal, subprocess, sys, time
S, mode = sys.argv[1], sys.argv[2]
if mode != "remote":
    sys.exit(0)
BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
CACHE = os.path.join(S, "fetchcache")
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", WICLI_CACHE_DIR=CACHE)
fails = []
def check(name, cond, extra=""):
    print(("PASS " if cond else "FAIL ") + name + (" " + str(extra) if extra else ""))
    if not cond: fails.append(name)
def box(cmd):
    return subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "sh", "-c", cmd], capture_output=True, text=True).stdout
def wi(*a):
    return subprocess.run([BIN, "fetch", "test", *a], capture_output=True, text=True, env=env)
def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for c in iter(lambda: f.read(1 << 20), b""): h.update(c)
    return h.hexdigest()
def files():
    return [os.path.join(d, f) for d, _, fs in os.walk(CACHE) for f in fs]
def rsha(p): return box(f"sha256sum {p}").split()[0]

shutil.rmtree(CACHE, ignore_errors=True)
box("head -c 5242880 /dev/urandom > /home/dev/f5.bin; head -c 157286400 /dev/urandom > /home/dev/f150.bin")
try:
    # 5 MB, exact bytes
    r = wi("/home/dev/f5.bin")
    p = r.stdout.strip()
    check("5MB fetch exit 0", r.returncode == 0, r.stderr)
    check("5MB under cache dir", os.path.realpath(p).startswith(os.path.realpath(CACHE) + "/test/"), p)
    check("5MB sha256 matches", os.path.exists(p) and sha(p) == rsha("/home/dev/f5.bin"))
    # second fetch reuses the copy
    mt = os.stat(p).st_mtime_ns; t = time.time()
    r = wi("/home/dev/f5.bin")
    check("second fetch same path, no .part", r.returncode == 0 and r.stdout.strip() == p and not [f for f in files() if f.endswith(".part")])
    check("second fetch did not rewrite", os.stat(p).st_mtime_ns == mt, f"{time.time()-t:.2f}s")
    # relative path with ~ style
    r = wi("f5.bin")
    check("relative path resolves to same cache file", r.stdout.strip() == p, r.stderr)
    # 150 MB needs confirmation
    r = wi("/home/dev/f150.bin")
    check("150MB without --yes exits 2", r.returncode == 2, r.stderr)
    check("150MB prompt created no local data", not any(f.endswith("f150.bin") or f.endswith(".part") for f in files()))
    # kill mid-transfer
    pr = subprocess.Popen([BIN, "fetch", "test", "/home/dev/f150.bin", "--yes"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
    t0 = time.time()
    while time.time() - t0 < 10 and not any(f.endswith(".part") for f in files()): time.sleep(0.02)
    time.sleep(0.4)
    still = pr.poll() is None
    pr.send_signal(signal.SIGKILL); pr.wait()
    fl = [f for f in files() if "f150" in f]
    check("killed mid-transfer (was still running)", still)
    check("kill leaves only a .part", len(fl) == 1 and fl[0].endswith(".part"), fl)
    # rerun, sampling RSS
    pr = subprocess.Popen([BIN, "fetch", "test", "/home/dev/f150.bin", "--yes"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
    peak = 0
    while pr.poll() is None:
        o = subprocess.run(["ps", "-o", "rss=", "-p", str(pr.pid)], capture_output=True, text=True).stdout.strip()
        if o: peak = max(peak, int(o))
        time.sleep(0.05)
    out = pr.stdout.read().decode().strip()
    check("rerun exit 0", pr.returncode == 0)
    check("rerun bytes correct", os.path.exists(out) and sha(out) == rsha("/home/dev/f150.bin"))
    check("no .part left", not [f for f in files() if f.endswith(".part")])
    check("peak RSS under 50 MB", 0 < peak < 50 * 1024, f"{peak/1024:.1f} MB")
    # path escape
    r = wi("../../etc/passwd")
    q = os.path.realpath(r.stdout.strip())
    check("../../etc/passwd stays in cache", r.returncode == 0 and q.startswith(os.path.realpath(CACHE) + "/test/"), r.stdout + r.stderr)
    check("escape content is the box's passwd", r.returncode == 0 and sha(q) == rsha("/etc/passwd"))
    outside = [e for e in os.listdir(os.path.dirname(CACHE)) if e == "etc"]
    check("nothing created beside the cache dir", not outside)
    r = wi("/home/dev/nope")
    check("missing file exits 1 with wi: message", r.returncode == 1 and r.stderr.startswith("wi: "), r.stderr)
finally:
    box("rm -f /home/dev/f5.bin /home/dev/f150.bin")
    shutil.rmtree(CACHE, ignore_errors=True)
print("FAILED: " + ", ".join(fails) if fails else "ALL PASS")
sys.exit(1 if fails else 0)
