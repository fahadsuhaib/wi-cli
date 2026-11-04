//! Data for the file explorer: detailed directory listings (size, modified time), sorting, git
//! marks from `git status`, and the small formatters the view uses. Local and remote go through
//! `Backend`; remote listings use the attributes SFTP already sends (no per-entry stat).

use crate::files::LIST_CAP;
use crate::workspace::Backend;
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// At most this many symlinks per remote listing get the extra stat that tells whether they
/// point at a directory.
const SYMLINK_STAT_CAP: usize = 200;
/// Entries past `LIST_CAP` are still counted (names only, no stat) up to this many.
const COUNT_CAP: usize = 100_000;
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    /// Symlinks to directories count as directories.
    pub is_dir: bool,
    pub is_symlink: bool,
    /// None for directories (and when the size is unknown).
    pub size: Option<u64>,
    /// Unix seconds.
    pub mtime: Option<i64>,
    pub hidden: bool,
}

impl DirEntry {
    fn new(name: String, is_dir: bool, is_symlink: bool, size: Option<u64>, mtime: Option<i64>) -> Self {
        let hidden = name.starts_with('.');
        DirEntry { name, is_dir, is_symlink, size: if is_dir { None } else { size }, mtime, hidden }
    }
}

#[derive(Debug)]
pub struct DetailedListing {
    pub entries: Vec<DirEntry>,
    pub truncated: bool,
    /// Entries seen in the folder (equals `entries.len()` unless truncated; counted up to a bound).
    pub total_seen: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    /// Modified time, newest first.
    Newest,
}

/// Directories first, then by `key`. Name ties break case-insensitively, then case-sensitively.
/// `descending` reverses the key order inside each group (directories stay first); for `Size`
/// directories are always ordered by name (ascending, or descending when `descending`).
/// Entries without the metadata sort last when ascending. Equal sizes/times fall back to name
/// ascending.
pub fn sort_entries(entries: &mut [DirEntry], key: SortKey, descending: bool) {
    use std::cmp::Ordering;
    let by_name = |a: &DirEntry, b: &DirEntry| {
        a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name))
    };
    // Missing values sort after present ones in ascending order.
    fn opt<T: Ord>(a: &Option<T>, b: &Option<T>) -> Ordering {
        match (a, b) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
    let dir_by_name_only = key == SortKey::Size;
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            let k = match key {
                SortKey::Name => Ordering::Equal,
                _ if a.is_dir && dir_by_name_only => Ordering::Equal,
                SortKey::Size => opt(&a.size, &b.size),
                SortKey::Modified => opt(&a.mtime, &b.mtime),
                SortKey::Newest => match (a.mtime, b.mtime) {
                    (Some(x), Some(y)) => y.cmp(&x),
                    _ => opt(&a.mtime, &b.mtime),
                },
            };
            let k = if descending && !matches!(key, SortKey::Name | SortKey::Newest) { k.reverse() } else { k };
            let n = by_name(a, b);
            let n = if descending && (key == SortKey::Name || (a.is_dir && dir_by_name_only)) { n.reverse() } else { n };
            k.then(n)
        })
    });
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub items: usize,
    pub dirs: usize,
    pub files: usize,
    /// Sum of the sizes of the files that are direct children.
    pub bytes: u64,
    pub hidden: usize,
}

pub fn summarize(entries: &[DirEntry]) -> Summary {
    let mut s = Summary { items: entries.len(), ..Summary::default() };
    for e in entries {
        if e.is_dir {
            s.dirs += 1;
        } else {
            s.files += 1;
            s.bytes += e.size.unwrap_or(0);
        }
        if e.hidden {
            s.hidden += 1;
        }
    }
    s
}

/// Join a folder and a child name with exactly one `/`.
pub fn child_path(dir: &str, name: &str) -> String {
    let base = dir.trim_end_matches('/');
    let name = name.trim_start_matches('/');
    if base.is_empty() && dir.starts_with('/') { format!("/{name}") } else if base.is_empty() { name.to_string() } else { format!("{base}/{name}") }
}

pub fn format_size(n: u64) -> String {
    crate::preview::human(n)
}

/// `just now`, `5m ago`, `2h ago`, `3d ago`, `4w ago`, `3mo ago`, `2y ago`.
pub fn relative_time(now: i64, mtime: i64) -> String {
    let d = now - mtime;
    if d < -60 {
        return "in the future".into();
    }
    if d < 60 {
        return "just now".into();
    }
    let (m, h, days) = (d / 60, d / 3600, d / 86400);
    if h < 1 {
        format!("{m}m ago")
    } else if days < 1 {
        format!("{h}h ago")
    } else if days < 7 {
        format!("{days}d ago")
    } else if days < 30 {
        format!("{}w ago", days / 7)
    } else if days < 365 {
        format!("{}mo ago", (days / 30).min(11))
    } else {
        format!("{}y ago", days / 365)
    }
}

// ---- git marks ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitMark {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
    /// Only produced for `!!` lines (git status run with `--ignored`); off by default.
    Ignored,
}

pub fn format_mark(m: GitMark) -> char {
    match m {
        GitMark::Modified => 'M',
        GitMark::Added => 'A',
        GitMark::Deleted => 'D',
        GitMark::Renamed => 'R',
        GitMark::Untracked => '?',
        GitMark::Conflict | GitMark::Ignored => '!',
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitMarks {
    /// Paths relative to the repository root, as git prints them.
    pub marks: HashMap<String, GitMark>,
    /// Every ancestor directory (relative, no trailing slash) of a changed path.
    pub dirty_dirs: HashSet<String>,
    pub branch: Option<String>,
    /// Absolute path of the repository root.
    pub repo_root: String,
    /// The git output hit the cap; marks past it are missing.
    pub truncated: bool,
}

fn classify(x: u8, y: u8) -> Option<GitMark> {
    Some(match (x, y) {
        (b'?', b'?') => GitMark::Untracked,
        (b'!', b'!') => GitMark::Ignored,
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => GitMark::Conflict,
        (b'R', _) | (_, b'R') | (b'C', _) | (_, b'C') => GitMark::Renamed,
        (b'D', _) | (_, b'D') => GitMark::Deleted,
        (b'A', _) | (_, b'A') => GitMark::Added,
        (b'M' | b'T', _) | (_, b'M' | b'T') => GitMark::Modified,
        _ => return None,
    })
}

/// Parse `git status --porcelain=v1 -z` output. Each entry is `XY path`, and a rename or copy
/// is followed by a field holding the original path (skipped). With `truncated`, the last
/// field may be cut short and is dropped. Returns the marks and the (relative) dirty folders.
pub fn parse_porcelain_z(data: &[u8], truncated: bool) -> (HashMap<String, GitMark>, HashSet<String>) {
    let mut fields: Vec<&[u8]> = data.split(|&b| b == 0).collect();
    // A well-formed stream ends with NUL, leaving an empty last piece. Anything else is a cut.
    // With `truncated` that piece may be a cut field; either way it is dropped.
    let _ = (fields.pop(), truncated);
    let mut marks = HashMap::new();
    let mut dirty = HashSet::new();
    let mut it = fields.into_iter();
    while let Some(f) = it.next() {
        if f.len() < 4 || f[2] != b' ' {
            continue;
        }
        let (x, y) = (f[0], f[1]);
        let path = String::from_utf8_lossy(&f[3..]).into_owned();
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            it.next(); // original path
        }
        let Some(mark) = classify(x, y) else { continue };
        if mark != GitMark::Ignored {
            let mut end = path.len();
            while let Some(i) = path[..end].rfind('/') {
                dirty.insert(path[..i].to_string());
                end = i;
            }
        }
        marks.insert(path, mark);
    }
    (marks, dirty)
}

/// Parse what `git_marks` runs: line 1 the repo root, line 2 the branch, the rest status -z.
pub fn parse_git_output(stdout: &[u8], truncated: bool) -> Option<GitMarks> {
    let nl1 = stdout.iter().position(|&b| b == b'\n')?;
    let nl2 = nl1 + 1 + stdout[nl1 + 1..].iter().position(|&b| b == b'\n')?;
    let repo_root = String::from_utf8_lossy(&stdout[..nl1]).into_owned();
    if repo_root.is_empty() {
        return None;
    }
    let branch = String::from_utf8_lossy(&stdout[nl1 + 1..nl2]).trim().to_string();
    let (marks, dirty_dirs) = parse_porcelain_z(&stdout[nl2 + 1..], truncated);
    Some(GitMarks { marks, dirty_dirs, branch: (!branch.is_empty()).then_some(branch), repo_root, truncated })
}

impl Backend {
    /// List a folder with size and modified time. Locally one `metadata` call per entry (two
    /// for symlinks); remotely the SFTP attributes, plus one stat for each of the first 200
    /// symlinks. An unreadable entry is listed with missing details, never fails the listing.
    /// Stops at `LIST_CAP` entries (`truncated`). The result is sorted by name.
    pub async fn list_dir_detailed(&self, dir: &str) -> Result<DetailedListing> {
        let p = self.absolutize(dir);
        let mut entries = Vec::new();
        let mut total = 0usize;
        match self {
            Backend::Local(_) => {
                let mut rd = tokio::fs::read_dir(&p).await?;
                while let Some(e) = rd.next_entry().await? {
                    total += 1;
                    if total > COUNT_CAP {
                        break;
                    }
                    if entries.len() >= LIST_CAP {
                        continue; // count only
                    }
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Ok(lm) = e.metadata().await else {
                        entries.push(DirEntry::new(name, false, false, None, None));
                        continue;
                    };
                    let is_link = lm.file_type().is_symlink();
                    let m = if is_link { tokio::fs::metadata(e.path()).await.ok() } else { Some(lm.clone()) };
                    let (is_dir, size, mtime) = match m.as_ref() {
                        Some(m) => (m.is_dir(), Some(m.len()), mtime_of(m)),
                        None => (false, None, mtime_of(&lm)), // broken link
                    };
                    entries.push(DirEntry::new(name, is_dir, is_link, size, mtime));
                }
            }
            Backend::Remote(s) => {
                let sftp = s.sftp().await?;
                let mut stats = 0usize;
                for e in sftp.read_dir(p.clone()).await? {
                    let name = e.file_name();
                    if name == "." || name == ".." {
                        continue;
                    }
                    total += 1;
                    if entries.len() >= LIST_CAP {
                        continue;
                    }
                    let a = e.metadata();
                    let is_link = e.file_type().is_symlink();
                    let (mut is_dir, mut size, mut mtime) = (e.file_type().is_dir(), a.size, a.mtime.map(i64::from));
                    if is_link {
                        // The attributes are the link's own: its size is the target path length.
                        size = None;
                        if stats < SYMLINK_STAT_CAP {
                            stats += 1;
                            if let Ok(t) = sftp.metadata(child_path(&p, &name)).await {
                                is_dir = t.is_dir();
                                size = t.size;
                                mtime = t.mtime.map(i64::from).or(mtime);
                            }
                        }
                    }
                    entries.push(DirEntry::new(name, is_dir, is_link, size, mtime));
                }
            }
        }
        let truncated = total > entries.len();
        sort_entries(&mut entries, SortKey::Name, false);
        Ok(DetailedListing { total_seen: total, entries, truncated })
    }

    /// Git status marks for the repository containing `dir`; None when it is not in a
    /// repository or git is missing. One shell call: repo root, branch, then
    /// `git status --porcelain=v1 -z --untracked-files=all`, output capped like other git calls.
    pub async fn git_marks(&self, dir: &str) -> Result<Option<GitMarks>> {
        let p = self.absolutize(dir);
        let script = "export GIT_OPTIONAL_LOCKS=0; top=$(git rev-parse --show-toplevel 2>/dev/null) || exit 3; \
             printf '%s\\n' \"$top\"; b=$(git symbolic-ref --short -q HEAD || git rev-parse --short HEAD 2>/dev/null); \
             printf '%s\\n' \"$b\"; git status --porcelain=v1 -z --untracked-files=all";
        let o = self.run_script(&p, script, crate::exec::OUT_CAP, GIT_TIMEOUT).await?;
        if o.timed_out && o.stdout.is_empty() {
            bail!("git status timed out");
        }
        if o.code == 3 || (o.code != 0 && !o.truncated && !o.timed_out) {
            return Ok(None);
        }
        Ok(parse_git_output(&o.stdout, o.truncated || o.timed_out))
    }
}

fn mtime_of(m: &std::fs::Metadata) -> Option<i64> {
    let t = m.modified().ok()?;
    Some(match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    })
}

// ---- hidden dev subcommand: wi __ls ----

/// `wi __ls [--box NAME] <dir> [--git] [--sort name|size|mtime]`, plain text for tests.
pub async fn ls_cmd() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let (mut box_name, mut dir, mut git, mut sort) = (None, None, false, SortKey::Name);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--box" => box_name = Some(it.next().ok_or_else(|| anyhow::anyhow!("--box needs a name"))?),
            "--git" => git = true,
            "--sort" => {
                sort = match it.next().as_deref() {
                    Some("name") => SortKey::Name,
                    Some("size") => SortKey::Size,
                    Some("mtime") => SortKey::Modified,
                    _ => bail!("--sort takes name, size or mtime"),
                }
            }
            _ if dir.is_none() => dir = Some(a),
            _ => bail!("usage: wi __ls [--box NAME] <dir> [--git] [--sort name|size|mtime]"),
        }
    }
    let Some(dir) = dir else { bail!("usage: wi __ls [--box NAME] <dir> [--git] [--sort name|size|mtime]") };
    let ws = match box_name {
        Some(n) => {
            let cfg = crate::config::Config::load()?;
            let b = match cfg.find_box(&n) {
                Some(b) => b.clone(),
                None => crate::config::Config::ssh_config_box(&n)?.ok_or_else(|| anyhow::anyhow!("unknown box '{n}'"))?,
            };
            crate::workspace::Workspace::Remote(b)
        }
        None => crate::workspace::Workspace::Local(std::env::current_dir()?),
    };
    let backend = ws.connect().await?;
    let mut l = backend.list_dir_detailed(&dir).await?;
    sort_entries(&mut l.entries, sort, false);
    let mut out = String::new();
    for e in &l.entries {
        let kind = if e.is_symlink && !e.is_dir && e.size.is_none() { 'l' } else if e.is_dir { 'd' } else { 'f' };
        let size = e.size.map_or("-".to_string(), |s| s.to_string());
        let mt = e.mtime.map_or("-".to_string(), |s| s.to_string());
        out.push_str(&format!("{kind} {size} {mt} {} {}\n", u8::from(e.hidden), e.name));
    }
    if l.truncated {
        out.push_str(&format!("truncated {}\n", l.total_seen));
    }
    if git {
        match backend.git_marks(&dir).await? {
            None => out.push_str("nogit\n"),
            Some(g) => {
                out.push_str(&format!("repo {}\n", g.repo_root));
                let mut m: Vec<_> = g.marks.iter().collect();
                m.sort_by(|a, b| a.0.cmp(b.0));
                for (p, k) in m {
                    out.push_str(&format!("git {} {p}\n", format_mark(*k)));
                }
                let mut d: Vec<_> = g.dirty_dirs.iter().collect();
                d.sort();
                for p in d {
                    out.push_str(&format!("dirty {p}\n"));
                }
                if let Some(b) = &g.branch {
                    out.push_str(&format!("branch {b}\n"));
                }
                if g.truncated {
                    out.push_str("git-truncated\n");
                }
            }
        }
    }
    print!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn de(name: &str, is_dir: bool, size: Option<u64>, mtime: Option<i64>) -> DirEntry {
        DirEntry::new(name.into(), is_dir, false, size, mtime)
    }
    fn names(v: &[DirEntry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn sort_name_dirs_first_case_insensitive_then_sensitive() {
        let mut v = vec![de("b.txt", false, None, None), de("B.txt", false, None, None), de("a", true, None, None), de("Zed", true, None, None), de("ärger", false, None, None), de("é", false, None, None)];
        sort_entries(&mut v, SortKey::Name, false);
        assert_eq!(names(&v), ["a", "Zed", "B.txt", "b.txt", "ärger", "é"]);
        sort_entries(&mut v, SortKey::Name, true);
        assert_eq!(names(&v), ["Zed", "a", "é", "ärger", "b.txt", "B.txt"]);
    }

    #[test]
    fn sort_size_with_equal_and_missing() {
        let mut v = vec![
            de("big", false, Some(900), None),
            de("small", false, Some(5), None),
            de("same_b", false, Some(5), None),
            de("none", false, None, None),
            de("dirB", true, None, None),
            de("dira", true, None, None),
        ];
        sort_entries(&mut v, SortKey::Size, false);
        assert_eq!(names(&v), ["dira", "dirB", "same_b", "small", "big", "none"]);
        sort_entries(&mut v, SortKey::Size, true);
        assert_eq!(names(&v), ["dirB", "dira", "none", "big", "same_b", "small"]);
    }

    #[test]
    fn sort_modified() {
        let mut v = vec![de("old", false, None, Some(10)), de("new", false, None, Some(99)), de("unk", false, None, None), de("d", true, None, Some(1))];
        sort_entries(&mut v, SortKey::Modified, false);
        assert_eq!(names(&v), ["d", "old", "new", "unk"]);
        sort_entries(&mut v, SortKey::Modified, true);
        assert_eq!(names(&v), ["d", "unk", "new", "old"]);
    }

    #[test]
    fn sort_newest_first_puts_unknown_last() {
        let mut v = vec![de("old", false, None, Some(10)), de("unk", false, None, None), de("new", false, None, Some(99)), de("d", true, None, Some(1))];
        sort_entries(&mut v, SortKey::Newest, false);
        assert_eq!(names(&v), ["d", "new", "old", "unk"]);
    }

    #[test]
    fn sizes_format() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(18 * 1024), "18 KB");
        assert_eq!(format_size(1536 * 1024), "1.5 MB");
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn relative_times() {
        let now = 1_000_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now - 59), "just now");
        assert_eq!(relative_time(now, now - 300), "5m ago");
        assert_eq!(relative_time(now, now - 2 * 3600), "2h ago");
        assert_eq!(relative_time(now, now - 3 * 86400), "3d ago");
        assert_eq!(relative_time(now, now - 28 * 86400), "4w ago");
        assert_eq!(relative_time(now, now - 95 * 86400), "3mo ago");
        assert_eq!(relative_time(now, now - 800 * 86400), "2y ago");
        assert_eq!(relative_time(now, now + 30), "just now");
        assert_eq!(relative_time(now, now + 3600), "in the future");
    }

    #[test]
    fn mark_chars() {
        assert_eq!(
            [GitMark::Modified, GitMark::Added, GitMark::Deleted, GitMark::Renamed, GitMark::Untracked, GitMark::Conflict].map(format_mark),
            ['M', 'A', 'D', 'R', '?', '!']
        );
    }

    #[test]
    fn summary_and_child_path() {
        let v = vec![de("a", true, None, None), de(".h", false, Some(10), None), de("b", false, Some(5), None), de("x", false, None, None)];
        assert_eq!(summarize(&v), Summary { items: 4, dirs: 1, files: 3, bytes: 15, hidden: 1 });
        assert_eq!(summarize(&[]), Summary::default());
        assert_eq!(child_path("/a/b", "c"), "/a/b/c");
        assert_eq!(child_path("/a/b/", "c"), "/a/b/c");
        assert_eq!(child_path("/", "c"), "/c");
        assert_eq!(child_path("", "c"), "c");
        assert_eq!(child_path("a//", "/c"), "a/c");
    }

    fn parse(s: &str) -> (HashMap<String, GitMark>, HashSet<String>) {
        parse_porcelain_z(s.as_bytes(), false)
    }

    #[test]
    fn porcelain_basic_states() {
        let (m, d) = parse("MM a.rs\0 M b.rs\0M  c.rs\0A  new.rs\0 D gone.rs\0D  gone2.rs\0?? src/u.txt\0");
        assert_eq!(m["a.rs"], GitMark::Modified);
        assert_eq!(m["b.rs"], GitMark::Modified);
        assert_eq!(m["c.rs"], GitMark::Modified);
        assert_eq!(m["new.rs"], GitMark::Added);
        assert_eq!(m["gone.rs"], GitMark::Deleted);
        assert_eq!(m["gone2.rs"], GitMark::Deleted);
        assert_eq!(m["src/u.txt"], GitMark::Untracked);
        assert_eq!(d, HashSet::from(["src".to_string()]));
    }

    #[test]
    fn porcelain_rename_copy_skip_original() {
        let (m, _) = parse("R  new name.rs\0old name.rs\0C  copy.rs\0orig.rs\0 M after.rs\0");
        assert_eq!(m["new name.rs"], GitMark::Renamed);
        assert_eq!(m["copy.rs"], GitMark::Renamed);
        assert_eq!(m["after.rs"], GitMark::Modified);
        assert!(!m.contains_key("old name.rs") && !m.contains_key("orig.rs"));
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn porcelain_conflicts_and_ignored() {
        let (m, _) = parse("UU a\0AA b\0DD c\0AU d\0UD e\0DU f\0UA g\0!! target/x\0");
        for k in ["a", "b", "c", "d", "e", "f", "g"] {
            assert_eq!(m[k], GitMark::Conflict, "{k}");
        }
        assert_eq!(m["target/x"], GitMark::Ignored);
        let (_, d) = parse("!! target/x\0");
        assert!(d.is_empty(), "ignored files do not dirty folders");
    }

    #[test]
    fn porcelain_odd_names_and_dirty_dirs() {
        let (m, d) = parse("?? dir with space/a\nb.txt\0 M ünï/çödé.rs\0?? a/b/c/d.txt\0");
        assert_eq!(m["dir with space/a\nb.txt"], GitMark::Untracked);
        assert_eq!(m["ünï/çödé.rs"], GitMark::Modified);
        let want: HashSet<String> = ["dir with space", "ünï", "a", "a/b", "a/b/c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(d, want);
    }

    #[test]
    fn porcelain_empty() {
        let (m, d) = parse("");
        assert!(m.is_empty() && d.is_empty());
    }

    #[test]
    fn porcelain_truncated_drops_partial_entry() {
        let (m, _) = parse_porcelain_z(b" M a.rs\0 M b.rs\0 M c", true);
        assert_eq!(m.len(), 2);
        assert!(!m.contains_key("c") && !m.contains_key("c.rs"));
        // cut right after a rename's new path: the rename still has its mark? no original yet
        let (m, _) = parse_porcelain_z(b"R  n.rs\0o.rs", true);
        assert_eq!(m.get("n.rs"), Some(&GitMark::Renamed));
    }

    #[test]
    fn git_output_framing() {
        let g = parse_git_output(b"/repo\nmain\n M a\0?? b/c\0", false).unwrap();
        assert_eq!(g.repo_root, "/repo");
        assert_eq!(g.branch.as_deref(), Some("main"));
        assert_eq!(g.marks.len(), 2);
        assert!(g.dirty_dirs.contains("b"));
        assert!(!g.truncated);
        let g = parse_git_output(b"/repo\n\n", false).unwrap();
        assert_eq!(g.branch, None);
        assert!(parse_git_output(b"", false).is_none());
        assert!(parse_git_output(b"\n\n", false).is_none());
    }
}
