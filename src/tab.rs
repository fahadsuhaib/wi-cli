//! One shell tab: its own emulator screen and shell task.

use crate::workspace::ShellHandle;

/// What the tab strip shows for a tab.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabState {
    /// Nothing new, or it is the tab you are looking at.
    Idle,
    /// A background tab that is still printing output.
    Streaming,
    /// A background tab that printed something and then stopped: finished or waiting for you.
    Quiet,
}

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
    /// Background tab with unseen output that has since stopped.
    pub quiet: bool,
    pub last_output: std::time::Instant,
}

pub fn nz(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n).unwrap_or(std::num::NonZeroU16::MIN)
}

impl Tab {
    pub fn new(id: usize, title: String, handle: ShellHandle, cols: u16, rows: u16, scrollback: usize) -> Self {
        Self {
            id,
            fallback: title.clone(),
            title,
            compat: Default::default(),
            parser: vt100::Parser::new(nz(rows), nz(cols), scrollback),
            handle,
            activity: false,
            quiet: false,
            last_output: std::time::Instant::now(),
        }
    }

    /// Feed shell output: updates the screen, answers terminal queries, tracks the title.
    pub fn output(&mut self, data: &[u8]) {
        let reply = self.compat.feed(&mut self.parser, data);
        if !reply.is_empty() {
            self.handle.write(reply);
        }
        self.title = self.compat.title.clone().unwrap_or_else(|| self.fallback.clone());
    }

    pub fn state(&self, is_active: bool) -> TabState {
        match (is_active, self.activity, self.quiet) {
            (true, _, _) | (_, false, _) => TabState::Idle,
            (false, true, false) => TabState::Streaming,
            (false, true, true) => TabState::Quiet,
        }
    }

    /// Called when new output arrives for a tab that is in the background.
    pub fn note_background_output(&mut self) {
        self.activity = true;
        self.quiet = false;
        self.last_output = std::time::Instant::now();
    }

    /// Called when the tab becomes the active one.
    pub fn mark_seen(&mut self) {
        self.activity = false;
        self.quiet = false;
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.screen_mut().set_size(nz(rows), nz(cols));
        self.handle.resize(cols, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> ShellHandle {
        crate::workspace::ShellHandle::dummy()
    }

    #[test]
    fn background_output_goes_streaming_then_quiet_then_seen() {
        let mut t = Tab::new(0, "shell 1".into(), handle(), 80, 24, 100);
        assert_eq!(t.state(false), TabState::Idle);
        t.note_background_output();
        assert_eq!(t.state(false), TabState::Streaming);
        t.quiet = true;
        assert_eq!(t.state(false), TabState::Quiet);
        assert_eq!(t.state(true), TabState::Idle, "the active tab never shows a marker");
        t.mark_seen();
        assert_eq!(t.state(false), TabState::Idle);
    }

    #[test]
    fn new_output_resets_quiet() {
        let mut t = Tab::new(0, "shell 1".into(), handle(), 80, 24, 100);
        t.note_background_output();
        t.quiet = true;
        t.note_background_output();
        assert_eq!(t.state(false), TabState::Streaming);
    }
}
