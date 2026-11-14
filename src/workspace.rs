//! A workspace is where the shell (and previews) live: a local directory.

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use tokio::sync::mpsc;

pub enum Workspace {
    Local(PathBuf),
}

impl Workspace {
    /// (name, detail) for the header.
    pub fn title(&self) -> (String, String) {
        match self {
            Workspace::Local(dir) => {
                let home = dirs::home_dir().unwrap_or_default();
                let shown = match dir.strip_prefix(&home) {
                    Ok(rel) => format!("~/{}", rel.display()),
                    Err(_) => dir.display().to_string(),
                };
                ("local".into(), shown.trim_end_matches('/').to_string())
            }
        }
    }

    /// The backend every tab opens its shell on.
    pub fn connect(&self) -> Backend {
        match self {
            Workspace::Local(dir) => Backend::Local(dir.clone()),
        }
    }
}

pub enum Backend {
    Local(PathBuf),
}

impl Backend {
    /// Open a shell. `dir` is the folder it starts in (the workspace folder when None).
    pub fn open_shell(&self, cols: u16, rows: u16, dir: Option<&str>) -> Result<Shell> {
        match self {
            Backend::Local(root) => {
                let start = dir.map(PathBuf::from).unwrap_or_else(|| root.clone());
                Ok(Shell::Local(LocalPty::spawn(&start, cols, rows)?))
            }
        }
    }
}

pub enum Shell {
    Local(LocalPty),
}

impl Shell {
    fn pid(&self) -> Option<u32> {
        match self {
            Shell::Local(l) => l.child.process_id(),
        }
    }

    /// Process group in the foreground of the pty (None on error).
    fn foreground(&self) -> Option<i32> {
        match self {
            Shell::Local(l) => l.master.process_group_leader(),
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<()> {
        match self {
            Shell::Local(l) => {
                l.writer.write_all(data)?;
                l.writer.flush()?;
            }
        }
        Ok(())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Shell::Local(l) => l.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?,
        }
        Ok(())
    }

    /// Next chunk of output, or None once the shell has ended.
    async fn next_output(&mut self) -> Option<Vec<u8>> {
        match self {
            Shell::Local(l) => {
                let mut data = l.rx.recv().await?;
                // Merge whatever is already queued: ptys hand output over in small
                // pieces and each piece would otherwise cost several thread wake-ups.
                while data.len() < COALESCE_MAX {
                    match l.rx.try_recv() {
                        Ok(more) => data.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }
                Some(data)
            }
        }
    }

    fn close(&mut self) {
        match self {
            Shell::Local(l) => {
                let _ = l.child.kill();
            }
        }
    }
}

/// Largest merged output event.
const COALESCE_MAX: usize = 64 * 1024;

pub enum Cmd {
    Write(Vec<u8>),
    Resize(u16, u16),
    /// Ask which process group is in the foreground.
    Foreground(tokio::sync::oneshot::Sender<Option<i32>>),
    Close,
}

pub enum Event {
    Output(usize, Vec<u8>),
    Exited(usize),
}

/// Handle to a shell running in its own task. Output arrives as `Event`s on a
/// bounded channel, so a flood of output slows the shell instead of growing memory.
#[derive(Clone)]
pub struct ShellHandle {
    tx: mpsc::UnboundedSender<Cmd>,
    /// Process id of the shell (used to find its working directory).
    pid: Option<u32>,
}

impl ShellHandle {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
    /// Foreground process group of the shell's pty; None when unknown.
    pub async fn foreground(&self) -> Option<i32> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx.send(Cmd::Foreground(tx)).ok()?;
        tokio::time::timeout(std::time::Duration::from_secs(1), rx).await.ok()?.ok()?
    }
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.tx.send(Cmd::Write(data));
    }
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.tx.send(Cmd::Resize(cols, rows));
    }
    pub fn close(&self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

impl ShellHandle {
    /// A handle connected to nothing, for tests.
    #[cfg(test)]
    pub fn dummy() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel();
        Self { tx, pid: None }
    }
}

pub fn spawn_shell(id: usize, mut shell: Shell, events: mpsc::Sender<Event>) -> ShellHandle {
    let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();
    let pid = shell.pid();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Write(d)) => { if shell.write(&d).is_err() { break; } }
                    Some(Cmd::Resize(c, r)) => { let _ = shell.resize(c, r); }
                    Some(Cmd::Foreground(reply)) => { let _ = reply.send(shell.foreground()); }
                    Some(Cmd::Close) | None => { shell.close(); break; }
                },
                out = shell.next_output() => match out {
                    Some(d) => { if events.send(Event::Output(id, d)).await.is_err() { break; } }
                    None => break,
                },
            }
        }
        let _ = events.send(Event::Exited(id)).await;
    });
    ShellHandle { tx, pid }
}

pub struct LocalPty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    rx: mpsc::Receiver<Vec<u8>>,
}

impl LocalPty {
    fn spawn(dir: &PathBuf, cols: u16, rows: u16) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("opening local pty")?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(dir);
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).context("starting local shell")?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        // Bounded: when the app falls behind, the reader blocks and the pty throttles the
        // child instead of queueing the whole flood in memory.
        let (tx, rx) = mpsc::channel(128);
        // Blocking reads live on their own thread; it ends when the shell exits.
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Ok(Self { master: pair.master, writer, child, rx })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A local shell reports itself as the foreground group at a prompt and something else
    /// while a program runs.
    #[tokio::test]
    async fn local_foreground_group_follows_the_running_program() {
        let dir = std::env::temp_dir();
        let shell = Shell::Local(LocalPty::spawn(&dir, 80, 24).unwrap());
        let (tx, mut rx) = mpsc::channel(64);
        let h = spawn_shell(0, shell, tx);
        let pid = h.pid().unwrap() as i32;
        let wait_for = |want_shell: bool| {
            let h = h.clone();
            async move {
                for _ in 0..50 {
                    let fg = h.foreground().await;
                    if fg.is_some() && (fg == Some(pid)) == want_shell { return true; }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                false
            }
        };
        assert!(wait_for(true).await, "idle shell is its own foreground group");
        h.write(b"sleep 5\n".to_vec());
        assert!(wait_for(false).await, "a running program takes the foreground");
        h.write(b"\x03".to_vec());
        assert!(wait_for(true).await, "back at the prompt after it ends");
        h.close();
        while rx.recv().await.is_some() {}
    }
}
