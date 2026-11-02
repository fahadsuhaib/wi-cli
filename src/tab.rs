//! One shell tab: its own emulator screen and shell task.

use crate::workspace::ShellHandle;

/// Lines of history kept per tab.
pub const SCROLLBACK_LINES: usize = 10_000;

pub struct Tab {
    pub id: usize,
    pub title: String,
    pub parser: vt100::Parser,
    pub handle: ShellHandle,
    /// New output arrived while this tab was in the background.
    pub activity: bool,
}

pub fn nz(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n).unwrap_or(std::num::NonZeroU16::MIN)
}

impl Tab {
    pub fn new(id: usize, title: String, handle: ShellHandle, cols: u16, rows: u16) -> Self {
        Self { id, title, parser: vt100::Parser::new(nz(rows), nz(cols), SCROLLBACK_LINES), handle, activity: false }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.screen_mut().set_size(nz(rows), nz(cols));
        self.handle.resize(cols, rows);
    }
}
