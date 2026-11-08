//! Per-file git diffs: a small unified-diff parser (pure, tested) and the call that fetches one
//! file's staged or unstaged diff through the same capped, timed shell path as `git_marks`
//! (local and remote). Git computes the diff; wi only reads it. Read-only: nothing here writes
//! to the repository, and `GIT_OPTIONAL_LOCKS=0` keeps git from touching the index.

use crate::exec::{OUT_CAP, sq};
use crate::workspace::Backend;
use anyhow::{Result, bail};
use std::time::Duration;

const GIT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Context(String),
    Add(String),
    Del(String),
    /// `\ No newline at end of file`
    NoNewline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The whole `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    /// The path before a rename or copy.
    pub old_path: Option<String>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    pub new_file: bool,
    pub deleted: bool,
    /// `(old mode, new mode)` when the file mode changed.
    pub mode: Option<(String, String)>,
    /// A merge-conflict (combined) diff.
    pub combined: bool,
    /// The output was cut at the cap or the time limit: only the first part is here.
    pub truncated: bool,
    pub added: u32,
    pub removed: u32,
}

impl FileDiff {
    #[cfg(test)]
    /// Nothing to show (no hunks, not binary, no mode or rename note).
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty() && !self.binary && self.mode.is_none() && self.old_path.is_none() && !self.new_file && !self.deleted
    }
}

// ---------------------------------------------------------------- parsing

/// Read a C-style quoted string (git quotes paths with special characters) from the start of `s`,
/// which begins with `"`. Returns the text and what follows the closing quote.
fn unquote_prefix(s: &str) -> (String, &str) {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'"' => return (String::from_utf8_lossy(&out).into_owned(), &s[i + 1..]),
            b'\\' if i + 1 < b.len() => {
                i += 1;
                match b[i] {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'a' => out.push(7),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'v' => out.push(11),
                    d @ b'0'..=b'3' => {
                        let mut v = u32::from(d - b'0');
                        let mut n = 1;
                        while n < 3 && i + 1 < b.len() && (b'0'..=b'7').contains(&b[i + 1]) {
                            i += 1;
                            v = v * 8 + u32::from(b[i] - b'0');
                            n += 1;
                        }
                        out.push(v as u8);
                    }
                    c => out.push(c),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    (String::from_utf8_lossy(&out).into_owned(), "")
}

/// A path as git prints it after a keyword: quoted or plain, trailing tab (from `---`/`+++`) cut.
fn path_text(s: &str) -> String {
    let s = s.trim_end_matches('\t');
    if s.starts_with('"') { unquote_prefix(s).0 } else { s.to_string() }
}

/// Drop the `a/` or `b/` prefix of a diff path.
fn strip_ab(p: &str) -> String {
    p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p).to_string()
}

/// The two paths of `diff --git <a> <b>` (without the `diff --git ` prefix).
fn header_paths(rest: &str) -> (String, String) {
    if rest.starts_with('"') {
        let (a, r) = unquote_prefix(rest);
        let r = r.trim_start();
        let b = if r.starts_with('"') { unquote_prefix(r).0 } else { r.to_string() };
        return (strip_ab(&a), strip_ab(&b));
    }
    if let Some(pos) = rest.find(" \"") {
        let (b, _) = unquote_prefix(&rest[pos + 1..]);
        return (strip_ab(&rest[..pos]), strip_ab(&b));
    }
    // Both paths plain, possibly with spaces: pick the split where both sides name the same file
    // (not a rename), else the first ` b/`.
    let cands: Vec<usize> = rest.match_indices(" b/").map(|(i, _)| i).collect();
    for &i in &cands {
        let (a, b) = (strip_ab(&rest[..i]), strip_ab(&rest[i + 1..]));
        if a == b {
            return (a, b);
        }
    }
    match cands.first() {
        Some(&i) => (strip_ab(&rest[..i]), strip_ab(&rest[i + 1..])),
        None => (strip_ab(rest), strip_ab(rest)),
    }
}

fn parse_range(s: &str) -> Option<(u32, u32)> {
    let s = s.trim_start_matches(['-', '+']);
    let mut it = s.split(',');
    let start = it.next()?.parse().ok()?;
    let count = match it.next() {
        Some(c) => c.parse().ok()?,
        None => 1,
    };
    Some((start, count))
}

/// `@@ -a,b +c,d @@` gives (a, b, c, d). A combined header (`@@@ -a,b -c,d +e,f @@@`) gives the
/// first parent's start and the result's start (counts unused).
fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32, bool)> {
    let combined = line.starts_with("@@@");
    let rest = line.trim_start_matches('@').trim_start();
    let end = rest.find(" @@").unwrap_or(rest.len());
    let parts: Vec<&str> = rest[..end].split_whitespace().collect();
    if combined {
        let old = parts.iter().find(|p| p.starts_with('-')).and_then(|p| parse_range(p))?;
        let new = parts.iter().rev().find(|p| p.starts_with('+')).and_then(|p| parse_range(p))?;
        return Some((old.0, 0, new.0, 0, true));
    }
    if parts.len() < 2 || !parts[0].starts_with('-') || !parts[1].starts_with('+') {
        return None;
    }
    let (o, n) = (parse_range(parts[0])?, parse_range(parts[1])?);
    Some((o.0, o.1, n.0, n.1, false))
}

/// Parse unified diff text (what `git diff` prints) into one `FileDiff` per file. `truncated`
/// says the text was cut (output cap or timeout): the last, possibly partial line is dropped
/// and the last file is flagged. Never panics on malformed input.
pub fn parse_diff(text: &str, truncated: bool) -> Vec<FileDiff> {
    let text = if truncated { text.rfind('\n').map_or("", |i| &text[..=i]) } else { text };
    let lines: Vec<&str> = text.lines().collect();
    let mut files: Vec<FileDiff> = Vec::new();
    let mut cur: Option<FileDiff> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.extend(cur.take());
            let (a, b) = header_paths(rest);
            cur = Some(FileDiff { old_path: (a != b).then(|| a.clone()), path: b, ..FileDiff::default() });
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --cc ").or_else(|| line.strip_prefix("diff --combined ")) {
            files.extend(cur.take());
            cur = Some(FileDiff { path: path_text(rest), combined: true, ..FileDiff::default() });
            continue;
        }
        if line.starts_with("diff ") {
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };
        if line.starts_with("@@") {
            let Some((os, oc, ns, nc, combined)) = parse_hunk_header(line) else { continue };
            let mut h = Hunk { header: line.to_string(), old_start: os, new_start: ns, lines: Vec::new() };
            let (mut ol, mut nl) = (oc, nc);
            let prefix = if combined { line.bytes().take_while(|&b| b == b'@').count().saturating_sub(1) } else { 1 };
            while i < lines.len() {
                let l = lines[i];
                if l.starts_with('\\') {
                    h.lines.push(DiffLine::NoNewline);
                    i += 1;
                    continue;
                }
                if combined {
                    if l.starts_with("diff ") || l.starts_with("@@") {
                        break;
                    }
                } else if ol == 0 && nl == 0 {
                    break;
                }
                let body = |n: usize| l.get(n..).unwrap_or("").to_string();
                if combined {
                    let marks = l.get(..prefix).unwrap_or(l);
                    if marks.contains('+') {
                        h.lines.push(DiffLine::Add(body(prefix)));
                        f.added += 1;
                    } else if marks.contains('-') {
                        h.lines.push(DiffLine::Del(body(prefix)));
                        f.removed += 1;
                    } else {
                        h.lines.push(DiffLine::Context(body(prefix)));
                    }
                } else {
                    match l.as_bytes().first() {
                        Some(b'+') => {
                            h.lines.push(DiffLine::Add(body(1)));
                            nl = nl.saturating_sub(1);
                            f.added += 1;
                        }
                        Some(b'-') => {
                            h.lines.push(DiffLine::Del(body(1)));
                            ol = ol.saturating_sub(1);
                            f.removed += 1;
                        }
                        Some(b' ') | None => {
                            h.lines.push(DiffLine::Context(body(1)));
                            ol = ol.saturating_sub(1);
                            nl = nl.saturating_sub(1);
                        }
                        Some(_) => break,
                    }
                }
                i += 1;
            }
            f.hunks.push(h);
            continue;
        }
        if let Some(r) = line.strip_prefix("new file mode ") {
            f.new_file = true;
            let _ = r;
        } else if line.starts_with("deleted file mode ") {
            f.deleted = true;
        } else if let Some(r) = line.strip_prefix("old mode ") {
            f.mode = Some((r.trim().to_string(), f.mode.take().map(|m| m.1).unwrap_or_default()));
        } else if let Some(r) = line.strip_prefix("new mode ") {
            f.mode = Some((f.mode.take().map(|m| m.0).unwrap_or_default(), r.trim().to_string()));
        } else if let Some(r) = line.strip_prefix("rename from ").or_else(|| line.strip_prefix("copy from ")) {
            f.old_path = Some(path_text(r));
        } else if let Some(r) = line.strip_prefix("rename to ").or_else(|| line.strip_prefix("copy to ")) {
            f.path = path_text(r);
        } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            f.binary = true;
        } else if let Some(r) = line.strip_prefix("+++ ") {
            let p = path_text(r);
            if p != "/dev/null" {
                f.path = strip_ab(&p);
            }
        } else if let Some(r) = line.strip_prefix("--- ") {
            let p = path_text(r);
            if p == "/dev/null" {
                f.new_file = true;
            } else if f.deleted {
                f.path = strip_ab(&p);
            }
        }
    }
    files.extend(cur);
    if truncated && let Some(l) = files.last_mut() {
        l.truncated = true;
    }
    files
}

// ---------------------------------------------------------------- fetching

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Staged,
    Unstaged,
}

impl Side {
    pub fn word(self) -> &'static str {
        match self {
            Side::Staged => "staged",
            Side::Unstaged => "unstaged",
        }
    }
}

/// What to fetch for one file: which sides have changes (from the status marks).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffSpec {
    pub staged: bool,
    pub unstaged: bool,
    /// The file is untracked: shown whole, as additions.
    pub untracked: bool,
    /// Repo-root-relative path before a staged rename.
    pub orig: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffData {
    pub staged: Option<FileDiff>,
    pub unstaged: Option<FileDiff>,
}

/// Flags that keep a user's pager, external diff tool, colour or prefix config out of the output.
const FLAGS: &str = "--no-ext-diff --no-color --no-textconv --src-prefix=a/ --dst-prefix=b/ -U3";
const GIT: &str = "export GIT_OPTIONAL_LOCKS=0; git -c core.quotepath=off -c color.ui=false diff";

/// The shell script for one diff. `path` is relative to the folder the script runs in; always
/// behind `--` and as a literal pathspec, so odd names (leading dash, glob characters) are safe.
pub fn diff_script(path: &str, side: Side, untracked: bool, orig: Option<&str>) -> String {
    if untracked {
        return format!("{GIT} --no-index {FLAGS} -- /dev/null {}", sq(path));
    }
    match side {
        Side::Unstaged => format!("{GIT} {FLAGS} -- {}", sq(&format!(":(literal){path}"))),
        Side::Staged => {
            let mut s = format!("{GIT} --cached -M {FLAGS} -- {}", sq(&format!(":(literal){path}")));
            if let Some(o) = orig {
                s.push(' ');
                s.push_str(&sq(&format!(":(top,literal){o}")));
            }
            s
        }
    }
}

/// The diff of the file `path` (relative to `dir`) that a status line and a render call want.
/// A diff that comes out empty is `Some` with an empty `FileDiff` (the file is unchanged now).
fn one(path: &str, text: &str, truncated: bool) -> FileDiff {
    let mut v = parse_diff(text, truncated);
    // A rename diff lists the file once; anything else with several entries is merged by taking
    // the entry for this path first.
    let at = v.iter().position(|f| f.path == path).unwrap_or(0);
    if v.is_empty() { FileDiff { path: path.to_string(), truncated, ..FileDiff::default() } } else { v.swap_remove(at) }
}

impl Backend {
    async fn git_diff_text(&self, dir: &str, script: &str, untracked: bool) -> Result<(String, bool)> {
        let o = self.run_script(dir, script, OUT_CAP, GIT_TIMEOUT).await?;
        if o.timed_out && o.stdout.is_empty() {
            bail!("git diff timed out");
        }
        // `diff --no-index` exits 1 when the files differ, plain `git diff` 0 (or 1 with --exit-code).
        let _ = untracked;
        if o.stdout.is_empty() && o.code > 1 && !o.stderr.is_empty() {
            bail!("{}", o.stderr.lines().next().unwrap_or("git failed"));
        }
        Ok((String::from_utf8_lossy(&o.stdout).into_owned(), o.truncated || o.timed_out))
    }

    /// Staged and/or unstaged diff of one file. `dir` is the explorer root (a folder in the
    /// repository) and `path` is relative to it. Each side is one capped, timed shell call.
    pub async fn file_diff(&self, dir: &str, path: &str, spec: &DiffSpec) -> Result<DiffData> {
        let mut out = DiffData::default();
        if spec.staged {
            let (t, cut) = self.git_diff_text(dir, &diff_script(path, Side::Staged, false, spec.orig.as_deref()), false).await?;
            out.staged = Some(one(path, &t, cut));
        }
        if spec.unstaged || spec.untracked {
            let (t, cut) = self.git_diff_text(dir, &diff_script(path, Side::Unstaged, spec.untracked, None), spec.untracked).await?;
            out.unstaged = Some(one(path, &t, cut));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(h: &Hunk) -> Vec<String> {
        h.lines
            .iter()
            .map(|l| match l {
                DiffLine::Context(s) => format!(" {s}"),
                DiffLine::Add(s) => format!("+{s}"),
                DiffLine::Del(s) => format!("-{s}"),
                DiffLine::NoNewline => "\\".to_string(),
            })
            .collect()
    }

    #[test]
    fn modified_file_with_two_hunks() {
        let d = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@ fn main\n one\n-two\n+TWO\n three\n@@ -10,2 +10,3 @@\n ten\n+ten and a half\n eleven\n";
        let f = parse_diff(d, false);
        assert_eq!(f.len(), 1);
        let f = &f[0];
        assert_eq!((f.path.as_str(), f.old_path.as_deref()), ("src/a.rs", None));
        assert_eq!((f.added, f.removed), (2, 1));
        assert_eq!(f.hunks.len(), 2);
        assert_eq!(f.hunks[0].header, "@@ -1,3 +1,3 @@ fn main");
        assert_eq!((f.hunks[0].old_start, f.hunks[0].new_start), (1, 1));
        assert_eq!(lines(&f.hunks[0]), [" one", "-two", "+TWO", " three"]);
        assert_eq!((f.hunks[1].old_start, f.hunks[1].new_start), (10, 10));
        assert_eq!(lines(&f.hunks[1]), [" ten", "+ten and a half", " eleven"]);
        assert!(!f.truncated && !f.binary && !f.new_file && !f.deleted);
    }

    #[test]
    fn added_and_deleted_files() {
        let add = "diff --git a/n.txt b/n.txt\nnew file mode 100644\nindex 0000000..e69de29\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n";
        let f = &parse_diff(add, false)[0];
        assert!(f.new_file && !f.deleted);
        assert_eq!((f.path.as_str(), f.added, f.removed), ("n.txt", 2, 0));
        assert_eq!(f.hunks[0].old_start, 0);
        let del = "diff --git a/o.txt b/o.txt\ndeleted file mode 100644\nindex e69de29..0000000\n--- a/o.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-a\n-b\n";
        let f = &parse_diff(del, false)[0];
        assert!(f.deleted && !f.new_file);
        assert_eq!((f.path.as_str(), f.added, f.removed), ("o.txt", 0, 2));
    }

    #[test]
    fn rename_with_and_without_edits() {
        let pure = "diff --git a/old.txt b/new.txt\nsimilarity index 100%\nrename from old.txt\nrename to new.txt\n";
        let f = &parse_diff(pure, false)[0];
        assert_eq!((f.path.as_str(), f.old_path.as_deref()), ("new.txt", Some("old.txt")));
        assert!(f.hunks.is_empty() && !f.is_empty());
        let edited = "diff --git a/old.txt b/new.txt\nsimilarity index 80%\nrename from old.txt\nrename to new.txt\nindex 1..2 100644\n--- a/old.txt\n+++ b/new.txt\n@@ -1 +1 @@\n-x\n+y\n";
        let f = &parse_diff(edited, false)[0];
        assert_eq!((f.path.as_str(), f.old_path.as_deref(), f.added, f.removed), ("new.txt", Some("old.txt"), 1, 1));
    }

    #[test]
    fn binary_file() {
        let d = "diff --git a/img.png b/img.png\nindex 1..2 100644\nBinary files a/img.png and b/img.png differ\n";
        let f = &parse_diff(d, false)[0];
        assert!(f.binary && f.hunks.is_empty());
        assert_eq!(f.path, "img.png");
        let n = "diff --git a/b.bin b/b.bin\nnew file mode 100644\nindex 0..1\nBinary files /dev/null and b/b.bin differ\n";
        let f = &parse_diff(n, false)[0];
        assert!(f.binary && f.new_file);
    }

    #[test]
    fn no_newline_markers() {
        let d = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        let f = &parse_diff(d, false)[0];
        assert_eq!(lines(&f.hunks[0]), ["-old", "\\", "+new", "\\"]);
        assert_eq!((f.added, f.removed), (1, 1));
    }

    #[test]
    fn quoted_and_spaced_paths() {
        let d = "diff --git \"a/we\\\"ird\\tname.txt\" \"b/we\\\"ird\\tname.txt\"\nindex 1..2 100644\n--- \"a/we\\\"ird\\tname.txt\"\n+++ \"b/we\\\"ird\\tname.txt\"\n@@ -1 +1 @@\n-a\n+b\n";
        let f = &parse_diff(d, false)[0];
        assert_eq!(f.path, "we\"ird\tname.txt");
        // octal escapes make UTF-8 bytes
        let o = "diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\nindex 1..2 100644\nBinary files a/x and b/x differ\n";
        assert_eq!(parse_diff(o, false)[0].path, "café.txt");
        // plain spaces: the +++ line carries a trailing tab
        let s = "diff --git a/a b.txt b/a b.txt\nindex 1..2 100644\n--- a/a b.txt\t\n+++ b/a b.txt\t\n@@ -1 +1 @@\n-a\n+b\n";
        let f = &parse_diff(s, false)[0];
        assert_eq!((f.path.as_str(), f.old_path.as_deref()), ("a b.txt", None));
        // a mode-only change on such a path has no ---/+++ lines to help
        let m = "diff --git a/a b.txt b/a b.txt\nold mode 100644\nnew mode 100755\n";
        assert_eq!(parse_diff(m, false)[0].path, "a b.txt");
    }

    #[test]
    fn mode_change_only() {
        let d = "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n";
        let f = &parse_diff(d, false)[0];
        assert_eq!(f.mode, Some(("100644".into(), "100755".into())));
        assert!(f.hunks.is_empty() && !f.is_empty());
        assert_eq!((f.added, f.removed), (0, 0));
    }

    #[test]
    fn empty_input_and_garbage() {
        assert!(parse_diff("", false).is_empty());
        assert!(parse_diff("\n\n", false).is_empty());
        assert!(parse_diff("warning: something\nfatal: nope\n", false).is_empty());
        // a header with a hunk header that does not parse is skipped, not a panic
        let f = parse_diff("diff --git a/x b/x\n@@ nonsense @@\n+a\n", false);
        assert_eq!(f.len(), 1);
        assert!(f[0].hunks.is_empty());
    }

    #[test]
    fn truncated_output_drops_the_partial_line_and_flags_it() {
        let d = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n one\n-two\n+TW";
        let f = &parse_diff(d, true)[0];
        assert!(f.truncated);
        assert_eq!(lines(&f.hunks[0]), [" one", "-two"]);
        assert_eq!((f.added, f.removed), (0, 1));
        // untruncated text keeps its last line
        assert_eq!(parse_diff(d, false)[0].hunks[0].lines.len(), 3);
        assert!(parse_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1", true)[0].hunks.is_empty());
    }

    #[test]
    fn content_that_looks_like_headers_stays_content() {
        // A removed line "-- x" prints as "--- x"; an added "++ y" as "+++ y".
        let d = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n--- x\n+++ y\n ctx\n";
        let f = &parse_diff(d, false)[0];
        assert_eq!(f.path, "f");
        assert_eq!(lines(&f.hunks[0]), ["--- x", "+++ y", " ctx"]);
        assert_eq!((f.added, f.removed), (1, 1));
    }

    #[test]
    fn two_files_and_conflict_diff() {
        let d = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-1\n+2\ndiff --git a/b b/b\n--- a/b\n+++ b/b\n@@ -1 +1 @@\n-3\n+4\n";
        let f = parse_diff(d, false);
        assert_eq!(f.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        let c = "diff --cc f.txt\nindex 1,2..3\n--- a/f.txt\n+++ b/f.txt\n@@@ -1,3 -1,3 +1,7 @@@\n  base\n++<<<<<<< ours\n +ours\n++=======\n+ theirs\n++>>>>>>> theirs\n";
        let f = &parse_diff(c, false)[0];
        assert!(f.combined);
        assert_eq!(f.path, "f.txt");
        assert_eq!(lines(&f.hunks[0]), [" base", "+<<<<<<< ours", "+ours", "+=======", "+theirs", "+>>>>>>> theirs"]);
    }

    #[test]
    fn crlf_diff_text_is_parsed() {
        let d = "diff --git a/f b/f\r\n--- a/f\r\n+++ b/f\r\n@@ -1 +1 @@\r\n-a\r\n+b\r\n";
        let f = &parse_diff(d, false)[0];
        assert_eq!(f.path, "f");
        assert_eq!((f.added, f.removed), (1, 1));
    }

    #[test]
    fn script_flags_and_quoting() {
        let s = diff_script("it's -x.txt", Side::Unstaged, false, None);
        for flag in ["--no-ext-diff", "--no-color", "--no-textconv", "GIT_OPTIONAL_LOCKS=0", "core.quotepath=off", " -- "] {
            assert!(s.contains(flag), "{flag} missing from {s}");
        }
        assert!(s.ends_with("-- ':(literal)it'\\''s -x.txt'"), "{s}");
        assert!(!s.contains("--cached"));
        let s = diff_script("n.txt", Side::Staged, false, Some("o.txt"));
        assert!(s.contains("--cached") && s.contains("-M") && s.contains("':(top,literal)o.txt'"), "{s}");
        let s = diff_script("-dash.txt", Side::Unstaged, true, None);
        assert!(s.contains("--no-index") && s.contains("-- /dev/null '-dash.txt'"), "{s}");
    }
}
