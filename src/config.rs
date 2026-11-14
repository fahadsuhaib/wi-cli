use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct Defaults {
    pub prefix_key: String,
    pub preview_read_kb: u64,
    /// Reload a visible file preview by itself when the file changes on disk (a stat of the
    /// visible file every second; no watchers).
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
    /// Ask before opening a file in its native app or opening a link: `always` (default) or
    /// `never`.
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
    /// Colour theme: `auto` (ask the terminal, then COLORFGBG, then dark) or a theme name (`dark`, `light`, `monokai`, ...; see README).
    pub theme: String,
    /// Map the terminal's named ANSI colours (0-15) in shell tabs to the theme palette.
    pub ansi_remap: bool,
    /// Paint the whole window (shell area included) with the theme's background and text
    /// colours. `false` leaves default colours to the terminal.
    pub paint_background: bool,
    /// Reopen the tabs of the last session (folders, names, agents, pinned layout) when wi starts
    /// in the same workspace; saved at a normal quit, ignored after 30 days. `wi --fresh` skips it.
    pub restore_tabs: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            prefix_key: "ctrl-space".into(),
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
            restore_tabs: true,
        }
    }
}

/// `[transcribe]`: voice dictation (macOS only for now). Everything else about it is internal.
#[derive(Debug, Deserialize, Clone)]
#[serde(default)]
pub struct Transcribe {
    /// Allow the Dictation menu entry to work.
    pub enabled: bool,
    /// Dictation turns itself off after this many minutes without speech (0 = never).
    pub idle_minutes: u32,
}

impl Default for Transcribe {
    fn default() -> Self {
        Self { enabled: true, idle_minutes: 10 }
    }
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
    #[serde(default)]
    pub transcribe: Transcribe,
}

pub fn config_path() -> PathBuf {
    if let Ok(p) = std::env::var("WICLI_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".config/wicli/config.toml")
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
    fn restore_tabs_defaults_on_and_can_be_turned_off() {
        let c: Config = toml::from_str("").unwrap();
        assert!(c.defaults.restore_tabs);
        let c: Config = toml::from_str("[defaults]\nrestore_tabs = false\n").unwrap();
        assert!(!c.defaults.restore_tabs);
    }

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
        let c: Config = toml::from_str("[defaults]\n").unwrap();
        assert_eq!(c.defaults.prefix_key, "ctrl-space");
        assert_eq!((c.defaults.key_protocol.as_str(), c.defaults.shift_enter.as_str()), ("auto", "auto"));
    }
}
