//! Text selection on the screen: positions, which cells are selected, and the selected text.

/// (row, col), 0-based, relative to the area being selected in.
pub type Pos = (u16, u16);
pub type Sel = (Pos, Pos);

/// Put the two ends in reading order. Tuples compare row first, then column.
pub fn normalize(a: Pos, b: Pos) -> Sel {
    if a <= b { (a, b) } else { (b, a) }
}

pub fn contains(sel: Sel, row: u16, col: u16) -> bool {
    (row, col) >= sel.0 && (row, col) <= sel.1
}

/// Marks the right half of a wide character; it contributes no text of its own.
const CONT: &str = "\u{0}";

fn is_word(c: &str) -> bool {
    c == CONT
        || (!c.is_empty() && !c.chars().all(|ch| ch.is_whitespace() || "\"'`()[]{}<>,;|".contains(ch)))
}

/// Start and end column of the word under `col` (a run of non-space, non-bracket characters).
pub fn word_bounds(cells: &[String], col: usize) -> (u16, u16) {
    let col = col.min(cells.len().saturating_sub(1));
    if cells.is_empty() || !is_word(&cells[col]) {
        return (col as u16, col as u16);
    }
    let (mut a, mut b) = (col, col);
    while a > 0 && is_word(&cells[a - 1]) {
        a -= 1;
    }
    while b + 1 < cells.len() && is_word(&cells[b + 1]) {
        b += 1;
    }
    (a as u16, b as u16)
}

/// Text of the cells of one row, columns `c1..=c2`. Empty cells are spaces; the right-hand
/// padding is dropped unless `keep_tail` (the row continues on the next line).
pub fn row_text(cells: &[String], c1: usize, c2: usize, keep_tail: bool) -> String {
    let mut s = String::new();
    for c in cells.iter().take(c2 + 1).skip(c1) {
        if c == CONT {
            continue;
        }
        s.push_str(if c.is_empty() { " " } else { c });
    }
    if keep_tail { s } else { s.trim_end().to_string() }
}

/// Join selected rows into text. `rows[i]` is the cell contents of row `first_row + i`;
/// `wrapped[i]` says row i is soft-wrapped into the next, so no newline is added there.
pub fn join_rows(sel: Sel, rows: &[Vec<String>], wrapped: &[bool], first_row: u16) -> String {
    let (s, e) = sel;
    let mut out = String::new();
    for row in s.0..=e.0 {
        let Some(cells) = rows.get((row - first_row) as usize) else { break };
        let c1 = if row == s.0 { s.1 as usize } else { 0 };
        let c2 = if row == e.0 { e.1 as usize } else { cells.len().saturating_sub(1) };
        let is_wrapped = wrapped.get((row - first_row) as usize).copied().unwrap_or(false);
        out.push_str(&row_text(cells, c1, c2, is_wrapped && row != e.0));
        if row != e.0 && !is_wrapped {
            out.push('\n');
        }
    }
    out
}

/// Cell contents of every row of a vt100 screen, plus which rows are soft-wrapped.
pub fn screen_rows(screen: &vt100::Screen) -> (Vec<Vec<String>>, Vec<bool>) {
    let (rows, cols) = (screen.size().0.get(), screen.size().1.get());
    let mut all = Vec::with_capacity(rows as usize);
    let mut wrapped = Vec::with_capacity(rows as usize);
    for r in 0..rows {
        let mut row = Vec::with_capacity(cols as usize);
        for c in 0..cols {
            match screen.cell(r, c) {
                Some(cell) if cell.is_wide_continuation() => row.push(CONT.to_string()),
                Some(cell) => row.push(cell.contents().to_string()),
                None => row.push(String::new()),
            }
        }
        all.push(row);
        wrapped.push(screen.row_wrapped(r));
    }
    (all, wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(s: &str) -> Vec<String> {
        s.chars().map(|c| c.to_string()).collect()
    }

    #[test]
    fn normalizes_and_contains() {
        assert_eq!(normalize((3, 5), (1, 2)), ((1, 2), (3, 5)));
        let sel = ((1, 4), (2, 2));
        assert!(contains(sel, 1, 4) && contains(sel, 1, 79) && contains(sel, 2, 0) && contains(sel, 2, 2));
        assert!(!contains(sel, 1, 3) && !contains(sel, 2, 3) && !contains(sel, 0, 9));
    }

    #[test]
    fn word_under_the_cursor() {
        let c = cells("see src/main.rs:12 now");
        assert_eq!(word_bounds(&c, 6), (4, 17));
        assert_eq!(word_bounds(&c, 0), (0, 2));
        assert_eq!(word_bounds(&c, 3), (3, 3), "a space selects just itself");
        let q = cells("open \"a b.txt\" ok");
        assert_eq!(word_bounds(&q, 8), (8, 12), "the quotes end the word");
    }

    #[test]
    fn single_row_selection_trims_padding() {
        let rows = vec![cells("hello world     ")];
        assert_eq!(join_rows(((0, 0), (0, 14)), &rows, &[false], 0), "hello world");
        assert_eq!(join_rows(((0, 6), (0, 10)), &rows, &[false], 0), "world");
    }

    #[test]
    fn multi_row_selection_joins_lines_and_wrapped_rows() {
        let rows = vec![cells("abcdef"), cells("ghi   "), cells("jkl   ")];
        assert_eq!(join_rows(((0, 2), (2, 1)), &rows, &[false, false, false], 0), "cdef\nghi\njk");
        // row 0 is soft-wrapped into row 1: they join without a newline
        assert_eq!(join_rows(((0, 2), (1, 2)), &rows, &[true, false, false], 0), "cdefghi");
    }

    #[test]
    fn reads_the_vt100_screen() {
        let mut p = vt100::Parser::new(std::num::NonZeroU16::new(3).unwrap(), std::num::NonZeroU16::new(10).unwrap(), 0);
        p.process(b"hello\r\nwide: \xe6\x97\xa5x");
        let (rows, wrapped) = screen_rows(p.screen());
        assert_eq!(join_rows(((0, 0), (1, 9)), &rows, &wrapped, 0), "hello\nwide: 日x");
    }
}
