//! A small local log of what dictation does (timings and reasons, never the words you said), so a
//! behaviour such as "the automatic Enter did not fire" can be traced. Kept on this computer only,
//! cut back when it grows past 200 KB.

use std::io::Write;
use std::path::PathBuf;

const MAX: u64 = 200 * 1024;

pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WICLI_DICTATION_LOG") {
        return Some(PathBuf::from(p));
    }
    Some(dirs::cache_dir()?.join("wicli").join("dictation.log"))
}

/// Append one line: `unix-seconds.millis  text`. Failures are ignored on purpose.
pub fn log(line: &str) {
    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > MAX) {
        let _ = std::fs::remove_file(&p);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{}.{:03}  {}", now.as_secs(), now.subsec_millis(), line.replace('\n', " "));
    }
}
