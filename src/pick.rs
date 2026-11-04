//! Finding a clicked relative path that is not where the shell says it is: one bounded search
//! under the workspace (depth, entry count and time all capped, through `Backend`), and a small
//! picker when several places match. Nothing is guessed silently.

use crate::ask::AskKey;
use crate::workspace::Backend;
use std::cell::Cell;
use std::time::Duration;

/// Deepest folder level looked at, and the most entries read.
pub const MAX_DEPTH: usize = 6;
pub const MAX_ENTRIES: usize = 5000;
const TIMEOUT: Duration = Duration::from_secs(4);

/// Folders print with a trailing `/`, files without; `.git` and `node_modules` are skipped.
fn script() -> String {
    format!(
        "find . -maxdepth {MAX_DEPTH} \\( -name .git -o -name node_modules \\) -prune -o -type d -exec printf '%s/\\n' {{}} + -o -print 2>/dev/null | head -n {}",
        MAX_ENTRIES + 1
    )
}

/// Entries (`./a/b.rs`, `./a/dir/`) whose tail equals `path`, as absolute paths under `base`.
/// A path with `..` or an empty one matches nothing.
pub fn matches(entries: &[String], path: &str, base: &str) -> Vec<String> {
    let want = path.trim_start_matches("./").trim_end_matches('/');
    if want.is_empty() || want.split('/').any(|c| c == ".." || c == "." || c.is_empty()) {
        return Vec::new();
    }
    let tail = format!("/{want}");
    let mut out: Vec<String> = Vec::new();
    for e in entries {
        let rel = e.strip_prefix("./").unwrap_or(e);
        let dir = rel.ends_with('/');
        let t = rel.trim_end_matches('/');
        if t == want || t.ends_with(&tail) {
            let abs = format!("{}/{}", base.trim_end_matches('/'), t);
            let abs = if dir { format!("{abs}/") } else { abs };
            if !out.contains(&abs) {
                out.push(abs);
            }
        }
    }
    out.sort();
    out
}

/// Search `bases` (deduplicated, outer folders first) for places `path` could mean.
pub async fn search(backend: &Backend, bases: &[String], path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in bases {
        let Ok(o) = backend.run_script(b, &script(), 1 << 20, TIMEOUT).await else { continue };
        let text = String::from_utf8_lossy(&o.stdout);
        let entries: Vec<String> = text.lines().take(MAX_ENTRIES).filter(|l| !l.chars().any(char::is_control)).map(str::to_string).collect();
        for m in matches(&entries, path, b) {
            if !out.contains(&m) {
                out.push(m);
            }
        }
    }
    out
}

pub enum PickOutcome {
    Choose(String),
    Cancel,
}

/// The picker: one row per candidate.
pub struct Pick {
    pub path: String,
    pub items: Vec<String>,
    pub sel: usize,
    pub line: Option<u32>,
    pub scroll: Cell<usize>,
}

impl Pick {
    pub fn new(path: String, items: Vec<String>, line: Option<u32>) -> Pick {
        Pick { path, items, sel: 0, line, scroll: Cell::new(0) }
    }

    pub fn key(&mut self, k: AskKey) -> Option<PickOutcome> {
        let n = self.items.len();
        match k {
            AskKey::Up => self.sel = if self.sel == 0 { n.saturating_sub(1) } else { self.sel - 1 },
            AskKey::Down => self.sel = if self.sel + 1 >= n { 0 } else { self.sel + 1 },
            AskKey::Home => self.sel = 0,
            AskKey::End => self.sel = n.saturating_sub(1),
            AskKey::Enter => return self.items.get(self.sel).cloned().map(PickOutcome::Choose),
            AskKey::Esc => return Some(PickOutcome::Cancel),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn unique_and_multiple_matches() {
        let e = v(&["./a/src/main.rs", "./b/src/main.rs", "./a/src/", "./a/other.rs", "./b/src/"]);
        assert_eq!(matches(&e, "src/main.rs", "/w"), ["/w/a/src/main.rs", "/w/b/src/main.rs"]);
        assert_eq!(matches(&e, "other.rs", "/w/"), ["/w/a/other.rs"]);
        assert_eq!(matches(&e, "nope.rs", "/w"), Vec::<String>::new());
    }

    #[test]
    fn folders_match_with_or_without_slash() {
        let e = v(&["./a/src/", "./a/src/x.rs", "./srcs/"]);
        assert_eq!(matches(&e, "src/", "/w"), ["/w/a/src/"]);
        assert_eq!(matches(&e, "./src", "/w"), ["/w/a/src/"]);
        // a whole-name match, not a suffix of a longer name
        assert_eq!(matches(&e, "rc/", "/w"), Vec::<String>::new());
    }

    #[test]
    fn dotdot_and_empty_match_nothing() {
        let e = v(&["./a/b.rs"]);
        assert!(matches(&e, "../b.rs", "/w").is_empty());
        assert!(matches(&e, "", "/w").is_empty());
        assert!(matches(&e, "a//b.rs", "/w").is_empty());
    }

    #[test]
    fn script_is_bounded() {
        let s = script();
        assert!(s.contains("-maxdepth 6") && s.contains("head -n 5001"));
    }

    #[test]
    fn picker_keys() {
        let mut p = Pick::new("m.rs".into(), v(&["/a/m.rs", "/b/m.rs", "/c/m.rs"]), None);
        assert!(p.key(AskKey::Up).is_none());
        assert_eq!(p.sel, 2);
        p.key(AskKey::Down);
        assert_eq!(p.sel, 0);
        p.key(AskKey::Down);
        assert!(matches!(p.key(AskKey::Enter), Some(PickOutcome::Choose(s)) if s == "/b/m.rs"));
        assert!(matches!(p.key(AskKey::Esc), Some(PickOutcome::Cancel)));
    }
}
