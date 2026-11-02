//! A list of rows split into aligned columns, with a header row, a full-row selection highlight,
//! scrolling that keeps the selection visible, and mouse hit-testing. The widget holds only view
//! state; the caller passes the rows on every render.

use super::{fill, fit, put, width};
use crate::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use std::cell::{Cell as StdCell, RefCell};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    /// Takes the space left over (shared equally between fill columns).
    Fill,
    Fixed(u16),
    /// As wide as its content, but at least this many cells; shrinks to the minimum when short of room.
    Min(u16),
}

#[derive(Clone, Debug)]
pub struct Column {
    pub title: String,
    pub width: Width,
    pub align: Align,
}

impl Column {
    pub fn new(title: impl Into<String>, width: Width) -> Self {
        Column { title: title.into(), width, align: Align::Left }
    }
    pub fn right(mut self) -> Self {
        self.align = Align::Right;
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub text: String,
    /// Patched over the row style (a colour for a git mark or a size). Ignored colours on the
    /// selected row: the selection row colours win so the row stays readable.
    pub style: Option<Style>,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Self {
        Cell { text: text.into(), style: None }
    }
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Cell { text: text.into(), style: Some(style) }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Row {
    /// Indentation and tree glyphs in front of the first cell (kept when the cell is cut).
    pub prefix: String,
    pub prefix_style: Option<Style>,
    pub cells: Vec<Cell>,
}

impl Row {
    pub fn new(cells: Vec<Cell>) -> Self {
        Row { prefix: String::new(), prefix_style: None, cells }
    }
    pub fn with_prefix(mut self, prefix: impl Into<String>, style: Option<Style>) -> Self {
        self.prefix = prefix.into();
        self.prefix_style = style;
        self
    }
}

pub const GAP: u16 = 2;

/// Column widths for `avail` cells. `content[i]` is the widest content of column i (used by Min).
pub fn layout_widths(columns: &[Column], gap: u16, avail: u16, content: &[usize]) -> Vec<u16> {
    let n = columns.len();
    let gaps = gap * n.saturating_sub(1) as u16;
    let avail = avail.saturating_sub(gaps);
    let mut w: Vec<u16> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| match c.width {
            Width::Fixed(f) => f,
            Width::Min(m) => (content.get(i).copied().unwrap_or(0).min(u16::MAX as usize) as u16).max(m),
            Width::Fill => 0,
        })
        .collect();
    let used: u16 = w.iter().sum();
    let fills: Vec<usize> = (0..n).filter(|i| columns[*i].width == Width::Fill).collect();
    if used > avail {
        // Short of room: Min columns shrink to their minimum first, then fixed ones from the end.
        let mut deficit = used - avail;
        for i in (0..n).rev() {
            if let Width::Min(m) = columns[i].width {
                let cut = w[i].saturating_sub(m).min(deficit);
                w[i] -= cut;
                deficit -= cut;
            }
        }
        for i in (0..n).rev() {
            let cut = w[i].min(deficit);
            w[i] -= cut;
            deficit -= cut;
        }
    } else if !fills.is_empty() {
        let left = avail - used;
        let each = left / fills.len() as u16;
        let extra = (left % fills.len() as u16) as usize;
        for (k, i) in fills.iter().enumerate() {
            w[*i] = each + u16::from(k < extra);
        }
    }
    w
}

pub struct ColumnList {
    pub columns: Vec<Column>,
    /// Selected row index (kept within the rows on every operation).
    pub selected: usize,
    /// Up on the first row goes to the last and the other way round.
    pub wrap: bool,
    pub show_header: bool,
    top: StdCell<usize>,
    page: StdCell<usize>,
    len: StdCell<usize>,
    body: StdCell<Rect>,
    head: StdCell<Rect>,
    spans: RefCell<Vec<(u16, u16)>>,
}

impl ColumnList {
    pub fn new(columns: Vec<Column>) -> Self {
        ColumnList {
            columns,
            selected: 0,
            wrap: true,
            show_header: true,
            top: StdCell::new(0),
            page: StdCell::new(10),
            len: StdCell::new(0),
            body: StdCell::new(Rect::default()),
            head: StdCell::new(Rect::default()),
            spans: RefCell::new(Vec::new()),
        }
    }

    pub fn select(&mut self, i: usize, len: usize) {
        self.selected = i.min(len.saturating_sub(1));
    }
    pub fn up(&mut self, len: usize) {
        if len == 0 {
            return;
        }
        self.selected = if self.selected == 0 {
            if self.wrap { len - 1 } else { 0 }
        } else {
            self.selected.min(len) - 1
        };
    }
    pub fn down(&mut self, len: usize) {
        if len == 0 {
            return;
        }
        self.selected = if self.selected + 1 >= len {
            if self.wrap { 0 } else { len - 1 }
        } else {
            self.selected + 1
        };
    }
    pub fn page_up(&mut self, len: usize) {
        self.selected = self.selected.min(len.saturating_sub(1)).saturating_sub(self.page.get().max(1));
    }
    pub fn page_down(&mut self, len: usize) {
        self.selected = (self.selected + self.page.get().max(1)).min(len.saturating_sub(1));
    }
    pub fn home(&mut self) {
        self.selected = 0;
    }
    pub fn end(&mut self, len: usize) {
        self.selected = len.saturating_sub(1);
    }
    /// Mouse wheel: scroll the view by `delta` rows; the selection stays on screen.
    pub fn scroll(&mut self, delta: isize, len: usize) {
        let h = self.page.get().max(1);
        let max_top = len.saturating_sub(h);
        let top = (self.top.get() as isize + delta).clamp(0, max_top as isize) as usize;
        self.top.set(top);
        self.selected = self.selected.clamp(top, (top + h - 1).min(len.saturating_sub(1)));
    }

    /// `12/58 · up/down wrap · left/right fold`; `extra` is the part after the count.
    pub fn footer(&self, len: usize, extra: &str) -> String {
        let pos = if len == 0 { 0 } else { self.selected.min(len - 1) + 1 };
        if extra.is_empty() { format!("{pos}/{len}") } else { format!("{pos}/{len} · {extra}") }
    }

    fn eff_top(&self, len: usize, h: usize) -> usize {
        let h = h.max(1);
        let sel = self.selected.min(len.saturating_sub(1));
        let mut top = self.top.get();
        if sel < top {
            top = sel;
        } else if sel >= top + h {
            top = sel + 1 - h;
        }
        top.min(len.saturating_sub(h))
    }

    /// Row under a screen cell (from the last render); None on the header or past the rows.
    pub fn hit_test(&self, col: u16, row: u16) -> Option<usize> {
        let b = self.body.get();
        if col < b.x || col >= b.x + b.width || row < b.y || row >= b.y + b.height {
            return None;
        }
        let i = self.top.get() + (row - b.y) as usize;
        (i < self.len.get()).then_some(i)
    }
    /// Column under a header cell (for clicking a column to sort).
    pub fn hit_header(&self, col: u16, row: u16) -> Option<usize> {
        let h = self.head.get();
        if !self.show_header || row != h.y || h.height == 0 || col < h.x || col >= h.x + h.width {
            return None;
        }
        self.spans.borrow().iter().position(|(x, w)| col >= *x && col < x + w)
    }
    /// Column widths of the last render.
    pub fn widths(&self) -> Vec<u16> {
        self.spans.borrow().iter().map(|s| s.1).collect()
    }

    pub fn render(&self, rows: &[Row], area: Rect, buf: &mut Buffer, t: &Theme) {
        let a = area.intersection(buf.area);
        self.len.set(rows.len());
        self.body.set(Rect::default());
        self.head.set(Rect::default());
        self.spans.borrow_mut().clear();
        if a.width == 0 || a.height == 0 {
            return;
        }
        let base = Style::default().fg(t.fg).bg(t.bg);
        fill(buf, a, base);
        let head_h = u16::from(self.show_header);
        let body = Rect::new(a.x, a.y + head_h.min(a.height), a.width, a.height.saturating_sub(head_h));
        let h = body.height as usize;
        self.page.set(h.max(1));
        let top = if rows.is_empty() { 0 } else { self.eff_top(rows.len(), h) };
        self.top.set(top);
        let visible = &rows[top.min(rows.len())..(top + h).min(rows.len())];

        // Widths: Min columns follow the visible content and the header.
        let mut content: Vec<usize> = self.columns.iter().map(|c| width(&c.title)).collect();
        for r in visible {
            for (i, c) in r.cells.iter().enumerate().take(content.len()) {
                let w = width(&c.text) + if i == 0 { width(&r.prefix) } else { 0 };
                content[i] = content[i].max(w);
            }
        }
        let widths = layout_widths(&self.columns, GAP, a.width, &content);
        let mut x = a.x;
        let mut spans = Vec::with_capacity(widths.len());
        for w in &widths {
            spans.push((x, *w));
            x = x.saturating_add(*w + GAP);
        }
        *self.spans.borrow_mut() = spans.clone();

        if self.show_header {
            let hs = Style::default().fg(t.col_header_fg).bg(t.col_header_bg).add_modifier(Modifier::BOLD);
            let hr = Rect::new(a.x, a.y, a.width, 1);
            fill(buf, hr, hs);
            self.head.set(hr);
            for (c, (x, w)) in self.columns.iter().zip(&spans) {
                put_aligned(buf, *x, a.y, *w, &c.title, c.align, hs);
            }
        }
        self.body.set(body);

        for (k, r) in visible.iter().enumerate() {
            let y = body.y + k as u16;
            let sel = top + k == self.selected.min(rows.len() - 1);
            let row_style = if sel { Style::default().fg(t.row_sel_fg).bg(t.row_sel_bg) } else { base };
            fill(buf, Rect::new(a.x, y, a.width, 1), row_style);
            for (i, (c, (x, w))) in self.columns.iter().zip(&spans).enumerate() {
                let w = *w;
                if w == 0 || *x >= a.x + a.width {
                    continue;
                }
                let empty = Cell::default();
                let cell = r.cells.get(i).unwrap_or(&empty);
                let cell_style = |s: Option<Style>| match (s, sel) {
                    (Some(s), true) => row_style.add_modifier(s.add_modifier),
                    (Some(s), false) => row_style.patch(s),
                    (None, _) => row_style,
                };
                let st = cell_style(cell.style);
                if i == 0 && !r.prefix.is_empty() {
                    let pw = width(&r.prefix);
                    if pw >= w as usize {
                        put(buf, *x, y, &r.prefix, w as usize, cell_style(r.prefix_style));
                    } else {
                        put(buf, *x, y, &r.prefix, pw, cell_style(r.prefix_style));
                        put_aligned(buf, *x + pw as u16, y, w - pw as u16, &cell.text, c.align, st);
                    }
                } else {
                    put_aligned(buf, *x, y, w, &cell.text, c.align, st);
                }
            }
        }
    }
}

fn put_aligned(buf: &mut Buffer, x: u16, y: u16, w: u16, text: &str, align: Align, st: Style) {
    let text = fit(text, w as usize);
    let pad = if align == Align::Right { w as usize - width(&text) } else { 0 };
    put(buf, x + pad as u16, y, &text, w as usize - pad, st);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DARK, LIGHT};

    fn cols() -> Vec<Column> {
        vec![Column::new("FILES", Width::Fill), Column::new("SIZE", Width::Min(4)).right(), Column::new("MODIFIED", Width::Fixed(8))]
    }
    fn rows(n: usize) -> Vec<Row> {
        (0..n).map(|i| Row::new(vec![Cell::new(format!("file{i}")), Cell::new(format!("{}", i * 10)), Cell::new("today")])).collect()
    }
    use crate::tui::row_text as line;

    #[test]
    fn width_rules() {
        let w = layout_widths(&cols(), 2, 40, &[5, 6, 8]);
        assert_eq!(w, vec![40 - 4 - 6 - 8, 6, 8]);
        // Min never goes below its minimum while there is room; Fill takes the rest.
        assert_eq!(layout_widths(&cols(), 2, 40, &[5, 1, 8])[1], 4);
        // Too narrow: Min shrinks to its minimum, then the tail columns give way; fill gets 0.
        let w = layout_widths(&cols(), 2, 20, &[5, 9, 8]);
        assert_eq!(w.iter().sum::<u16>() + 4, 20);
        assert_eq!(w[0], 0);
        let w = layout_widths(&cols(), 2, 5, &[5, 9, 8]);
        assert!(w.iter().sum::<u16>() <= 1);
        // Two fills share evenly, the first gets the odd cell.
        let two = vec![Column::new("A", Width::Fill), Column::new("B", Width::Fill)];
        assert_eq!(layout_widths(&two, 2, 11, &[0, 0]), vec![5, 4]);
    }

    #[test]
    fn renders_header_rows_and_right_aligned_numbers() {
        let l = ColumnList::new(cols());
        let mut b = Buffer::empty(Rect::new(0, 0, 30, 6));
        l.render(&rows(3), b.area, &mut b, &DARK);
        assert_eq!(line(&b, 0), "FILES           SIZE  MODIFIED");
        assert_eq!(line(&b, 1), "file0              0  today   ");
        assert_eq!(line(&b, 2), "file1             10  today   ");
        assert_eq!(b[(0, 0)].style().bg, Some(DARK.col_header_bg));
        assert_eq!(b[(0, 1)].style().bg, Some(DARK.row_sel_bg), "row 0 is selected");
        assert_eq!(b[(29, 1)].style().bg, Some(DARK.row_sel_bg), "highlight spans the full row");
        assert_eq!(b[(0, 2)].style().bg, Some(DARK.bg));
    }

    #[test]
    fn first_column_prefix_and_truncation_with_wide_chars() {
        let l = ColumnList::new(vec![Column::new("NAME", Width::Fixed(10)), Column::new("N", Width::Fixed(3)).right()]);
        let r = vec![
            Row::new(vec![Cell::new("日本語テキスト"), Cell::new("1")]).with_prefix("▾ ", None),
            Row::new(vec![Cell::new("abcdefghijklmnop"), Cell::new("22")]).with_prefix("    ▸ ", None),
        ];
        let mut b = Buffer::empty(Rect::new(0, 0, 20, 3));
        l.render(&r, b.area, &mut b, &LIGHT);
        let a = line(&b, 1);
        assert!(a.starts_with("▾ 日本語…"), "{a:?}");
        assert!(a.trim_end().ends_with('1'));
        assert_eq!(line(&b, 2).trim_end(), "    ▸ abc…   22");
    }

    #[test]
    fn selection_scrolls_and_wraps() {
        let mut l = ColumnList::new(cols());
        let data = rows(20);
        let mut b = Buffer::empty(Rect::new(0, 0, 30, 6)); // header + 5 rows
        l.render(&data, b.area, &mut b, &DARK);
        for _ in 0..7 {
            l.down(20);
        }
        l.render(&data, b.area, &mut b, &DARK);
        assert_eq!(l.selected, 7);
        assert!(line(&b, 1).starts_with("file3"), "{}", line(&b, 1));
        assert!(line(&b, 5).starts_with("file7"));
        // Up never scrolls past the selection; wrap goes to the end and back.
        l.select(0, 20);
        l.up(20);
        assert_eq!(l.selected, 19);
        l.render(&data, b.area, &mut b, &DARK);
        assert!(line(&b, 5).starts_with("file19"));
        l.down(20);
        assert_eq!(l.selected, 0);
        l.wrap = false;
        l.up(20);
        assert_eq!(l.selected, 0);
        l.end(20);
        l.down(20);
        assert_eq!(l.selected, 19);
        l.home();
        l.page_down(20);
        assert_eq!(l.selected, 5);
        l.page_down(20);
        l.page_down(20);
        l.page_down(20);
        assert_eq!(l.selected, 19);
        l.page_up(20);
        assert_eq!(l.selected, 14);
        l.up(0);
        l.down(0);
        // Wheel keeps the selection inside the view.
        l.scroll(-100, 20);
        assert!(l.selected < 5);
        assert_eq!(l.footer(20, "up/down wrap"), format!("{}/20 · up/down wrap", l.selected + 1));
        assert_eq!(l.footer(0, ""), "0/0");
    }

    #[test]
    fn hit_testing_rows_and_header() {
        let l = ColumnList::new(cols());
        let data = rows(3);
        let mut b = Buffer::empty(Rect::new(0, 0, 40, 10));
        l.render(&data, Rect::new(5, 2, 30, 8), &mut b, &DARK);
        assert_eq!(l.hit_test(5, 3), Some(0));
        assert_eq!(l.hit_test(34, 5), Some(2));
        assert_eq!(l.hit_test(5, 6), None, "past the last row");
        assert_eq!(l.hit_test(5, 2), None, "header is not a row");
        assert_eq!(l.hit_test(35, 3), None);
        assert_eq!(l.hit_test(4, 3), None);
        let w = l.widths();
        assert_eq!(l.hit_header(5, 2), Some(0));
        assert_eq!(l.hit_header(5 + w[0] + 2 + 1, 2), Some(1));
        assert_eq!(l.hit_header(5 + w[0], 2), None, "gap between columns");
        assert_eq!(l.hit_header(34, 2), Some(2));
        assert_eq!(l.hit_header(5, 3), None);
        let mut l2 = ColumnList::new(cols());
        l2.show_header = false;
        l2.render(&data, Rect::new(0, 0, 30, 4), &mut b, &DARK);
        assert_eq!(l2.hit_test(0, 0), Some(0));
        assert_eq!(l2.hit_header(0, 0), None);
    }

    #[test]
    fn cell_styles_apply_except_on_the_selected_row() {
        let l = ColumnList::new(cols());
        let hot = Style::default().fg(DARK.hot);
        let data = vec![
            Row::new(vec![Cell::new("a"), Cell::styled("9", hot), Cell::new("x")]),
            Row::new(vec![Cell::new("b"), Cell::styled("9", hot), Cell::new("x")]),
        ];
        let mut b = Buffer::empty(Rect::new(0, 0, 30, 3));
        l.render(&data, b.area, &mut b, &DARK);
        let sx = l.widths()[0] + 2 + 3; // the number sits at the right edge of the SIZE column
        assert_eq!(b[(sx, 1)].style().fg, Some(DARK.row_sel_fg));
        assert_eq!(b[(sx, 2)].style().fg, Some(DARK.hot));
        assert_eq!(b[(sx, 2)].style().bg, Some(DARK.bg));
    }

    #[test]
    fn empty_and_tiny_areas_do_not_panic() {
        let l = ColumnList::new(cols());
        for (w, h) in [(0u16, 0u16), (1, 1), (3, 2), (30, 1)] {
            let mut b = Buffer::empty(Rect::new(0, 0, 30.max(w), 5.max(h)));
            l.render(&rows(4), Rect::new(0, 0, w, h), &mut b, &DARK);
            l.render(&[], Rect::new(0, 0, w, h), &mut b, &DARK);
        }
        assert_eq!(l.hit_test(0, 0), None);
    }
}
