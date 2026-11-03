//! Restart wi in place: the layout of the session is written to `state.toml`, wi executes itself
//! (the same file, so after an update it is the new binary) with `WICLI_RESTORE=1`, and the new
//! process reopens the tabs and removes the snapshot. Local shells cannot survive (no tmux behind
//! them), so they start fresh in the same folders; remote tmux sessions simply reattach.

use serde::{Deserialize, Serialize};

/// Set in the environment of the restarted wi.
pub const MARKER: &str = "WICLI_RESTORE";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapTab {
    /// `shell`, `agent` or `preview`.
    pub kind: String,
    /// `local` or `remote`.
    pub backend: String,
    /// Live folder of a shell or agent tab.
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub title: String,
    /// `claude` or `codex` for an agent tab.
    #[serde(default)]
    pub agent: String,
    /// File shown in a preview tab.
    #[serde(default)]
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// `agents::workspace_key` of the workspace it was taken in; only restored into the same one.
    pub key: String,
    #[serde(default)]
    pub active: usize,
    /// Index of the pinned tab.
    #[serde(default)]
    pub pinned: Option<usize>,
    #[serde(default)]
    pub pin_side: String,
    /// Where side previews go (`right`, `left`, `tab`) and how wide (percent).
    #[serde(default)]
    pub side: String,
    #[serde(default)]
    pub pct: u16,
    #[serde(default)]
    pub tabs: Vec<SnapTab>,
}

impl Snapshot {
    /// Indexes are kept inside the tab list.
    pub fn clamp(&mut self) {
        if self.active >= self.tabs.len() {
            self.active = 0;
        }
        if self.pinned.is_some_and(|p| p >= self.tabs.len()) {
            self.pinned = None;
        }
    }
}

/// `/usr/bin/wi (deleted)` (Linux, after the file was replaced) back to the real path.
pub fn strip_deleted(p: &str) -> &str {
    p.strip_suffix(" (deleted)").unwrap_or(p)
}

/// Plain words for the "Restart wi now?" question.
pub fn summary(tabs: usize, remote: bool, tmux: bool) -> String {
    let n = format!("{tabs} tab{}", if tabs == 1 { "" } else { "s" });
    match (remote, tmux) {
        (false, _) => format!("{n} will be restarted: shells and agents start fresh in the same folders, previews reopen."),
        (true, true) => format!("{n}: remote shells reattach and keep running, previews reopen."),
        (true, false) => format!("{n} will be restarted: this box has no tmux, so shells start fresh in the same folders."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            key: "local:/work".into(),
            active: 2,
            pinned: Some(1),
            pin_side: "left".into(),
            side: "right".into(),
            pct: 45,
            tabs: vec![
                SnapTab { kind: "shell".into(), backend: "local".into(), cwd: "/work/a b".into(), title: "a b".into(), ..Default::default() },
                SnapTab { kind: "agent".into(), backend: "local".into(), cwd: "/work".into(), title: "claude".into(), agent: "claude".into(), ..Default::default() },
                SnapTab { kind: "preview".into(), backend: "local".into(), title: "x.md".into(), path: "/work/x.md".into(), ..Default::default() },
            ],
        }
    }

    #[test]
    fn snapshot_round_trips_through_state_toml() {
        let st = crate::agents::State { restart: Some(sample()), ..Default::default() };
        let text = toml::to_string_pretty(&st).unwrap();
        let back: crate::agents::State = toml::from_str(&text).unwrap();
        assert_eq!(back.restart, Some(sample()));
        // Nothing saved means no section at all.
        assert!(!toml::to_string_pretty(&crate::agents::State::default()).unwrap().contains("restart"));
    }

    #[test]
    fn clamp_drops_out_of_range_indexes() {
        let mut s = sample();
        s.active = 9;
        s.pinned = Some(7);
        s.clamp();
        assert_eq!((s.active, s.pinned), (0, None));
    }

    #[test]
    fn deleted_suffix_and_summary() {
        assert_eq!(strip_deleted("/usr/local/bin/wi (deleted)"), "/usr/local/bin/wi");
        assert_eq!(strip_deleted("/usr/local/bin/wi"), "/usr/local/bin/wi");
        assert!(summary(1, false, false).starts_with("1 tab will be restarted"));
        assert!(summary(3, true, true).contains("reattach"));
        assert!(summary(2, true, false).contains("no tmux"));
    }
}
