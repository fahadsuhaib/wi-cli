# usage: python dev/pty_preview.py <scratch-dir-with-test.toml> local|remote   (needs pyte)
# remote: yes
import os, pty, sys, time, select, struct, fcntl, termios, subprocess, re, pyte
S, mode = sys.argv[1], sys.argv[2]
MENU = b"\x00"
env = dict(os.environ, WICLI_CONFIG=S + "/test.toml", TERM="xterm-256color")
remote = mode == "remote"
rel = "pv"   # directory holding the fixtures, relative to the workspace root / login dir

def make_fixtures():
    script = r'''
set -e
mkdir -p pv && cd pv
printf '# Hello Title\n\nSome *emphasis* and `code` and a [link](https://x.dev).\n\n- one\n- two\n\n| a | bb |\n|---|----|\n| 1 | 2 |\n' > hello.md
printf 'id,name,note\n1,ann,"hi, there"\n2,bob,x\n' > data.csv
printf '{"a":1,"b":[true,null],"c":"x y"}' > data.json
printf 'text\0binary\1\2\3' > notes.txt
head -c 600000 /dev/zero | tr '\0' 'a' | fold -w 80 > big.log
printf 'first line\nsecond line\n' > live.txt
if [ ! -f huge.csv ] || [ $(wc -c < huge.csv) -lt 500000000 ]; then
  (echo "id,name,value"; yes "12345,abcdefgh,3.14159" ) | head -c 500000000 > huge.csv
fi
'''
    if remote:
        subprocess.run(["docker", "exec", "-u", "dev", "-w", "/home/dev", "devbox-test", "bash", "-c", script], check=True)
    else:
        os.makedirs(S + "/ws", exist_ok=True)
        subprocess.run(["bash", "-c", script], check=True, cwd=S + "/ws")
make_fixtures()

args = ["wi", "test"] if remote else ["wi"]
pid, fd = pty.fork()
if pid == 0:
    if not remote: os.chdir(S + "/ws")
    os.execve(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "target/debug/wi"), args, env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
scr = pyte.Screen(100, 30); st = pyte.ByteStream(scr)
def pump(t, until=None):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try: st.feed(os.read(fd, 65536))
            except OSError: return False
        if until and until():
            return True
    return until is None
def text(): return "\n".join(l.rstrip() for l in scr.display)
fails = []
def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond: fails.append(name)
def open_file(path, wait=1.2):
    os.write(fd, MENU + b"p"); pump(0.4)
    os.write(fd, path.encode()); pump(0.2)
    os.write(fd, b"\r"); pump(wait)

pump(3.0)
os.write(fd, MENU + b"p"); pump(0.5)
check("prompt opens", "open file:" in text())
os.write(fd, f"{rel}/he".encode() + b"\t"); pump(0.8)
check("Tab completes the file name", f"open file: {rel}/hello.md" in text())
os.write(fd, b"\r"); pump(1.2)
t = text()
check("markdown heading rendered", "# Hello Title" in t)
check("markdown list bullets", "• one" in t and "• two" in t)
check("markdown link shows url", "link (https://x.dev)" in t)
check("markdown table aligned", re.search(r"a\s*│\s*bb", t) is not None)
check("tab strip shows the preview", "hello.md" in scr.display[1])
os.write(fd, b"\x1b"); pump(0.6)
check("Esc returns to the shell", "hello.md" in scr.display[1] and "Hello Title" not in text())

open_file(f"{rel}/data.csv")
t = text()
check("csv is an aligned table", re.search(r"id\s*│\s*name\s*│\s*note", t) is not None and "hi, there" in t)
os.write(fd, b"\x1b"); pump(0.4)

open_file(f"{rel}/data.json")
check("json pretty printed", '"a": 1' in text() and '"c": "x y"' in text())
os.write(fd, b"\x1b"); pump(0.4)

open_file(f"{rel}/notes.txt")
check("binary file named .txt shows the stub", "binary data" in text() and "text" not in text().replace("type", "").split("size")[0].replace("notes.txt", ""))
os.write(fd, b"\x1b"); pump(0.4)

open_file(f"{rel}/big.log")
check("truncated file shows the banner", re.search(r"showing first 256 KB of 5\d\d KB", text()) is not None)
os.write(fd, b"\x1b[6~\x1b[6~"); pump(0.5)
check("PgDn scrolls", "lines 1-" not in text())
os.write(fd, b"\x1b"); pump(0.4)

open_file(f"{rel}/nope.txt")
check("missing file reports an error and keeps the prompt", "open file:" in text() and ("No such file" in text() or "not found" in text().lower()))
os.write(fd, b"\x1b"); pump(0.4)

# reload picks up changes
open_file(f"{rel}/live.txt")
check("text shows line numbers", "1  first line" in text())
if remote:
    subprocess.run(["docker", "exec", "-u", "dev", "-w", "/home/dev/pv", "devbox-test", "bash", "-c", "echo third line >> live.txt"], check=True)
else:
    open(S + "/ws/pv/live.txt", "a").write("third line\n")
os.write(fd, b"r"); pump(1.0)
check("R reloads the file", "third line" in text())
os.write(fd, b"\x1b"); pump(0.4)

# a 500 MB CSV opens in under a second and shows the rows banner
t0 = time.time()
os.write(fd, MENU + b"p"); pump(0.4)
os.write(fd, f"{rel}/huge.csv".encode() + b"\r")
ok = pump(5.0, until=lambda: "of ~" in text())
dt = time.time() - t0 - 0.4
check(f"500 MB CSV opens in under 1 s ({dt:.2f}s)", ok and dt < 1.0)
check("csv banner says 200 of ~N rows", re.search(r"200 of ~[\d,]+ rows", text()) is not None)
os.write(fd, b"\x1b"); pump(0.3)

os.write(fd, MENU + (b"e" if mode == "remote" else b"q")); pump(1.5)  # remote: End sessions and quit, so no tmux session leaks into the next test
p, _ = os.waitpid(pid, os.WNOHANG)
check("quit exits", p != 0)
sys.exit(1 if fails else 0)
