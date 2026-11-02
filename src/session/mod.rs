//! SSH connection and PTY channel. One connection per box; shell, SFTP and
//! exec channels all share it.

use crate::config::{BoxConfig, expand_tilde};
use anyhow::{Context, Result, bail};
use russh::client::{self, Handle};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::{Channel, client::Msg};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub struct Client {
    host: String,
    port: u16,
    known_hosts: PathBuf,
}

impl client::Handler for Client {
    type Error = anyhow::Error;

    /// Host key policy: known and matching -> accept; changed -> hard refuse;
    /// first sight -> trust on first use (like `accept-new`) and record it.
    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            bail!("host certificates are not supported yet");
        };
        match check_known_hosts_path(&self.host, self.port, key, &self.known_hosts) {
            Ok(true) => Ok(true),
            Ok(false) => {
                learn_known_hosts_path(&self.host, self.port, key, &self.known_hosts)?;
                eprintln!(
                    "wi: first connection to {}, recorded host key {}",
                    self.host,
                    key.fingerprint(Default::default())
                );
                Ok(true)
            }
            Err(e) => bail!(
                "HOST KEY CHECK FAILED for {} ({e}). If the box was rebuilt, remove its line from {}",
                self.host,
                self.known_hosts.display()
            ),
        }
    }
}

pub struct Session {
    pub handle: Handle<Client>,
    /// SFTP subsystem channel on the same connection, opened on first use.
    sftp: tokio::sync::OnceCell<russh_sftp::client::SftpSession>,
    /// Shells run inside tmux sessions on the box (it has tmux and the box config allows it).
    tmux: bool,
    /// Numbers N of the `wicli-N` sessions this client knows to exist.
    used: std::sync::Mutex<BTreeSet<u32>>,
    /// Sessions found at connect time, to be reattached.
    restored: Vec<u32>,
}

/// Our own tmux server (socket `wicli`), so the user's tmux and its config are untouched.
const TMUX: &str = "tmux -L wicli -f /dev/null -u";

pub fn session_name(n: u32) -> String {
    format!("wicli-{n}")
}

/// Numbers of the `wicli-N` sessions in `tmux list-sessions -F '#S'` output, sorted.
pub fn parse_sessions(out: &str) -> Vec<u32> {
    let set: BTreeSet<u32> = out
        .lines()
        .filter_map(|l| l.trim().strip_prefix("wicli-")?.parse::<u32>().ok())
        .collect();
    set.into_iter().collect()
}

/// Smallest session number not in use, starting at 1.
pub fn next_free(used: &BTreeSet<u32>) -> u32 {
    (1..).find(|n| !used.contains(n)).unwrap_or(1)
}

/// Command that creates (if needed) and attaches the named tmux session with tmux's UI
/// switched off: no status line, no prefix key, no mouse capture. Runs under `sh -c` so the
/// user's login shell does not matter. Options are only set when the server starts.
pub fn attach_command(name: &str, cols: u16, rows: u16) -> String {
    let t = TMUX;
    let script = format!(
        "T=\"{t}\"; $T has-session -t ={name} 2>/dev/null || $T start-server \\; \
         set -g status off \\; set -g prefix None \\; set -g prefix2 None \\; set -g mouse off \\; \
         set -g escape-time 10 \\; set -g default-terminal tmux-256color \\; \
         set -g set-titles on \\; set -g set-titles-string \\#T \\; \
         new-session -d -s {name} -x {cols} -y {rows} \\; \
         set -ga terminal-features ,\\*:RGB; \
         exec $T attach-session -d -t ={name}"
    );
    format!("sh -c '{script}'")
}

fn default_identities() -> Vec<PathBuf> {
    let ssh = dirs::home_dir().unwrap_or_default().join(".ssh");
    ["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|n| ssh.join(n)).filter(|p| p.exists()).collect()
}

impl Session {
    pub async fn connect(b: &BoxConfig) -> Result<Self> {
        let known_hosts = b
            .known_hosts
            .as_deref()
            .map(expand_tilde)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".ssh/known_hosts"));

        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(10)),
            keepalive_max: 3,
            ..Default::default()
        });
        let handler = Client { host: b.host.clone(), port: b.port, known_hosts };
        let mut handle = client::connect(config, (b.host.as_str(), b.port), handler)
            .await
            .with_context(|| format!("connecting to {}:{}", b.host, b.port))?;

        if !authenticate(&mut handle, b).await? {
            bail!("authentication failed for {}@{}", b.user, b.host)
        }
        let mut me = Self {
            handle,
            sftp: tokio::sync::OnceCell::new(),
            tmux: false,
            used: Default::default(),
            restored: Vec::new(),
        };
        if b.use_tmux {
            me.tmux = matches!(me.exec("command -v tmux").await, Ok((out, 0)) if !out.is_empty());
            if me.tmux {
                me.restored = me.list_sessions().await;
                *me.used.lock().unwrap() = me.restored.iter().copied().collect();
            }
        }
        Ok(me)
    }

    /// Run a command on an exec channel (no pty). Output is capped at 64 KB, 10 s limit.
    /// Returns (stdout, exit status).
    pub async fn exec(&self, cmd: &str) -> Result<(Vec<u8>, u32)> {
        let work = async {
            let mut ch = self.handle.channel_open_session().await?;
            ch.exec(true, cmd).await?;
            let mut out = Vec::new();
            let mut status = 0;
            while let Some(msg) = ch.wait().await {
                match msg {
                    russh::ChannelMsg::Data { data } if out.len() < EXEC_CAP => out.extend_from_slice(&data),
                    russh::ChannelMsg::ExitStatus { exit_status } => status = exit_status,
                    russh::ChannelMsg::Close => break,
                    _ => {}
                }
            }
            out.truncate(EXEC_CAP);
            Ok::<_, anyhow::Error>((out, status))
        };
        tokio::time::timeout(Duration::from_secs(10), work).await.context("command timed out")?
    }

    async fn list_sessions(&self) -> Vec<u32> {
        match self.exec(&format!("{TMUX} list-sessions -F '#S' 2>/dev/null")).await {
            Ok((out, _)) => parse_sessions(&String::from_utf8_lossy(&out)),
            Err(_) => Vec::new(),
        }
    }

    /// Current directory of the tmux pane behind a tab (so relative paths in its output resolve).
    pub async fn tmux_pane_path(&self, name: &str) -> Option<String> {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return None;
        }
        let cmd = format!("{TMUX} display-message -p -t ={name} '#{{pane_current_path}}' 2>/dev/null");
        let (out, status) = self.exec(&cmd).await.ok()?;
        let p = String::from_utf8_lossy(&out).trim().to_string();
        (status == 0 && p.starts_with('/')).then_some(p)
    }

    pub fn has_tmux(&self) -> bool {
        self.tmux
    }

    /// Session names to reattach, in order.
    pub fn restored_sessions(&self) -> Vec<String> {
        self.restored.iter().map(|n| session_name(*n)).collect()
    }

    /// Reserve the next free `wicli-N` name.
    pub fn new_session_name(&self) -> String {
        let mut used = self.used.lock().unwrap();
        let n = next_free(&used);
        used.insert(n);
        session_name(n)
    }

    /// End one tmux session (its programs are killed).
    pub async fn end_session(&self, name: &str) {
        if let Some(n) = name.strip_prefix("wicli-").and_then(|n| n.parse().ok()) {
            self.used.lock().unwrap().remove(&n);
        }
        let _ = self.exec(&format!("{TMUX} kill-session -t ={name} 2>/dev/null")).await;
    }

    /// End every wicli session: the dedicated tmux server goes away with them.
    pub async fn end_all_sessions(&self) {
        let _ = self.exec(&format!("{TMUX} kill-server 2>/dev/null")).await;
    }

    /// SFTP session over the existing connection (no second login, nothing installed on the box).
    pub async fn sftp(&self) -> Result<&russh_sftp::client::SftpSession> {
        self.sftp
            .get_or_try_init(|| async {
                let ch = self.handle.channel_open_session().await?;
                ch.request_subsystem(true, "sftp").await?;
                let s = russh_sftp::client::SftpSession::new(ch.into_stream()).await?;
                Ok::<_, anyhow::Error>(s)
            })
            .await
    }

    /// Open an interactive shell on a PTY of the given size; with `tmux_name`, inside that
    /// tmux session (created if missing, reattached if it exists).
    pub async fn open_shell(&self, cols: u16, rows: u16, tmux_name: Option<&str>) -> Result<Channel<Msg>> {
        let ch = self.handle.channel_open_session().await?;
        let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into());
        ch.request_pty(false, &term, cols as u32, rows as u32, 0, 0, &[]).await?;
        match tmux_name {
            Some(name) => ch.exec(false, attach_command(name, cols, rows)).await?,
            None => ch.request_shell(false).await?,
        }
        Ok(ch)
    }
}

const EXEC_CAP: usize = 64 * 1024;

/// Try the ssh-agent's keys, then key files. Ok(false) when nothing was accepted.
async fn authenticate(handle: &mut Handle<Client>, b: &BoxConfig) -> Result<bool> {
    if std::env::var_os("SSH_AUTH_SOCK").is_some() {
        match AgentClient::connect_env().await {
            Ok(mut agent) => match agent.request_identities().await {
                Ok(ids) => {
                    for id in ids {
                        let AgentIdentity::PublicKey { key, .. } = id else { continue };
                        let hash = handle.best_supported_rsa_hash().await?.flatten();
                        if let Ok(res) = handle.authenticate_publickey_with(b.user.clone(), key, hash, &mut agent).await
                            && res.success()
                        {
                            return Ok(true);
                        }
                    }
                }
                Err(e) => eprintln!("wi: ssh-agent did not list keys ({e})"),
            },
            Err(e) => eprintln!("wi: cannot reach ssh-agent ({e})"),
        }
    }
    let identities: Vec<PathBuf> = match &b.identity {
        Some(p) => vec![expand_tilde(p)],
        None => default_identities(),
    };
    for path in identities {
        let key = match load_secret_key(&path, None) {
            Ok(k) => k,
            Err(russh::keys::Error::KeyIsEncrypted) => {
                eprintln!("wi: skipping key {}: it is passphrase-protected; add it with ssh-add so wi can use it through ssh-agent", path.display());
                continue;
            }
            Err(e) => {
                eprintln!("wi: skipping key {} ({e})", path.display());
                continue;
            }
        };
        let hash = handle.best_supported_rsa_hash().await?.flatten();
        let res = handle
            .authenticate_publickey(b.user.clone(), PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await?;
        if res.success() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_lists() {
        assert_eq!(parse_sessions("wicli-2\nwicli-1\nother\nwicli-x\nwicli-10\nwicli-2\n"), vec![1, 2, 10]);
        assert_eq!(parse_sessions(""), Vec::<u32>::new());
        assert_eq!(parse_sessions("mywicli-3\n"), Vec::<u32>::new());
    }

    #[test]
    fn picks_next_free_number() {
        let mut used = BTreeSet::new();
        assert_eq!(next_free(&used), 1);
        used.extend([1, 2, 4]);
        assert_eq!(next_free(&used), 3);
        assert_eq!(session_name(3), "wicli-3");
    }

    #[test]
    fn attach_command_hides_tmux() {
        let c = attach_command("wicli-1", 90, 24);
        assert!(c.contains("status off") && c.contains("prefix None") && c.contains("-s wicli-1"));
        assert!(c.contains("attach-session -d -t =wicli-1"));
        assert!(!c.contains('\''.to_string().repeat(2).as_str()));
    }
}
