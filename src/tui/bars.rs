//! One-row bars: inverted title bar, dim key-hint line, status line with a badge.

use super::{Tone, first_row, fill, fit, put, tone_style, width};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Full-width inverted bar: left text and an optional right-aligned text.
#[derive(Clone, Debug, Default)]
pub struct TitleBar {
    pub left: String,
    pub right: Option<String>,
}

impl TitleBar {
    pub fn new(left: impl Into<String>) -> Self {
        TitleBar { left: left.into(), right: None }
    }
    pub fn right(mut self, r: impl Into<String>) -> Self {
        self.right = Some(r.into());
        self
    }
    /// Draws into the first row of `area`. The right text keeps up to half the width.
    pub fn render(&self, area: Rect, buf: &mut Buffer, t: &Theme) {
        let Some(a) = first_row(area, buf) else { return };
        let st = Style::default().fg(t.title_fg).bg(t.title_bg).add_modifier(Modifier::BOLD);
        fill(buf, a, st);
        let inner = if a.width >= 3 { Rect::new(a.x + 1, a.y, a.width - 2, 1) } else { a };
        let w = inner.width as usize;
        let right = self.right.as_deref().map(|r| fit(r, w / 2)).unwrap_or_default();
        let rw = width(&right);
        let lw = w.saturating_sub(rw + usize::from(rw > 0));
        put(buf, inner.x, inner.y, &self.left, lw, st);
        if rw > 0 {
            put(buf, inner.x + (w - rw) as u16, inner.y, &right, rw, st);
        }
    }
}

/// One dim line of `(key, label)` pairs joined with ` · `. Trailing hints that do not fit are
/// dropped; the line never wraps.
#[derive(Clone, Debug, Default)]
pub struct HintBar {
    pub hints: Vec<(String, String)>,
}

const SEP: &str = " · ";

impl HintBar {
    pub fn new(hints: &[(&str, &str)]) -> Self {
        HintBar { hints: hints.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect() }
    }
    fn item_w(&self, i: usize) -> usize {
        let (k, l) = &self.hints[i];
        width(k) + 1 + width(l)
    }
    /// How many hints fit in `w` cells (at least one when there is any: that one is truncated).
    pub fn fitting(&self, w: usize) -> usize {
        let mut used = 0;
        let mut n = 0;
        for i in 0..self.hints.len() {
            let add = self.item_w(i) + if i > 0 { width(SEP) } else { 0 };
            if used + add > w {
                break;
            }
            used += add;
            n += 1;
        }
        if n == 0 && !self.hints.is_empty() { 1 } else { n }
    }
    pub fn render(&self, area: Rect, buf: &mut Buffer, t: &Theme) {
        let Some(a) = first_row(area, buf) else { return };
        let base = Style::default().fg(t.hintline_fg).bg(t.bg);
        let key = Style::default().fg(t.fg).bg(t.bg).add_modifier(Modifier::BOLD);
        fill(buf, a, base);
        let inner = if a.width >= 2 { Rect::new(a.x + 1, a.y, a.width - 1, 1) } else { a };
        let w = inner.width as usize;
        let mut x = inner.x;
        let end = inner.x + inner.width;
        for i in 0..self.fitting(w) {
            if i > 0 {
                x += put(buf, x, a.y, SEP, (end - x) as usize, base);
            }
            let (k, l) = &self.hints[i];
            x += put(buf, x, a.y, k, (end - x) as usize, key);
            x += put(buf, x, a.y, " ", (end - x) as usize, base);
            x += put(buf, x, a.y, l, (end - x) as usize, base);
        }
    }
}

/// A coloured badge (`LOCAL`, a box name) followed by dim segments with optional tones.
#[derive(Clone, Debug, Default)]
pub struct StatusLine {
    pub badge: String,
    pub segments: Vec<(String, Tone)>,
}

impl StatusLine {
    pub fn new(badge: impl Into<String>) -> Self {
        StatusLine { badge: badge.into(), segments: Vec::new() }
    }
    pub fn dim(self, text: impl Into<String>) -> Self {
        self.seg(text, Tone::Dim)
    }
    pub fn seg(mut self, text: impl Into<String>, tone: Tone) -> Self {
        self.segments.push((text.into(), tone));
        self
    }
    /// Segments that do not fit are dropped from the end; the badge is cut last.
    pub fn render(&self, area: Rect, buf: &mut Buffer, t: &Theme) {
        let Some(a) = first_row(area, buf) else { return };
        fill(buf, a, Style::default().fg(t.dim).bg(t.bg));
        let end = a.x + a.width;
        let mut x = a.x;
        if !self.badge.is_empty() {
            let st = Style::default().fg(t.badge_fg).bg(t.badge_bg).add_modifier(Modifier::BOLD);
            let text = format!(" {} ", self.badge);
            x += put(buf, x, a.y, &text, a.width as usize, st);
        }
        for (text, tone) in &self.segments {
            let room = (end - x) as usize;
            if room < 3 || width(text) + 2 > room {
                break;
            }
            x += 2;
            x += put(buf, x, a.y, text, room - 2, tone_style(t, *tone));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DARK, LIGHT};

    use crate::tui::row_text as row;

    #[test]
    fn title_bar_left_right_and_truncation() {
        let mut b = Buffer::empty(Rect::new(0, 0, 30, 1));
        TitleBar::new("wi explorer").right("~/work").render(b.area, &mut b, &DARK);
        assert_eq!(row(&b, 0), " wi explorer           ~/work ");
        let mut b = Buffer::empty(Rect::new(0, 0, 12, 1));
        TitleBar::new("日本語のタイトルです").right("right").render(b.area, &mut b, &LIGHT);
        let r = row(&b, 0);
        assert!(r.ends_with("right "), "{r:?}");
        assert!(r.contains('…'));
        assert_eq!(crate::tui::width(&r), 12);
        let mut b = Buffer::empty(Rect::new(0, 0, 2, 1));
        TitleBar::new("abcdef").render(b.area, &mut b, &DARK);
        assert_eq!(row(&b, 0), "a…");
    }

    #[test]
    fn hint_bar_drops_trailing_hints_and_never_wraps() {
        let h = HintBar::new(&[("Enter", "open"), ("Esc", "close"), ("F", "find")]);
        assert_eq!(h.fitting(100), 3);
        assert_eq!(h.fitting(22), 2);
        assert_eq!(h.fitting(21), 1);
        assert_eq!(h.fitting(10), 1);
        assert_eq!(h.fitting(2), 1);
        let mut b = Buffer::empty(Rect::new(0, 0, 40, 2));
        h.render(b.area, &mut b, &DARK);
        assert_eq!(row(&b, 0).trim_end(), " Enter open · Esc close · F find");
        assert_eq!(row(&b, 1).trim(), "");
        let mut b = Buffer::empty(Rect::new(0, 0, 8, 1));
        h.render(b.area, &mut b, &DARK);
        assert_eq!(row(&b, 0), " Enter …");
    }

    #[test]
    fn status_line_badge_and_tones() {
        let mut b = Buffer::empty(Rect::new(0, 0, 50, 1));
        StatusLine::new("LOCAL").dim("12 files").seg("3 changed", Tone::Warn).seg("1 error", Tone::Hot).render(b.area, &mut b, &DARK);
        assert_eq!(row(&b, 0).trim_end(), " LOCAL   12 files  3 changed  1 error");
        assert_eq!(b[(1, 0)].style().bg, Some(DARK.badge_bg));
        assert_eq!(b[(1, 0)].style().fg, Some(DARK.badge_fg));
        let mut b = Buffer::empty(Rect::new(0, 0, 25, 1));
        StatusLine::new("LOCAL").dim("12 files").seg("3 changed", Tone::Warn).seg("1 error", Tone::Hot).render(b.area, &mut b, &DARK);
        assert_eq!(row(&b, 0).trim_end(), " LOCAL   12 files");
    }
}
