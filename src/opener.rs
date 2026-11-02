//! Opening files the way VS Code does: simple files preview inside wi, "complex" ones (images,
//! PDF, Office, archives, media, binaries) open in the native app. Remote files are fetched over
//! SFTP into a bounded local cache first. Engine only; the UI is wired up separately.

use crate::workspace::Backend;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ---------------------------------------------------------------- classification

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // used by the click/hint integration, not wired up yet
pub enum Handling {
    Preview,
    Native,
}

/// Optional overrides from the future `[open]` config table (extensions without the dot).
#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // read from config once the integration pass lands
pub struct OpenCfg {
    pub native_ext: Vec<String>,
    pub preview_ext: Vec<String>,
}

const NATIVE_EXT: &[&str] = &[
    // images
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "ico", "svg",
    // documents
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "pages", "numbers", "key",
    // archives
    "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar",
    // audio / video
    "mp3", "wav", "flac", "m4a", "mp4", "mov", "mkv", "avi", "webm",
    // html is delegated to the browser
    "html", "htm",
    // fonts
    "ttf", "otf", "woff", "woff2",
    // databases and disk images
    "sqlite", "sqlite3", "db", "dmg", "iso", "img",
];

/// Extensions that are known text: a huge file of these still previews (reads are bounded).
const TEXT_EXT: &[&str] = &[
    "txt", "log", "md", "markdown", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml", "ini", "cfg", "conf", "xml", "rs", "py", "js",
    "ts", "tsx", "jsx", "c", "h", "cpp", "hpp", "go", "java", "rb", "sh", "sql", "css",
];

const BIG_FILE: u64 = 50 * 1024 * 1024;
const SNIFF: usize = 4096;

fn ext_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_lowercase(),
        _ => String::new(),
    }
}

fn listed(list: &[String], ext: &str) -> bool {
    !ext.is_empty() && list.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(ext))
}

/// Decide who shows the file. `head` is the start of the file (only the first 4 KB is looked at).
/// Content wins over extension for binaries: a NUL byte always means Native. Built-in native
/// extensions stay Native even when the content is text (svg, html). `preview_ext` beats the
/// built-in native list, `native_ext` always forces Native.
#[allow(dead_code)] // used by the click/hint integration, not wired up yet
pub fn classify(path: &str, head: &[u8], size: u64, cfg: &OpenCfg) -> Handling {
    let ext = ext_of(path);
    if head[..head.len().min(SNIFF)].contains(&0) {
        return Handling::Native;
    }
    if listed(&cfg.native_ext, &ext) {
        return Handling::Native;
    }
    if listed(&cfg.preview_ext, &ext) {
        return Handling::Preview;
    }
    if NATIVE_EXT.contains(&ext.as_str()) {
        return Handling::Native;
    }
    if size > BIG_FILE && !TEXT_EXT.contains(&ext.as_str()) {
        return Handling::Native;
    }
    Handling::Preview
}

// ---------------------------------------------------------------- native open

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|c| c.join(p)).unwrap_or_else(|_| p.to_path_buf()) }
}

/// Try each launcher in turn with `arg` as a single argv entry (never through a shell).
fn launch(arg: &std::ffi::OsStr) -> Result<(), String> {
    let tools: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else {
        &[("xdg-open", &[]), ("gio", &["open"]), ("sensible-browser", &[])]
    };
    for (tool, pre) in tools {
        match std::process::Command::new(tool)
            .args(*pre)
            .arg(arg)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // Detached: reap in a thread so no zombie is left, never block the caller.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("could not run {tool}: {e}")),
        }
    }
    let names: Vec<&str> = tools.iter().map(|t| t.0).collect();
    Err(format!("no tool to open files found (tried {})", names.join(", ")))
}

/// Open a local file in the system's default app.
#[allow(dead_code)] // used by the click/hint integration, not wired up yet
pub fn native_open(local_path: &Path) -> Result<(), String> {
    let p = absolute(local_path); // absolute, so a name starting with '-' is never an option
    launch(p.as_os_str())
}

/// True for strings `open_url` accepts: http, https, file or mailto only.
pub fn url_allowed(url: &str) -> bool {
    if url.starts_with('-') || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    ["http://", "https://", "file://", "mailto:"].iter().any(|s| lower.starts_with(s) && lower.len() > s.len())
}

/// Open a URL in the default browser / mail client.
#[allow(dead_code)] // used by the click/hint integration, not wired up yet
pub fn open_url(url: &str) -> Result<(), String> {
    if !url_allowed(url) {
        return Err(format!("refusing to open '{url}': only http, https, file and mailto links are allowed"));
    }
    launch(std::ffi::OsStr::new(url))
}

// ---------------------------------------------------------------- remote fetch cache

#[allow(dead_code)] // used by the click/hint integration, not wired up yet
pub struct FetchOpts {
    pub confirm_over: u64,
    pub confirmed: bool,
    pub cache_dir: PathBuf,
    pub box_name: String,
    pub max_cache_bytes: u64,
}

impl FetchOpts {
    #[allow(dead_code)] // used by the click/hint integration, not wired up yet
    pub fn new(box_name: &str) -> Self {
        FetchOpts {
            confirm_over: 100 * 1024 * 1024,
            confirmed: false,
            cache_dir: default_cache_dir(),
            box_name: box_name.to_string(),
            max_cache_bytes: 1024 * 1024 * 1024,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)] // used by the click/hint integration, not wired up yet
pub enum FetchResult {
    Ready(PathBuf),
    NeedsConfirm { size: u64 },
}

/// `$WICLI_CACHE_DIR` if set (tests), else `<user cache dir>/wicli`.
#[allow(dead_code)] // used by the click/hint integration, not wired up yet
pub fn default_cache_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("WICLI_CACHE_DIR") {
        return PathBuf::from(d);
    }
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".cache")).join("wicli")
}

fn safe_component(c: &str) -> String {
    c.chars().map(|ch| if ch == '\\' || ch == ':' || ch.is_control() { '_' } else { ch }).collect()
}

/// Map a remote path to `<cache_dir>/<box>/<path>`. `.` is dropped and `..` only removes an
/// earlier component of the same path, so the result never leaves the box directory.
pub fn cache_path(cache_dir: &Path, box_name: &str, remote_path: &str) -> PathBuf {
    let mut parts: Vec<String> = Vec::new();
    for c in remote_path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(safe_component(c)),
        }
    }
    let boxdir = match safe_component(box_name.replace('/', "_").as_str()).as_str() {
        "" | "." | ".." => "_".to_string(),
        b => b.to_string(),
    };
    let mut p = cache_dir.join(boxdir);
    if parts.is_empty() {
        parts.push("_".into());
    }
    for c in parts {
        p.push(c);
    }
    p
}

fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk(&e.path(), out);
        } else if md.is_file() {
            out.push((e.path(), md.len(), md.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        }
    }
}

/// Delete oldest files (by mtime) until `total + incoming <= max`. Returns bytes removed.
pub fn evict(cache_dir: &Path, incoming: u64, max: u64, keep: &Path) -> u64 {
    let mut files = Vec::new();
    walk(cache_dir, &mut files);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.2);
    let mut freed = 0;
    for (p, len, _) in files {
        if total + incoming <= max {
            break;
        }
        if p == keep {
            continue;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
            freed += len;
        }
    }
    freed
}

fn mtime_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Backend {
    /// Make `remote_path` available as a local file. Local backends just resolve the path.
    /// Remote ones stream it over SFTP into the cache (see module docs); `progress(done, total)`
    /// is called at most 10 times a second.
    #[allow(dead_code)] // used by the click/hint integration, not wired up yet
    pub async fn fetch_to_cache(
        &self,
        remote_path: &str,
        opts: &FetchOpts,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<FetchResult, String> {
        let session = match self {
            Backend::Local(_) => {
                let p = self.resolve(remote_path).await.map_err(|e| format!("{remote_path}: {e}"))?;
                return Ok(FetchResult::Ready(PathBuf::from(p)));
            }
            Backend::Remote(s) => s,
        };
        let sftp = session.sftp().await.map_err(|e| format!("sftp: {e}"))?;
        let abs = sftp.canonicalize(remote_path.to_string()).await.map_err(|e| format!("{remote_path}: {e}"))?;
        let meta = sftp.metadata(abs.clone()).await.map_err(|e| format!("{abs}: {e}"))?;
        if meta.is_dir() {
            return Err(format!("{abs} is a directory"));
        }
        let size = meta.size.unwrap_or(0);
        let rmtime = meta.mtime.map(u64::from);

        let dest = cache_path(&opts.cache_dir, &opts.box_name, &abs);
        // Reuse a previous download of the same size and mtime.
        if let (Ok(lm), Some(rm)) = (std::fs::metadata(&dest), rmtime)
            && lm.is_file()
            && lm.len() == size
            && lm.modified().map(mtime_secs).ok() == Some(rm)
        {
            return Ok(FetchResult::Ready(dest));
        }
        if size > opts.confirm_over && !opts.confirmed {
            return Ok(FetchResult::NeedsConfirm { size });
        }
        let dir = dest.parent().ok_or("bad cache path")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        evict(&opts.cache_dir, size, opts.max_cache_bytes, &dest);

        let mut part_name = dest.file_name().unwrap_or_default().to_os_string();
        part_name.push(".part");
        let part = dest.with_file_name(part_name);
        let result = Self::download(sftp, &abs, &part, size, progress, cancel).await;
        if let Err(e) = result {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        if let Some(rm) = rmtime
            && let Ok(f) = std::fs::File::options().write(true).open(&part)
        {
            let _ = f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(rm));
        }
        std::fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        Ok(FetchResult::Ready(dest))
    }

    async fn download(
        sftp: &russh_sftp::client::SftpSession,
        abs: &str,
        part: &Path,
        size: u64,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let mut src = sftp.open(abs.to_string()).await.map_err(|e| format!("{abs}: {e}"))?;
        let mut out = tokio::fs::File::create(part).await.map_err(|e| format!("{}: {e}", part.display()))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        let mut last = Instant::now() - Duration::from_secs(1);
        progress(0, size);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let n = src.read(&mut buf).await.map_err(|e| format!("{abs}: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).await.map_err(|e| format!("{}: {e}", part.display()))?;
            done += n as u64;
            if last.elapsed() >= Duration::from_millis(100) {
                last = Instant::now();
                progress(done, size);
            }
        }
        out.flush().await.map_err(|e| e.to_string())?;
        progress(done, size);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cl(p: &str, head: &[u8], size: u64) -> Handling {
        classify(p, head, size, &OpenCfg::default())
    }

    #[test]
    fn text_previews() {
        for p in ["a.txt", "x.log", "src/main.rs", "README.md", "d.csv", "d.tsv", "c.json", "c.yaml", "c.toml", "Makefile", "weird.zzz", ".gitignore"] {
            assert_eq!(cl(p, b"hello\nworld\n", 100), Handling::Preview, "{p}");
        }
    }

    #[test]
    fn native_extensions() {
        for e in NATIVE_EXT {
            assert_eq!(cl(&format!("/x/y.{e}"), b"text only", 10), Handling::Native, "{e}");
        }
        assert_eq!(cl("A.PNG", b"x", 1), Handling::Native);
        assert_eq!(cl("report.DOCX", b"x", 1), Handling::Native);
    }

    #[test]
    fn content_wins() {
        assert_eq!(cl("notes.txt", b"ab\0cd", 5), Handling::Native);
        assert_eq!(cl("blob", b"\x7fELF\0\0", 5), Handling::Native);
        assert_eq!(cl("data.bin", &[0u8; 10], 10), Handling::Native);
        // NUL after the first 4 KB is not seen
        let mut h = vec![b'a'; 5000];
        h[4500] = 0;
        assert_eq!(cl("a.txt", &h, 5000), Handling::Preview);
    }

    #[test]
    fn overrides() {
        let cfg = OpenCfg { native_ext: vec!["log".into(), ".MD".into()], preview_ext: vec!["svg".into(), "log".into()] };
        assert_eq!(classify("a.log", b"x", 1, &cfg), Handling::Native, "native_ext beats preview behaviour");
        assert_eq!(classify("a.md", b"x", 1, &cfg), Handling::Native);
        assert_eq!(classify("a.svg", b"<svg/>", 1, &cfg), Handling::Preview, "preview_ext beats built-in native");
        assert_eq!(classify("a.svg", b"\0", 1, &cfg), Handling::Native, "binary content still native");
        assert_eq!(classify("a.png", b"x", 1, &cfg), Handling::Native);
    }

    #[test]
    fn size_rule() {
        let big = 60 * 1024 * 1024;
        assert_eq!(cl("huge.dat", b"text", big), Handling::Native);
        assert_eq!(cl("huge.log", b"text", big), Handling::Preview);
        assert_eq!(cl("huge.csv", b"text", big), Handling::Preview);
        assert_eq!(cl("ok.dat", b"text", 50 * 1024 * 1024), Handling::Preview);
    }

    #[test]
    fn sanitiser() {
        let c = Path::new("/c");
        assert_eq!(cache_path(c, "box", "/a/b.txt"), Path::new("/c/box/a/b.txt"));
        assert_eq!(cache_path(c, "box", "../../etc/passwd"), Path::new("/c/box/etc/passwd"));
        assert_eq!(cache_path(c, "box", "/a/../../../etc/passwd"), Path::new("/c/box/etc/passwd"));
        assert_eq!(cache_path(c, "box", "a//./b"), Path::new("/c/box/a/b"));
        assert_eq!(cache_path(c, "../evil", "/x"), Path::new("/c/.._evil/x"));
        assert_eq!(cache_path(c, "..", "/x"), Path::new("/c/_/x"));
        assert_eq!(cache_path(c, "a/b", "/x"), Path::new("/c/a_b/x"));
        assert_eq!(cache_path(c, "box", "/"), Path::new("/c/box/_"));
        assert_eq!(cache_path(c, "box", "/dä/文件 ✓.txt"), Path::new("/c/box/dä/文件 ✓.txt"));
        assert_eq!(cache_path(c, "box", "C:\\x\\y"), Path::new("/c/box/C__x_y"));
        for p in ["../../..", "/..", "..", "a/../..", "\0/..\0"] {
            assert!(cache_path(c, "box", p).starts_with("/c/box"), "{p}");
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wicli-opener-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn eviction_oldest_first() {
        let d = tmp("evict");
        let mut names = vec![];
        for i in 0..4 {
            let p = d.join("box").join(format!("f{i}"));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, vec![0u8; 100]).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1000 + i * 10)).unwrap();
            names.push(p);
        }
        // total 400, incoming 150, max 500: must free 50 -> one file (the oldest)
        let freed = evict(&d, 150, 500, Path::new("/nonexistent"));
        assert_eq!(freed, 100);
        assert!(!names[0].exists() && names[1].exists() && names[3].exists());
        // fits already: nothing removed
        assert_eq!(evict(&d, 0, 10_000, Path::new("/x")), 0);
        // incoming alone over the max: everything goes except `keep`
        evict(&d, 1000, 500, &names[3]);
        assert!(names[3].exists() && !names[1].exists() && !names[2].exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn url_filter() {
        for u in ["http://a.b", "https://a.b/x?y=1", "HTTPS://A.B", "file:///tmp/x", "mailto:a@b.c"] {
            assert!(url_allowed(u), "{u}");
        }
        for u in ["", "-h", "--version", "javascript:alert(1)", "ftp://x", "ssh://x", "/etc/passwd", "http://", "https://a b", "http://a\nb", "data:text/html,x", "-https://a", " https://a"] {
            assert!(!url_allowed(u), "{u}");
            assert!(open_url(u).is_err(), "{u}");
        }
    }

    #[tokio::test]
    async fn local_fetch_returns_resolved_path() {
        let d = tmp("local");
        let f = d.join("a.txt");
        std::fs::write(&f, "hi").unwrap();
        let b = Backend::Local(d.clone());
        let opts = FetchOpts::new("local");
        let mut calls = 0;
        let r = b.fetch_to_cache("a.txt", &opts, &mut |_, _| calls += 1, &AtomicBool::new(false)).await.unwrap();
        assert_eq!(r, FetchResult::Ready(std::fs::canonicalize(&f).unwrap()));
        assert_eq!(calls, 0);
        assert!(b.fetch_to_cache("missing", &opts, &mut |_, _| {}, &AtomicBool::new(false)).await.is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
