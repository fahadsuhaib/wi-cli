//! Find in output: search a shell tab's scrollback and screen (everything the emulator still
//! holds, which `scrollback_lines` bounds). Pure functions over rows of cells, so a scripted
//! `vt100::Parser` can test them.

use super::{fold, smart_sensitive};

/// One match: absolute row (0 = oldest line still held) and columns `[start, end)`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hit {
    pub row: usize,
    pub start: u16,
    pub end: u16,
}

/// All matches of `needle` in the rows (cells per column, as `select::screen_rows` gives them),
/// oldest first. A match never spans rows.
pub fn find_in_rows(rows: &[Vec<String>], needle: &str) -> Vec<Hit> {
    let sens = smart_sensitive(needle);
    let n: Vec<char> = needle.chars().map(|c| fold(c, sens)).collect();
    let mut out = Vec::new();
    if n.is_empty() {
        return out;
    }
    for (r, cells) in rows.iter().enumerate() {
        // Text of the row with the column each character sits in (wide characters take two).
        let mut text: Vec<char> = Vec::new();
        let mut col_of: Vec<u16> = Vec::new();
        for (c, cell) in cells.iter().enumerate() {
            if cell == "\u{0}" {
                continue;
            }
            if cell.is_empty() {
                text.push(' ');
                col_of.push(c as u16);
            } else {
                for ch in cell.chars() {
                    text.push(fold(ch, sens));
                    col_of.push(c as u16);
                }
            }
        }
        for (s, len) in super::find_in_chars(&text, &n) {
            let start = col_of[s];
            let last = s + len - 1;
            let mut end = col_of[last] + 1;
            // A wide character at the end covers its continuation column too.
            if cells.get(end as usize).is_some_and(|c| c == "\u{0}") {
                end += 1;
            }
            out.push(Hit { row: r, start, end });
        }
    }
    out
}

/// Every row the emulator holds, oldest first (history, then the screen), as cells. The view's
/// scrollback offset is put back afterwards.
pub fn collect_rows(parser: &mut vt100::Parser) -> Vec<Vec<String>> {
    let saved = parser.screen().scrollback();
    let (rows, cols) = (parser.screen().size().0.get() as usize, parser.screen().size().1.get());
    parser.screen_mut().set_scrollback(usize::MAX);
    let hist = parser.screen().scrollback();
    let total = hist + rows;
    let mut out: Vec<Vec<String>> = vec![Vec::new(); total];
    // The view at offset `o` shows absolute rows `hist - o .. hist - o + rows`.
    let mut o = hist;
    loop {
        parser.screen_mut().set_scrollback(o);
        let eff = parser.screen().scrollback();
        let base = hist - eff;
        for r in 0..rows {
            let mut row = Vec::with_capacity(cols as usize);
            for c in 0..cols {
                match parser.screen().cell(r as u16, c) {
                    Some(cell) if cell.is_wide_continuation() => row.push("\u{0}".to_string()),
                    Some(cell) => row.push(cell.contents().to_string()),
                    None => row.push(String::new()),
                }
            }
            out[base + r] = row;
        }
        if o == 0 {
            break;
        }
        o = o.saturating_sub(rows);
    }
    parser.screen_mut().set_scrollback(saved);
    out
}

/// The scrollback offset that puts absolute row `row` about a third of the way down the view.
pub fn offset_for(total_rows: usize, view_rows: usize, history: usize, row: usize) -> usize {
    let top = row.saturating_sub(view_rows / 3);
    // top of the view at offset o is `history - o`
    history.saturating_sub(top).min(history).min(total_rows)
}

/// State of "find in output" while the user steps through matches.
pub struct OutFind {
    /// Oldest first.
    pub hits: Vec<Hit>,
    /// Index into `hits`.
    pub cur: usize,
    pub history: usize,
    pub view_rows: usize,
}

impl OutFind {
    /// Search a parser's history and screen. The newest match is current.
    pub fn new(parser: &mut vt100::Parser, needle: &str) -> OutFind {
        let rows = collect_rows(parser);
        let hits = find_in_rows(&rows, needle);
        let view_rows = parser.screen().size().0.get() as usize;
        let history = rows.len().saturating_sub(view_rows);
        OutFind { cur: hits.len().saturating_sub(1), hits, history, view_rows }
    }

    /// Next match going back in time (`N`).
    pub fn older(&mut self) {
        if self.cur > 0 {
            self.cur -= 1;
        }
    }

    /// Next match going forward (`B`).
    pub fn newer(&mut self) {
        if self.cur + 1 < self.hits.len() {
            self.cur += 1;
        }
    }

    /// `match 3 of 17`, numbered from the newest (1 = newest).
    pub fn label(&self) -> String {
        format!("match {} of {}", self.hits.len() - self.cur, self.hits.len())
    }

    /// Scrollback offset that shows the current match.
    pub fn offset(&self) -> usize {
        self.hits.get(self.cur).map_or(0, |h| offset_for(self.history + self.view_rows, self.view_rows, self.history, h.row))
    }

    /// Matches inside the view at scrollback `offset`: (view row, start col, end col, is current).
    pub fn in_view(&self, offset: usize) -> Vec<(u16, u16, u16, bool)> {
        let top = self.history.saturating_sub(offset);
        self.hits
            .iter()
            .enumerate()
            .filter(|(_, h)| h.row >= top && h.row < top + self.view_rows)
            .map(|(i, h)| ((h.row - top) as u16, h.start, h.end, i == self.cur))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU16;

    fn cells(s: &str) -> Vec<String> {
        s.chars().map(|c| c.to_string()).collect()
    }

    fn parser(rows: u16, cols: u16, scrollback: usize) -> vt100::Parser {
        vt100::Parser::new(NonZeroU16::new(rows).unwrap(), NonZeroU16::new(cols).unwrap(), scrollback)
    }

    #[test]
    fn finds_matches_per_row_with_columns() {
        let rows = vec![cells("error: one"), cells("fine      "), cells("an Error, error")];
        let h = find_in_rows(&rows, "error");
        assert_eq!(h, vec![Hit { row: 0, start: 0, end: 5 }, Hit { row: 2, start: 3, end: 8 }, Hit { row: 2, start: 10, end: 15 }]);
        assert_eq!(find_in_rows(&rows, "Error"), vec![Hit { row: 2, start: 3, end: 8 }], "a capital means exact case");
        assert!(find_in_rows(&rows, "").is_empty());
    }

    #[test]
    fn wide_characters_map_to_columns() {
        let rows = vec![vec!["日".to_string(), "\u{0}".to_string(), "本".to_string(), "\u{0}".to_string(), "x".to_string()]];
        assert_eq!(find_in_rows(&rows, "本x"), vec![Hit { row: 0, start: 2, end: 5 }]);
        assert_eq!(find_in_rows(&rows, "日"), vec![Hit { row: 0, start: 0, end: 2 }]);
    }

    #[test]
    fn reads_history_and_screen_from_the_parser() {
        let mut p = parser(5, 20, 100);
        for i in 1..=30 {
            p.process(format!("line {i}\r\n").as_bytes());
        }
        let rows = collect_rows(&mut p);
        let text: Vec<String> = rows.iter().map(|r| r.concat().trim_end().to_string()).collect();
        assert_eq!(text.len(), p.screen().scrollback_len() + 5);
        assert_eq!(text[0], "line 1");
        assert_eq!(text[29], "line 30");
        assert_eq!(p.screen().scrollback(), 0, "the view offset is restored");
        p.screen_mut().set_scrollback(7);
        let _ = collect_rows(&mut p);
        assert_eq!(p.screen().scrollback(), 7);
    }

    #[test]
    fn scripted_session_finds_an_early_number() {
        let mut p = parser(10, 40, 1000);
        for i in 1..=300 {
            p.process(format!("row number {i}\r\n").as_bytes());
        }
        let mut f = OutFind::new(&mut p, "number 42");
        // "number 42" appears in "number 42" only (not 142, 242: those contain "number 142"? no: "number 142" does not contain "number 42")
        assert_eq!(f.hits.len(), 1);
        assert_eq!(f.label(), "match 1 of 1");
        let off = f.offset();
        p.screen_mut().set_scrollback(off);
        let shown: Vec<String> = p.screen().rows(0, 40).collect();
        assert!(shown.iter().any(|r| r.contains("row number 42")), "{shown:?}");
        let v = f.in_view(p.screen().scrollback());
        assert_eq!(v.len(), 1);
        assert!(v[0].3);
        // many matches: newest first, then older, then back
        let mut g = OutFind::new(&mut p, "row number 1");
        let n = g.hits.len();
        assert_eq!(n, 1 + 10 + 100, "1, 10-19, 100-199");
        assert_eq!(g.label(), format!("match 1 of {n}"));
        g.older();
        assert_eq!(g.label(), format!("match 2 of {n}"));
        g.newer();
        g.newer();
        assert_eq!(g.label(), format!("match 1 of {n}"), "stops at the newest");
        for _ in 0..n {
            g.older();
        }
        assert_eq!(g.label(), format!("match {n} of {n}"), "stops at the oldest");
        f.older();
        assert_eq!(f.label(), "match 1 of 1");
    }

    #[test]
    fn the_current_match_is_inside_the_view_at_the_computed_offset() {
        let mut p = parser(8, 30, 500);
        for i in 1..=200 {
            p.process(format!("n{i}\r\n").as_bytes());
        }
        let mut f = OutFind::new(&mut p, "n7");
        for _ in 0..f.hits.len() {
            f.older();
            let off = f.offset();
            let v = f.in_view(off);
            assert!(v.iter().any(|x| x.3), "current match visible at offset {off}");
            p.screen_mut().set_scrollback(off);
            let (row, s, e, _) = *v.iter().find(|x| x.3).unwrap();
            let line: String = (s..e).map(|c| p.screen().cell(row, c).map_or(String::new(), |c| c.contents().to_string())).collect();
            assert_eq!(line, "n7", "{row} {s} {e}");
        }
    }

    #[test]
    fn empty_and_missing_matches() {
        let mut p = parser(3, 10, 10);
        p.process(b"abc");
        let f = OutFind::new(&mut p, "zzz");
        assert!(f.hits.is_empty());
        assert_eq!(f.offset(), 0);
        assert!(f.in_view(0).is_empty());
    }

    #[test]
    fn offset_math() {
        // 100 history rows + 10 on screen: row 50 near a third of the way down the view
        assert_eq!(offset_for(110, 10, 100, 50), 100 - (50 - 3));
        assert_eq!(offset_for(110, 10, 100, 2), 100, "near the top: clamp to the oldest view");
        assert_eq!(offset_for(110, 10, 100, 109), 0, "the live screen needs no scrolling");
        assert_eq!(offset_for(10, 10, 0, 4), 0);
    }
}
