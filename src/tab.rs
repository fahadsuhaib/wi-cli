//! One shell tab: its own emulator screen and shell task.

use crate::workspace::ShellHandle;

pub struct Tab {
    pub id: usize,
    pub title: String,
    /// Title used when the program has not set one.
    pub fallback: String,
    pub compat: crate::compat::Compat,
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
        Self { id, fallback: title.clone(), title, compat: Default::default(), parser: vt100::Parser::new(nz(rows), nz(cols), 0), handle, activity: false }
    }

    /// Feed shell output: updates the screen, answers terminal queries, tracks the title.
    pub fn output(&mut self, data: &[u8]) {
        let reply = self.compat.feed(&mut self.parser, data);
        if !reply.is_empty() {
            self.handle.write(reply);
        }
        self.title = self.compat.title.clone().unwrap_or_else(|| self.fallback.clone());
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.screen_mut().set_size(nz(rows), nz(cols));
        self.handle.resize(cols, rows);
    }
}
