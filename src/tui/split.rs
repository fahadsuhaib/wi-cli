//! Two panes with a one-column divider. Mirrors the shell/preview split in `src/tab.rs`, but
//! with its own minimums and collapse to a single pane on narrow areas.

use ratatui::layout::Rect;

pub const PCT_MIN: u16 = 15;
pub const PCT_MAX: u16 = 85;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub struct SplitView {
    /// Share of the width for the left pane, in percent.
    pub pct: u16,
    pub left_min: u16,
    pub right_min: u16,
    /// The pane that survives when the area is too narrow for both.
    pub prefer: Pane,
}

/// Rectangles of a laid-out split. A missing pane has a zero-width rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SplitLayout {
    pub left: Rect,
    pub divider: Rect,
    pub right: Rect,
    /// Some(pane) when only that pane is shown (it fills the whole area).
    pub single: Option<Pane>,
}

impl SplitView {
    pub fn new(pct: u16, left_min: u16, right_min: u16) -> Self {
        SplitView { pct, left_min, right_min, prefer: Pane::Left }
    }

    pub fn layout(&self, area: Rect) -> SplitLayout {
        let w = area.width;
        let empty = |x: u16| Rect::new(x, area.y, 0, area.height);
        if w < self.left_min.saturating_add(self.right_min).saturating_add(1) {
            return match self.prefer {
                Pane::Left => SplitLayout { left: area, divider: empty(area.x + w), right: empty(area.x + w), single: Some(Pane::Left) },
                Pane::Right => SplitLayout { left: empty(area.x), divider: empty(area.x), right: area, single: Some(Pane::Right) },
            };
        }
        let lw = (u32::from(w) * u32::from(self.pct.clamp(PCT_MIN, PCT_MAX)) / 100) as u16;
        let lw = lw.clamp(self.left_min, w - 1 - self.right_min);
        SplitLayout {
            left: Rect::new(area.x, area.y, lw, area.height),
            divider: Rect::new(area.x + lw, area.y, 1, area.height),
            right: Rect::new(area.x + lw + 1, area.y, w - lw - 1, area.height),
            single: None,
        }
    }

    /// The percentage that puts the divider at screen column `col` (for dragging).
    pub fn pct_for_col(&self, area: Rect, col: u16) -> u16 {
        let w = u32::from(area.width.max(1));
        let lw = u32::from(col.saturating_sub(area.x));
        (((lw * 100 + w / 2) / w) as u16).clamp(PCT_MIN, PCT_MAX)
    }
}

impl SplitLayout {
    pub fn on_divider(&self, col: u16, row: u16) -> bool {
        self.single.is_none() && col == self.divider.x && row >= self.divider.y && row < self.divider.y + self.divider.height
    }
    /// Which pane a cell belongs to (None on the divider or outside).
    pub fn pane_at(&self, col: u16, row: u16) -> Option<Pane> {
        let inside = |r: Rect| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height;
        if inside(self.left) {
            Some(Pane::Left)
        } else if inside(self.right) {
            Some(Pane::Right)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentage_and_minimums() {
        let s = SplitView::new(40, 20, 20);
        let l = s.layout(Rect::new(0, 0, 100, 10));
        assert_eq!((l.left.width, l.divider.x, l.right.x, l.right.width), (40, 40, 41, 59));
        assert_eq!(l.single, None);
        // A tiny percentage still leaves the left minimum; a huge one leaves the right minimum.
        let l = SplitView { pct: 1, ..s }.layout(Rect::new(0, 0, 100, 10));
        assert_eq!(l.left.width, 20);
        let l = SplitView { pct: 99, ..s }.layout(Rect::new(0, 0, 100, 10));
        assert_eq!(l.right.width, 20);
        let l = SplitView::new(50, 40, 40).layout(Rect::new(5, 2, 81, 10));
        assert_eq!((l.left.width, l.right.width), (40, 40));
    }

    #[test]
    fn collapses_to_the_preferred_pane() {
        let s = SplitView::new(40, 20, 20);
        let l = s.layout(Rect::new(0, 0, 40, 5));
        assert_eq!(l.single, Some(Pane::Left));
        assert_eq!(l.left, Rect::new(0, 0, 40, 5));
        assert_eq!(l.right.width, 0);
        let l = SplitView { prefer: Pane::Right, ..s }.layout(Rect::new(0, 0, 40, 5));
        assert_eq!((l.single, l.right.width, l.left.width), (Some(Pane::Right), 40, 0));
        assert!(!l.on_divider(0, 0));
        assert_eq!(s.layout(Rect::new(0, 0, 41, 5)).single, None);
    }

    #[test]
    fn divider_hits_and_drag_percentage() {
        let s = SplitView::new(40, 20, 20);
        let area = Rect::new(0, 1, 100, 10);
        let l = s.layout(area);
        assert!(l.on_divider(40, 1) && l.on_divider(40, 10));
        assert!(!l.on_divider(41, 1) && !l.on_divider(40, 0) && !l.on_divider(40, 11));
        assert_eq!(l.pane_at(10, 3), Some(Pane::Left));
        assert_eq!(l.pane_at(60, 3), Some(Pane::Right));
        assert_eq!(l.pane_at(40, 3), None);
        assert_eq!(s.pct_for_col(area, 40), 40);
        assert_eq!(s.pct_for_col(area, 0), PCT_MIN);
        assert_eq!(s.pct_for_col(area, 99), PCT_MAX);
        let dragged = SplitView { pct: s.pct_for_col(area, 55), ..s }.layout(area);
        assert_eq!(dragged.divider.x, 55);
    }
}
