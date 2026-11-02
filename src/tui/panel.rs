//! A titled pane: one coloured header row (optional right-aligned text), a body, and an optional
//! footer row inside the pane. No box drawing; `divider` draws a single column between panes.

use super::{fill, first_row, fit, put, width};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

#[derive(Clone, Debug, Default)]
pub struct Panel {
    pub title: String,
    pub right: Option<String>,
    pub footer: Option<String>,
}

/// Where the parts of a panel landed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelAreas {
    pub header: Rect,
    pub body: Rect,
    pub footer: Option<Rect>,
}

impl Panel {
    pub fn new(title: impl Into<String>) -> Self {
        Panel { title: title.into(), right: None, footer: None }
    }
    pub fn right(mut self, r: impl Into<String>) -> Self {
        self.right = Some(r.into());
        self
    }
    pub fn footer(mut self, f: impl Into<String>) -> Self {
        self.footer = Some(f.into());
        self
    }

    /// Rectangles only. The footer needs a header, a body row and itself (height >= 3).
    pub fn layout(&self, area: Rect) -> PanelAreas {
        let header = Rect::new(area.x, area.y, area.width, area.height.min(1));
        let has_footer = self.footer.is_some() && area.height >= 3;
        let footer = has_footer.then(|| Rect::new(area.x, area.y + area.height - 1, area.width, 1));
        let body_h = area.height.saturating_sub(1 + u16::from(has_footer));
        PanelAreas { header, body: Rect::new(area.x, area.y + header.height, area.width, body_h), footer }
    }

    /// Paints the canvas, header and footer; the caller draws into `body`.
    pub fn render(&self, area: Rect, buf: &mut Buffer, t: &Theme) -> PanelAreas {
        let areas = self.layout(area);
        fill(buf, area, Style::default().fg(t.fg).bg(t.bg));
        if let Some(h) = first_row(areas.header, buf) {
            let st = Style::default().fg(t.badge_fg).bg(t.badge_bg).add_modifier(Modifier::BOLD);
            fill(buf, h, st);
            let inner = if h.width >= 3 { Rect::new(h.x + 1, h.y, h.width - 2, 1) } else { h };
            let w = inner.width as usize;
            let right = self.right.as_deref().map(|r| fit(r, w / 2)).unwrap_or_default();
            let rw = width(&right);
            put(buf, inner.x, inner.y, &self.title, w.saturating_sub(rw + usize::from(rw > 0)), st);
            if rw > 0 {
                put(buf, inner.x + (w - rw) as u16, inner.y, &right, rw, st);
            }
        }
        if let (Some(f), Some(text)) = (areas.footer, &self.footer) {
            let st = Style::default().fg(t.dim).bg(t.bg);
            let inner = if f.width >= 2 { Rect::new(f.x + 1, f.y, f.width - 1, 1) } else { f };
            put(buf, inner.x, inner.y, text, inner.width as usize, st);
        }
        areas
    }
}

/// A one-column divider (`│`) down `area` (uses its first column).
pub fn divider(area: Rect, buf: &mut Buffer, t: &Theme, focused: bool) {
    let a = area.intersection(buf.area);
    let col = if focused { t.divider_focus } else { t.divider_idle };
    for y in a.y..a.y + a.height {
        if area.x >= a.x && area.x < a.x + a.width {
            let cell = &mut buf[(area.x, y)];
            cell.set_symbol("│");
            cell.set_style(Style::default().fg(col).bg(t.bg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DARK, LIGHT};

    #[test]
    fn layout_gives_header_body_footer() {
        let p = Panel::new("FILES").footer("1/2");
        let a = p.layout(Rect::new(2, 3, 20, 10));
        assert_eq!(a.header, Rect::new(2, 3, 20, 1));
        assert_eq!(a.body, Rect::new(2, 4, 20, 8));
        assert_eq!(a.footer, Some(Rect::new(2, 12, 20, 1)));
        let a = Panel::new("X").layout(Rect::new(0, 0, 5, 4));
        assert_eq!(a.body.height, 3);
        assert_eq!(a.footer, None);
        let a = p.layout(Rect::new(0, 0, 5, 2));
        assert_eq!((a.body.height, a.footer), (1, None));
        let a = p.layout(Rect::new(0, 0, 5, 0));
        assert_eq!((a.header.height, a.body.height), (0, 0));
    }

    #[test]
    fn render_header_and_footer_text() {
        for t in [DARK, LIGHT] {
            let mut b = Buffer::empty(Rect::new(0, 0, 24, 5));
            Panel::new("BRANCHES").right("main").footer("2/6 · wrap").render(b.area, &mut b, &t);
            let row = |y: u16| (0..24).map(|x| b[(x, y)].symbol().to_string()).collect::<String>();
            assert_eq!(row(0), " BRANCHES          main ");
            assert_eq!(row(4).trim_end(), " 2/6 · wrap");
            assert_eq!(b[(1, 0)].style().bg, Some(t.badge_bg));
            assert_eq!(b[(1, 4)].style().fg, Some(t.dim));
        }
    }

    #[test]
    fn divider_draws_a_column() {
        let mut b = Buffer::empty(Rect::new(0, 0, 5, 3));
        divider(Rect::new(2, 0, 1, 3), &mut b, &DARK, true);
        assert_eq!(b[(2, 1)].symbol(), "│");
        assert_eq!(b[(2, 1)].style().fg, Some(DARK.divider_focus));
        assert_eq!(b[(1, 1)].symbol(), " ");
    }
}
