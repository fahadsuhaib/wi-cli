//! The read-only diff viewer: turns a parsed `FileDiff` into display rows (unified view, line
//! numbers for both sides, theme-derived add/remove/hunk colours) and keeps the scroll and hunk
//! position. No I/O here; the explorer feeds it `DiffData` and draws what `visible` returns.

use crate::gitdiff::{DiffData, DiffLine, FileDiff, Side};
use crate::theme::{DIFF_HUNK_TINT, DIFF_LINE_TINT, diff_tint, theme};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DRow {
    Note(String),
    Hunk(String),
    Line { kind: Kind, old: Option<u32>, new: Option<u32>, text: String },
    NoNewline,
}

pub struct DiffView {
    pub path: String,
    pub staged: Option<FileDiff>,
    pub unstaged: Option<FileDiff>,
    pub side: Side,
    pub top: usize,
    /// Index of the current hunk (the one `N`/`P` last went to, or the one at the top).
    pub hunk: usize,
    rows: Vec<DRow>,
    hunk_rows: Vec<usize>,
}

fn clean(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim_end_matches('\r').chars() {
        match c {
            '\t' => out.push_str("    "),
            c if c.is_control() => out.push('·'),
            c => out.push(c),
        }
    }
    out
}

/// Cut `s` to `width` columns, ending in an ellipsis when it was cut.
fn clip(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= width {
        return s.to_string();
    }
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// A path cut in the middle when it does not fit.
pub fn mid_cut(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max || max < 5 {
        return s.to_string();
    }
    let keep = max - 1;
    let (head, tail) = (keep / 2, keep - keep / 2);
    format!("{}…{}", chars[..head].iter().collect::<String>(), chars[chars.len() - tail..].iter().collect::<String>())
}

/// The display rows of one file's diff.
pub fn build_rows(d: &FileDiff) -> (Vec<DRow>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut hunk_rows = Vec::new();
    if let Some(o) = &d.old_path {
        rows.push(DRow::Note(format!("renamed from {o}")));
    }
    if d.new_file {
        rows.push(DRow::Note("new file".into()));
    }
    if d.deleted {
        rows.push(DRow::Note("deleted file".into()));
    }
    if let Some((a, b)) = &d.mode {
        rows.push(DRow::Note(format!("mode {a} -> {b}")));
    }
    if d.combined {
        rows.push(DRow::Note("merge conflict: combined diff".into()));
    }
    if d.binary {
        rows.push(DRow::Note("binary file differs (not shown)".into()));
    }
    for h in &d.hunks {
        hunk_rows.push(rows.len());
        rows.push(DRow::Hunk(h.header.clone()));
        let (mut o, mut n) = (h.old_start, h.new_start);
        for l in &h.lines {
            match l {
                DiffLine::Context(t) => {
                    rows.push(DRow::Line { kind: Kind::Context, old: Some(o), new: Some(n), text: clean(t) });
                    o += 1;
                    n += 1;
                }
                DiffLine::Add(t) => {
                    rows.push(DRow::Line { kind: Kind::Add, old: None, new: Some(n), text: clean(t) });
                    n += 1;
                }
                DiffLine::Del(t) => {
                    rows.push(DRow::Line { kind: Kind::Del, old: Some(o), new: None, text: clean(t) });
                    o += 1;
                }
                DiffLine::NoNewline => rows.push(DRow::NoNewline),
            }
        }
    }
    if d.hunks.is_empty() && rows.is_empty() {
        rows.push(DRow::Note("no changes".into()));
    }
    if d.truncated {
        rows.push(DRow::Note("diff too large: showing the first part only".into()));
    }
    (rows, hunk_rows)
}

impl DiffView {
    /// A view of `data`; shows the unstaged side first when both exist.
    pub fn new(path: &str, data: DiffData) -> DiffView {
        let side = if data.unstaged.is_some() || data.staged.is_none() { Side::Unstaged } else { Side::Staged };
        let mut v = DiffView { path: path.to_string(), staged: data.staged, unstaged: data.unstaged, side, top: 0, hunk: 0, rows: Vec::new(), hunk_rows: Vec::new() };
        v.rebuild();
        v
    }

    fn rebuild(&mut self) {
        let empty = FileDiff::default();
        let (rows, hunk_rows) = build_rows(self.cur().unwrap_or(&empty));
        self.rows = rows;
        self.hunk_rows = hunk_rows;
    }

    pub fn cur(&self) -> Option<&FileDiff> {
        match self.side {
            Side::Staged => self.staged.as_ref(),
            Side::Unstaged => self.unstaged.as_ref(),
        }
    }

    pub fn has_both(&self) -> bool {
        self.staged.is_some() && self.unstaged.is_some()
    }

    /// Show the other side (staged / unstaged) when both exist. Returns whether it switched.
    pub fn switch_side(&mut self) -> bool {
        if !self.has_both() {
            return false;
        }
        self.side = if self.side == Side::Staged { Side::Unstaged } else { Side::Staged };
        self.top = 0;
        self.hunk = 0;
        self.rebuild();
        true
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn hunk_count(&self) -> usize {
        self.hunk_rows.len()
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[DRow] {
        &self.rows
    }

    /// Hunk position for the header, 1-based, `(0, 0)` without hunks.
    pub fn hunk_pos(&self) -> (usize, usize) {
        let m = self.hunk_count();
        if m == 0 { (0, 0) } else { (self.hunk.min(m - 1) + 1, m) }
    }

    pub fn max_top(&self, height: usize) -> usize {
        self.len().saturating_sub(height.max(1))
    }

    pub fn scroll(&mut self, delta: isize, height: usize) {
        self.top = (self.top as isize + delta).clamp(0, self.max_top(height) as isize) as usize;
        self.sync_hunk(height);
    }

    /// Point `hunk` at the last hunk that starts at or above the top row.
    fn sync_hunk(&mut self, height: usize) {
        let at = self.hunk_rows.iter().rposition(|&r| r <= self.top).unwrap_or(0);
        let bottom = self.top >= self.max_top(height);
        self.hunk = if bottom { at.max(self.hunk.min(self.hunk_count().saturating_sub(1))) } else { at };
    }

    /// Go to the next or previous hunk.
    pub fn jump(&mut self, forward: bool, height: usize) {
        let m = self.hunk_count();
        if m == 0 {
            return;
        }
        let cur = self.hunk.min(m - 1);
        // A hunk scrolled past by hand: "previous" goes to the one at the top first.
        let cur_row = self.hunk_rows[cur];
        self.hunk = if forward {
            (cur + 1).min(m - 1)
        } else if self.top > cur_row {
            cur
        } else {
            cur.saturating_sub(1)
        };
        self.top = self.hunk_rows[self.hunk].min(self.max_top(height));
    }

    /// Take over the scroll position of the view this one replaces (same file, newer content):
    /// the hunk is found again by its header text and the offset inside it kept.
    pub fn carry_over(&mut self, old: &DiffView, height: usize) {
        if old.side != self.side && old.has_both() == self.has_both() && self.has_both() {
            self.switch_side();
        }
        // Anchor on the hunk that was current (it can sit below the top row when the view was
        // scrolled as far as it goes) and keep the same distance from it.
        let m = old.hunk_count();
        let anchor = (m > 0).then(|| old.hunk.min(m - 1)).and_then(|oi| {
            let DRow::Hunk(hdr) = &old.rows[old.hunk_rows[oi]] else { return None };
            let ni = self.hunk_rows.iter().position(|&r| matches!(&self.rows[r], DRow::Hunk(h) if h == hdr))?;
            Some((ni, self.hunk_rows[ni] as isize + old.top as isize - old.hunk_rows[oi] as isize))
        });
        match anchor {
            Some((ni, top)) => {
                self.top = top.clamp(0, self.max_top(height) as isize) as usize;
                self.hunk = ni;
            }
            None => {
                self.top = old.top.min(self.max_top(height));
                self.hunk = self.hunk_rows.iter().rposition(|&r| r <= self.top).unwrap_or(0);
            }
        }
    }

    /// `+added -removed` of the side on screen.
    pub fn counts(&self) -> (u32, u32) {
        self.cur().map_or((0, 0), |d| (d.added, d.removed))
    }

    /// The header row: path, side, counts, hunk position.
    pub fn header(&self, width: usize) -> Line<'static> {
        let t = theme();
        let dim = Style::default().fg(t.dim);
        let (a, r) = self.counts();
        let (n, m) = self.hunk_pos();
        let mut tail: Vec<Span<'static>> = vec![
            Span::styled(format!(" · diff {} · ", self.side.word()), dim),
            Span::styled(format!("+{a}"), Style::default().fg(t.diff_add).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(format!("-{r}"), Style::default().fg(t.diff_remove).add_modifier(Modifier::BOLD)),
        ];
        if m > 0 {
            tail.push(Span::styled(format!(" · hunk {n}/{m}"), dim));
        }
        if self.has_both() {
            let other = if self.side == Side::Staged { "unstaged" } else { "staged" };
            tail.push(Span::styled(format!(" · Tab: {other}"), dim));
        }
        let tw: usize = tail.iter().map(|s| s.content.chars().count()).sum();
        let mut spans = vec![Span::styled(mid_cut(&self.path, width.saturating_sub(tw + 1).max(8)), Style::default().add_modifier(Modifier::BOLD))];
        spans.extend(tail);
        Line::from(spans)
    }

    /// The rows for a window of `height` rows and `width` columns.
    pub fn visible(&self, height: usize, width: usize) -> Vec<Line<'static>> {
        let t = theme();
        let nums = self.rows.iter().filter_map(|r| if let DRow::Line { old, new, .. } = r { Some((*old).max(*new).unwrap_or(0)) } else { None }).max().unwrap_or(0);
        let nw = nums.to_string().len().max(3);
        let bg = |c| diff_tint(c, t.bg, DIFF_LINE_TINT);
        let (add_bg, del_bg) = (bg(t.diff_add), bg(t.diff_remove));
        let hunk_bg = diff_tint(t.diff_hunk, t.bg, DIFF_HUNK_TINT);
        let pad = |mut spans: Vec<Span<'static>>, style: Style| {
            let used: usize = spans.iter().map(|s| s.content.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>()).sum();
            if used < width {
                spans.push(Span::styled(" ".repeat(width - used), style));
            }
            Line::from(spans)
        };
        self.rows
            .iter()
            .skip(self.top)
            .take(height)
            .map(|r| match r {
                DRow::Note(s) => Line::from(Span::styled(clip(s, width), Style::default().fg(t.dim).add_modifier(Modifier::ITALIC))),
                DRow::NoNewline => Line::from(Span::styled(clip(&format!("{}\\ No newline at end of file", " ".repeat(2 * nw + 3)), width), Style::default().fg(t.dim))),
                DRow::Hunk(h) => {
                    let mut st = Style::default().fg(t.diff_hunk);
                    if let Some(b) = hunk_bg {
                        st = st.bg(b);
                    }
                    pad(vec![Span::styled(clip(&clean(h), width), st)], st)
                }
                DRow::Line { kind, old, new, text } => {
                    let (sign, accent, tint) = match kind {
                        Kind::Add => ('+', Some(t.diff_add), add_bg),
                        Kind::Del => ('-', Some(t.diff_remove), del_bg),
                        Kind::Context => (' ', None, None),
                    };
                    let mut base = Style::default().fg(t.fg);
                    let mut gut = Style::default().fg(t.gutter);
                    let mut sg = Style::default().fg(accent.unwrap_or(t.fg)).add_modifier(Modifier::BOLD);
                    match tint {
                        Some(b) => {
                            base = base.bg(b);
                            gut = gut.bg(b);
                            sg = sg.bg(b);
                        }
                        // No tint to draw (not an RGB theme): colour the text itself.
                        None => {
                            if let Some(a) = accent {
                                base = base.fg(a);
                            }
                        }
                    }
                    let num = |n: &Option<u32>| n.map_or(" ".repeat(nw), |n| format!("{n:>nw$}"));
                    let g = format!("{} {} ", num(old), num(new));
                    let room = width.saturating_sub(g.chars().count() + 2);
                    pad(vec![Span::styled(g, gut), Span::styled(format!("{sign} "), sg), Span::styled(clip(text, room), base)], base)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitdiff::parse_diff;

    fn view(text: &str) -> DiffView {
        let f = parse_diff(text, false).remove(0);
        DiffView::new("f.txt", DiffData { staged: None, unstaged: Some(f) })
    }

    const TWO: &str = "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n@@ -20,2 +20,3 @@\n x\n+y\n z\n";

    fn plain(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn rows_carry_line_numbers() {
        let v = view(TWO);
        let lines = v.visible(20, 60);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!(text[0].starts_with("@@ -1,2 +1,2 @@"));
        assert!(text[1].starts_with("  1   1   a"), "{:?}", text[1]);
        assert!(text[2].starts_with("  2     - b"), "{:?}", text[2]);
        assert!(text[3].starts_with("      2 + B"), "{:?}", text[3]);
        assert!(text[5].starts_with(" 20  20   x"), "{:?}", text[5]);
        assert!(text[6].starts_with("     21 + y"), "{:?}", text[6]);
        assert_eq!(v.counts(), (2, 1));
        assert_eq!(v.hunk_pos(), (1, 2));
    }

    #[test]
    fn hunk_jumps_and_scroll_keep_position() {
        let mut v = view(TWO);
        v.jump(true, 3);
        assert_eq!(v.hunk_pos(), (2, 2));
        assert_eq!(v.top, v.max_top(3).min(4));
        v.jump(true, 3);
        assert_eq!(v.hunk_pos(), (2, 2));
        v.jump(false, 3);
        assert_eq!(v.hunk_pos(), (1, 2));
        assert_eq!(v.top, 0);
        v.scroll(100, 3);
        assert!(v.top <= v.max_top(3));
        assert_eq!(v.hunk_pos().1, 2);
    }

    #[test]
    fn carry_over_finds_the_hunk_again() {
        let mut old = view(TWO);
        old.jump(true, 3);
        old.top += 1;
        // The first hunk grew by two lines; the second hunk's header text is unchanged.
        let newer = TWO.replace("+B\n", "+B\n+B2\n+B3\n").replace("@@ -1,2 +1,2 @@", "@@ -1,2 +1,4 @@");
        let mut v = view(&newer);
        v.carry_over(&old, 3);
        let DRow::Hunk(h) = &v.rows()[v.hunk_rows[1]] else { panic!() };
        assert!(h.starts_with("@@ -20"));
        assert_eq!(v.top, v.hunk_rows[1] + 1);
        assert_eq!(v.hunk_pos(), (2, 2));
    }

    #[test]
    fn sides_notes_and_empty() {
        let f = parse_diff(TWO, false).remove(0);
        let mut v = DiffView::new("f.txt", DiffData { staged: Some(f.clone()), unstaged: Some(f) });
        assert_eq!(v.side, Side::Unstaged);
        assert!(v.switch_side());
        assert_eq!(v.side, Side::Staged);
        assert!(plain(&v.header(80)).contains("diff staged"));
        let one = view(TWO);
        let mut one = one;
        assert!(!one.switch_side());
        let e = DiffView::new("e", DiffData { staged: None, unstaged: Some(FileDiff::default()) });
        assert_eq!(e.rows(), &[DRow::Note("no changes".into())]);
        let b = view("diff --git a/i.png b/i.png\nBinary files a/i.png and b/i.png differ\n");
        assert!(matches!(&b.rows()[0], DRow::Note(s) if s.contains("binary")));
    }

    #[test]
    fn display_cleaning_and_clipping() {
        assert_eq!(clean("a\tb\r"), "a    b");
        assert_eq!(clean("a\x1bb"), "a·b");
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("日本語日本語", 5), "日本…");
        assert_eq!(mid_cut("src/very/long/path/name.rs", 12).chars().count(), 12);
        assert_eq!(mid_cut("short", 12), "short");
    }
}
