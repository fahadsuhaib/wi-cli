//! The list of files recently shown in previews, newest first, kept per workspace in state.toml.

use serde::{Deserialize, Serialize};

/// How many files are remembered per workspace.
pub const MAX: usize = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Absolute path as the preview showed it.
    pub path: String,
    /// When it was last shown (seconds since the epoch).
    pub at: u64,
}

/// Record `path` as shown at `at`: moved to the front when already there, list capped at `MAX`.
pub fn push(list: &mut Vec<Entry>, path: &str, at: u64) {
    list.retain(|e| e.path != path);
    list.insert(0, Entry { path: path.to_string(), at });
    list.truncate(MAX);
}

/// Seconds since the epoch now (0 if the clock is before it).
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// How long ago, in a few characters: `just now`, `5m ago`, `3h ago`, `2d ago`.
pub fn ago(now: u64, at: u64) -> String {
    let s = now.saturating_sub(at);
    match s {
        0..=44 => "just now".to_string(),
        45..=3599 => format!("{}m ago", (s / 60).max(1)),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

/// The path as listed: relative to `root` when it is inside it, else as it is.
pub fn display(path: &str, root: &str) -> String {
    let root = root.trim_end_matches('/');
    match path.strip_prefix(root).and_then(|r| r.strip_prefix('/')) {
        Some(rel) if !root.is_empty() && !rel.is_empty() => rel.to_string(),
        _ => path.to_string(),
    }
}

/// Fit `path` into `max` characters by cutting out the middle (marked with `…`), keeping the
/// start and always the end with the file name.
pub fn shorten_middle(path: &str, max: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max {
        return path.to_string();
    }
    if max <= 1 {
        return "…".chars().take(max).collect();
    }
    let name_len = chars.iter().rev().position(|&c| c == '/').unwrap_or(chars.len());
    // The file name (with its slash) is kept whole when it fits; the rest is shared by head and dir tail.
    let keep_tail = (name_len + 1).min(max - 1);
    let spare = (max - 1) - keep_tail;
    let head = spare.div_ceil(2);
    let tail = keep_tail + spare / 2;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(l: &[Entry]) -> Vec<&str> {
        l.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn newest_first_and_deduped() {
        let mut l = Vec::new();
        push(&mut l, "/a", 1);
        push(&mut l, "/b", 2);
        push(&mut l, "/c", 3);
        assert_eq!(paths(&l), ["/c", "/b", "/a"]);
        push(&mut l, "/a", 9);
        assert_eq!(paths(&l), ["/a", "/c", "/b"]);
        assert_eq!(l[0].at, 9);
        push(&mut l, "/a", 10);
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn bounded() {
        let mut l = Vec::new();
        for i in 0..(MAX + 12) {
            push(&mut l, &format!("/f{i}"), i as u64);
        }
        assert_eq!(l.len(), MAX);
        assert_eq!(l[0].path, format!("/f{}", MAX + 11));
        assert_eq!(l[MAX - 1].path, "/f12");
    }

    #[test]
    fn ages() {
        assert_eq!(ago(100, 100), "just now");
        assert_eq!(ago(100, 200), "just now");
        assert_eq!(ago(1000, 700), "5m ago");
        assert_eq!(ago(10_000, 10_000 - 7300), "2h ago");
        assert_eq!(ago(1_000_000, 1_000_000 - 3 * 86_400 - 5), "3d ago");
    }

    #[test]
    fn display_relative_to_root() {
        assert_eq!(display("/w/proj/src/a.rs", "/w/proj"), "src/a.rs");
        assert_eq!(display("/w/proj/src/a.rs", "/w/proj/"), "src/a.rs");
        assert_eq!(display("/w/projx/a.rs", "/w/proj"), "/w/projx/a.rs");
        assert_eq!(display("/etc/hosts", "/w/proj"), "/etc/hosts");
        assert_eq!(display("/a.rs", "/"), "/a.rs");
    }

    #[test]
    fn middle_is_cut_and_name_kept() {
        let p = "/home/me/work/project/src/deep/module/file_name.rs";
        assert_eq!(shorten_middle(p, 100), p);
        for max in [12, 20, 30, 40] {
            let s = shorten_middle(p, max);
            assert_eq!(s.chars().count(), max, "{s}");
            assert!(s.ends_with("file_name.rs") || max < 13, "{s}");
            assert!(s.contains('…'));
        }
        assert!(shorten_middle(p, 20).starts_with("/ho"));
        // too narrow for the name: its end survives
        assert_eq!(shorten_middle("/a/b/long_file_name.rs", 8).chars().count(), 8);
        assert_eq!(shorten_middle("abc", 0), "");
    }

    #[test]
    fn persists_through_state_toml() {
        let mut st = crate::agents::State::default();
        let mut l = Vec::new();
        push(&mut l, "/w/a b.rs", 5);
        push(&mut l, "/w/\"q\".md", 6);
        st.files.insert("local:/w".into(), l.clone());
        let text = toml::to_string_pretty(&st).unwrap();
        let back: crate::agents::State = toml::from_str(&text).unwrap();
        assert_eq!(back.files.get("local:/w"), Some(&l));
        assert!(toml::from_str::<crate::agents::State>("").unwrap().files.is_empty());
    }
}
