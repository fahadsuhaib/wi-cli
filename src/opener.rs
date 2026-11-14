//! Opening files the way VS Code does: simple files preview inside wi, "complex" ones (images,
//! PDF, Office, archives, media, binaries) open in the native app. Engine only; the UI is wired
//! up separately.

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- classification

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handling {
    Preview,
    Native,
}

/// Optional overrides from the future `[open]` config table (extensions without the dot).
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default)]
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

/// Types that may be handed to the operating system's default app: documents, images, audio,
/// video, web pages, fonts and archives.
const SAFE_NATIVE: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "ico", "svg", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt",
    "ods", "odp", "pages", "numbers", "key", "rtf", "mp3", "wav", "flac", "m4a", "aac", "ogg", "mp4", "mov", "mkv", "avi", "webm", "m4v", "html",
    "htm", "ttf", "otf", "woff", "woff2", "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar",
];

/// Programs, installers, shortcuts, libraries, disk images and databases. These are never opened
/// natively, whatever the config says: handing them to `open` could run code or mount things.
const NEVER_NATIVE: &[&str] = &[
    "app", "command", "pkg", "mpkg", "dmg", "iso", "img", "exe", "msi", "bat", "cmd", "com", "scr", "vbs", "vbe", "jar", "appimage", "deb", "rpm",
    "run", "bin", "webloc", "inetloc", "url", "terminal", "workflow", "action", "scpt", "scptd", "so", "dylib", "o", "a", "lnk", "ps1", "desktop",
    "sqlite", "sqlite3", "db", "prefpane", "saver", "kext", "xpc", "plugin", "bundle",
];

/// Whether wi may ask the OS to open this file. Err carries the reason shown to the user.
pub fn native_safe(path: &str) -> Result<(), String> {
    let ext = ext_of(path);
    let name = path.rsplit('/').next().unwrap_or(path);
    if NEVER_NATIVE.contains(&ext.as_str()) {
        return Err(format!("{name}: programs, installers, disk images and databases are never opened by wi"));
    }
    if ext.is_empty() {
        return Err(format!("{name}: unknown file type, not opened"));
    }
    if SAFE_NATIVE.contains(&ext.as_str()) {
        Ok(())
    } else {
        Err(format!("{name}: .{ext} is not a document or media type wi opens natively"))
    }
}

/// Open a local file in the system's default app.
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
pub fn open_url(url: &str) -> Result<(), String> {
    if !url_allowed(url) {
        return Err(format!("refusing to open '{url}': only http, https, file and mailto links are allowed"));
    }
    launch(std::ffi::OsStr::new(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_allowlist_and_blocklist() {
        for ok in ["/a/report.pdf", "/a/Photo.PNG", "x.docx", "/a/b/clip.mp4", "page.html", "a.zip"] {
            assert!(native_safe(ok).is_ok(), "{ok}");
        }
        for bad in ["/Applications/Tool.app", "setup.pkg", "x.dmg", "run.command", "a.exe", "x.jar", "link.webloc", "data.sqlite", "noext", "lib.dylib", "x.unknown"] {
            assert!(native_safe(bad).is_err(), "{bad}");
        }
        assert!(native_safe("/a/b/evil.pdf.app").is_err(), "the last extension decides");
        assert!(native_safe("/a/.hidden").is_err());
    }

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
    fn url_filter() {
        for u in ["http://a.b", "https://a.b/x?y=1", "HTTPS://A.B", "file:///tmp/x", "mailto:a@b.c"] {
            assert!(url_allowed(u), "{u}");
        }
        for u in ["", "-h", "--version", "javascript:alert(1)", "ftp://x", "ssh://x", "/etc/passwd", "http://", "https://a b", "http://a\nb", "data:text/html,x", "-https://a", " https://a"] {
            assert!(!url_allowed(u), "{u}");
            assert!(open_url(u).is_err(), "{u}");
        }
    }
}
