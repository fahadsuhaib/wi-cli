//! SSH connection and PTY channel. One connection per box; shell, SFTP and
//! exec channels all share it.

use crate::config::{BoxConfig, expand_tilde};
use anyhow::{Context, Result, bail};
use russh::client::{self, Handle};
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::{Channel, client::Msg};
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

        let identities: Vec<PathBuf> = match &b.identity {
            Some(p) => vec![expand_tilde(p)],
            None => default_identities(),
        };
        if identities.is_empty() {
            bail!("no identity configured and no default key found in ~/.ssh");
        }
        for path in identities {
            let key = match load_secret_key(&path, None) {
                Ok(k) => k,
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
                return Ok(Self { handle });
            }
        }
        bail!("authentication failed for {}@{}", b.user, b.host)
    }

    /// Open an interactive shell on a PTY of the given size.
    pub async fn open_shell(&self, cols: u16, rows: u16) -> Result<Channel<Msg>> {
        let ch = self.handle.channel_open_session().await?;
        let term = std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into());
        ch.request_pty(false, &term, cols as u32, rows as u32, 0, 0, &[]).await?;
        ch.request_shell(false).await?;
        Ok(ch)
    }
}
