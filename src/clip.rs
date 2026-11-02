//! Copying text to the user's clipboard. wi always runs on the user's own machine (even for
//! remote boxes), so the local clipboard tool is tried first; the terminal's OSC 52 escape is
//! the fallback and covers wi itself running over SSH.

use std::io::Write;
use std::process::{Command, Stdio};

/// Largest text we will copy.
pub const MAX_BYTES: usize = 1024 * 1024;

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The escape sequence that asks the terminal to put `text` on the clipboard.
pub fn osc52(text: &str) -> Vec<u8> {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes())).into_bytes()
}

/// Local clipboard programs to try, best first.
fn tools() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        vec![("pbcopy", &[])]
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        vec![("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])]
    } else {
        vec![("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"]), ("wl-copy", &[])]
    }
}

fn pipe_to(program: &str, args: &[&str], text: &str) -> bool {
    let Ok(mut child) = Command::new(program).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return false;
    };
    let ok = child.stdin.take().is_some_and(|mut s| s.write_all(text.as_bytes()).is_ok());
    // Do not block on clipboard programs that stay alive to serve the selection.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    ok
}

/// Copy `text`; returns how it was done (for the status bar), or an error message.
/// `WICLI_CLIPBOARD=file:<path>` writes to a file instead (used by the tests, so they never
/// touch the real clipboard).
pub fn copy(text: &str) -> Result<&'static str, String> {
    if text.len() > MAX_BYTES {
        return Err(format!("selection is larger than {} KB", MAX_BYTES / 1024));
    }
    if let Some(path) = std::env::var("WICLI_CLIPBOARD").ok().and_then(|v| v.strip_prefix("file:").map(str::to_string)) {
        return std::fs::write(path, text).map(|_| "file").map_err(|e| e.to_string());
    }
    // wi running inside an SSH session: only the terminal can reach the user's clipboard.
    let over_ssh = std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some();
    if !over_ssh {
        for (prog, args) in tools() {
            if pipe_to(prog, args, text) {
                return Ok("clipboard");
            }
        }
    }
    let mut out = std::io::stdout();
    out.write_all(&osc52(text)).and_then(|_| out.flush()).map_err(|e| e.to_string())?;
    Ok("terminal clipboard")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("héllo".as_bytes()), "aMOpbGxv");
    }

    #[test]
    fn osc52_wraps_the_payload() {
        assert_eq!(osc52("foo"), b"\x1b]52;c;Zm9v\x07".to_vec());
    }

    #[test]
    fn oversized_text_is_refused() {
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(copy(&big).is_err());
    }
}
