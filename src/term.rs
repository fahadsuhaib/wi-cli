//! Host terminal setup and teardown.

use anyhow::Result;
use crossterm::{execute, terminal};
use std::io::stdout;

pub struct TermGuard;
impl TermGuard {
    pub fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(stdout(), terminal::EnterAlternateScreen)?;
        Ok(Self)
    }
}
impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = execute!(stdout(), terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

/// Size of the shell area: the terminal minus header, status bar and, with
/// two or more tabs, the tab strip.
pub fn shell_size(tab_count: usize) -> Result<(u16, u16)> {
    let (c, r) = terminal::size()?;
    let chrome = 2 + u16::from(tab_count > 1);
    Ok((c.max(1), r.saturating_sub(chrome).max(1)))
}

