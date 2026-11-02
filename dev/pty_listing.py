# usage: python dev/pty_listing.py <scratch-dir> local|remote   (box "test" in <scratch-dir>/test.toml; plain subprocess test, no pty; drives the hidden `wi __ls`)
# remote: yes
import os, shutil, subprocess, sys, tempfile, time
S, mode = sys.argv[1], sys.argv[2]
BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi")
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml")
fails = []
def check(name, cond, extra=""):
    print(("PASS " if cond else "FAIL ") + name + (" " + str(extra)[:300] if extra else ""))
    if not cond: fails.append(name)

remote = mode == "remote"
if remote:
    ROOT = "/home/dev/explorer-test"
    def sh(cmd):
        r = subprocess.run(["docker", "exec", "-u", "dev", "devbox-test", "sh", "-c", cmd], capture_output=True, text=True)
        if r.returncode: print("fixture error:", r.stderr)
    sh(f"rm -rf {ROOT}")
    sh(f"mkdir -p {ROOT}")
else:
    ROOT = os.path.realpath(tempfile.mkdtemp(prefix="explorer-test-", dir=S))
    def sh(cmd):
        r = subprocess.run(["sh", "-c", cmd], capture_output=True, text=True)
        if r.returncode: print("fixture error:", r.stderr)

def ls(path, *extra):
    t = time.time()
    r = subprocess.run([BIN, "__ls", *(["--box", "test"] if remote else []), path, *extra], capture_output=True, text=True, env=env)
    return r, time.time() - t
def rows(r):
    return [l.split(" ", 4) for l in r.stdout.splitlines() if l[:2] in ("d ", "f ", "l ")]
def lines(r, prefix):
    return [l[len(prefix):] for l in r.stdout.splitlines() if l.startswith(prefix)]

try:
    sh(f"""cd {ROOT} && mkdir -p plain sub dir "uni dir" many repo
printf 'hello' > plain/a.txt; printf 'x' > plain/.hidden; mkdir plain/Zdir
ln -s ../sub plain/linkdir; ln -s nowhere plain/broken; ln -s a.txt plain/linkfile
printf 'ab' > "uni dir/ünï çödé.txt"; printf 'abc' > "uni dir/with space.txt"
cd many && seq 1 5200 | sed 's/^/f/' | xargs touch && cd ..
cd repo && git init -q . && git config user.email t@t && git config user.name t
mkdir src && printf 'a' > src/mod.rs && printf 'b' > src/del.rs && printf 'c' > src/old.rs && printf 'd' > top.txt && printf 'e' > stage.txt
git add . && git commit -q -m one
printf 'aa' >> src/mod.rs && git rm -q src/del.rs && git mv src/old.rs src/new.rs && printf 'ee' >> stage.txt && git add stage.txt
printf 'dd' >> top.txt && printf 'u' > "src/new file.txt" && printf 'n' > added.txt && git add added.txt""")
    r, _ = ls(f"{ROOT}/plain")
    check("plain exit 0", r.returncode == 0, r.stderr)
    got = {x[4]: x[:4] for x in rows(r)}
    check("plain names", sorted(got) == [".hidden", "Zdir", "a.txt", "broken", "linkdir", "linkfile"], list(got))
    check("name order: dirs first, then case-insensitive", [x[4] for x in rows(r)] == ["linkdir", "Zdir", ".hidden", "a.txt", "broken", "linkfile"], [x[4] for x in rows(r)])
    check("file size", got["a.txt"][:2] == ["f", "5"], got["a.txt"])
    check("file has mtime", got["a.txt"][2].isdigit() and abs(int(got["a.txt"][2]) - time.time()) < 3600 * 24)
    check("hidden flag", got[".hidden"][3] == "1" and got["a.txt"][3] == "0")
    check("dir has no size", got["Zdir"][:2] == ["d", "-"], got["Zdir"])
    check("symlink to dir is a dir", got["linkdir"][0] == "d", got["linkdir"])
    check("symlink to file is a file with target size", got["linkfile"][:2] == ["f", "5"], got["linkfile"])
    check("broken symlink listed as l", got["broken"][0] == "l" and got["broken"][1] == "-", got["broken"])
    r, _ = ls(f"{ROOT}/plain", "--sort", "size")
    check("size sort: dirs first, then ascending size, missing last", [x[4] for x in rows(r)] == ["linkdir", "Zdir", ".hidden", "a.txt", "linkfile", "broken"], [x[4] for x in rows(r)])
    r, _ = ls(f"{ROOT}/uni dir")
    check("unicode and spaces", sorted(x[4] for x in rows(r)) == ["with space.txt", "ünï çödé.txt"], r.stdout + r.stderr)
    r, dt = ls(f"{ROOT}/many")
    n = len(rows(r))
    check("5200 files capped at 5000 with notice", n == 5000 and "truncated 5200" in r.stdout, (n, r.stdout[-40:]))
    print(f"INFO many listing took {dt:.2f}s")
    if remote:
        check("5200-entry remote folder lists in under 3 s", dt < 3, f"{dt:.2f}s")
    r, _ = ls(f"{ROOT}/nope")
    check("missing dir: exit 1 and wi: message", r.returncode == 1 and r.stderr.startswith("wi: "), r.stderr)
    r, _ = ls(f"{ROOT}/plain", "--git")
    check("not a repo: nogit", r.returncode == 0 and "nogit" in r.stdout, r.stdout[-60:])
    r, _ = ls(f"{ROOT}/repo", "--git")
    g = {l[6:]: l[4] for l in r.stdout.splitlines() if l.startswith("git ")}
    check("git exit 0", r.returncode == 0, r.stderr)
    want = {"src/mod.rs": "M", "src/del.rs": "D", "src/new.rs": "R", "stage.txt": "M", "top.txt": "M", "added.txt": "A", "src/new file.txt": "?"}
    check("git marks", g == want, g)
    check("git branch", any(l.startswith("branch ") and len(l) > 7 for l in r.stdout.splitlines()), r.stdout[-80:])
    check("git repo root", f"repo {ROOT}/repo" in r.stdout.splitlines(), [l for l in r.stdout.splitlines() if l.startswith("repo")])
    check("dirty folder", lines(r, "dirty ") == ["src"], lines(r, "dirty "))
    r, _ = ls(f"{ROOT}/repo/src", "--git")
    check("git from a subfolder keeps repo-relative keys", "git M src/mod.rs" in r.stdout and f"repo {ROOT}/repo" in r.stdout.splitlines(), r.stdout[-200:])
finally:
    if remote: sh(f"rm -rf {ROOT}")
    else: shutil.rmtree(ROOT, ignore_errors=True)
print("FAILED: " + ", ".join(fails) if fails else "ALL PASS")
sys.exit(1 if fails else 0)
