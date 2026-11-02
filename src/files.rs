//! File access shared by local and remote workspaces. Every read is bounded:
//! previews read a capped prefix and directory listings are capped.

use crate::workspace::Backend;
use anyhow::{Result, bail};
use tokio::io::AsyncReadExt;

/// Directory listings stop here and say so.
pub const LIST_CAP: usize = 5000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

pub struct Listing {
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

fn home() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_default()
}

impl Backend {
    /// Turn what the user typed into a path the backend understands. Relative paths start at
    /// the workspace root (local) or the login directory (remote); `~/` means home.
    fn absolutize(&self, path: &str) -> String {
        match self {
            Backend::Local(root) => {
                let p = if path == "~" {
                    home()
                } else if let Some(rest) = path.strip_prefix("~/") {
                    home().join(rest)
                } else if path.starts_with('/') {
                    std::path::PathBuf::from(path)
                } else if path.is_empty() {
                    root.clone()
                } else {
                    root.join(path)
                };
                p.to_string_lossy().into_owned()
            }
            Backend::Remote(_) => {
                let p = path.strip_prefix("~/").unwrap_or(path);
                if p == "~" || p.is_empty() { ".".to_string() } else { p.to_string() }
            }
        }
    }

    /// Absolute path with symlinks resolved; fails if it does not exist.
    pub async fn resolve(&self, path: &str) -> Result<String> {
        let p = self.absolutize(path);
        match self {
            Backend::Local(_) => Ok(tokio::fs::canonicalize(&p).await?.to_string_lossy().into_owned()),
            Backend::Remote(s) => Ok(s.sftp().await?.canonicalize(p).await?),
        }
    }

    /// Read at most `max` bytes from the start of a file. Returns the bytes and the file's full size.
    pub async fn read_prefix(&self, path: &str, max: usize) -> Result<(Vec<u8>, u64)> {
        let p = self.absolutize(path);
        match self {
            Backend::Local(_) => {
                let meta = tokio::fs::metadata(&p).await?;
                if meta.is_dir() {
                    bail!("{path} is a directory");
                }
                let f = tokio::fs::File::open(&p).await?;
                let mut buf = Vec::with_capacity(max.min(meta.len() as usize));
                f.take(max as u64).read_to_end(&mut buf).await?;
                Ok((buf, meta.len()))
            }
            Backend::Remote(s) => {
                let sftp = s.sftp().await?;
                let meta = sftp.metadata(p.clone()).await?;
                if meta.is_dir() {
                    bail!("{path} is a directory");
                }
                let size = meta.size.unwrap_or(0);
                let f = sftp.open(p).await?;
                let mut buf = Vec::with_capacity(max.min(size as usize));
                f.take(max as u64).read_to_end(&mut buf).await?;
                Ok((buf, size))
            }
        }
    }

    /// List a directory, sorted (directories first), stopping at `LIST_CAP` entries.
    pub async fn list_dir(&self, dir: &str) -> Result<Listing> {
        let p = self.absolutize(dir);
        let mut entries = Vec::new();
        let mut truncated = false;
        match self {
            Backend::Local(_) => {
                let mut rd = tokio::fs::read_dir(&p).await?;
                while let Some(e) = rd.next_entry().await? {
                    if entries.len() >= LIST_CAP {
                        truncated = true;
                        break;
                    }
                    let mut is_dir = e.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                    if !is_dir && e.file_type().await.map(|t| t.is_symlink()).unwrap_or(false) {
                        is_dir = tokio::fs::metadata(e.path()).await.map(|m| m.is_dir()).unwrap_or(false);
                    }
                    entries.push(Entry { name: e.file_name().to_string_lossy().into_owned(), is_dir });
                }
            }
            Backend::Remote(s) => {
                for e in s.sftp().await?.read_dir(p).await? {
                    let name = e.file_name();
                    if name == "." || name == ".." {
                        continue;
                    }
                    if entries.len() >= LIST_CAP {
                        truncated = true;
                        break;
                    }
                    entries.push(Entry { name, is_dir: e.file_type().is_dir() });
                }
            }
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(Listing { entries, truncated })
    }
}

/// Result of a Tab press in the path prompt.
#[derive(Debug, PartialEq, Eq)]
pub struct Completion {
    /// The input after completing as far as is unambiguous.
    pub input: String,
    /// Matching names to show (directories end in `/`), at most `MAX_SHOWN`.
    pub shown: Vec<String>,
    pub total: usize,
}

pub const MAX_SHOWN: usize = 8;

/// Split typed input into (directory part including the trailing slash, name prefix).
pub fn split_input(input: &str) -> (&str, &str) {
    match input.rfind('/') {
        Some(i) => (&input[..=i], &input[i + 1..]),
        None => ("", input),
    }
}

/// Pure completion step over a directory listing.
pub fn complete(input: &str, entries: &[Entry]) -> Completion {
    let (dir, prefix) = split_input(input);
    let show_hidden = prefix.starts_with('.');
    let matches: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.name.starts_with(prefix) && (show_hidden || !e.name.starts_with('.')))
        .collect();
    let names: Vec<String> = matches.iter().map(|e| if e.is_dir { format!("{}/", e.name) } else { e.name.clone() }).collect();
    let mut common = names.first().cloned().unwrap_or_else(|| prefix.to_string());
    for n in names.iter().skip(1) {
        let keep = common.chars().zip(n.chars()).take_while(|(a, b)| a == b).count();
        common = common.chars().take(keep).collect();
    }
    let new_input = if names.is_empty() { input.to_string() } else { format!("{dir}{common}") };
    Completion { input: new_input, total: names.len(), shown: names.into_iter().take(MAX_SHOWN).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, is_dir: bool) -> Entry {
        Entry { name: name.into(), is_dir }
    }

    #[test]
    fn splits_input() {
        assert_eq!(split_input("src/ap"), ("src/", "ap"));
        assert_eq!(split_input("README"), ("", "README"));
        assert_eq!(split_input("a/b/"), ("a/b/", ""));
    }

    #[test]
    fn completes_unique_and_common_prefix() {
        let l = [e("src", true), e("scripts", true), e("Cargo.toml", false), e(".git", true)];
        assert_eq!(complete("Ca", &l).input, "Cargo.toml");
        assert_eq!(complete("s", &l).input, "s");
        assert_eq!(complete("s", &l).total, 2);
        assert_eq!(complete("sr", &l).input, "src/");
        assert!(complete("", &l).shown.iter().all(|n| !n.starts_with('.')), "hidden files stay hidden");
        assert_eq!(complete(".g", &l).input, ".git/");
        assert_eq!(complete("zzz", &l).input, "zzz");
    }

    #[test]
    fn completes_inside_directories() {
        let l = [e("main.rs", false), e("mod.rs", false)];
        assert_eq!(complete("src/m", &l).input, "src/m");
        assert_eq!(complete("src/ma", &l).input, "src/main.rs");
    }
}
