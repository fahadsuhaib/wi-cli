//! A workspace is where the shell (and, later, previews and transfers) live:
//! either a local directory or a remote box over SSH.

use crate::config::BoxConfig;
use crate::session::Session;
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use russh::{Channel, ChannelMsg, client::Msg};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

pub enum Workspace {
    Local(PathBuf),
    Remote(BoxConfig),
}

impl Workspace {
    pub fn is_remote(&self) -> bool {
        matches!(self, Workspace::Remote(_))
    }

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
            Workspace::Remote(b) => (b.name.clone(), format!("{}@{}:{}", b.user, b.host, b.port)),
        }
    }

    /// Establish the connection once; every tab then opens its shell on it.
    pub async fn connect(&self) -> Result<Backend> {
        match self {
            Workspace::Local(dir) => Ok(Backend::Local(dir.clone())),
            Workspace::Remote(b) => Ok(Backend::Remote(Arc::new(Session::connect(b).await?))),
        }
    }
}

pub enum Backend {
    Local(PathBuf),
    Remote(Arc<Session>),
}

impl Backend {
    /// Open a shell; on a remote box with tmux it runs in a tmux session, returned by name.
    /// `attach` reattaches an existing session, otherwise a fresh name is picked. `dir` is the
    /// folder a new shell starts in (the workspace folder / login directory when None).
    pub async fn open_session_shell(&self, cols: u16, rows: u16, attach: Option<&str>, dir: Option<&str>) -> Result<(Shell, Option<String>)> {
        match self {
            Backend::Local(root) => {
                let start = dir.map(PathBuf::from).unwrap_or_else(|| root.clone());
                Ok((Shell::Local(LocalPty::spawn(&start, cols, rows)?), None))
            }
            Backend::Remote(session) if session.has_tmux() => {
                let name = attach.map(str::to_string).unwrap_or_else(|| session.new_session_name());
                let ch = session.open_shell(cols, rows, Some(&name), dir).await?;
                Ok((Shell::Remote(ch), Some(name)))
            }
            Backend::Remote(session) => Ok((Shell::Remote(session.open_shell(cols, rows, None, dir).await?), None)),
        }
    }

    /// True when shells live in tmux on the box, so quitting can detach.
    pub fn has_tmux(&self) -> bool {
        matches!(self, Backend::Remote(s) if s.has_tmux())
    }

    /// tmux sessions that already exist on the box, to reattach.
    pub fn restored_sessions(&self) -> Vec<String> {
        match self {
            Backend::Remote(s) => s.restored_sessions(),
            Backend::Local(_) => Vec::new(),
        }
    }

    pub async fn end_session(&self, name: &str) {
        if let Backend::Remote(s) = self {
            s.end_session(name).await;
        }
    }

    pub async fn end_all_sessions(&self) {
        if let Backend::Remote(s) = self {
            s.end_all_sessions().await;
        }
    }

    /// True when the SSH connection has died (always false for local).
    pub fn is_lost(&self) -> bool {
        matches!(self, Backend::Remote(s) if s.handle.is_closed())
    }

    pub async fn close(&self) {
        if let Backend::Remote(session) = self {
            let _ = session.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
        }
    }
}

pub enum Shell {
    Remote(Channel<Msg>),
    Local(LocalPty),
}

impl Shell {
    fn pid(&self) -> Option<u32> {
        match self {
            Shell::Local(l) => l.child.process_id(),
            Shell::Remote(_) => None,
        }
    }

    /// Process group in the foreground of a local pty (None for remote shells or on error).
    fn foreground(&self) -> Option<i32> {
        match self {
            Shell::Local(l) => l.master.process_group_leader(),
            Shell::Remote(_) => None,
        }
    }

    async fn write(&mut self, data: &[u8]) -> Result<()> {
        match self {
            Shell::Remote(channel) => channel.data(data).await?,
            Shell::Local(l) => {
                l.writer.write_all(data)?;
                l.writer.flush()?;
            }
        }
        Ok(())
    }

    async fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Shell::Remote(channel) => channel.window_change(cols as u32, rows as u32, 0, 0).await?,
            Shell::Local(l) => l.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?,
        }
        Ok(())
    }

    /// Next chunk of output, or None once the shell has ended.
    async fn next_output(&mut self) -> Option<Vec<u8>> {
        match self {
            Shell::Remote(channel) => loop {
                match channel.wait().await? {
                    ChannelMsg::Data { data } => return Some(data.to_vec()),
                    ChannelMsg::ExtendedData { data, .. } => return Some(data.to_vec()),
                    ChannelMsg::Eof | ChannelMsg::Close => return None,
                    _ => {}
                }
            },
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

    async fn close(&mut self) {
        match self {
            Shell::Remote(channel) => {
                let _ = channel.close().await;
            }
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
    /// Ask which process group is in the foreground (local ptys only).
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
    /// Process id of a local shell (used to find its working directory).
    pid: Option<u32>,
}

impl ShellHandle {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
    /// Foreground process group of a local shell's pty; None when unknown or remote.
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
                    Some(Cmd::Write(d)) => { if shell.write(&d).await.is_err() { break; } }
                    Some(Cmd::Resize(c, r)) => { let _ = shell.resize(c, r).await; }
                    Some(Cmd::Foreground(reply)) => { let _ = reply.send(shell.foreground()); }
                    Some(Cmd::Close) | None => { shell.close().await; break; }
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
