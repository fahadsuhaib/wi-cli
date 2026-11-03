//! The theme picker: a list popup (Auto, dark themes, light themes) with a colour swatch per
//! row. Moving the highlight previews the theme live; the caller applies and reverts.

use super::{bordered, sanitize};
use crate::theme::{self, Kind, Mode, theme};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::cell::Cell;

/// One selectable row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Entry {
    Auto,
    Theme(usize),
}

impl Entry {
    pub fn mode(self) -> Mode {
        match self {
            Entry::Auto => Mode::Auto,
            Entry::Theme(i) => Mode::Named(i),
        }
    }
}

pub struct ThemePick {
    /// The setting when the picker opened (marked in the list, restored on Esc).
    pub orig: Mode,
    pub filter: String,
    /// Index into `entries()`.
    pub sel: usize,
    /// First list row shown, kept between frames so the list scrolls smoothly.
    top: Cell<usize>,
}

impl ThemePick {
    pub fn new(cur: Mode) -> ThemePick {
        let mut p = ThemePick { orig: cur, filter: String::new(), sel: 0, top: Cell::new(0) };
        p.sel = p.entries().iter().position(|e| e.mode() == cur).unwrap_or(0);
        p
    }

    /// The rows that match the filter, in list order: Auto, dark themes, light themes.
    pub fn entries(&self) -> Vec<Entry> {
        let f = self.filter.trim().to_lowercase();
        let hit = |label: &str, name: &str| f.is_empty() || label.to_lowercase().contains(&f) || name.contains(&f.replace(' ', "-"));
        let mut v = Vec::new();
        if hit("auto", "auto") {
            v.push(Entry::Auto);
        }
        for kind in [Kind::Dark, Kind::Light] {
            v.extend(theme::ALL.iter().enumerate().filter(|(_, t)| t.kind == kind && hit(t.label, t.name)).map(|(i, _)| Entry::Theme(i)));
        }
        v
    }

    /// The highlighted row's mode, if the filter left any row.
    pub fn current(&self) -> Option<Mode> {
        let e = self.entries();
        e.get(self.sel.min(e.len().saturating_sub(1))).map(|e| e.mode())
    }

    /// Move the highlight by `d` rows (wrapping).
    pub fn step(&mut self, d: isize) {
        let n = self.entries().len() as isize;
        if n > 0 {
            self.sel = (self.sel as isize + d).rem_euclid(n) as usize;
        }
    }

    pub fn type_char(&mut self, c: char) {
        if c != ' ' || !self.filter.is_empty() {
            self.filter.push(c);
            self.sel = 0;
        }
    }
}

enum Row {
    Header(&'static str),
    Item(usize),
}

/// The theme an entry is drawn with (auto shows what it resolves to).
fn entry_theme(e: Entry) -> theme::Theme {
    e.mode().theme()
}

pub fn draw(f: &mut Frame, main: Rect, p: &ThemePick) {
    let t = theme();
    let entries = p.entries();
    let sel = p.sel.min(entries.len().saturating_sub(1));
    let mut rows: Vec<Row> = Vec::new();
    let mut last: Option<Kind> = None;
    for (n, e) in entries.iter().enumerate() {
        if let Entry::Theme(i) = e {
            let k = theme::ALL[*i].kind;
            if last != Some(k) {
                rows.push(Row::Header(if k == Kind::Dark { "DARK" } else { "LIGHT" }));
                last = Some(k);
            }
        }
        rows.push(Row::Item(n));
    }
    let w = 46.min(main.width);
    let inner_w = w.saturating_sub(2) as usize;
    // border 2 + filter row 1 + list + hint 1
    let list_h = (rows.len().max(1) as u16).min(main.height.saturating_sub(5).max(1)) as usize;
    let h = (4 + list_h as u16).min(main.height);
    let area = Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h);
    let sel_row = rows.iter().position(|r| matches!(r, Row::Item(n) if *n == sel)).unwrap_or(0);
    let mut top = p.top.get().min(rows.len().saturating_sub(list_h));
    if sel_row < top {
        top = sel_row;
    }
    if sel_row >= top + list_h {
        top = sel_row + 1 - list_h;
    }
    // Keep the header above the first visible entry in view when the list is at its start.
    if top == 1 && matches!(rows.first(), Some(Row::Header(_))) && sel_row == 1 {
        top = 0;
    }
    p.top.set(top);

    let plain = Style::default().bg(t.menu_bg).fg(t.menu_fg);
    let dim = Style::default().bg(t.menu_bg).fg(t.dim);
    let pad = |used: usize| " ".repeat(inner_w.saturating_sub(used));
    let mut lines: Vec<Line> = Vec::new();
    let prefix = " type to filter: ";
    if p.filter.is_empty() {
        lines.push(Line::from(vec![Span::styled(" type to filter", dim), Span::styled(pad(15), dim)]));
    } else {
        let shown = sanitize(&p.filter);
        let used = prefix.chars().count() + shown.chars().count();
        lines.push(Line::from(vec![Span::styled(prefix, dim), Span::styled(shown, plain.add_modifier(Modifier::BOLD)), Span::styled(pad(used), plain)]));
    }
    for r in rows.iter().skip(top).take(list_h) {
        match r {
            Row::Header(h) => {
                let text = format!(" {h}");
                lines.push(Line::from(vec![Span::styled(text.clone(), dim.add_modifier(Modifier::BOLD)), Span::styled(pad(text.chars().count()), dim)]));
            }
            Row::Item(n) => {
                let e = entries[*n];
                let on = *n == sel;
                let base = if on { Style::default().bg(t.menu_sel_bg).fg(t.menu_sel_fg) } else { plain };
                let et = entry_theme(e);
                let label = match e {
                    Entry::Auto => format!("Auto ({})", et.kind.name()),
                    Entry::Theme(i) => theme::ALL[i].label.to_string(),
                };
                let current = e.mode() == p.orig;
                // " › Label ... [5 swatch cells] ● "
                let swatch_w = 10;
                let left = format!(" {} {}", if on { "›" } else { " " }, label);
                let left_w = unicode_width::UnicodeWidthStr::width(left.as_str());
                let gap = inner_w.saturating_sub(left_w + swatch_w + 3);
                let mut spans = vec![Span::styled(format!("{left}{}", " ".repeat(gap)), base.add_modifier(if on { Modifier::BOLD } else { Modifier::empty() })), Span::styled(" ", base)];
                let cols: [Color; 5] = [et.bg, et.fg, et.tab_active_bg, et.error, et.good];
                for c in cols {
                    spans.push(Span::styled("  ", Style::default().bg(c)));
                }
                spans.push(Span::styled(if current { " ●" } else { "  " }, base));
                lines.push(Line::from(spans));
            }
        }
    }
    if entries.is_empty() {
        lines.push(Line::from(vec![Span::styled(" no match", dim), Span::styled(pad(9), dim)]));
    }
    let hint = " ● current · arrows preview · Enter saves · Esc cancels";
    lines.push(Line::from(vec![Span::styled(hint, dim), Span::styled(pad(hint.chars().count()), dim)]));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).style(plain).block(bordered(" theme ", t.menu_border, t.menu_bg, t.menu_fg)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_auto_then_dark_then_light_and_starts_on_the_current_one() {
        let mono = theme::index_of("monokai").unwrap();
        let p = ThemePick::new(Mode::Named(mono));
        let e = p.entries();
        assert_eq!(e.len(), 17);
        assert_eq!(e[0], Entry::Auto);
        assert_eq!(e[p.sel], Entry::Theme(mono));
        let kinds: Vec<Kind> = e[1..].iter().map(|e| match e { Entry::Theme(i) => theme::ALL[*i].kind, Entry::Auto => unreachable!() }).collect();
        assert!(kinds.windows(2).all(|w| w[0] == Kind::Dark || w[1] == Kind::Light), "dark first, then light");
        assert_eq!(ThemePick::new(Mode::Auto).sel, 0);
    }

    #[test]
    fn typing_filters_by_label_or_name_and_steps_wrap() {
        let mut p = ThemePick::new(Mode::Auto);
        for c in "solar".chars() {
            p.type_char(c);
        }
        let names: Vec<_> = p.entries().iter().map(|e| match e { Entry::Theme(i) => theme::ALL[*i].name, Entry::Auto => "auto" }).collect();
        assert_eq!(names, ["solarized-dark", "solarized-light"]);
        p.step(-1);
        assert_eq!(p.current(), Some(Mode::Named(theme::index_of("solarized-light").unwrap())));
        p.step(1);
        assert_eq!(p.current(), Some(Mode::Named(theme::index_of("solarized-dark").unwrap())));
        p.filter = "dark mod".into();
        assert_eq!(p.entries(), [Entry::Theme(theme::index_of("dark-modern").unwrap())]);
        p.filter = "zzz".into();
        assert!(p.entries().is_empty() && p.current().is_none());
    }
}
