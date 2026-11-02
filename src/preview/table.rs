//! CSV / TSV: an aligned table (RFC 4180 parsing), header styled, first N rows only.

use crate::theme::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const MAX_COL: usize = 40;

pub struct Table {
    pub lines: Vec<Line<'static>>,
    pub banner: Option<String>,
}

fn clip(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if flat.width() <= max {
        return flat;
    }
    let mut out = String::new();
    let mut w = 0;
    for c in flat.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

fn commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `rows` counts data rows (the header is extra). `total` is the full file size, `truncated`
/// says the text we were given is only a prefix of it.
pub fn render(body: &str, delim: u8, rows: usize, total: u64, truncated: bool) -> Table {
    let mut rdr = csv::ReaderBuilder::new().delimiter(delim).has_headers(false).flexible(true).from_reader(body.as_bytes());
    let mut recs: Vec<Vec<String>> = Vec::new();
    let mut bytes_at_limit = 0u64;
    let mut more_in_prefix = false;
    let mut r = csv::StringRecord::new();
    while rdr.read_record(&mut r).unwrap_or(false) {
        if recs.len() > rows {
            more_in_prefix = true;
            break;
        }
        recs.push(r.iter().map(|f| clip(f, MAX_COL)).collect());
        bytes_at_limit = rdr.position().byte();
    }
    if recs.is_empty() {
        return Table { lines: vec![Line::from("(empty file)")], banner: None };
    }
    let ncols = recs.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![1usize; ncols];
    for r in &recs {
        for (i, f) in r.iter().enumerate() {
            widths[i] = widths[i].max(f.width());
        }
    }
    let bold = Style::default().fg(theme().table_header).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let dim = Style::default().fg(theme().table_sep);
    let mut lines = Vec::new();
    for (ri, r) in recs.iter().enumerate() {
        let mut spans = Vec::new();
        for (i, w) in widths.iter().enumerate() {
            let f = r.get(i).map(String::as_str).unwrap_or("");
            let pad = " ".repeat(w.saturating_sub(f.width()));
            let st = if ri == 0 { bold } else { Style::default() };
            spans.push(Span::styled(format!("{f}{pad}"), st));
            if i + 1 < widths.len() {
                spans.push(Span::styled(" │ ", dim));
            }
        }
        lines.push(Line::from(spans));
    }
    let shown = recs.len().saturating_sub(1);
    let banner = if more_in_prefix || truncated {
        // Estimate the total from the average row size so far; marked approximate.
        let avg = if shown > 0 { bytes_at_limit as f64 / recs.len() as f64 } else { 0.0 };
        let est = if avg > 0.0 { ((total as f64 / avg) as u64).saturating_sub(1).max(shown as u64) } else { shown as u64 };
        Some(format!("{} of ~{} rows", commas(shown as u64), commas(est)))
    } else {
        None
    };
    Table { lines, banner }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn aligns_columns_and_handles_quotes() {
        let t = render("name,note\nann,\"hi, there\"\nbob,x\n", b',', 200, 30, false);
        let rows: Vec<String> = t.lines.iter().map(plain).collect();
        assert_eq!(rows[0], "name │ note     ");
        assert_eq!(rows[1], "ann  │ hi, there");
        assert!(t.banner.is_none());
    }

    #[test]
    fn embedded_newlines_stay_in_one_row() {
        let t = render("a,b\n\"x\ny\",2\n", b',', 200, 12, false);
        assert_eq!(t.lines.len(), 2);
    }

    #[test]
    fn caps_rows_and_estimates_total() {
        let mut s = String::from("id,v\n");
        for i in 0..1000 {
            s.push_str(&format!("{i},aaaa\n"));
        }
        let total = s.len() as u64 * 10; // pretend the file is 10x larger than what we read
        let t = render(&s, b',', 200, total, true);
        assert_eq!(t.lines.len(), 201);
        let b = t.banner.unwrap();
        assert!(b.starts_with("200 of ~"), "{b}");
    }

    #[test]
    fn tsv_and_ragged_rows() {
        let t = render("a\tb\n1\n", b'\t', 200, 6, false);
        assert_eq!(t.lines.len(), 2);
    }

    #[test]
    fn long_cells_are_clipped() {
        let long = "x".repeat(100);
        let t = render(&format!("{long}\n"), b',', 200, 101, false);
        assert!(plain(&t.lines[0]).chars().count() <= 40);
    }
}
