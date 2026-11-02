//! A scrollable list with a selection: the state behind the search results and the file finder.
//! Pure (no drawing), so selection, scrolling and hit-testing are unit-tested.

#[derive(Clone, Debug, Default)]
pub struct ListState {
    /// Rows that can be selected (false for file headers).
    pub selectable: Vec<bool>,
    pub sel: usize,
    /// First visible row.
    pub top: usize,
}

impl ListState {
    pub fn new(selectable: Vec<bool>) -> ListState {
        let mut l = ListState { selectable, sel: 0, top: 0 };
        l.sel = l.first_selectable().unwrap_or(0);
        l
    }

    /// A list where every row can be selected.
    pub fn plain(len: usize) -> ListState {
        ListState::new(vec![true; len])
    }

    pub fn len(&self) -> usize {
        self.selectable.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selectable.is_empty()
    }

    fn first_selectable(&self) -> Option<usize> {
        self.selectable.iter().position(|&s| s)
    }

    fn last_selectable(&self) -> Option<usize> {
        self.selectable.iter().rposition(|&s| s)
    }

    /// Nearest selectable row at or after `i` going down (or up).
    fn seek(&self, i: usize, down: bool) -> Option<usize> {
        if down {
            (i..self.len()).find(|&j| self.selectable[j])
        } else if self.is_empty() {
            None
        } else {
            (0..=i.min(self.len() - 1)).rev().find(|&j| self.selectable[j])
        }
    }

    /// Move the selection by `delta` selectable rows (negative = up), stopping at the ends.
    pub fn step(&mut self, delta: isize, height: usize) {
        for _ in 0..delta.unsigned_abs() {
            let next = if delta > 0 { self.seek(self.sel + 1, true) } else if self.sel > 0 { self.seek(self.sel - 1, false) } else { None };
            match next {
                Some(n) => self.sel = n,
                None => break,
            }
        }
        self.reveal(height);
    }

    /// Move by a page of rows.
    pub fn page(&mut self, down: bool, height: usize) {
        let n = height.saturating_sub(1).max(1);
        let target = if down { (self.sel + n).min(self.len().saturating_sub(1)) } else { self.sel.saturating_sub(n) };
        self.sel = self.seek(target, down).or_else(|| self.seek(target, !down)).unwrap_or(self.sel);
        self.reveal(height);
    }

    pub fn home(&mut self, height: usize) {
        self.sel = self.first_selectable().unwrap_or(0);
        self.top = 0;
        self.reveal(height);
    }

    pub fn end(&mut self, height: usize) {
        self.sel = self.last_selectable().unwrap_or(0);
        self.reveal(height);
    }

    /// Scroll the window so the selection is visible. A header just above the selection is kept in
    /// view too, so each file's name shows with its first match.
    pub fn reveal(&mut self, height: usize) {
        let h = height.max(1);
        let mut want = self.sel;
        if want > 0 && !self.selectable[want - 1] {
            want -= 1;
        }
        if want < self.top {
            self.top = want;
        } else if self.sel >= self.top + h {
            self.top = self.sel + 1 - h;
        }
        self.top = self.top.min(self.len().saturating_sub(h));
    }

    /// Which row is `row_in_list` rows below the top of the list area? None for padding rows and
    /// rows that cannot be selected.
    pub fn hit(&self, row_in_list: usize, height: usize) -> Option<usize> {
        if row_in_list >= height {
            return None;
        }
        let i = self.top + row_in_list;
        (i < self.len() && self.selectable[i]).then_some(i)
    }

    /// Select a row by hit-test result.
    pub fn select(&mut self, i: usize, height: usize) {
        if i < self.len() && self.selectable[i] {
            self.sel = i;
            self.reveal(height);
        }
    }

    /// Replace the rows (the finder narrowing): selection back to the first selectable row.
    pub fn reset(&mut self, selectable: Vec<bool>) {
        *self = ListState::new(selectable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// header, 2 matches, header, 3 matches
    fn grouped() -> ListState {
        ListState::new(vec![false, true, true, false, true, true, true])
    }

    #[test]
    fn starts_on_the_first_selectable_row() {
        assert_eq!(grouped().sel, 1);
        assert_eq!(ListState::plain(3).sel, 0);
        assert_eq!(ListState::plain(0).sel, 0);
    }

    #[test]
    fn arrows_skip_headers_and_stop_at_the_ends() {
        let mut l = grouped();
        l.step(1, 10);
        assert_eq!(l.sel, 2);
        l.step(1, 10);
        assert_eq!(l.sel, 4, "the header row is skipped");
        l.step(-1, 10);
        assert_eq!(l.sel, 2);
        l.step(-5, 10);
        assert_eq!(l.sel, 1, "stops at the first match");
        l.step(50, 10);
        assert_eq!(l.sel, 6);
    }

    #[test]
    fn scrolls_to_keep_the_selection_visible() {
        let mut l = ListState::plain(100);
        l.step(10, 5);
        assert_eq!((l.sel, l.top), (10, 6));
        l.step(-9, 5);
        assert_eq!((l.sel, l.top), (1, 1));
        l.end(5);
        assert_eq!((l.sel, l.top), (99, 95));
        l.home(5);
        assert_eq!((l.sel, l.top), (0, 0));
    }

    #[test]
    fn the_header_above_the_selection_stays_visible() {
        let mut l = grouped();
        l.top = 1;
        l.sel = 1;
        l.reveal(3);
        assert_eq!(l.top, 0);
    }

    #[test]
    fn pages_move_by_height_minus_one() {
        let mut l = ListState::plain(100);
        l.page(true, 10);
        assert_eq!(l.sel, 9);
        l.page(true, 10);
        assert_eq!(l.sel, 18);
        l.page(false, 10);
        assert_eq!(l.sel, 9);
        let mut g = grouped();
        g.page(true, 3);
        assert_eq!(g.sel, 3 + 1, "lands on a selectable row");
    }

    #[test]
    fn hit_testing_follows_the_scroll_offset() {
        let mut l = grouped();
        l.top = 3;
        assert_eq!(l.hit(0, 4), None, "a header is not clickable");
        assert_eq!(l.hit(1, 4), Some(4));
        assert_eq!(l.hit(3, 4), Some(6));
        assert_eq!(l.hit(4, 4), None, "outside the list area");
        l.top = 4;
        assert_eq!(l.hit(3, 4), None, "past the last row");
        l.select(5, 4);
        assert_eq!(l.sel, 5);
        l.select(3, 4);
        assert_eq!(l.sel, 5, "selecting a header does nothing");
    }

    #[test]
    fn empty_lists_are_safe() {
        let mut l = ListState::plain(0);
        l.step(1, 5);
        l.page(true, 5);
        l.end(5);
        l.home(5);
        assert_eq!(l.hit(0, 5), None);
    }
}
