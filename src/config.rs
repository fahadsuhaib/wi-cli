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
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            prefix_key: "ctrl-space".into(),
            download_dir: "~/devbox".into(),
            download_confirm_mb: 100,
            preview_read_kb: 256,
            csv_rows: 200,
            fps_cap: 30,
            scrollback_lines: 1000,
            quiet_secs: 5,
            bell_on_quiet: false,
            mouse: true,
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
}

fn default_port() -> u16 {
    22
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Config {
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
    }
}
