use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct Defaults {
    pub prefix_key: String,
    pub download_dir: String,
    pub download_confirm_mb: u64,
    pub preview_read_kb: u64,
    /// Reload a visible file preview by itself when the file changes on disk (a stat of the
    /// visible file every second, every two on a box; no watchers).
    pub preview_autorefresh: bool,
    /// How long a changed file must stay unchanged before the preview reloads (milliseconds).
    pub preview_refresh_debounce_ms: u64,
    pub csv_rows: usize,
    pub fps_cap: u32,
    /// Lines of scrollback kept per tab. Costs roughly 7 KB per line at 90 columns, per tab.
    pub scrollback_lines: usize,
    /// Seconds without output before a background tab is marked as having gone quiet.
    pub quiet_secs: u64,
    /// Ring the terminal bell when a background tab goes quiet.
    pub bell_on_quiet: bool,
    /// Capture the mouse (clicks, wheel). Turning it off restores native text selection.
    pub mouse: bool,
    /// What to do when an AI-agent workspace is detected: `ask`, `always` or `never`.
    pub agent_start: String,
    /// Which click opens a file path or URL shown in terminal output: `auto` (the default: Cmd on a
    /// Mac, Ctrl on Linux, Alt on both; right-click always works), `cmd`, `ctrl`, `alt`, `any`,
    /// `plain` or `off`.
    pub link_click: String,
    /// Ask before opening a file in its native app, opening a link, or downloading from a box:
    /// `always` (default) or `never`.
    pub confirm_open: String,
    /// Where a file preview opens from a shell tab: `right` (default), `left`, or `tab` (a
    /// separate full-screen preview tab).
    pub preview_side: String,
    /// Width of the side preview pane as a percentage of the window (clamped to 25..75).
    pub preview_width_pct: u16,
    /// Ask the host terminal for enhanced key reporting (kitty protocol, modifyOtherKeys) and
    /// translate it back to classic bytes: `auto` (default) or `off`.
    pub key_protocol: String,
    /// What Shift+Enter sends: `auto` (newline in agent tabs, Enter elsewhere), `newline`,
    /// `esc-cr` or `enter`.
    pub shift_enter: String,
    /// Colour theme: `auto` (ask the terminal, then COLORFGBG, then dark), `dark` or `light`.
    pub theme: String,
    /// Map the terminal's named ANSI colours (0-15) in shell tabs to the theme palette.
    pub ansi_remap: bool,
    /// Paint the whole window (shell area included) with the theme's background and text
    /// colours. `false` leaves default colours to the terminal.
    pub paint_background: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            prefix_key: "ctrl-space".into(),
            download_dir: "~/devbox".into(),
            download_confirm_mb: 100,
            preview_read_kb: 256,
            preview_autorefresh: true,
            preview_refresh_debounce_ms: 5000,
            csv_rows: 200,
            fps_cap: 30,
            scrollback_lines: 1000,
            quiet_secs: 5,
            bell_on_quiet: false,
            mouse: true,
            agent_start: "ask".into(),
            link_click: "auto".into(),
            confirm_open: "always".into(),
            preview_side: "right".into(),
            preview_width_pct: 50,
            key_protocol: "auto".into(),
            shift_enter: "auto".into(),
            theme: "auto".into(),
            ansi_remap: true,
            paint_background: true,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct BoxConfig {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    pub identity: Option<String>,
    /// Override known_hosts file (default: ~/.ssh/known_hosts)
    pub known_hosts: Option<String>,
    /// Run shells inside tmux on the box so they survive disconnects (default true).
    /// Set false to always use plain shells.
    #[serde(default = "default_true")]
    pub use_tmux: bool,
}

fn default_true() -> bool {
    true
}

fn default_port() -> u16 {
    22
}

/// Your usual ways of starting an agent, e.g. `claude --continue`.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AgentCfg {
    /// One usual command.
    #[serde(default)]
    pub command: String,
    /// Several commands, offered after `command` in the start list.
    #[serde(default)]
    pub commands: Vec<String>,
}

impl AgentCfg {
    /// `command` first, then `commands`; blanks and repeats dropped.
    pub fn all(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for c in std::iter::once(&self.command).chain(self.commands.iter()) {
            let c = c.trim();
            if !c.is_empty() && !out.iter().any(|o| o == c) {
                out.push(c.to_string());
            }
        }
        out
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Config {
    /// Overrides for which files wi previews itself and which go to the native app.
    #[serde(default)]
    pub open: crate::opener::OpenCfg,
    #[serde(default)]
    pub agents: std::collections::HashMap<String, AgentCfg>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default, rename = "box")]
    pub boxes: Vec<BoxConfig>,
}

pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("WICLI_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".config/wicli/config.toml")
}

pub fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn find_box(&self, name: &str) -> Option<&BoxConfig> {
        self.boxes.iter().find(|b| b.name == name)
    }
}

/// What `~/.ssh/config` says about one host alias.
#[derive(Debug, PartialEq, Eq, Default, Clone)]
pub struct SshHost {
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity: Option<String>,
    /// Set when ProxyJump/ProxyCommand applies; wi does not support those.
    pub proxy: Option<String>,
}

pub fn ssh_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("WICLI_SSH_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".ssh/config")
}

/// Look `alias` up in ssh config text. Returns None unless a `Host` line names the alias
/// explicitly (a lone `Host *` must not make every typo look like a box). As in OpenSSH,
/// the first value found for each option wins, and wildcard blocks still contribute.
pub fn parse_ssh_config(text: &str, alias: &str) -> Option<SshHost> {
    let mut host = SshHost::default();
    let mut named = false;
    let mut active = false;
    let mut hostname: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, val) = match line.find(|c: char| c.is_whitespace() || c == '=') {
            Some(i) => (&line[..i], line[i..].trim_start_matches(|c: char| c.is_whitespace() || c == '=').trim()),
            None => (line, ""),
        };
        let val = unquote(val);
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                let pats: Vec<&str> = val.split_whitespace().collect();
                let negated = pats.iter().any(|p| p.strip_prefix('!').is_some_and(|n| glob(n, alias)));
                active = !negated && pats.iter().any(|p| !p.starts_with('!') && glob(p, alias));
                if active && pats.contains(&alias) {
                    named = true;
                }
            }
            // Conditional blocks are not supported: ignore everything inside them.
            "match" => active = false,
            _ if !active => {}
            "hostname" => hostname = hostname.or(Some(val.to_string())),
            "user" => host.user = host.user.take().or(Some(val.to_string())),
            "port" => host.port = host.port.or(val.parse().ok()),
            "identityfile" => host.identity = host.identity.take().or(Some(val.to_string())),
            "proxyjump" | "proxycommand" if !val.eq_ignore_ascii_case("none") => {
                host.proxy = host.proxy.take().or(Some(key.to_string()));
            }
            _ => {}
        }
    }
    if !named {
        return None;
    }
    host.hostname = hostname.unwrap_or_else(|| alias.to_string()).replace("%h", alias);
    Some(host)
}

fn unquote(v: &str) -> &str {
    v.trim().trim_matches('"')
}

/// ssh-style pattern match: `*` any run, `?` one character.
fn glob(pat: &str, s: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(b'*') => (0..=s.len()).any(|i| go(&p[1..], &s[i..])),
            Some(b'?') => !s.is_empty() && go(&p[1..], &s[1..]),
            Some(c) => s.first() == Some(c) && go(&p[1..], &s[1..]),
        }
    }
    go(pat.as_bytes(), s.as_bytes())
}

impl Config {
    /// A box taken from `~/.ssh/config` (or WICLI_SSH_CONFIG). Ok(None) when the alias is not there.
    pub fn ssh_config_box(alias: &str) -> Result<Option<BoxConfig>> {
        let path = ssh_config_path();
        let Ok(text) = std::fs::read_to_string(&path) else { return Ok(None) };
        let Some(h) = parse_ssh_config(&text, alias) else { return Ok(None) };
        if let Some(p) = h.proxy {
            anyhow::bail!("'{alias}' in {} uses {p}, which wi does not support yet", path.display());
        }
        let user = h
            .user
            .or_else(|| std::env::var("USER").ok())
            .context("no User in ssh config and $USER is not set")?;
        Ok(Some(BoxConfig {
            name: alias.to_string(),
            host: h.hostname,
            port: h.port.unwrap_or(22),
            user,
            identity: h.identity,
            known_hosts: None,
            use_tmux: true,
        }))
    }
}

/// Parse a key name like "ctrl-space" or "ctrl-g" into the control byte the
/// terminal sends. Only Ctrl-combinations are supported: they never clash with
/// text input.
pub fn parse_prefix_key(name: &str) -> Option<u8> {
    let n = name.trim().to_ascii_lowercase();
    let rest = n.strip_prefix("ctrl-").or_else(|| n.strip_prefix("ctrl+"))?;
    if rest == "space" {
        return Some(0x00);
    }
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_lowercase() => Some(c as u8 - b'a' + 1),
        _ => None,
    }
}

/// Human-readable form for the status bar, e.g. "Ctrl-Space".
pub fn prefix_label(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    match n.strip_prefix("ctrl-").or_else(|| n.strip_prefix("ctrl+")) {
        Some(rest) => {
            let mut c = rest.chars();
            let cap: String = c.next().map(|f| f.to_ascii_uppercase().to_string()).unwrap_or_default() + c.as_str();
            format!("Ctrl-{cap}")
        }
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_keys() {
        assert_eq!(parse_prefix_key("ctrl-space"), Some(0));
        assert_eq!(parse_prefix_key("Ctrl-G"), Some(7));
        assert_eq!(parse_prefix_key("ctrl+a"), Some(1));
        assert_eq!(parse_prefix_key("ctrl-]"), None);
        assert_eq!(parse_prefix_key("g"), None);
        assert_eq!(prefix_label("ctrl-space"), "Ctrl-Space");
    }

    #[test]
    fn defaults_parse() {
        let c: Config = toml::from_str("[[box]]\nname=\"a\"\nhost=\"h\"\nuser=\"u\"\n").unwrap();
        assert_eq!(c.boxes[0].port, 22);
        assert_eq!(c.defaults.prefix_key, "ctrl-space");
        assert_eq!((c.defaults.key_protocol.as_str(), c.defaults.shift_enter.as_str()), ("auto", "auto"));
    }

    const SSH: &str = "# comment\nHost dev devbox\n  HostName 10.0.0.5\n  User fahad\n  Port 2200\n  IdentityFile ~/.ssh/id_dev\n\nHost jump\n  HostName j.example.com\n  ProxyJump bastion\n\nHost web-*\n  User www\n\nHost *\n  User fallback\n  Port 22\n  IdentityFile ~/.ssh/id_any\n";

    #[test]
    fn ssh_config_host() {
        let h = parse_ssh_config(SSH, "devbox").unwrap();
        assert_eq!(h.hostname, "10.0.0.5");
        assert_eq!(h.user.as_deref(), Some("fahad"));
        assert_eq!(h.port, Some(2200));
        assert_eq!(h.identity.as_deref(), Some("~/.ssh/id_dev"));
        assert!(h.proxy.is_none());
    }

    #[test]
    fn ssh_config_wildcards() {
        // Not named explicitly: only wildcard blocks match, so it is not an alias.
        assert!(parse_ssh_config(SSH, "unknown").is_none());
        assert!(parse_ssh_config(SSH, "web-1").is_none());
        // Wildcard block fills what the named block lacks.
        let h = parse_ssh_config("Host a\n HostName h\nHost *\n User u\n IdentityFile ~/k\n", "a").unwrap();
        assert_eq!((h.user.as_deref(), h.identity.as_deref()), (Some("u"), Some("~/k")));
        // First value wins.
        let h = parse_ssh_config("Host a\n User first\nHost *\n User second\n", "a").unwrap();
        assert_eq!(h.user.as_deref(), Some("first"));
        // Defaults: hostname is the alias.
        let h = parse_ssh_config("Host a\n", "a").unwrap();
        assert_eq!((h.hostname.as_str(), h.port), ("a", None));
    }

    #[test]
    fn ssh_config_proxy_rejected() {
        let h = parse_ssh_config(SSH, "jump").unwrap();
        assert_eq!(h.proxy.as_deref(), Some("ProxyJump"));
        let h = parse_ssh_config("Host a\n ProxyCommand ssh -W %h:%p x\n", "a").unwrap();
        assert!(h.proxy.is_some());
        assert!(parse_ssh_config("Host a\n ProxyJump none\n", "a").unwrap().proxy.is_none());
    }

    #[test]
    fn ssh_config_syntax() {
        let h = parse_ssh_config("host=a\n  hostname = \"h.example\"\n  port=2022\n", "a").unwrap();
        assert_eq!((h.hostname.as_str(), h.port), ("h.example", Some(2022)));
        assert!(parse_ssh_config("Host !a *\n User u\nHost a\n", "a").is_some());
        assert!(parse_ssh_config("Host b\n HostName x\n", "a").is_none());
    }
}
