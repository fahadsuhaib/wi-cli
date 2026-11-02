//! Read-only file previews rendered on the client. The box (or disk) only serves bytes.

pub mod keys;
mod json;
mod markdown;
mod table;
mod text;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Markdown,
    Table,
    Json,
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
    wrapped: Option<(u16, Vec<Line<'static>>)>,
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
            wrapped: None,
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
                let w = self.lines.iter().flat_map(|l| wrap_line(l, g, width as usize)).collect();
                self.wrapped = Some((width, w));
            }
        } else {
            self.wrapped = None;
        }
        let max = self.display_len().saturating_sub(height.max(1));
        self.top = self.top.min(max);
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
            Reload | Back => return false,
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
        match (&self.wrapped, self.wrap) {
            (Some((_, w)), true) => w.iter().skip(self.top).take(height).cloned().collect(),
            _ => self.lines.iter().skip(self.top).take(height).map(|l| slice_line(l, self.gutter, self.left, width)).collect(),
        }
    }
}

fn stub_lines(path: &str, total: u64, kind: &str) -> Vec<Line<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
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

    #[test]
    fn layout_clamps_scroll() {
        let mut p = Preview::build("a.txt", b"1\n2\n3\n", 6, &opts());
        p.top = 99;
        p.layout(80, 2);
        assert_eq!(p.top, 1);
    }
}
