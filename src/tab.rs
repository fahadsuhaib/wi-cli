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

/// Which side of a shell tab the preview pane goes on (`Tab` = no pane, a separate tab).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Right,
    Left,
    Tab,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Right => "right",
            Side::Left => "left",
            Side::Tab => "tab",
        }
    }
    /// Parse the `preview_side` setting; anything unknown means the default, right.
    pub fn parse(s: &str) -> Side {
        match s.trim().to_ascii_lowercase().as_str() {
            "left" => Side::Left,
            "tab" => Side::Tab,
            _ => Side::Right,
        }
    }
}

/// Which pane of a tab has the keyboard.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Pane {
    #[default]
    Shell,
    Side,
}

pub const SHELL_MIN: u16 = 60;
pub const PREVIEW_MIN: u16 = 40;
pub const PCT_MIN: u16 = 25;
pub const PCT_MAX: u16 = 75;

/// Pane rectangles of a split main area.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Split {
    pub shell: ratatui::layout::Rect,
    pub divider: ratatui::layout::Rect,
    pub side: ratatui::layout::Rect,
}

/// Width in cells of the close button in the top corner of the preview pane.
pub const CLOSE_W: u16 = 3;
/// Width in cells of the swap button (⇄) just left of the close button.
pub const SWAP_W: u16 = 3;

/// The title-bar buttons of a pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Btn {
    Close,
    Swap,
}

/// Which button of a pane occupying `r` is at this cell: ✕ in the top outer corner, ⇄ next to it.
pub fn button_at(r: ratatui::layout::Rect, col: u16, row: u16) -> Option<Btn> {
    if r.width < (CLOSE_W + SWAP_W) * 2 || row != r.y || col >= r.x + r.width {
        return None;
    }
    let from_right = r.x + r.width - 1 - col.min(r.x + r.width - 1);
    if col < r.x || from_right >= CLOSE_W + SWAP_W {
        None
    } else if from_right < CLOSE_W {
        Some(Btn::Close)
    } else {
        Some(Btn::Swap)
    }
}

impl Split {
    pub fn on_divider(&self, col: u16, row: u16) -> bool {
        col == self.divider.x && row >= self.divider.y && row < self.divider.y + self.divider.height
    }
}

/// Split `main` into shell pane, 1-column divider and preview pane. None when the setting is
/// `tab` or the area is too narrow for a shell of at least 60 columns plus a preview of at
/// least 40 columns.
pub fn split_main(main: ratatui::layout::Rect, side: Side, pct: u16) -> Option<Split> {
    use ratatui::layout::Rect;
    let w = main.width;
    if side == Side::Tab || w < SHELL_MIN + 1 + PREVIEW_MIN {
        return None;
    }
    let pw = (u32::from(w) * u32::from(pct.clamp(PCT_MIN, PCT_MAX)) / 100) as u16;
    let pw = pw.clamp(PREVIEW_MIN, w - 1 - SHELL_MIN);
    let sw = w - 1 - pw;
    let (x, y, h) = (main.x, main.y, main.height);
    Some(match side {
        Side::Left => Split { side: Rect::new(x, y, pw, h), divider: Rect::new(x + pw, y, 1, h), shell: Rect::new(x + pw + 1, y, sw, h) },
        _ => Split { shell: Rect::new(x, y, sw, h), divider: Rect::new(x + sw, y, 1, h), side: Rect::new(x + sw + 1, y, pw, h) },
    })
}

/// Like `split_main` for a pinned tab on `pin_side` (Left or Right; anything else counts as Right).
/// In the result `shell` is the rectangle of the ACTIVE tab and `side` the other one, so the
/// pinned tab sits on `pin_side` whichever of the two has the keyboard.
pub fn split_pinned(main: ratatui::layout::Rect, pin_side: Side, pct: u16, active_is_pinned: bool) -> Option<Split> {
    let side = if pin_side == Side::Left { Side::Left } else { Side::Right };
    let s = split_main(main, side, pct)?;
    Some(if active_is_pinned { Split { shell: s.side, divider: s.divider, side: s.shell } } else { s })
}

/// The preview percentage that puts the divider at screen column `col`.
pub fn pct_for_col(main: ratatui::layout::Rect, side: Side, col: u16) -> u16 {
    let w = u32::from(main.width.max(1));
    let rel = u32::from(col.saturating_sub(main.x));
    let pw = if side == Side::Left { rel } else { w.saturating_sub(rel + 1) };
    (((pw * 100 + w / 2) / w) as u16).clamp(PCT_MIN, PCT_MAX)
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
    /// A preview shown beside the shell (shell tabs only).
    pub side: Option<crate::preview::Preview>,
    pub focus: Pane,
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
            side: None,
            focus: Pane::Shell,
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

    /// The program in this tab is an AI agent: started as one, or its title says claude/codex.
    pub fn is_agent(&self) -> bool {
        self.agent.is_some()
            || self.compat.title.as_deref().is_some_and(|t| {
                let t = t.to_ascii_lowercase();
                t.contains("claude") || t.contains("codex")
            })
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
            side: None,
            focus: Pane::Shell,
        }
    }

    /// The side pane has the keyboard.
    pub fn side_focused(&self) -> bool {
        self.side.is_some() && self.focus == Pane::Side
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
    use ratatui::layout::Rect;

    fn main(w: u16) -> Rect {
        Rect::new(0, 2, w, 26)
    }

    #[test]
    fn right_split_at_120() {
        let s = split_main(main(120), Side::Right, 50).unwrap();
        assert_eq!((s.shell.x, s.shell.width), (0, 60), "shell keeps its 60-column minimum");
        assert_eq!((s.divider.x, s.divider.width), (60, 1));
        assert_eq!((s.side.x, s.side.width), (61, 59));
        assert_eq!(s.shell.width + 1 + s.side.width, 120);
        assert_eq!((s.side.y, s.side.height), (2, 26));
    }

    #[test]
    fn left_split_flips() {
        let s = split_main(main(120), Side::Left, 50).unwrap();
        assert_eq!((s.side.x, s.side.width), (0, 59));
        assert_eq!(s.divider.x, 59);
        assert_eq!((s.shell.x, s.shell.width), (60, 60));
    }

    #[test]
    fn percentages_and_clamps() {
        let wide = main(200);
        assert_eq!(split_main(wide, Side::Right, 50).unwrap().side.width, 100);
        assert_eq!(split_main(wide, Side::Right, 30).unwrap().side.width, 60);
        assert_eq!(split_main(wide, Side::Right, 5).unwrap().side.width, 50, "below 25% is 25%");
        assert_eq!(split_main(wide, Side::Right, 99).unwrap().side.width, 139, "75% would leave the shell under 60");
        assert_eq!(split_main(main(160), Side::Right, 99).unwrap().side.width, 99, "capped at 75% (and shell >= 60)");
        assert_eq!(split_main(main(110), Side::Right, 25).unwrap().side.width, 40, "preview minimum");
    }

    #[test]
    fn too_narrow_or_tab_setting_gives_no_split() {
        assert!(split_main(main(100), Side::Right, 50).is_none());
        assert!(split_main(main(80), Side::Left, 50).is_none());
        assert!(split_main(main(101), Side::Right, 50).is_some());
        assert!(split_main(main(200), Side::Tab, 50).is_none());
    }

    #[test]
    fn divider_hit_testing_and_drag_percentage() {
        let s = split_main(main(120), Side::Right, 50).unwrap();
        assert!(s.on_divider(60, 2) && s.on_divider(60, 27));
        assert!(!s.on_divider(59, 5) && !s.on_divider(61, 5) && !s.on_divider(60, 1) && !s.on_divider(60, 28));
        // dragging the divider to column 80 of 120 leaves 39 columns on the right: 33%
        assert_eq!(pct_for_col(main(120), Side::Right, 80), 33);
        assert_eq!(pct_for_col(main(120), Side::Left, 40), 33);
        assert_eq!(pct_for_col(main(120), Side::Right, 119), 25, "clamped");
        assert_eq!(pct_for_col(main(120), Side::Right, 0), 75);
    }

    #[test]
    fn swap_and_close_buttons() {
        let r = Rect::new(61, 2, 59, 26);
        assert_eq!(button_at(r, 119, 2), Some(Btn::Close));
        assert_eq!(button_at(r, 117, 2), Some(Btn::Close));
        assert_eq!(button_at(r, 116, 2), Some(Btn::Swap));
        assert_eq!(button_at(r, 114, 2), Some(Btn::Swap));
        assert_eq!(button_at(r, 113, 2), None);
        assert_eq!(button_at(r, 119, 3), None);
        assert_eq!(button_at(r, 120, 2), None);
        assert_eq!(button_at(Rect::new(0, 0, 10, 5), 9, 0), None, "too narrow for buttons");
    }

    #[test]
    fn pinned_split_keeps_the_pinned_pane_on_its_side() {
        let m = main(120);
        let a = split_pinned(m, Side::Right, 40, false).unwrap();
        assert_eq!((a.side.x, a.side.width, a.shell.x), (72, 48, 0), "pinned on the right, active on the left");
        let b = split_pinned(m, Side::Right, 40, true).unwrap();
        assert_eq!((b.shell.x, b.shell.width, b.side.x), (72, 48, 0), "active is the pinned one: it stays on the right");
        assert_eq!(b.divider, a.divider);
        let l = split_pinned(m, Side::Left, 40, true).unwrap();
        assert_eq!((l.shell.x, l.shell.width), (0, 48));
        assert!(split_pinned(main(100), Side::Right, 40, false).is_none());
    }

    #[test]
    fn side_parsing() {
        assert_eq!(Side::parse("left"), Side::Left);
        assert_eq!(Side::parse("TAB"), Side::Tab);
        assert_eq!(Side::parse("right"), Side::Right);
        assert_eq!(Side::parse("bogus"), Side::Right);
    }

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

    #[test]
    fn agent_detection_by_start_or_title() {
        let mut t = Tab::new(0, "shell 1".into(), handle(), 80, 24, 100);
        assert!(!t.is_agent());
        t.output(b"\x1b]0;vim x\x07");
        assert!(!t.is_agent());
        t.output(b"\x1b]0;\xe2\x9c\xb3 Claude Code\x07");
        assert!(t.is_agent());
        t.output(b"\x1b]0;codex\x07");
        assert!(t.is_agent());
        t.output(b"\x1b]0;bash\x07");
        assert!(!t.is_agent());
        t.agent = Some("claude".into());
        assert!(t.is_agent());
    }
}
