//! Drawing and geometry for the search screens: the query prompt, the results list, the file
//! finder and the find prompt. State lives in `crate::search`; mouse hit-testing uses the same
//! rectangles as drawing.

use crate::search::grep::{self, Results, Row};
use crate::search::{Finder, FindPrompt, FindTarget, GrepPrompt, fuzzy};
use ratatui::Frame;
use ratatui::layout::Rect;
use crate::theme::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

/// What the search screens need to draw.
pub enum SearchView<'a> {
    Grep(&'a GrepPrompt),
    Results(&'a Results),
    Finder(&'a Finder),
    Find(&'a FindPrompt),
}

/// The list area of the results screen: everything under the one-line summary.
pub fn results_list_rect(main: Rect) -> Rect {
    Rect::new(main.x, main.y + 1.min(main.height), main.width, main.height.saturating_sub(1))
}

/// The list area of the finder: under the title line, above the input line.
pub fn finder_list_rect(main: Rect) -> Rect {
    Rect::new(main.x, main.y + 1.min(main.height), main.width, main.height.saturating_sub(2))
}

/// The two bottom lines of the search prompt: option chips above, the query below.
pub fn grep_prompt_rect(main: Rect) -> Rect {
    let h = 2.min(main.height);
    Rect::new(main.x, main.y + main.height - h, main.width, h)
}

/// The option chips of the search prompt: (text, key it stands for, first column, width).
pub fn grep_chips(p: &GrepPrompt) -> Vec<(String, &'static [u8], u16, u16)> {
    let onoff = |b: bool| if b { "on" } else { "off" };
    let chips = [
        (format!(" Tab case: {} ", p.query.case.label()), &b"\t"[..]),
        (format!(" Up whole word: {} ", onoff(p.query.word)), &b"\x1b[A"[..]),
        (format!(" Down regex: {} ", onoff(p.query.regex)), &b"\x1b[B"[..]),
    ];
    let mut x = 1u16;
    chips
        .into_iter()
        .map(|(t, k)| {
            let w = t.chars().count() as u16;
            let r = (t, k, x, w);
            x += w + 1;
            r
        })
        .collect()
}

/// The key bytes of the chip at (col, row), when the click is on one.
pub fn grep_chip_at(main: Rect, p: &GrepPrompt, col: u16, row: u16) -> Option<&'static [u8]> {
    let r = grep_prompt_rect(main);
    if r.height < 2 || row != r.y {
        return None;
    }
    grep_chips(p).into_iter().find(|(_, _, x, w)| col >= r.x + x && col < r.x + x + w).map(|(_, k, _, _)| k)
}

/// Normal text on the theme's canvas (these screens cover the whole area).
fn canvas() -> Style {
    let t = theme();
    Style::default().fg(t.fg).bg(t.bg)
}

fn dim() -> Style {
    canvas().fg(theme().dim)
}

/// The one-line titles above the lists.
fn label_style() -> Style {
    let t = theme();
    Style::default().fg(t.bar_fg).bg(t.bar_bg).add_modifier(Modifier::BOLD)
}

/// The selected row.
fn sel_style() -> Style {
    let t = theme();
    Style::default().fg(t.menu_sel_fg).bg(t.menu_sel_bg)
}

/// Matched text in results and previews.
fn lit_style() -> Style {
    let t = theme();
    Style::default().fg(t.hint_fg).bg(t.hint_bg)
}

fn input_line(f: &mut Frame, area: Rect, label: &str, text: &str) {
    let line = Line::from(vec![Span::styled(label.to_string(), canvas().add_modifier(Modifier::BOLD)), Span::raw(text.to_string())]);
    f.render_widget(Paragraph::new(line).style(canvas()), area);
    let x = area.x + (label.width() + text.width()) as u16;
    f.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y));
}

pub fn draw(f: &mut Frame, main: Rect, v: &SearchView) {
    match v {
        SearchView::Grep(p) => draw_grep_prompt(f, main, p),
        SearchView::Results(r) => draw_results(f, main, r),
        SearchView::Finder(fd) => draw_finder(f, main, fd),
        SearchView::Find(p) => {
            let area = Rect::new(main.x, main.y + main.height.saturating_sub(1), main.width, 1.min(main.height));
            f.render_widget(Clear, area);
            let label = match p.target {
                FindTarget::Output => " find in output: ",
                FindTarget::Preview => " find in preview: ",
            };
            input_line(f, area, label, &p.text);
        }
    }
}

fn draw_grep_prompt(f: &mut Frame, main: Rect, p: &GrepPrompt) {
    let area = grep_prompt_rect(main);
    f.render_widget(Clear, area);
    if area.height < 2 {
        input_line(f, area, " search: ", &p.query.text);
        return;
    }
    let mut spans = vec![Span::raw(" ")];
    for (t, _, _, _) in grep_chips(p) {
        spans.push(Span::styled(t, sel_style()));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(canvas()), Rect::new(area.x, area.y, area.width, 1));
    input_line(f, Rect::new(area.x, area.y + 1, area.width, 1), " search in files: ", &p.query.text);
}

fn hl_spans(text: &str, ranges: &[(usize, usize)], base: Style, lit: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut pos = 0;
    for &(s, l) in ranges {
        if s < pos || s >= chars.len() {
            continue;
        }
        if s > pos {
            out.push(Span::styled(chars[pos..s].iter().collect::<String>(), base));
        }
        let e = (s + l).min(chars.len());
        out.push(Span::styled(chars[s..e].iter().collect::<String>(), lit));
        pos = e;
    }
    if pos < chars.len() {
        out.push(Span::styled(chars[pos..].iter().collect::<String>(), base));
    }
    out
}

fn draw_results(f: &mut Frame, main: Rect, r: &Results) {
    f.render_widget(Clear, main);
    let head = format!(" search: {}   {}   [{}]", crate::ui::sanitize(&r.query.text), r.summary(), r.tool.name());
    f.render_widget(Paragraph::new(head).style(label_style()), Rect::new(main.x, main.y, main.width, 1.min(main.height)));
    let list = results_list_rect(main);
    let width = list.width as usize;
    let lit = lit_style();
    let mut lines: Vec<Line> = Vec::new();
    for i in r.list.top..(r.list.top + list.height as usize).min(r.rows.len()) {
        let selected = i == r.list.sel;
        let line = match r.rows[i] {
            Row::Header(fi) => {
                let (path, n) = &r.files[fi];
                Line::from(vec![
                    Span::styled(format!(" {}", crate::ui::sanitize(path)), canvas().fg(theme().diff_file).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  ({n})"), dim()),
                ])
            }
            Row::Match(mi) => {
                let m = &r.matches[mi];
                let ranges = r.re.as_ref().map(|re| grep::highlight_ranges(re, &m.text)).unwrap_or_default();
                let prefix = format!(" {:>6}  ", m.line);
                let (snip, rs) = grep::snippet(&m.text, &ranges, width.saturating_sub(prefix.chars().count()));
                let (base, hi) = if selected { (sel_style(), lit.add_modifier(Modifier::BOLD)) } else { (canvas(), lit) };
                let mut spans = vec![Span::styled(prefix, if selected { base } else { dim() })];
                spans.extend(hl_spans(&snip, &rs, base, hi));
                if selected {
                    let used: usize = spans.iter().map(|s| s.content.width()).sum();
                    spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), base));
                }
                Line::from(spans)
            }
        };
        lines.push(line);
    }
    if r.rows.is_empty() {
        let msg = match &r.error {
            Some(e) => format!(" {e}"),
            None => " nothing found. S searches again, Esc closes".to_string(),
        };
        lines.push(Line::from(Span::styled(msg, dim())));
    }
    f.render_widget(Paragraph::new(lines).style(canvas()), list);
}

fn draw_finder(f: &mut Frame, main: Rect, fd: &Finder) {
    f.render_widget(Clear, main);
    let mut title = format!(" find file: {} of {} files", crate::search::thousands(fd.hits.len()), crate::search::thousands(fd.files.len()));
    if let Some(n) = &fd.notice {
        title.push_str(&format!("   ({n})"));
    }
    f.render_widget(Paragraph::new(title).style(label_style()), Rect::new(main.x, main.y, main.width, 1.min(main.height)));
    let list = finder_list_rect(main);
    let lit = canvas().fg(theme().hint_bg).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut lines: Vec<Line> = Vec::new();
    for i in fd.list.top..(fd.list.top + list.height as usize).min(fd.hits.len()) {
        let path = &fd.files[fd.hits[i].idx];
        let selected = i == fd.list.sel;
        let base = if selected { sel_style() } else { canvas() };
        let pos = fuzzy::score(&fd.input, path).map(|(_, p)| p).unwrap_or_default();
        let max = (list.width as usize).saturating_sub(2);
        let chars: Vec<char> = path.chars().collect();
        // Long paths lose their left part so the file name stays visible.
        let skip = chars.len().saturating_sub(max);
        let mut spans = vec![Span::styled(" ", base)];
        let mut used = 1;
        for (ci, c) in chars.iter().enumerate().skip(skip) {
            let st = if pos.contains(&ci) { base.patch(lit) } else { base };
            spans.push(Span::styled(c.to_string(), st));
            used += 1;
        }
        if selected {
            spans.push(Span::styled(" ".repeat((list.width as usize).saturating_sub(used)), base));
        }
        lines.push(Line::from(spans));
    }
    if fd.hits.is_empty() {
        lines.push(Line::from(Span::styled(" no file matches", dim())));
    }
    f.render_widget(Paragraph::new(lines).style(canvas()), list);
    let input = Rect::new(main.x, main.y + main.height.saturating_sub(1), main.width, 1.min(main.height));
    input_line(f, input, " > ", &fd.input);
}
