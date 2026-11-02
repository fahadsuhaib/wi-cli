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
    Download,
    OpenLocal,
    NewTab,
    NextTab,
    PrevTab,
    CloseTab,
    Reconnect,
    Scroll,
    Info,
    Quit,
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

pub const MENU: &[MenuItem] = &[
    MenuItem { label: "Preview a file", key: 'p', action: Action::Preview, remote_only: false },
    MenuItem { label: "Download", key: 'd', action: Action::Download, remote_only: true },
    MenuItem { label: "Open local copy", key: 'o', action: Action::OpenLocal, remote_only: false },
    MenuItem { label: "New shell tab", key: 'n', action: Action::NewTab, remote_only: false },
    MenuItem { label: "Next tab", key: 't', action: Action::NextTab, remote_only: false },
    MenuItem { label: "Previous tab", key: 'b', action: Action::PrevTab, remote_only: false },
    MenuItem { label: "Close tab", key: 'c', action: Action::CloseTab, remote_only: false },
    MenuItem { label: "Reconnect", key: 'r', action: Action::Reconnect, remote_only: true },
    MenuItem { label: "Scroll", key: 's', action: Action::Scroll, remote_only: false },
    MenuItem { label: "Info", key: 'i', action: Action::Info, remote_only: false },
    MenuItem { label: "Quit", key: 'q', action: Action::Quit, remote_only: false },
];

pub struct View<'a> {
    pub screen: &'a vt100::Screen,
    pub name: &'a str,
    pub detail: &'a str,
    pub remote: bool,
    pub menu_label: &'a str,
    /// (title, active, has new output) per tab; the strip shows only with 2+ tabs.
    pub tabs: &'a [(String, bool, bool)],
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

struct ScreenWidget<'a>(&'a vt100::Screen);

impl Widget for ScreenWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let screen = self.0;
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
                if cell.inverse() { m |= Modifier::REVERSED; }
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
        for (i, (title, active, activity)) in v.tabs.iter().enumerate() {
            let mark = if *activity && !*active { "*" } else { "" };
            let label = format!(" {} {}{} ", i + 1, title, mark);
            let style = if *active {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else if *activity {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            spans.push(Span::styled(label, style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), strip);
    }

    f.render_widget(ScreenWidget(v.screen), main);

    let scroll_hint = v.scroll.map(|(n, m)| format!(" scroll: arrows, PgUp/PgDn, Home/End   Esc exits   scrollback {n}/{m}"));
    let hint = match (v.menu, v.message) {
        _ if scroll_hint.is_some() => scroll_hint.unwrap_or_default(),
        (Some(_), _) => " arrows + Enter, a letter, or a tab number   Esc closes".to_string(),
        (None, Some(m)) => format!(" {m}"),
        (None, None) => format!(" {}  menu", v.menu_label),
    };
    f.render_widget(Paragraph::new(hint).style(bar), status);

    match v.menu {
        Some(sel) => draw_menu(f, main, sel, v.remote),
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
    let items = menu_items(remote);
    let w = 30.min(main.width);
    let h = (items.len() as u16 + 2).min(main.height);
    let area = Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h);
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
