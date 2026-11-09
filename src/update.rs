//! "Update wi": check the latest release, download the archive for this platform, verify its
//! SHA256 and replace the running binary. Everything here is blocking and runs on a worker
//! thread (never the UI loop). The network is `curl` (or `wget`) run as a subprocess with a
//! timeout; output goes to files, never unbounded pipes. Nothing is contacted until the caller
//! has shown a confirmation and got a yes.
//!
//! Test hook: `WICLI_UPDATE_URL` is a base URL (`file:///dir` works with curl) that holds
//! `latest.json` (same shape as the GitHub API: `tag_name`, optional `assets[].name/size`) and the
//! assets, instead of github.com.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const REPO: &str = "fahadsuhaib/wi-cli";
const API_LATEST: &str = "https://api.github.com/repos/fahadsuhaib/wi-cli/releases/latest";
const DOWNLOAD_BASE: &str = "https://github.com/fahadsuhaib/wi-cli/releases/download";
/// Largest archive accepted.
const MAX_ARCHIVE: u64 = 100 * 1024 * 1024;
/// Largest small text file (release JSON, checksums) read.
const MAX_TEXT: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub asset: &'static str,
    /// Archive size in bytes when the release lists it.
    pub size: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Checked {
    UpToDate(String),
    Newer(Release),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    /// Replaced the running binary with this version.
    Done(String),
    /// The folder is not writable: the new binary waits at `tmp`; run this command.
    NeedsSudo { tmp: PathBuf, target: PathBuf, version: String },
}

const NEEDS_FETCHER: &str = "needs curl or wget";

/// OS and CPU; `WICLI_UPDATE_PLATFORM=<os>/<arch>` overrides them (tests).
fn platform() -> (String, String) {
    if let Ok(v) = std::env::var("WICLI_UPDATE_PLATFORM") && let Some((o, a)) = v.split_once('/') {
        return (o.to_string(), a.to_string());
    }
    (std::env::consts::OS.to_string(), std::env::consts::ARCH.to_string())
}

pub fn override_url() -> Option<String> {
    std::env::var("WICLI_UPDATE_URL").ok().filter(|s| !s.trim().is_empty()).map(|s| s.trim_end_matches('/').to_string())
}

// ---------------------------------------------------------------- pure logic

/// `v1.2.3`, `1.2.3` or `1.2.3-rc1` to numbers (a pre-release suffix is ignored).
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next().unwrap_or("0").parse().ok()?;
    let c = it.next().unwrap_or("0").parse().ok()?;
    it.next().is_none().then_some((a, b, c))
}

/// True when `latest` is a higher version than `current`. Unparseable input is never newer.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// Release asset for an OS and CPU (`std::env::consts` values), as install.sh names them.
pub fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("linux", "x86_64") => Some("wi-linux-x86_64.tar.gz"),
        ("linux", "aarch64") => Some("wi-linux-aarch64.tar.gz"),
        ("macos", _) => Some("wi-macos-universal.tar.gz"),
        _ => None,
    }
}

/// Directory the archive unpacks to (`wi-linux-x86_64.tar.gz` -> `wi-linux-x86_64`).
pub fn archive_dir(asset: &str) -> &str {
    asset.strip_suffix(".tar.gz").unwrap_or(asset)
}

/// The hash listed for `file` in a `sha256sum` style file (`<hex>  name` or `<hex> *name`).
pub fn parse_sums(text: &str, file: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let (h, n) = (it.next()?, it.next()?);
        let n = n.strip_prefix('*').unwrap_or(n);
        (n == file && h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
    })
}

/// Tag and the size of `asset` from the release JSON.
pub fn parse_release(json: &str, asset: &str) -> Option<(String, Option<u64>)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v.get("tag_name")?.as_str()?.to_string();
    let size = v.get("assets").and_then(|a| a.as_array()).and_then(|a| {
        a.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some(asset)).and_then(|x| x.get("size")).and_then(|s| s.as_u64())
    });
    Some((tag, size))
}

/// `sudo install` command for a binary that could not be written in place (paths quoted), on
/// three lines joined with backslashes so a long path is not cut off on screen.
pub fn sudo_command(tmp: &Path, target: &Path) -> String {
    let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
    format!("sudo install -m 755 \\\n  {} \\\n  {}", q(tmp), q(target))
}

// ---------------------------------------------------------------- subprocess helpers

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

/// Run to completion or kill after `limit`. Output is discarded (callers use files).
fn run_limited(cmd: &mut Command, limit: Duration, mut tick: impl FnMut()) -> Result<std::process::ExitStatus, String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let end = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Ok(st),
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        if Instant::now() >= end {
            let _ = child.kill();
            let _ = child.wait();
            return Err("timed out".into());
        }
        tick();
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Download `url` to `dest` with curl, else wget. `tick` is called while it runs.
fn download(url: &str, dest: &Path, limit: Duration, max_bytes: u64, tick: impl FnMut()) -> Result<(), String> {
    let mut cmd;
    if have("curl") {
        cmd = Command::new("curl");
        cmd.args(["-fsSL", "--connect-timeout", "15", "--max-filesize", &max_bytes.to_string(), "-o"]).arg(dest).arg(url);
    } else if have("wget") {
        cmd = Command::new("wget");
        cmd.args(["-q", "-T", "15", "-O"]).arg(dest).arg(url);
    } else {
        return Err(NEEDS_FETCHER.into());
    }
    let st = run_limited(&mut cmd, limit, tick).map_err(|e| format!("download of {url}: {e}"))?;
    if !st.success() {
        return Err(format!("could not download {url}"));
    }
    match std::fs::metadata(dest) {
        Ok(m) if m.len() <= max_bytes => Ok(()),
        Ok(_) => Err(format!("{url} is larger than {} MB", max_bytes / 1024 / 1024)),
        Err(e) => Err(e.to_string()),
    }
}

fn read_small(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.take(MAX_TEXT).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

pub(crate) fn sha256_of(path: &Path) -> Result<String, String> {
    let attempts: [(&str, &[&str]); 2] = [("sha256sum", &[]), ("shasum", &["-a", "256"])];
    for (cmd, args) in attempts {
        let out = Command::new(cmd).args(args).arg(path).stdin(Stdio::null()).stderr(Stdio::null()).output();
        if let Ok(o) = out && o.status.success() {
            let text = String::from_utf8_lossy(&o.stdout[..o.stdout.len().min(4096)]).into_owned();
            if let Some(h) = text.split_whitespace().next() && h.len() == 64 {
                return Ok(h.to_ascii_lowercase());
            }
        }
    }
    Err("needs sha256sum or shasum to verify the download".into())
}

pub(crate) fn scratch_dir() -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("wi-update-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).map_err(|e| format!("could not make a temporary folder: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// `download` without a progress callback, for the voice helper installer.
pub(crate) fn download_to(url: &str, dest: &Path, limit: Duration, max_bytes: u64) -> Result<(), String> {
    download(url, dest, limit, max_bytes, || {})
}

pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    read_small(path)
}

/// Unpack a `.tar.gz` into `out`.
pub(crate) fn unpack(archive: &Path, out: &Path) -> Result<(), String> {
    let st = run_limited(Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(out), Duration::from_secs(60), || {}).map_err(|e| format!("tar: {e}"))?;
    if st.success() { Ok(()) } else { Err("could not unpack the archive".into()) }
}

// ---------------------------------------------------------------- the two steps

/// Ask where the latest release is and compare it with `current`.
pub fn check(current: &str) -> Result<Checked, String> {
    let (os, arch) = platform();
    let asset = asset_name(&os, &arch).ok_or_else(|| format!("no wi build for {os}/{arch}: update by hand from github.com/{REPO}/releases"))?;
    let url = match override_url() {
        Some(b) => format!("{b}/latest.json"),
        None => API_LATEST.to_string(),
    };
    let dir = scratch_dir()?;
    let file = dir.join("latest.json");
    let got = download(&url, &file, Duration::from_secs(30), MAX_TEXT, || {}).and_then(|_| read_small(&file));
    let _ = std::fs::remove_dir_all(&dir);
    let got = got.map_err(|e| {
        if e == NEEDS_FETCHER {
            "update needs curl or wget, and neither was found".to_string()
        } else {
            let host = if override_url().is_some() { "the update source" } else { "github.com" };
            format!("Could not reach {host} — check your connection")
        }
    });
    let (tag, size) = parse_release(&got?, asset).ok_or("could not read the latest release information")?;
    if parse_version(&tag).is_none() {
        return Err(format!("unexpected release tag '{tag}'"));
    }
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    if is_newer(current, &tag) {
        Ok(Checked::Newer(Release { tag, version, asset, size }))
    } else {
        Ok(Checked::UpToDate(current.to_string()))
    }
}

/// Download, verify and install `rel`, replacing the running executable. `status` shows progress.
pub fn install(rel: &Release, status: &mut dyn FnMut(String)) -> Result<Installed, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this binary: {e}"))?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let base = match override_url() {
        Some(b) => b,
        None => format!("{DOWNLOAD_BASE}/{}", rel.tag),
    };
    let dir = scratch_dir()?;
    let keep = match install_in(rel, &exe, &base, &dir, status) {
        Ok(Installed::NeedsSudo { tmp, target, version }) => return Ok(Installed::NeedsSudo { tmp, target, version }),
        Ok(done) => {
            // The voice helper, when we installed one, is updated from the same release.
            if let Some(r) = crate::voice::update_helper(&base, status) {
                status(match r {
                    Ok(_) => "voice helper updated".to_string(),
                    Err(e) => format!("voice helper not updated ({e})"),
                });
            }
            Ok(done)
        }
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_dir_all(&dir);
    keep
}

fn install_in(rel: &Release, exe: &Path, base: &str, dir: &Path, status: &mut dyn FnMut(String)) -> Result<Installed, String> {
    let archive = dir.join(rel.asset);
    let total = rel.size;
    status(format!("downloading {} ...", rel.asset));
    let mut last = Instant::now();
    download(&format!("{base}/{}", rel.asset), &archive, Duration::from_secs(600), MAX_ARCHIVE, || {
        if last.elapsed() >= Duration::from_millis(500) {
            last = Instant::now();
            let have = std::fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
            status(match total {
                Some(t) if t > 0 => format!("downloading {} ... {}%", rel.asset, (have * 100 / t).min(99)),
                _ => format!("downloading {} ... {} KB", rel.asset, have / 1024),
            });
        }
    })?;
    status("verifying checksum ...".into());
    let sums = dir.join("SHA256SUMS");
    download(&format!("{base}/SHA256SUMS"), &sums, Duration::from_secs(30), MAX_TEXT, || {})?;
    let want = parse_sums(&read_small(&sums)?, rel.asset).ok_or_else(|| format!("{} is not listed in SHA256SUMS; nothing was installed", rel.asset))?;
    let got = sha256_of(&archive)?;
    if want != got {
        return Err(format!("checksum mismatch for {} (expected {want}, got {got}); nothing was installed", rel.asset));
    }
    status("unpacking ...".into());
    let out = dir.join("out");
    std::fs::create_dir(&out).map_err(|e| e.to_string())?;
    let st = run_limited(Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&out), Duration::from_secs(60), || {})
        .map_err(|e| format!("tar: {e}"))?;
    if !st.success() {
        return Err("could not unpack the archive".into());
    }
    let new = out.join(archive_dir(rel.asset)).join("wi");
    match std::fs::symlink_metadata(&new) {
        Ok(m) if m.is_file() => {}
        _ => return Err(format!("the archive does not contain {}/wi", archive_dir(rel.asset))),
    }
    set_mode_755(&new)?;
    // The new binary must run and say it is the version we expect before it replaces anything.
    let o = Command::new(&new).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| format!("the new binary does not run: {e}"))?;
    let said = String::from_utf8_lossy(&o.stdout[..o.stdout.len().min(1024)]).into_owned();
    if !o.status.success() || !said.contains(&rel.version) {
        return Err(format!("the new binary reports '{}' instead of {}; nothing was installed", said.trim(), rel.version));
    }
    status("replacing this binary ...".into());
    replace_exe(&new, exe, dir, &rel.version)
}

fn set_mode_755(p: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let _ = p;
    Ok(())
}

/// Write the new binary next to `exe` and rename it over. Never uses sudo: when the folder cannot
/// be written, the binary is left in a temporary folder and the caller shows the command.
fn replace_exe(new: &Path, exe: &Path, scratch: &Path, version: &str) -> Result<Installed, String> {
    let parent = exe.parent().ok_or("this binary has no folder")?;
    let staged = parent.join(format!(".wi-update-{}", std::process::id()));
    match std::fs::copy(new, &staged) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(30) => {
            let _ = std::fs::remove_file(&staged);
            // Keep the unpacked binary somewhere stable for the sudo command.
            let keep = scratch.join("wi");
            std::fs::copy(new, &keep).map_err(|e| e.to_string())?;
            set_mode_755(&keep)?;
            return Ok(Installed::NeedsSudo { tmp: keep, target: exe.to_path_buf(), version: version.to_string() });
        }
        Err(e) => return Err(format!("could not write {}: {e}", staged.display())),
    }
    let finish = || -> Result<(), String> {
        set_mode_755(&staged)?;
        clear_quarantine(&staged);
        std::fs::rename(&staged, exe).map_err(|e| format!("could not replace {}: {e}", exe.display()))
    };
    if let Err(e) = finish() {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    Ok(Installed::Done(version.to_string()))
}

/// macOS: drop the quarantine flag, only when it is there.
fn clear_quarantine(p: &Path) {
    if cfg!(target_os = "macos") {
        let has = Command::new("xattr").args(["-p", "com.apple.quarantine"]).arg(p).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
        if has {
            let _ = Command::new("xattr").args(["-d", "com.apple.quarantine"]).arg(p).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1.10"), Some((1, 10, 0)));
        assert_eq!(parse_version("2.0.1-rc1"), Some((2, 0, 1)));
        assert_eq!(parse_version("x.y"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(is_newer("0.2.0", "v0.3.0"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.3.0", "v0.2.9"));
        assert!(!is_newer("0.2.0", "latest"));
    }

    #[test]
    fn asset_names_per_platform() {
        assert_eq!(asset_name("linux", "x86_64"), Some("wi-linux-x86_64.tar.gz"));
        assert_eq!(asset_name("linux", "aarch64"), Some("wi-linux-aarch64.tar.gz"));
        assert_eq!(asset_name("macos", "aarch64"), Some("wi-macos-universal.tar.gz"));
        assert_eq!(asset_name("macos", "x86_64"), Some("wi-macos-universal.tar.gz"));
        assert_eq!(asset_name("windows", "x86_64"), None);
        assert_eq!(archive_dir("wi-linux-x86_64.tar.gz"), "wi-linux-x86_64");
    }

    #[test]
    fn checksum_lines_parse() {
        let h = "a".repeat(64);
        let g = "B".repeat(64);
        let text = format!("{h}  wi-linux-x86_64.tar.gz\n{g} *wi-macos-universal.tar.gz\nshort  wi-x\n");
        assert_eq!(parse_sums(&text, "wi-linux-x86_64.tar.gz"), Some(h));
        assert_eq!(parse_sums(&text, "wi-macos-universal.tar.gz"), Some("b".repeat(64)));
        assert_eq!(parse_sums(&text, "wi-x"), None);
        assert_eq!(parse_sums(&text, "missing"), None);
    }

    #[test]
    fn release_json_gives_tag_and_size() {
        let j = r#"{"tag_name":"v0.3.0","assets":[{"name":"SHA256SUMS","size":10},{"name":"wi-linux-x86_64.tar.gz","size":4200000}]}"#;
        assert_eq!(parse_release(j, "wi-linux-x86_64.tar.gz"), Some(("v0.3.0".into(), Some(4_200_000))));
        assert_eq!(parse_release(j, "wi-other.tar.gz"), Some(("v0.3.0".into(), None)));
        assert_eq!(parse_release("{}", "x"), None);
        assert_eq!(parse_release("not json", "x"), None);
    }

    #[test]
    fn sudo_command_quotes_paths() {
        let c = sudo_command(Path::new("/tmp/a b/wi"), Path::new("/usr/local/bin/wi"));
        assert_eq!(c, "sudo install -m 755 \\\n  '/tmp/a b/wi' \\\n  '/usr/local/bin/wi'");
        assert!(sudo_command(Path::new("/tmp/it's/wi"), Path::new("/x/wi")).contains("'\\''"));
    }
}
