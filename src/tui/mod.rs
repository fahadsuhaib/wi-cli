//! Widget kit for dense, aligned screens: title bar, hint line, status line, panels with coloured
//! header rows, column lists, a tree model and a two-pane split. Everything draws into a ratatui
//! `Buffer` with colours from the theme; widths are measured with unicode-width.

// A kit: the explorer uses most of it; the rest is for the next screens.
#![allow(dead_code)]

pub mod bars;
pub mod columns;
pub mod panel;
pub mod split;
pub mod tree;

#[cfg(test)]
mod golden;

use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthChar;

/// Semantic colour of a text segment or cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Normal,
    Dim,
    Good,
    Warn,
    Hot,
}

/// Foreground style of a tone, drawn over the canvas.
pub fn tone_style(t: &Theme, tone: Tone) -> Style {
    let fg = match tone {
        Tone::Normal => t.fg,
        Tone::Dim => t.dim,
        Tone::Good => t.good,
        Tone::Warn => t.warn,
        Tone::Hot => t.hot,
    };
    Style::default().fg(fg).bg(t.bg)
}

fn char_w(c: char) -> usize {
    if c.is_control() { 1 } else { c.width().unwrap_or(0) }
}

/// Display width in terminal cells (control characters count as one space).
pub fn width(s: &str) -> usize {
    s.chars().map(char_w).sum()
}

/// `s` cut to at most `w` cells; when cut, the last cell is `…`. Never splits a wide character.
/// Control characters become spaces.
pub fn fit(s: &str, w: usize) -> String {
    let clean = s.chars().map(|c| if c.is_control() { ' ' } else { c });
    if width(s) <= w {
        return clean.collect();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut acc = 0;
    for c in clean {
        let cw = char_w(c);
        if acc + cw > w - 1 {
            break;
        }
        out.push(c);
        acc += cw;
    }
    out.push('…');
    out
}

/// Draw `s` (already fitted by the caller or cut here) at `x,y` within `w` cells; returns the
/// number of cells used. Out-of-buffer positions are ignored.
pub fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, w: usize, style: Style) -> u16 {
    let b = buf.area;
    if y < b.y || y >= b.y + b.height || x < b.x || x >= b.x + b.width || w == 0 {
        return 0;
    }
    let room = (b.x + b.width - x) as usize;
    let text = fit(s, w.min(room));
    let used = width(&text) as u16;
    buf.set_stringn(x, y, &text, w.min(room), style);
    used
}

/// Paint `style` over every cell of `area` (clipped to the buffer), replacing the text with spaces.
pub fn fill(buf: &mut Buffer, area: Rect, style: Style) {
    let a = area.intersection(buf.area);
    for y in a.y..a.y + a.height {
        for x in a.x..a.x + a.width {
            let cell = &mut buf[(x, y)];
            cell.set_symbol(" ");
            cell.set_style(style);
        }
    }
}

/// First row of `area`, clipped to the buffer; None when nothing is visible.
pub(crate) fn first_row(area: Rect, buf: &Buffer) -> Option<Rect> {
    let a = area.intersection(buf.area);
    (a.width > 0 && a.height > 0).then(|| Rect::new(a.x, a.y, a.width, 1))
}

/// Text of one buffer row for tests: wide characters appear once (their trailing cell is skipped).
#[cfg(test)]
pub fn row_text(buf: &Buffer, y: u16) -> String {
    let mut out = String::new();
    let mut skip = 0;
    for x in 0..buf.area.width {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let sym = buf[(x, y)].symbol();
        skip = width(sym).saturating_sub(1);
        out.push_str(sym);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_truncates_with_ellipsis_and_keeps_wide_chars_whole() {
        assert_eq!(fit("hello", 5), "hello");
        assert_eq!(fit("hello world", 6), "hello…");
        assert_eq!(fit("hello", 0), "");
        assert_eq!(fit("hello", 1), "…");
        // Each CJK character is two cells; the ellipsis never lands in the middle of one.
        assert_eq!(fit("日本語テキスト", 7), "日本語…");
        assert_eq!(fit("日本語テキスト", 6), "日本…");
        for w in 0..16 {
            assert!(width(&fit("a日b本c語d", w)) <= w);
        }
        assert_eq!(fit("a\tb", 3), "a b");
    }
}
