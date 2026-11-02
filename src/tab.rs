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
    /// None for preview tabs.
    pub handle: Option<ShellHandle>,
    /// Some for preview tabs; those have no shell and ignore the terminal emulator.
    pub preview: Option<crate::preview::Preview>,
    /// New output arrived while this tab was in the background.
    pub activity: bool,
    /// Background tab with unseen output that has since stopped.
    pub quiet: bool,
    pub last_output: std::time::Instant,
    /// Base folder of a shell tab (absolute, on the machine the shell runs on). Empty for previews.
    pub dir: String,
    /// Agent running in this tab (`claude`, `codex`), for the label.
    pub agent: Option<String>,
    /// Number in "shell N" (when the folder is not shown).
    pub number: usize,
    /// Show the folder name in the label (several tabs, or a folder other than the workspace's).
    pub show_dir: bool,
}

/// Last path component of a folder (`/` for the root, empty when unknown).
pub fn folder_name(dir: &str) -> String {
    let t = dir.trim_end_matches('/');
    if t.is_empty() {
        return if dir.is_empty() { String::new() } else { "/".into() };
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// Label text of a shell tab in the strip.
///
/// - folder shown: `web-app`; with an agent `claude · web-app`
/// - folder not shown: `shell 2`, or just `claude`
/// - a title set by the running program (OSC 0/2) is appended after the folder: `web-app · vim x.rs`;
///   with no folder shown it replaces the name, as it always has
/// - agent tabs ignore the program's title: `claude · web-app` stays as it is
///
/// The folder name therefore stays visible whenever it is being shown at all.
pub fn tab_title(number: usize, agent: Option<&str>, dir: &str, show_dir: bool, osc: Option<&str>) -> String {
    let base = folder_name(dir);
    let show = show_dir && !base.is_empty();
    let name = match (agent, show) {
        (Some(a), true) => format!("{a} · {base}"),
        (Some(a), false) => a.to_string(),
        (None, true) => base.clone(),
        (None, false) => format!("shell {number}"),
    };
    if agent.is_some() {
        return name; // the agent and folder identify the tab; the program's own title would hide them
    }
    match osc.map(str::trim).filter(|t| !t.is_empty() && *t != base && *t != name) {
        Some(t) if show => format!("{base} · {t}"),
        Some(t) => t.to_string(),
        None => name,
    }
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
            handle: Some(handle),
            preview: None,
            activity: false,
            quiet: false,
            last_output: std::time::Instant::now(),
            dir: String::new(),
            agent: None,
            number: 0,
            show_dir: false,
        }
    }

    /// Recompute the label from the folder, agent and the program's own title.
    pub fn retitle(&mut self) {
        if self.preview.is_some() {
            return;
        }
        self.fallback = tab_title(self.number, self.agent.as_deref(), &self.dir, self.show_dir, None);
        self.title = tab_title(self.number, self.agent.as_deref(), &self.dir, self.show_dir, self.compat.title.as_deref());
    }

    /// Feed shell output: updates the screen, answers terminal queries, tracks the title.
    pub fn output(&mut self, data: &[u8]) {
        let reply = self.compat.feed(&mut self.parser, data);
        if !reply.is_empty() {
            self.write(reply);
        }
        self.retitle();
    }

    /// A read-only file preview tab.
    pub fn new_preview(id: usize, preview: crate::preview::Preview) -> Self {
        let handle = None;
        Self {
            id,
            fallback: preview.title.clone(),
            title: preview.title.clone(),
            compat: Default::default(),
            parser: vt100::Parser::new(nz(1), nz(1), 0),
            handle,
            preview: Some(preview),
            activity: false,
            quiet: false,
            last_output: std::time::Instant::now(),
            dir: String::new(),
            agent: None,
            number: 0,
            show_dir: false,
        }
    }

    pub fn is_preview(&self) -> bool {
        self.preview.is_some()
    }

    /// Send input to this tab's shell. Preview tabs have none.
    pub fn write(&self, bytes: Vec<u8>) {
        if let Some(h) = &self.handle {
            h.write(bytes);
        }
    }

    pub fn close(&self) {
        if let Some(h) = &self.handle {
            h.close();
        }
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
        if let Some(h) = &self.handle {
            h.resize(cols, rows);
        }
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
    fn folder_names() {
        assert_eq!(folder_name("/home/dev/web-app"), "web-app");
        assert_eq!(folder_name("/home/dev/web-app/"), "web-app");
        assert_eq!(folder_name("/"), "/");
        assert_eq!(folder_name(""), "");
    }

    #[test]
    fn tab_labels() {
        // folder not shown: the old names
        assert_eq!(tab_title(2, None, "/w", false, None), "shell 2");
        assert_eq!(tab_title(2, Some("claude"), "/w", false, None), "claude");
        assert_eq!(tab_title(2, None, "/w", false, Some("vim")), "vim");
        // folder shown
        assert_eq!(tab_title(2, None, "/p/web-app", true, None), "web-app");
        assert_eq!(tab_title(2, Some("claude"), "/p/web-app", true, None), "claude · web-app");
        assert_eq!(tab_title(2, None, "/p/web-app", true, Some("vim x.rs")), "web-app · vim x.rs");
        assert_eq!(tab_title(2, Some("claude"), "/p/web-app", true, Some("✳ Claude Code")), "claude · web-app");
        // a title that is just the folder or the name adds nothing; blank titles are ignored
        assert_eq!(tab_title(2, None, "/p/web-app", true, Some("web-app")), "web-app");
        assert_eq!(tab_title(2, None, "/p/web-app", true, Some("  ")), "web-app");
        // unknown folder falls back to the number
        assert_eq!(tab_title(3, None, "", true, None), "shell 3");
    }

    #[test]
    fn retitle_uses_folder_and_program_title() {
        let mut t = Tab::new(0, "shell 1".into(), handle(), 80, 24, 100);
        t.dir = "/p/web-app".into();
        t.number = 1;
        t.retitle();
        assert_eq!(t.title, "shell 1");
        t.show_dir = true;
        t.retitle();
        assert_eq!(t.title, "web-app");
        t.output(b"\x1b]0;htop\x07");
        assert_eq!(t.title, "web-app · htop");
        t.agent = Some("claude".into());
        t.compat.title = None;
        t.retitle();
        assert_eq!(t.title, "claude · web-app");
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
