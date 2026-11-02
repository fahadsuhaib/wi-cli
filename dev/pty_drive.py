import os, pty, sys, time, select, struct, fcntl, termios, pyte
# usage: python dev/pty_drive.py <scratch-dir-with-test.toml> local|remote   (needs: pip install pyte)
S, mode = sys.argv[1], sys.argv[2]
env=dict(os.environ, WICLI_CONFIG=S+"/test.toml", TERM="xterm-256color")
args=["wi"] if mode=="local" else ["wi","test"]
pid, fd = pty.fork()
if pid==0:
    if mode=="local": os.chdir(S+"/ws")
    os.execve("/Users/fahad/work/devterm/target/debug/wi",args,env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH",24,90,0,0))
scr=pyte.Screen(90,24); st=pyte.ByteStream(scr)
def pump(t):
    end=time.time()+t
    while time.time()<end:
        r,_,_=select.select([fd],[],[],0.1)
        if r:
            try: st.feed(os.read(fd,65536))
            except OSError: return
def show(t):
    print("=====",t)
    for l in scr.display:
        if l.strip(): print(l.rstrip())
pump(2.5); os.write(fd,b"pwd; ls; echo MARK$((20+22)); stty size\n"); pump(1.2); show("shell")
os.write(fd,b"\x00"); pump(0.7); show("menu")
os.write(fd,b"\x1b"); pump(0.3)
os.write(fd,b"vim\n"); pump(1.2); os.write(fd,b":q\n"); pump(0.8)
os.write(fd,b"exit\n" if mode=="local" else b"\x00q"); pump(1.5)
p,stt=os.waitpid(pid,os.WNOHANG); print("exited:",p!=0,"status",stt)
