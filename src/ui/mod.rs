//! Rendering: header, status bar, shell screen, menu overlay.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// Everything the menu can do. Every user-visible action goes through this
/// enum so extensions can contribute entries later.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Preview,
    Git,
    Download,
    /// Label every file path and URL on screen so one keypress opens it.
    OpenLink,
    NewTab,
    /// Prompt for a folder and open a shell tab there.
    NewTabIn,
    /// Prompt for a folder and move this idle shell tab to it.
    ChangeDir,
    NextTab,
    PrevTab,
    Mouse,
    Agent,
    CloseTab,
    Reconnect,
    Scroll,
    Info,
    Quit,
    EndAndQuit,
}

pub struct MenuItem {
    pub label: &'static str,
    pub key: char,
    pub action: Action,
    /// Only shown for remote workspaces.
    pub remote_only: bool,
}

/// Menu entries that apply to this kind of workspace.
pub fn menu_items(remote: bool) -> Vec<&'static MenuItem> {
    MENU.iter().filter(|m| remote || !m.remote_only).collect()
}

/// On a remote box with tmux, "Quit" only detaches: the shells keep running there.
pub const DETACH_ITEM: MenuItem = MenuItem { label: "Detach (keep sessions)", key: 'q', action: Action::Quit, remote_only: true };
pub const END_ITEM: MenuItem = MenuItem { label: "End sessions and quit", key: 'e', action: Action::EndAndQuit, remote_only: true };

static TMUX_MENU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tell the menu that shells live in tmux (set once after connecting).
pub fn set_tmux_menu(on: bool) {
    TMUX_MENU.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Menu entries for a workspace, with the Quit entry replaced by Detach / End for tmux boxes.
pub fn menu_items_for(remote: bool, tmux: bool) -> Vec<&'static MenuItem> {
    let mut items = menu_items(remote);
    if tmux && let Some(i) = items.iter().position(|m| m.action == Action::Quit) {
        items[i] = &DETACH_ITEM;
        items.insert(i + 1, &END_ITEM);
    }
    items
}

pub const MENU: &[MenuItem] = &[
    MenuItem { label: "Preview a file", key: 'p', action: Action::Preview, remote_only: false },
    MenuItem { label: "Git changes", key: 'g', action: Action::Git, remote_only: false },
    MenuItem { label: "Download", key: 'd', action: Action::Download, remote_only: true },
    MenuItem { label: "Open path on screen", key: 'o', action: Action::OpenLink, remote_only: false },
    MenuItem { label: "New shell tab", key: 'n', action: Action::NewTab, remote_only: false },
    MenuItem { label: "New tab in folder", key: 'f', action: Action::NewTabIn, remote_only: false },
    MenuItem { label: "Change folder of this tab", key: 'h', action: Action::ChangeDir, remote_only: false },
    MenuItem { label: "Next tab", key: 't', action: Action::NextTab, remote_only: false },
    MenuItem { label: "Previous tab", key: 'b', action: Action::PrevTab, remote_only: false },
    MenuItem { label: "Close tab", key: 'c', action: Action::CloseTab, remote_only: false },
    MenuItem { label: "Reconnect", key: 'r', action: Action::Reconnect, remote_only: true },
    MenuItem { label: "Scroll", key: 's', action: Action::Scroll, remote_only: false },
    MenuItem { label: "Start / change agent", key: 'a', action: Action::Agent, remote_only: false },
    MenuItem { label: "Mouse on/off", key: 'm', action: Action::Mouse, remote_only: false },
    MenuItem { label: "Info", key: 'i', action: Action::Info, remote_only: false },
    MenuItem { label: "Quit", key: 'q', action: Action::Quit, remote_only: false },
];

/// The "start an agent?" prompt.
#[derive(Clone)]
pub struct Ask {
    pub agents: Vec<crate::agents::Agent>,
    pub commands: Vec<String>,
    pub sel: usize,
    /// Some while the command is being edited.
    pub editing: Option<String>,
}

impl Ask {
    pub fn new(agents: Vec<crate::agents::Agent>, cfg: &crate::config::Config, state: &crate::agents::State, key: &str) -> Ask {
        let commands = agents.iter().map(|a| crate::agents::command_for(*a, cfg, state, key)).collect();
        Ask { agents, commands, sel: 0, editing: None }
    }
}

/// What the prompt is asking for.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptKind {
    /// A file to preview.
    #[default]
    File,
    /// A folder for a new shell tab.
    NewFolder,
    /// A folder to move the active (idle) tab to.
    ChangeFolder,
}

impl PromptKind {
    pub fn is_folder(self) -> bool {
        self != PromptKind::File
    }
    pub fn label(self) -> &'static str {
        match self {
            PromptKind::File => " open file: ",
            PromptKind::NewFolder => " new tab in folder: ",
            PromptKind::ChangeFolder => " change folder to: ",
        }
    }
}

impl Prompt {
    /// The folder prompt: starts as `start` (the active tab's folder, with a trailing slash) and
    /// lists `recent` folders until the input is edited.
    pub fn folder(kind: PromptKind, start: &str, recent: Vec<String>) -> Prompt {
        let total = recent.len();
        let input = if start.ends_with('/') { start.to_string() } else { format!("{start}/") };
        Prompt { input, shown: recent, total, kind, recent: total > 0, ..Prompt::default() }
    }
}

/// State of the "open a file" / folder prompt.
#[derive(Default, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    /// `shown` lists recent folders (the input has not been edited yet) rather than completions.
    pub recent: bool,
    /// Highlighted recent folder (arrow keys).
    pub sel: Option<usize>,
    pub input: String,
    /// Completion candidates to show (directories end in `/`).
    pub shown: Vec<String>,
    /// Total number of matches, which may exceed what is shown.
    pub total: usize,
    pub error: Option<String>,
}

/// Text of one tab in the strip, e.g. ` 2 build * `.
pub fn tab_label(i: usize, title: &str, state: crate::tab::TabState) -> String {
    use crate::tab::TabState;
    let mark = match state {
        TabState::Streaming => " *",
        TabState::Quiet => " !",
        TabState::Idle => "",
    };
    format!(" {} {}{} ", i + 1, title, mark)
}

/// Which tab is under column `x` of the strip.
pub fn tab_at(tabs: &[(String, bool, crate::tab::TabState)], x: u16) -> Option<usize> {
    let mut start = 0usize;
    for (i, (title, _, state)) in tabs.iter().enumerate() {
        let w = tab_label(i, title, *state).chars().count();
        if (x as usize) < start + w {
            return Some(i);
        }
        start += w;
    }
    None
}

/// Where the menu box is drawn inside the main area.
pub fn menu_rect(main: Rect, items: usize) -> Rect {
    let w = 30.min(main.width);
    let h = (items as u16 + 2).min(main.height);
    Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h)
}

/// Where the open-file prompt (candidates plus input line) is drawn.
pub fn prompt_rect(main: Rect, p: &Prompt) -> Rect {
    let more = p.total.saturating_sub(p.shown.len());
    let rows = p.shown.len() + usize::from(more > 0) + usize::from(p.error.is_some()) + 1;
    let h = (rows as u16).min(main.height);
    Rect::new(main.x, main.y + main.height - h, main.width, h)
}

/// A yes/no question shown before anything is opened or downloaded.
#[derive(Clone)]
pub struct Confirm {
    pub title: String,
    pub lines: Vec<String>,
    /// What "yes" does, e.g. "Open".
    pub yes: String,
}

/// A label drawn over a link on screen (position relative to the whole terminal screen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub label: String,
    pub row: u16,
    pub col: u16,
    pub width: u16,
}

/// Make text safe to show in a dialog: control characters and the bidirectional overrides that
/// can make a name read differently from what it is are replaced.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' => '·',
            c if c.is_control() => '·',
            c => c,
        })
        .collect()
}

/// Short labels for `n` links: single letters from the home row first, then pairs.
pub fn hint_labels(n: usize) -> Vec<String> {
    const KEYS: &str = "asdfghjklqwertyuiopzxcvbnm";
    let keys: Vec<char> = KEYS.chars().collect();
    if n <= keys.len() {
        return keys.iter().take(n).map(|c| c.to_string()).collect();
    }
    let mut out = Vec::with_capacity(n);
    'outer: for a in &keys {
        for b in &keys {
            out.push(format!("{a}{b}"));
            if out.len() == n {
                break 'outer;
            }
        }
    }
    out
}

pub struct View<'a> {
    pub confirm: Option<&'a Confirm>,
    pub hints: Option<(&'a [Hint], &'a str)>,
    /// Selected cells, relative to the shell area (or the preview body).
    pub selection: Option<crate::select::Sel>,
    pub ask: Option<&'a Ask>,
    pub preview: Option<&'a crate::preview::Preview>,
    pub prompt: Option<&'a Prompt>,
    pub screen: &'a vt100::Screen,
    pub name: &'a str,
    pub detail: &'a str,
    pub remote: bool,
    pub menu_label: &'a str,
    /// (title, active, state) per tab; the strip shows only with 2+ tabs.
    pub tabs: &'a [(String, bool, crate::tab::TabState)],
    pub message: Option<&'a str>,
    /// Some(selected index) when the menu is open.
    pub menu: Option<usize>,
    /// Some((offset, total)) while scrolling back through history.
    pub scroll: Option<(usize, usize)>,
}

fn vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

struct ScreenWidget<'a>(&'a vt100::Screen, Option<crate::select::Sel>);

impl Widget for ScreenWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let screen = self.0;
        let sel = self.1;
        for row in 0..area.height {
            for col in 0..area.width {
                let Some(cell) = screen.cell(row, col) else { continue };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style = Style::default().fg(vt_color(cell.fgcolor())).bg(vt_color(cell.bgcolor()));
                let mut m = Modifier::empty();
                if cell.bold() { m |= Modifier::BOLD; }
                if cell.italic() { m |= Modifier::ITALIC; }
                if cell.underline() { m |= Modifier::UNDERLINED; }
                let selected = sel.is_some_and(|s| crate::select::contains(s, row, col));
                if cell.inverse() != selected { m |= Modifier::REVERSED; }
                style = style.add_modifier(m);
                let out = &mut buf[(area.x + col, area.y + row)];
                let text = cell.contents();
                out.set_symbol(if text.is_empty() { " " } else { text });
                out.set_style(style);
            }
        }
    }
}

pub fn draw(f: &mut Frame, v: &View) {
    let strip_h = if v.tabs.len() > 1 { 1 } else { 0 };
    let [header, strip, main, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(strip_h),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let bar = Style::default().bg(Color::DarkGray).fg(Color::White);

    let tail = if v.remote { "  connected" } else { "" };
    let fixed = format!(" wicli  {}  ", v.name).chars().count() + tail.chars().count();
    let room = (header.width as usize).saturating_sub(fixed);
    let detail = left_trim(v.detail, room);
    f.render_widget(
        Paragraph::new(format!(" wicli  {}  {detail}{tail}", v.name)).style(bar.add_modifier(Modifier::BOLD)),
        header,
    );

    if strip_h == 1 {
        let mut spans = Vec::new();
        for (i, (title, active, state)) in v.tabs.iter().enumerate() {
            use crate::tab::TabState;
            let label = tab_label(i, title, *state);
            let style = if *active {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                match state {
                    TabState::Streaming => Style::default().fg(Color::Yellow),
                    TabState::Quiet => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    TabState::Idle => Style::default(),
                }
            };
            spans.push(Span::styled(label, style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), strip);
    }

    let mut preview_rows = 0usize;
    if let Some(p) = v.preview {
        let (banner_area, body) = if p.banner.is_some() {
            let [b, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(main);
            (Some(b), body)
        } else {
            (None, main)
        };
        if let (Some(b), Some(text)) = (banner_area, p.banner.as_deref()) {
            f.render_widget(Paragraph::new(format!(" {text}")).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)), b);
        }
        preview_rows = body.height as usize;
        let mut vis = p.visible(body.height as usize, body.width as usize);
        if let Some(sel) = v.selection {
            for (r, line) in vis.iter_mut().enumerate() {
                *line = crate::preview::highlight_line(line, r as u16, sel);
            }
        }
        f.render_widget(Paragraph::new(vis), body);
    } else {
        f.render_widget(ScreenWidget(v.screen, v.selection), main);
    }

    if let Some((hints, typed)) = v.hints {
        let buf = f.buffer_mut();
        let area = buf.area;
        for h in hints.iter().filter(|h| h.label.starts_with(typed)) {
            for i in 0..h.width {
                let (x, y) = (h.col + i, h.row);
                if x < area.width && y < area.height {
                    let cell = &mut buf[(x, y)];
                    let st = cell.style().add_modifier(Modifier::UNDERLINED);
                    cell.set_style(st);
                }
            }
            for (i, ch) in h.label.chars().enumerate() {
                let (x, y) = (h.col + i as u16, h.row);
                if x < area.width && y < area.height {
                    let cell = &mut buf[(x, y)];
                    cell.set_char(ch);
                    cell.set_style(Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD));
                }
            }
        }
    }

    let scroll_hint = v.scroll.map(|(n, m)| format!(" scroll: arrows, PgUp/PgDn, Home/End   Esc exits   scrollback {n}/{m}"));
    let preview_hint = v.preview.map(|p| {
        let total = p.display_len();
        let last = (p.top + preview_rows).min(total);
        let files = if p.kind == crate::preview::Kind::Diff { "N/P file · " } else { "" };
        format!(" arrows scroll · {files}W wrap · R reload · Esc back   lines {}-{} of {}", (p.top + 1).min(total), last, total)
    });
    let hint = match (v.menu, v.message) {
        _ if v.confirm.is_some_and(|c| c.yes.is_empty()) => " Esc cancels the download".to_string(),
        _ if v.confirm.is_some() => " Y or Enter to continue · N or Esc to cancel".to_string(),
        _ if v.hints.is_some() => " type a label to open that link · Esc cancels".to_string(),
        _ if v.ask.is_some() => " Enter starts · E edits the command · N not now · X never here".to_string(),
        _ if v.prompt.is_some_and(|p| p.kind.is_folder()) => " Tab completes folders · arrows pick a recent one · Enter confirms · Esc cancels".to_string(),
        _ if v.prompt.is_some() => " Tab completes · Enter opens · Esc cancels".to_string(),
        _ if scroll_hint.is_some() => scroll_hint.unwrap_or_default(),
        (None, None) if preview_hint.is_some() => preview_hint.unwrap_or_default(),
        (Some(_), _) => " arrows + Enter, a letter, or a tab number   Esc closes".to_string(),
        (None, Some(m)) => format!(" {m}"),
        (None, None) => format!(" {}  menu", v.menu_label),
    };
    f.render_widget(Paragraph::new(hint).style(bar), status);

    if let Some(c) = v.confirm {
        draw_confirm(f, main, c);
        return;
    }
    if let Some(a) = v.ask {
        draw_ask(f, main, a);
        return;
    }
    if let Some(pr) = v.prompt {
        draw_prompt(f, main, pr);
        return;
    }
    match v.menu {
        Some(sel) => draw_menu(f, main, sel, v.remote),
        None if v.preview.is_some() => {}
        None if !v.screen.hide_cursor() && v.scroll.is_none() => {
            let (r, c) = v.screen.cursor_position();
            if r < main.height && c < main.width {
                f.set_cursor_position((main.x + c, main.y + r));
            }
        }
        None => {}
    }
}

fn draw_menu(f: &mut Frame, main: Rect, selected: usize, remote: bool) {
    let items = menu_items_for(remote, TMUX_MENU.load(std::sync::atomic::Ordering::Relaxed));
    let area = menu_rect(main, items.len());
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let base = if i == selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let mut chars = item.label.chars();
            let first = chars.next().unwrap_or(' ');
            // Underline the hotkey letter wherever it first appears in the label.
            let label = item.label;
            let pos = label.to_lowercase().find(item.key).unwrap_or(0);
            let (a, rest) = label.split_at(pos);
            let mut rest_chars = rest.chars();
            let hot = rest_chars.next().unwrap_or(first);
            Line::from(vec![
                Span::styled(format!(" {a}"), base),
                Span::styled(hot.to_string(), base.add_modifier(Modifier::UNDERLINED | Modifier::BOLD)),
                Span::styled(format!("{} ", rest_chars.as_str()), base),
            ])
        })
        .collect();
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" menu ")), area);
}

/// Keep the tail of a long path, marking the cut with an ellipsis.
fn left_trim(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max == 0 {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    format!("…{}", s.chars().skip(n - keep).collect::<String>())
}

fn draw_prompt(f: &mut Frame, main: Rect, p: &Prompt) {
    let more = p.total.saturating_sub(p.shown.len());
    let area = prompt_rect(main, p);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines: Vec<Line> = p
        .shown
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let st = if p.recent && p.sel == Some(i) { Style::default().add_modifier(Modifier::REVERSED) } else { dim };
            Line::from(Span::styled(format!("   {s}"), st))
        })
        .collect();
    if more > 0 {
        lines.push(Line::from(Span::styled(format!("   … {more} more"), dim)));
    }
    if let Some(e) = &p.error {
        lines.push(Line::from(Span::styled(format!(" {e}"), Style::default().fg(Color::Red))));
    }
    let label = p.kind.label();
    lines.push(Line::from(vec![Span::styled(label, Style::default().add_modifier(Modifier::BOLD)), Span::raw(p.input.clone())]));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(Color::Black)), area);
    let x = area.x + (label.chars().count() + unicode_width::UnicodeWidthStr::width(p.input.as_str())) as u16;
    f.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y + area.height - 1));
}

fn draw_ask(f: &mut Frame, main: Rect, a: &Ask) {
    let w = 72.min(main.width);
    let h = (a.agents.len() as u16 + 5).min(main.height);
    let area = Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines = vec![Line::from(Span::styled(" Start an agent here. Your choice is remembered for this workspace.", dim)), Line::from("")];
    for (i, ag) in a.agents.iter().enumerate() {
        let sel = i == a.sel;
        let base = if sel { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
        let cmd = match (&a.editing, sel) {
            (Some(buf), true) => format!("{buf}▏"),
            _ => a.commands[i].clone(),
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", if sel { "›" } else { " " }), base),
            Span::styled(format!("{:<12}", ag.label()), base.add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {cmd} "), base),
        ]));
    }
    lines.push(Line::from(""));
    let hint = if a.editing.is_some() { " type the command, Enter starts it, Esc cancels" } else { " Enter start · E edit · N not now · X never here" };
    lines.push(Line::from(Span::styled(hint, dim)));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" start an agent ")), area);
}

fn draw_confirm(f: &mut Frame, main: Rect, c: &Confirm) {
    let w = 78.min(main.width);
    let h = (c.lines.len() as u16 + 5).min(main.height);
    let area = Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h);
    let dim = Style::default().add_modifier(Modifier::DIM);
    let mut lines: Vec<Line> = vec![Line::from("")];
    for l in &c.lines {
        lines.push(Line::from(format!(" {}", sanitize(l))));
    }
    lines.push(Line::from(""));
    if c.yes.is_empty() {
        lines.push(Line::from(vec![Span::styled(" Cancel ", Style::default().add_modifier(Modifier::REVERSED)), Span::styled("   Esc", dim)]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", c.yes), Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(" Cancel ", Style::default().add_modifier(Modifier::REVERSED)),
            Span::styled("   Y / Enter  ·  N / Esc", dim),
        ]));
    }
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(format!(" {} ", sanitize(&c.title)))), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_removes_control_and_bidi_tricks() {
        assert_eq!(sanitize("a\x1b[31mb\n"), "a·[31mb·");
        assert_eq!(sanitize("evil\u{202E}gpj.exe"), "evil·gpj.exe");
        assert_eq!(sanitize("日本語 ok"), "日本語 ok");
    }

    #[test]
    fn folder_prompt_starts_in_the_tab_folder_with_recents() {
        let p = Prompt::folder(PromptKind::NewFolder, "/p/web", vec!["/p/a".into(), "/p/b".into()]);
        assert_eq!(p.input, "/p/web/");
        assert!(p.recent && p.shown.len() == 2 && p.sel.is_none());
        let p = Prompt::folder(PromptKind::ChangeFolder, "/p/web/", vec![]);
        assert_eq!(p.input, "/p/web/");
        assert!(!p.recent);
        assert!(PromptKind::NewFolder.is_folder() && !PromptKind::File.is_folder());
    }

    #[test]
    fn menu_keys_are_unique() {
        let mut keys: Vec<char> = MENU.iter().map(|m| m.key).collect();
        keys.push(END_ITEM.key);
        let set: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(set.len(), keys.len(), "{keys:?}");
    }

    #[test]
    fn hint_labels_are_unique_and_prefix_free() {
        for n in [1, 5, 26, 27, 100, 600] {
            let l = hint_labels(n);
            assert_eq!(l.len(), n);
            let set: std::collections::HashSet<_> = l.iter().collect();
            assert_eq!(set.len(), n);
            if n > 26 {
                assert!(l.iter().all(|x| x.chars().count() == 2), "pairs only, so no label is a prefix of another");
            }
        }
        assert_eq!(hint_labels(3), vec!["a", "s", "d"]);
    }
}
