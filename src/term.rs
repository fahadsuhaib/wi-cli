//! Host terminal setup and teardown.

use anyhow::Result;
use crossterm::event::EnableBracketedPaste;
use crossterm::{execute, terminal};
use std::io::stdout;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct TermGuard;
impl TermGuard {
    pub fn enter(mouse: bool) -> Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(stdout(), terminal::EnterAlternateScreen, EnableBracketedPaste)?;
        if mouse {
            use std::io::Write;
            stdout().write_all(crate::mouse::ENABLE)?;
            stdout().flush()?;
        }
        Ok(Self)
    }
}
impl Drop for TermGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

static KEYS_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Ask the host terminal for enhanced key reports: kitty keyboard flags (disambiguate) and
/// xterm modifyOtherKeys level 2. Terminals that know neither ignore the requests.
pub fn enable_key_protocol() {
    use std::io::Write;
    KEYS_REQUESTED.store(true, Ordering::SeqCst);
    let mut out = stdout();
    let _ = out.write_all(KEYS_ON);
    let _ = out.flush();
}

/// Put the host terminal back: mouse/paste modes off, cursor shown, alternate
/// screen left, raw mode off. Safe to call any number of times, from a panic
/// hook or a signal path as well as from `TermGuard::drop`.
pub fn restore_terminal() {
    use std::io::Write;
    let mut out = stdout();
    // Disable mouse reporting (1000/1002/1003/1006) and bracketed paste (2004),
    // show the cursor, then leave the alternate screen.
    if KEYS_REQUESTED.swap(false, Ordering::SeqCst) {
        // Pop what enable_key_protocol pushed while still on the alternate screen.
        let _ = out.write_all(KEYS_OFF);
    }
    let _ = out.write_all(RESTORE_SEQ);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

const KEYS_ON: &[u8] = b"\x1b[>1u\x1b[>4;2m\x1b[?u";
const KEYS_OFF: &[u8] = b"\x1b[<u\x1b[>4;0m";
const RESTORE_SEQ: &[u8] = b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[?1049l";

/// Panic hook: restore the terminal before the message is printed, exit 1.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
        std::process::exit(1);
    }));
}

/// Ask the terminal for its background colour (OSC 11) once, before the main loop. Waits at most
/// ~150 ms for the reply; terminals that do not answer simply time out. Returns 0..=1 channels.
pub fn query_background() -> Option<(f64, f64, f64)> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !stdout().is_terminal() || std::env::var("TERM").is_ok_and(|t| t == "dumb") {
        return None;
    }
    terminal::enable_raw_mode().ok()?;
    let mut out = stdout();
    let sent = out.write_all(b"\x1b]11;?\x07").and_then(|_| out.flush()).is_ok();
    let mut reply = Vec::new();
    if sent {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
            // SAFETY: one valid pollfd for stdin.
            let n = unsafe { libc::poll(&mut fds, 1, left.as_millis() as libc::c_int) };
            if n <= 0 {
                break;
            }
            let mut buf = [0u8; 256];
            // SAFETY: reading into a local buffer of the stated length.
            let got = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            if got <= 0 {
                break;
            }
            reply.extend_from_slice(&buf[..got as usize]);
            if reply.ends_with(b"\x07") || reply.ends_with(b"\x1b\\") {
                break;
            }
        }
    }
    let _ = terminal::disable_raw_mode();
    crate::theme::parse_osc11(&reply)
}

/// Size of the shell area: the terminal minus header, status bar and, with
/// two or more tabs, the tab strip.
pub fn shell_size(tab_count: usize) -> Result<(u16, u16)> {
    let (c, r) = terminal::size()?;
    let chrome = 2 + u16::from(tab_count > 1);
    Ok((c.max(1), r.saturating_sub(chrome).max(1)))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_is_idempotent() {
        restore_terminal();
        restore_terminal();
    }

    #[test]
    fn key_protocol_sequences() {
        assert_eq!(KEYS_ON, b"\x1b[>1u\x1b[>4;2m\x1b[?u");
        assert_eq!(KEYS_OFF, b"\x1b[<u\x1b[>4;0m");
    }

    #[test]
    fn restore_sequence_ends_with_leave_alt_screen() {
        assert!(RESTORE_SEQ.ends_with(b"\x1b[?1049l"));
    }

    #[test]
    fn panic_hook_restores_then_chains() {
        // The hook must run restore before the previous hook; verify the hook
        // survives a caught panic without aborting the test process.
        let prev = std::panic::take_hook();
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = called.clone();
        std::panic::set_hook(Box::new(move |_| {
            restore_terminal();
            c.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        let r = std::panic::catch_unwind(|| panic!("boom"));
        std::panic::set_hook(prev);
        assert!(r.is_err());
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    }
}
