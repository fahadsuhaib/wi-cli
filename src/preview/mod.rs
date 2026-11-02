//! Read-only file previews rendered on the client. The box (or disk) only serves bytes.

pub mod diff;
pub mod keys;
mod json;
mod markdown;
mod table;
mod text;

use crate::theme::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Markdown,
    Table,
    Json,
    /// Git changes: status summary and coloured diff.
    Diff,
    /// Binary or otherwise not shown: a small info card instead.
    Stub,
}

pub struct Opts {
    pub csv_rows: usize,
}

pub struct Preview {
    pub path: String,
    pub title: String,
    pub kind: Kind,
    /// Always-visible note when only part of the file is shown.
    pub banner: Option<String>,
    pub lines: Vec<Line<'static>>,
    /// Width in characters of the first span of each line when it is a line-number gutter.
    pub gutter: usize,
    pub wrap: bool,
    pub top: usize,
    pub left: usize,
    /// Tab that was active when this preview opened; Esc returns there.
    pub from_tab: usize,
    /// Line indices where each changed file starts (Diff only), for N/P.
    pub jumps: Vec<usize>,
    wrapped: Option<(u16, Vec<Line<'static>>)>,
    /// For each source line, the display row where it starts (only while wrapping).
    starts: Vec<usize>,
    /// 1-based source line to bring into view and mark (text previews only).
    pub target_line: Option<usize>,
    goto_pending: bool,
    /// Find-in-preview state (matches, the current one) and a copy of the lines with matches lit.
    pub find: Option<Find>,
    hl: Option<Vec<Line<'static>>>,
    find_pending: bool,
}

/// Matches of the text typed in the preview's find prompt.
#[derive(Clone, Debug)]
pub struct Find {
    /// (source line, start, length) in characters of the line's text (after the line-number gutter).
    pub hits: Vec<(usize, usize, usize)>,
    pub cur: usize,
}

/// Human-readable size: `256 KB`, `48 MB`.
pub fn human(n: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 || v >= 10.0 { format!("{:.0} {}", v, U[i]) } else { format!("{:.1} {}", v, U[i]) }
}

fn sniff_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(4096).any(|&b| b == 0)
}

fn extension(path: &str) -> String {
    std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

impl Preview {
    /// Build a preview from the bytes that were read (`total` is the file's full size).
    pub fn build(path: &str, bytes: &[u8], total: u64, opts: &Opts) -> Preview {
        let truncated = (bytes.len() as u64) < total;
        let cap_note = || format!("showing first {} of {}", human(bytes.len() as u64), human(total));
        let ext = extension(path);
        let mut p = Preview {
            path: path.to_string(),
            title: file_name(path),
            kind: Kind::Text,
            banner: truncated.then(cap_note),
            lines: Vec::new(),
            gutter: 0,
            wrap: false,
            top: 0,
            left: 0,
            from_tab: 0,
            jumps: Vec::new(),
            wrapped: None,
            starts: Vec::new(),
            target_line: None,
            goto_pending: false,
            find: None,
            hl: None,
            find_pending: false,
        };
        // Content wins over the extension: a binary file named notes.txt is never shown as text.
        if sniff_binary(bytes) {
            p.kind = Kind::Stub;
            p.lines = stub_lines(path, total, "binary data (contains NUL bytes)");
            p.banner = None;
            return p;
        }
        let body = String::from_utf8_lossy(bytes);
        match ext.as_str() {
            "md" | "markdown" => {
                p.kind = Kind::Markdown;
                p.wrap = true;
                p.lines = markdown::render(&body);
            }
            "csv" | "tsv" => {
                let delim = if ext == "tsv" { b'\t' } else { b',' };
                let t = table::render(&body, delim, opts.csv_rows, total, truncated);
                p.kind = Kind::Table;
                p.lines = t.lines;
                p.banner = t.banner.or(p.banner);
            }
            "json" => match (!truncated).then(|| json::render(&body)).flatten() {
                Some(lines) => {
                    p.kind = Kind::Json;
                    p.lines = lines;
                }
                None => {
                    // Invalid or over the cap: plain text, and say why.
                    let why = if truncated { cap_note() } else { "not valid JSON, shown as text".to_string() };
                    p.banner = Some(why);
                    let (lines, gutter) = text::render(&body);
                    p.lines = lines;
                    p.gutter = gutter;
                }
            },
            _ => {
                let (lines, gutter) = text::render(&body);
                p.lines = lines;
                p.gutter = gutter;
            }
        }
        p
    }

    /// The Git changes view built from a snapshot of git's output.
    pub fn from_git(snap: &diff::Snapshot) -> Preview {
        let r = diff::render(snap);
        Preview {
            path: "git:changes".to_string(),
            title: "git".to_string(),
            kind: Kind::Diff,
            banner: snap.truncated.then(|| format!("diff truncated at {}: showing the first part only", human(crate::exec::OUT_CAP as u64))),
            lines: r.lines,
            gutter: 0,
            wrap: false,
            top: 0,
            left: 0,
            from_tab: 0,
            jumps: r.jumps,
            wrapped: None,
            starts: Vec::new(),
            target_line: None,
            goto_pending: false,
            find: None,
            hl: None,
            find_pending: false,
        }
    }

    /// Display-line positions of the file starts (in wrapped rows when wrapping is on).
    fn jump_rows(&self) -> Vec<usize> {
        match (&self.wrapped, self.wrap) {
            (Some((w, _)), true) => {
                let mut pos = Vec::new();
                let mut row = 0;
                for (i, l) in self.src().iter().enumerate() {
                    if self.jumps.contains(&i) {
                        pos.push(row);
                    }
                    row += wrap_line(l, self.gutter, *w as usize).len();
                }
                pos
            }
            _ => self.jumps.clone(),
        }
    }

    /// Scroll so the next (or previous) file starts at the top.
    fn jump_file(&mut self, forward: bool, page: usize) {
        let rows = self.jump_rows();
        let target = if forward { rows.iter().copied().find(|&r| r > self.top) } else { rows.iter().copied().rev().find(|&r| r < self.top) };
        if let Some(t) = target {
            self.top = t.min(self.max_top(page));
        }
    }

    /// Number of display lines for the given width (wrapped lines when wrapping is on).
    pub fn display_len(&self) -> usize {
        match (&self.wrapped, self.wrap) {
            (Some((_, w)), true) => w.len(),
            _ => self.lines.len(),
        }
    }

    /// Refresh the wrap cache for `width` and clamp the scroll position. Call before drawing.
    pub fn layout(&mut self, width: u16, height: usize) {
        if self.wrap {
            if self.wrapped.as_ref().map(|(w, _)| *w) != Some(width) {
                let g = self.gutter;
                let mut w: Vec<Line<'static>> = Vec::new();
                let mut starts = Vec::with_capacity(self.lines.len());
                for l in self.hl.as_ref().unwrap_or(&self.lines) {
                    starts.push(w.len());
                    w.extend(wrap_line(l, g, width as usize));
                }
                self.starts = starts;
                self.wrapped = Some((width, w));
            }
        } else {
            self.wrapped = None;
        }
        let max = self.display_len().saturating_sub(height.max(1));
        if self.goto_pending {
            self.goto_pending = false;
            if let Some((first, _)) = self.target_rows() {
                // Leave a third of the window above the line so there is context.
                self.top = first.saturating_sub(height / 3);
            }
        }
        if std::mem::take(&mut self.find_pending) {
            self.show_current_match(width as usize, height);
        }
        self.top = self.top.min(max);
    }

    fn src(&self) -> &Vec<Line<'static>> {
        self.hl.as_ref().unwrap_or(&self.lines)
    }

    /// Scroll (and, unwrapped, pan) so the current find match is on screen.
    fn show_current_match(&mut self, width: usize, height: usize) {
        let Some(&(line, start, len)) = self.find.as_ref().and_then(|f| f.hits.get(f.cur)) else { return };
        let row = self.match_row(line, start, width);
        let h = height.max(1);
        if row < self.top || row >= self.top + h {
            self.top = row.saturating_sub(h / 3);
        }
        if !self.wrap {
            let room = width.saturating_sub(self.gutter).max(1);
            if start < self.left || start + len > self.left + room {
                self.left = start.saturating_sub(room / 3);
            }
        }
    }

    /// Display row of character `start` of source line `line` (wrapped rows when wrapping).
    fn match_row(&self, line: usize, start: usize, width: usize) -> usize {
        match (&self.wrapped, self.wrap) {
            (Some(_), true) => self.starts.get(line).copied().unwrap_or(0) + wrap_row_of(&self.lines[line], self.gutter, width, start),
            _ => line,
        }
    }

    /// Light up every match of `needle` (smart case) and make the first one at or below the top of
    /// the view current. Returns the number of matches.
    pub fn set_find(&mut self, needle: &str) -> usize {
        let mut hits = Vec::new();
        for (i, l) in self.lines.iter().enumerate() {
            let text: String = cells(l).into_iter().skip(self.gutter).map(|(c, _)| c).collect();
            hits.extend(crate::search::find_matches(&text, needle).into_iter().map(|(s, n)| (i, s, n)));
        }
        let n = hits.len();
        if n == 0 {
            self.clear_find();
            return 0;
        }
        let top_line = match (&self.wrapped, self.wrap) {
            (Some(_), true) => self.starts.partition_point(|&r| r < self.top),
            _ => self.top,
        };
        let cur = hits.iter().position(|h| h.0 >= top_line).unwrap_or(0);
        self.find = Some(Find { hits, cur });
        self.refresh_find();
        n
    }

    pub fn clear_find(&mut self) {
        self.find = None;
        self.hl = None;
        self.wrapped = None;
    }

    /// Step to the next (or previous) match, wrapping around.
    pub fn find_step(&mut self, forward: bool) {
        if let Some(f) = self.find.as_mut() {
            let n = f.hits.len();
            f.cur = if forward { (f.cur + 1) % n } else { (f.cur + n - 1) % n };
            self.refresh_find();
        }
    }

    /// `match 3 of 17`.
    pub fn find_label(&self) -> Option<String> {
        self.find.as_ref().map(|f| format!("match {} of {}", f.cur + 1, f.hits.len()))
    }

    /// Rebuild the lit copy of the lines and ask the next layout to bring the current match in view.
    fn refresh_find(&mut self) {
        let Some(f) = &self.find else { return };
        let t = crate::theme::theme();
        let lit = Style::default().fg(t.hint_fg).bg(t.hint_bg);
        let cur = Style::default().fg(t.bg).bg(t.diff_add).add_modifier(Modifier::BOLD);
        let mut by_line: std::collections::BTreeMap<usize, Vec<(usize, usize, bool)>> = Default::default();
        for (i, &(l, s, n)) in f.hits.iter().enumerate() {
            by_line.entry(l).or_default().push((s, n, i == f.cur));
        }
        let mut out = self.lines.clone();
        for (l, hs) in by_line {
            let mut cs = cells(&self.lines[l]);
            for (s, n, is_cur) in hs {
                for c in cs.iter_mut().skip(self.gutter + s).take(n) {
                    c.1 = if is_cur { cur } else { lit };
                }
            }
            out[l] = from_cells(&cs);
        }
        self.hl = Some(out);
        self.wrapped = None;
        self.find_pending = true;
    }

    /// Bring a source line (1-based) into view and mark it. Only text previews have source lines.
    pub fn goto_line(&mut self, line: usize) {
        if self.kind == Kind::Text && line > 0 {
            self.target_line = Some(line.min(self.lines.len().max(1)));
            self.goto_pending = true;
        }
    }

    /// Display rows (first, one past last) occupied by the target line.
    fn target_rows(&self) -> Option<(usize, usize)> {
        let i = self.target_line? - 1;
        if i >= self.lines.len() {
            return None;
        }
        match (&self.wrapped, self.wrap) {
            (Some((_, w)), true) => {
                let first = *self.starts.get(i)?;
                let next = self.starts.get(i + 1).copied().unwrap_or(w.len());
                Some((first, next))
            }
            _ => Some((i, i + 1)),
        }
    }

    pub fn max_top(&self, height: usize) -> usize {
        self.display_len().saturating_sub(height.max(1))
    }

    /// Apply one navigation key. `page` is the visible height in rows. Returns false for keys
    /// that are not navigation (the caller handles Back and Reload).
    pub fn navigate(&mut self, key: keys::PvKey, page: usize) -> bool {
        use keys::PvKey::*;
        let max = self.max_top(page);
        match key {
            Up => self.top = self.top.saturating_sub(1),
            Down => self.top = (self.top + 1).min(max),
            PageUp => self.top = self.top.saturating_sub(page.saturating_sub(1).max(1)),
            PageDown => self.top = (self.top + page.saturating_sub(1).max(1)).min(max),
            Top => self.top = 0,
            Bottom => self.top = max,
            Left => self.left = self.left.saturating_sub(4),
            Right => {
                if !self.wrap {
                    self.left += 4;
                }
            }
            Wrap => self.toggle_wrap(),
            NextFile if self.find.is_some() && self.kind != Kind::Diff => self.find_step(true),
            PrevFile if self.find.is_some() && self.kind != Kind::Diff => self.find_step(false),
            NextFile => self.jump_file(true, page),
            PrevFile => self.jump_file(false, page),
            MatchNext => self.find_step(true),
            MatchPrev => self.find_step(false),
            Reload | Back | Find => return false,
        }
        true
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        self.left = 0;
        self.wrapped = None;
    }

    /// The lines to draw for a window of `height` rows and `width` columns.
    pub fn visible(&self, height: usize, width: usize) -> Vec<Line<'static>> {
        let mut rows: Vec<Line<'static>> = match (&self.wrapped, self.wrap) {
            (Some((_, w)), true) => w.iter().skip(self.top).take(height).cloned().collect(),
            _ => self.src().iter().skip(self.top).take(height).map(|l| slice_line(l, self.gutter, self.left, width)).collect(),
        };
        if let Some((a, b)) = self.target_rows() {
            for (i, row) in rows.iter_mut().enumerate() {
                if (a..b).contains(&(self.top + i)) {
                    let marked: Vec<Cell> = cells(row).into_iter().map(|(c, st)| (c, st.bg(theme().goto_bg))).collect();
                    *row = from_cells(&marked);
                }
            }
        }
        rows
    }
}

fn stub_lines(path: &str, total: u64, kind: &str) -> Vec<Line<'static>> {
    let dim = Style::default().fg(theme().dim);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    vec![
        Line::from(Span::styled(file_name(path), bold)),
        Line::from(""),
        Line::from(vec![Span::styled("size  ", dim), Span::raw(human(total))]),
        Line::from(vec![Span::styled("type  ", dim), Span::raw(kind.to_string())]),
        Line::from(vec![Span::styled("path  ", dim), Span::raw(path.to_string())]),
        Line::from(""),
        Line::from(Span::styled("Not shown in wi. Open it from the shell, e.g. `open <file>` on macOS.", dim)),
    ]
}

/// One string per screen column for a rendered line: the character, or a zero-width marker for
/// the right half of a wide character. The first `gutter` columns (line numbers) are marked so
/// they are never copied.
pub fn row_cells(line: &Line<'_>, gutter: usize, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, (c, _)) in cells(line).into_iter().enumerate() {
        if i < gutter {
            out.push("\u{0}".to_string());
            continue;
        }
        out.push(c.to_string());
        if c.width().unwrap_or(1) == 2 {
            out.push("\u{0}".to_string());
        }
    }
    out.resize(width.max(out.len()), String::new());
    out
}

/// Draw a selection on one rendered row by painting the selected columns in the selection colours.
pub fn highlight_line(line: &Line<'_>, row: u16, sel: crate::select::Sel) -> Line<'static> {
    let mut col = 0u16;
    let flipped: Vec<Cell> = cells(line)
        .into_iter()
        .map(|(c, st)| {
            let here = col;
            col += c.width().unwrap_or(0) as u16;
            if crate::select::contains(sel, row, here) {
                (c, st.fg(theme().sel_fg).bg(theme().sel_bg))
            } else {
                (c, st)
            }
        })
        .collect();
    from_cells(&flipped)
}

type Cell = (char, Style);

fn cells(line: &Line<'_>) -> Vec<Cell> {
    line.spans.iter().flat_map(|s| s.content.chars().map(move |c| (c, s.style.patch(line.style)))).collect()
}

fn from_cells(cs: &[Cell]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut cur: Option<Style> = None;
    for (c, st) in cs {
        if cur != Some(*st) {
            if let Some(s) = cur {
                spans.push(Span::styled(std::mem::take(&mut run), s));
            }
            cur = Some(*st);
        }
        run.push(*c);
    }
    if let Some(s) = cur {
        spans.push(Span::styled(run, s));
    }
    Line::from(spans)
}

/// Break a line at `width` columns. Continuation lines get a blank gutter of the same width.
pub fn wrap_line(line: &Line<'_>, gutter: usize, width: usize) -> Vec<Line<'static>> {
    let all = cells(line);
    let (g, body) = all.split_at(gutter.min(all.len()));
    let room = width.saturating_sub(gutter).max(1);
    let mut out = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut w = 0;
    let mut first = true;
    let flush = |cur: &mut Vec<Cell>, first: &mut bool, out: &mut Vec<Line<'static>>| {
        let mut row: Vec<Cell> = if *first { g.to_vec() } else { vec![(' ', Style::default()); gutter] };
        row.append(cur);
        out.push(from_cells(&row));
        *first = false;
    };
    for &(c, st) in body {
        let cw = c.width().unwrap_or(0);
        if w + cw > room && !cur.is_empty() {
            flush(&mut cur, &mut first, &mut out);
            w = 0;
        }
        cur.push((c, st));
        w += cw;
    }
    flush(&mut cur, &mut first, &mut out);
    out
}

/// Which of the rows a line wraps into (0-based) holds character `idx` of its text. Follows the
/// same breaking rules as `wrap_line`.
pub fn wrap_row_of(line: &Line<'_>, gutter: usize, width: usize, idx: usize) -> usize {
    let all = cells(line);
    let body = &all[gutter.min(all.len())..];
    let room = width.saturating_sub(gutter).max(1);
    let (mut row, mut w, mut count) = (0, 0, 0);
    for (i, &(c, _)) in body.iter().enumerate() {
        let cw = c.width().unwrap_or(0);
        if w + cw > room && count > 0 {
            row += 1;
            w = 0;
            count = 0;
        }
        if i == idx {
            return row;
        }
        w += cw;
        count += 1;
    }
    row
}

/// Horizontal window: keep the gutter, drop `left` columns of content, cut at `width`.
pub fn slice_line(line: &Line<'_>, gutter: usize, left: usize, width: usize) -> Line<'static> {
    let all = cells(line);
    let (g, body) = all.split_at(gutter.min(all.len()));
    let mut out: Vec<Cell> = g.to_vec();
    let mut skipped = 0;
    let mut used = g.iter().map(|(c, _)| c.width().unwrap_or(0)).sum::<usize>();
    for &(c, st) in body {
        let cw = c.width().unwrap_or(0);
        if skipped < left {
            skipped += cw;
            continue;
        }
        if used + cw > width {
            break;
        }
        out.push((c, st));
        used += cw;
    }
    from_cells(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Opts {
        Opts { csv_rows: 200 }
    }

    fn plain(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn binary_named_txt_is_a_stub() {
        let mut bytes = b"hello".to_vec();
        bytes.extend_from_slice(&[0, 1, 2, 3]);
        let p = Preview::build("notes.txt", &bytes, bytes.len() as u64, &opts());
        assert_eq!(p.kind, Kind::Stub);
        assert!(p.lines.iter().any(|l| plain(l).contains("binary")));
    }

    #[test]
    fn truncated_text_shows_banner() {
        let p = Preview::build("big.log", b"line one\nline two\n", 48 * 1024 * 1024, &opts());
        assert_eq!(p.banner.as_deref(), Some("showing first 18 B of 48 MB"));
    }

    #[test]
    fn unknown_extension_with_text_is_text() {
        let p = Preview::build("Makefile", b"all:\n\techo hi\n", 14, &opts());
        assert_eq!(p.kind, Kind::Text);
        assert!(plain(&p.lines[1]).contains("echo hi"));
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(256 * 1024), "256 KB");
        assert_eq!(human(48 * 1024 * 1024), "48 MB");
        assert_eq!(human(1536), "1.5 KB");
    }

    #[test]
    fn wrapping_keeps_gutter_and_splits_at_width() {
        let line = Line::from(vec![Span::raw("  1 "), Span::raw("abcdefghij")]);
        let w = wrap_line(&line, 4, 9); // room for 5 content columns
        assert_eq!(w.iter().map(plain).collect::<Vec<_>>(), vec!["  1 abcde", "    fghij"]);
    }

    #[test]
    fn horizontal_slice_keeps_gutter() {
        let line = Line::from(vec![Span::raw("  1 "), Span::raw("abcdefghij")]);
        assert_eq!(plain(&slice_line(&line, 4, 3, 9)), "  1 defgh");
    }

    fn git_view() -> Preview {
        let diff = (0..3).map(|i| format!("diff --git a/f{i} b/f{i}\n@@ -1 +1 @@\n-a\n+b\n")).collect::<String>();
        Preview::from_git(&diff::Snapshot { status: "## main\n".into(), diff, ..Default::default() })
    }

    #[test]
    fn n_and_p_jump_between_files() {
        let mut p = git_view();
        assert_eq!(p.kind, Kind::Diff);
        let starts = p.jumps.clone();
        assert_eq!(starts.len(), 3);
        p.navigate(keys::PvKey::NextFile, 3);
        assert_eq!(p.top, starts[0]);
        p.navigate(keys::PvKey::NextFile, 3);
        assert_eq!(p.top, starts[1]);
        p.navigate(keys::PvKey::PrevFile, 3);
        assert_eq!(p.top, starts[0]);
        p.navigate(keys::PvKey::PrevFile, 3);
        assert_eq!(p.top, starts[0], "no earlier file: stays put");
    }

    #[test]
    fn file_jumps_work_when_wrapped() {
        let mut p = git_view();
        p.toggle_wrap();
        p.layout(10, 3);
        p.navigate(keys::PvKey::NextFile, 3);
        let first = p.top;
        p.navigate(keys::PvKey::NextFile, 3);
        assert!(p.top > first);
    }

    #[test]
    fn truncated_git_view_has_banner() {
        let p = Preview::from_git(&diff::Snapshot { truncated: true, ..Default::default() });
        assert!(p.banner.unwrap().contains("truncated"));
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line number {i}\n")).collect()
    }

    fn has_mark(l: &Line<'_>) -> bool {
        l.spans.iter().any(|s| s.style.bg == Some(theme().goto_bg))
    }

    #[test]
    fn goto_line_scrolls_and_marks_the_line() {
        let body = numbered(100);
        let mut p = Preview::build("a.rs", body.as_bytes(), body.len() as u64, &opts());
        p.goto_line(50);
        p.layout(80, 20);
        assert_eq!(p.top, 49 - 20 / 3);
        let vis = p.visible(20, 80);
        let marked: Vec<String> = vis.iter().filter(|l| has_mark(l)).map(plain).collect();
        assert_eq!(marked.len(), 1);
        assert!(marked[0].contains("line number 50"), "{marked:?}");
    }

    #[test]
    fn goto_line_with_wrapping_uses_display_rows() {
        let long = "word ".repeat(40);
        let body: String = (1..=30).map(|i| format!("{i} {long}\n")).collect();
        let mut p = Preview::build("a.log", body.as_bytes(), body.len() as u64, &opts());
        p.toggle_wrap();
        p.goto_line(20);
        p.layout(40, 10);
        let marked = p.visible(10, 40).iter().filter(|l| has_mark(l)).count();
        assert!(marked >= 2, "a wrapped line is marked on every row it occupies");
        assert!(p.top > 19, "display rows are more than source lines");
    }

    #[test]
    fn goto_line_clamps_and_ignores_other_kinds() {
        let mut p = Preview::build("a.txt", b"one\ntwo\n", 8, &opts());
        p.goto_line(999);
        p.layout(80, 5);
        assert_eq!(p.target_line, Some(2));
        let mut m = Preview::build("a.md", b"# hi\n", 5, &opts());
        m.goto_line(1);
        assert_eq!(m.target_line, None);
    }

    fn lit(l: &Line<'_>) -> String {
        l.spans.iter().filter(|s| s.style.bg == Some(crate::theme::theme().hint_bg) || s.style.bg == Some(crate::theme::theme().diff_add)).map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn find_lights_matches_and_steps_through_them() {
        let body: String = (1..=60).map(|i| if i % 10 == 0 { format!("needle on line {i}\n") } else { format!("plain {i}\n") }).collect();
        let mut p = Preview::build("a.txt", body.as_bytes(), body.len() as u64, &opts());
        assert_eq!(p.set_find("NEEDLE"), 0, "capital letters match exactly");
        assert!(p.find.is_none());
        assert_eq!(p.set_find("needle"), 6);
        assert_eq!(p.find_label().as_deref(), Some("match 1 of 6"));
        p.layout(80, 6);
        assert_eq!(p.top, 9 - 6 / 3, "the first match is brought into view");
        let vis = p.visible(6, 80);
        assert_eq!(vis.iter().map(lit).filter(|s| !s.is_empty()).collect::<Vec<_>>(), vec!["needle"]);
        p.navigate(keys::PvKey::NextFile, 6);
        assert_eq!(p.find_label().as_deref(), Some("match 2 of 6"));
        p.layout(80, 6);
        assert!(p.visible(6, 80).iter().any(|l| plain(l).contains("line 20")));
        for _ in 0..5 {
            p.navigate(keys::PvKey::MatchNext, 6);
        }
        assert_eq!(p.find_label().as_deref(), Some("match 1 of 6"), "wraps around");
        p.navigate(keys::PvKey::MatchPrev, 6);
        assert_eq!(p.find_label().as_deref(), Some("match 6 of 6"));
        p.layout(80, 6);
        assert!(p.visible(6, 80).iter().any(|l| plain(l).contains("line 60")));
        p.clear_find();
        assert!(p.visible(6, 80).iter().all(|l| lit(l).is_empty()));
    }

    #[test]
    fn in_the_git_view_n_still_means_next_file() {
        let mut p = git_view();
        assert!(p.set_find("diff") > 0);
        let starts = p.jumps.clone();
        p.navigate(keys::PvKey::NextFile, 3);
        assert_eq!(p.top, starts[0], "N jumps files");
        let before = p.find_label();
        p.navigate(keys::PvKey::MatchNext, 3);
        assert_ne!(p.find_label(), before, "Enter steps matches");
    }

    #[test]
    fn matches_in_wrapped_text_map_to_display_rows() {
        let long = format!("{}needle{}", "a".repeat(37), "b".repeat(30));
        let line = Line::from(vec![Span::raw("  1 "), Span::raw(long.clone())]);
        // width 24, gutter 4: 20 content columns per row: needle starts at char 37 -> row 1, ends in row 2
        assert_eq!(wrap_row_of(&line, 4, 24, 0), 0);
        assert_eq!(wrap_row_of(&line, 4, 24, 19), 0);
        assert_eq!(wrap_row_of(&line, 4, 24, 20), 1);
        assert_eq!(wrap_row_of(&line, 4, 24, 37), 1);
        assert_eq!(wrap_row_of(&line, 4, 24, 40), 2);
        // it agrees with wrap_line: the row that contains the first character of the match
        let rows = wrap_line(&line, 4, 24);
        assert!(plain(&rows[wrap_row_of(&line, 4, 24, 37)]).contains(&long[37..40]));
        // end to end: the marked match is on the display row the preview scrolled to
        let body: String = (1..=30).map(|i| if i == 25 { format!("{long}\n") } else { format!("{i} {}\n", "word ".repeat(12)) }).collect();
        let mut p = Preview::build("a.log", body.as_bytes(), body.len() as u64, &opts());
        p.toggle_wrap();
        p.layout(30, 6);
        assert_eq!(p.set_find("needle"), 1);
        p.layout(30, 6);
        let vis = p.visible(6, 30);
        assert_eq!(vis.iter().map(lit).filter(|s| !s.is_empty()).collect::<Vec<_>>().concat(), "needle", "{:?}", vis.iter().map(plain).collect::<Vec<_>>());
    }

    #[test]
    fn find_pans_unwrapped_text_to_the_match() {
        let body = format!("{}needle\n", "x".repeat(200));
        let mut p = Preview::build("a.txt", body.as_bytes(), body.len() as u64, &opts());
        p.set_find("needle");
        p.layout(40, 5);
        assert!(p.left > 0);
        assert_eq!(p.visible(5, 40).iter().map(lit).collect::<Vec<_>>().concat(), "needle");
    }

    #[test]
    fn layout_clamps_scroll() {
        let mut p = Preview::build("a.txt", b"1\n2\n3\n", 6, &opts());
        p.top = 99;
        p.layout(80, 2);
        assert_eq!(p.top, 1);
    }
}
