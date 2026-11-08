//! Rendered markdown: a parsed document laid out for a given pane width. The renderer does all
//! its own wrapping (hanging indents for lists and quotes, panels for code, boxes for tables).

mod mermaid;
mod parse;

pub use parse::{Doc, parse};

/// True while a mermaid diagram is being drawn under `catch_unwind` (the panic hook stays quiet).
/// Whether the document has a mermaid block anywhere (so `D` means something).
pub fn has_mermaid(doc: &Doc) -> bool {
    fn any(nodes: &[Node]) -> bool {
        nodes.iter().any(|n| match &n.block {
            Block::Code { lang, .. } => mermaid::is_mermaid(lang),
            Block::Quote(inner) => any(inner),
            Block::List { items, .. } => items.iter().any(|i| any(&i.blocks)),
            _ => false,
        })
    }
    any(&doc.nodes)
}

pub fn mermaid_guard_active() -> bool {
    mermaid::in_guard()
}
use parse::{Block, Fl, Inl, Inlines, Item, Node};
use crate::theme::theme;
use pulldown_cmark::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

type Cell = (char, Style);
type Row = Vec<Span<'static>>;

/// Widest text column; wider panes keep a readable measure.
const MAX_WIDTH: usize = 100;

pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    /// For each display line, the 0-based source line of the top-level block it belongs to.
    pub row_src: Vec<usize>,
}

fn cw(c: char) -> usize {
    c.width().unwrap_or(0)
}

fn spans(cells: &[Cell]) -> Row {
    let mut out: Row = Vec::new();
    let mut run = String::new();
    let mut cur: Option<Style> = None;
    for &(c, st) in cells {
        if cur != Some(st) {
            if let Some(s) = cur {
                out.push(Span::styled(std::mem::take(&mut run), s));
            }
            cur = Some(st);
        }
        run.push(c);
    }
    if let Some(s) = cur {
        out.push(Span::styled(run, s));
    }
    out
}

fn seg_style(base: Style, fl: Fl) -> Style {
    let t = theme();
    let mut s = base;
    if fl.code {
        s = s.fg(t.md_code).bg(t.md_code_bg);
    }
    if fl.link {
        s = s.fg(t.md_link).add_modifier(Modifier::UNDERLINED);
    }
    if fl.bold {
        s = s.add_modifier(Modifier::BOLD);
    }
    if fl.italic {
        s = s.add_modifier(Modifier::ITALIC);
    }
    if fl.strike {
        s = s.add_modifier(Modifier::CROSSED_OUT);
    }
    if fl.dim {
        s = s.fg(t.dim);
    }
    s
}

fn trim_end(cells: &mut Vec<Cell>) {
    while cells.last().is_some_and(|c| c.0 == ' ') {
        cells.pop();
    }
}

fn cells_w(cells: &[Cell]) -> usize {
    cells.iter().map(|c| cw(c.0)).sum()
}

/// Greedy word wrap of inline runs into rows of at most `w` columns. Breaks at spaces and around
/// wide characters; a word longer than the row is cut.
fn wrap_cells(inl: &[Inl], w: usize, base: Style) -> Vec<Vec<Cell>> {
    let w = w.max(1);
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut width = 0usize;
    let mut brk: Option<usize> = None;
    for i in inl {
        match i {
            Inl::Break => {
                trim_end(&mut cur);
                rows.push(std::mem::take(&mut cur));
                width = 0;
                brk = None;
            }
            Inl::Text(t, fl) => {
                let st = seg_style(base, *fl);
                for ch in t.chars() {
                    let ch = if ch.is_control() { ' ' } else { ch };
                    let w1 = cw(ch);
                    if ch == ' ' && (cur.is_empty() || !fl.code && cur.last().is_some_and(|c| c.0 == ' ')) {
                        continue;
                    }
                    while width + w1 > w && !cur.is_empty() {
                        if ch == ' ' {
                            trim_end(&mut cur);
                            rows.push(std::mem::take(&mut cur));
                            width = 0;
                            brk = None;
                            break;
                        }
                        match brk {
                            Some(b) if b > 0 && b <= cur.len() => {
                                let tail = cur.split_off(b);
                                trim_end(&mut cur);
                                rows.push(std::mem::replace(&mut cur, tail));
                                width = cells_w(&cur);
                            }
                            _ => {
                                trim_end(&mut cur);
                                rows.push(std::mem::take(&mut cur));
                                width = 0;
                            }
                        }
                        brk = None;
                    }
                    if ch == ' ' && cur.is_empty() {
                        continue;
                    }
                    if w1 == 2 {
                        brk = Some(cur.len());
                    }
                    cur.push((ch, st));
                    width += w1;
                    if ch == ' ' || w1 == 2 {
                        brk = Some(cur.len());
                    }
                }
            }
        }
    }
    trim_end(&mut cur);
    if !cur.is_empty() {
        rows.push(cur);
    }
    rows
}

fn wrap_rows(inl: &[Inl], w: usize, base: Style) -> Vec<Row> {
    wrap_cells(inl, w, base).iter().map(|r| spans(r)).collect()
}

/// Put `first` before the first row and `rest` before the others. Blank rows only get the prefix
/// when `blanks` is set (quote bars run through blank lines, indentation need not).
fn indent(rows: Vec<Row>, first: Span<'static>, rest: Span<'static>, blanks: bool) -> Vec<Row> {
    rows.into_iter()
        .enumerate()
        .map(|(i, mut r)| {
            if i == 0 || blanks || !r.is_empty() {
                r.insert(0, if i == 0 { first.clone() } else { rest.clone() });
            }
            r
        })
        .collect()
}

fn pad(n: usize) -> Span<'static> {
    Span::raw(" ".repeat(n))
}

fn seq(nodes: &[Node], w: usize, depth: usize, base: Style, gap: bool, dg: bool) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for n in nodes {
        let r = block_rows(&n.block, w, depth, base, dg);
        if r.is_empty() {
            continue;
        }
        if gap && !out.is_empty() {
            out.push(Vec::new());
        }
        out.extend(r);
    }
    out
}

fn rule_row(ch: char, w: usize, color: ratatui::style::Color) -> Row {
    vec![Span::styled(ch.to_string().repeat(w), Style::default().fg(color))]
}

fn block_rows(b: &Block, w: usize, depth: usize, base: Style, dg: bool) -> Vec<Row> {
    let t = theme();
    match b {
        Block::Heading(level, inl) => {
            let bold = Style::default().add_modifier(Modifier::BOLD);
            let st = match level {
                1 => bold.fg(t.md_h1),
                2 => bold.fg(t.md_h2),
                3 => bold.fg(t.md_h3),
                4 => bold.fg(t.dim),
                5 => bold.fg(t.dim).add_modifier(Modifier::ITALIC),
                _ => Style::default().fg(t.dim).add_modifier(Modifier::ITALIC),
            };
            let mut rows = wrap_rows(inl, w, st);
            if rows.is_empty() {
                return rows;
            }
            match level {
                1 => rows.push(rule_row('═', w, t.md_h1)),
                2 => rows.push(rule_row('─', w, t.md_rule)),
                _ => {}
            }
            rows
        }
        Block::Para(inl) | Block::Text(inl) => wrap_rows(inl, w, base),
        Block::Rule => vec![rule_row('─', w, t.md_rule)],
        Block::Quote(nodes) => {
            let inner = seq(nodes, w.saturating_sub(2).max(1), depth, base.fg(t.md_quote), true, dg);
            let bar = Span::styled("▌ ", Style::default().fg(t.md_quote_bar));
            indent(inner, bar.clone(), bar, true)
        }
        Block::List { start, items, loose } => list_rows(*start, items, *loose, w, depth, base, dg),
        Block::Code { lang, text } if dg && mermaid::is_mermaid(lang) => mermaid_rows(lang, text, w),
        Block::Code { lang, text } => code_rows(lang, text, w),
        Block::Table { align, rows } => table_rows(align, rows, w, base),
        Block::Html(s) => {
            if s.trim_start().starts_with("<!--") {
                return Vec::new();
            }
            let mut inl: Inlines = Vec::new();
            for (i, l) in s.trim_end().lines().enumerate() {
                if i > 0 {
                    inl.push(Inl::Break);
                }
                inl.push(Inl::Text(l.trim_end().to_string(), Fl { dim: true, ..Fl::default() }));
            }
            wrap_rows(&inl, w, base)
        }
    }
}

fn list_rows(start: Option<u64>, items: &[Item], loose: bool, w: usize, depth: usize, base: Style, dg: bool) -> Vec<Row> {
    let t = theme();
    let last = start.map(|s| s + items.len().saturating_sub(1) as u64);
    let digits = last.map(|n| n.to_string().len()).unwrap_or(0);
    let mut out: Vec<Row> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let (marker, st) = match it.task {
            Some(true) => ("☑ ".to_string(), Style::default().fg(t.md_task_done)),
            Some(false) => ("☐ ".to_string(), Style::default().fg(t.md_task_open)),
            None => match start {
                Some(s) => (format!("{:>digits$}. ", s + i as u64), Style::default().fg(t.md_bullet)),
                None => (format!("{} ", ["•", "◦", "▪"][depth % 3]), Style::default().fg(t.md_bullet)),
            },
        };
        let mw: usize = marker.chars().map(cw).sum();
        let inner = seq(&it.blocks, w.saturating_sub(mw).max(1), depth + 1, base, loose, dg);
        let rows = if inner.is_empty() { vec![Vec::new()] } else { inner };
        if loose && !out.is_empty() {
            out.push(Vec::new());
        }
        out.extend(indent(rows, Span::styled(marker, st), pad(mw), false));
    }
    out
}

fn code_rows(lang: &str, text: &str, w: usize) -> Vec<Row> {
    let t = theme();
    let st = Style::default().fg(t.md_code).bg(t.md_code_bg);
    let inner = w.saturating_sub(2).max(1);
    let mut out: Vec<Row> = Vec::new();
    if !lang.is_empty() {
        out.push(vec![Span::styled(lang.to_string(), Style::default().fg(t.dim))]);
    }
    let body = text.trim_end_matches('\n');
    for line in body.split('\n') {
        let line = line.trim_end_matches('\r').replace('\t', "    ");
        let mut chunk = String::new();
        let mut width = 0;
        let mut chunks: Vec<(String, usize)> = Vec::new();
        for c in line.chars() {
            let c = if c.is_control() { '·' } else { c };
            let c1 = cw(c);
            if width + c1 > inner && !chunk.is_empty() {
                chunks.push((std::mem::take(&mut chunk), width));
                width = 0;
            }
            chunk.push(c);
            width += c1;
        }
        chunks.push((chunk, width));
        for (c, cwid) in chunks {
            let fill = w.max(inner + 2).saturating_sub(cwid + 2);
            out.push(vec![Span::styled(format!(" {c}{} ", " ".repeat(fill)), st)]);
        }
    }
    out
}

/// A mermaid block as a bordered text diagram, or its source plus a dim note when it cannot be drawn.
fn mermaid_rows(lang: &str, text: &str, w: usize) -> Vec<Row> {
    let t = theme();
    match mermaid::render(text, w) {
        Ok(d) => {
            let dim = Style::default().fg(t.dim);
            let content = d.lines.iter().map(|l| l.width()).max().unwrap_or(0);
            let title = format!(" mermaid · {} ", d.title);
            let inner = (content + 2).max(title.width() + 2);
            let mut out: Vec<Row> = Vec::new();
            out.push(vec![Span::styled(format!("╭─{title}{}╮", "─".repeat(inner.saturating_sub(title.width() + 1))), dim)]);
            for l in &d.lines {
                let fill = inner.saturating_sub(l.width() + 1);
                out.push(vec![
                    Span::styled("│ ", dim),
                    Span::raw(l.clone()),
                    Span::styled(format!("{}│", " ".repeat(fill)), dim),
                ]);
            }
            out.push(vec![Span::styled(format!("╰{}╯", "─".repeat(inner)), dim)]);
            out
        }
        Err(why) => {
            let mut out = code_rows(lang, text, w);
            let note = vec![Inl::Text(format!("mermaid: not rendered ({why})"), Fl { dim: true, ..Fl::default() })];
            out.extend(wrap_rows(&note, w, Style::default()));
            out
        }
    }
}

fn flat_width(inl: &[Inl]) -> usize {
    inl.iter()
        .map(|i| match i {
            Inl::Text(t, _) => t.chars().map(cw).sum(),
            Inl::Break => 1,
        })
        .sum()
}

fn table_rows(align: &[Alignment], rows: &[Vec<Inlines>], w: usize, base: Style) -> Vec<Row> {
    let t = theme();
    let n = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if n == 0 {
        return Vec::new();
    }
    let mut cols = vec![3usize; n];
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            cols[i] = cols[i].max(flat_width(c));
        }
    }
    let mut total: usize = cols.iter().sum::<usize>() + 3 * n + 1;
    while total > w {
        let (idx, &widest) = cols.iter().enumerate().max_by_key(|(_, c)| **c).unwrap_or((0, &3));
        if widest <= 3 {
            break;
        }
        cols[idx] -= 1;
        total -= 1;
    }
    let border = Style::default().fg(t.table_sep);
    let line = |l: &str, m: &str, r: &str| -> Row {
        let body: Vec<String> = cols.iter().map(|c| "─".repeat(c + 2)).collect();
        vec![Span::styled(format!("{l}{}{r}", body.join(m)), border)]
    };
    let mut out: Vec<Row> = vec![line("┌", "┬", "┐")];
    for (ri, r) in rows.iter().enumerate() {
        let st = if ri == 0 { base.fg(t.table_header).add_modifier(Modifier::BOLD) } else { base };
        let wrapped: Vec<Vec<Vec<Cell>>> = (0..n).map(|i| r.get(i).map(|c| wrap_cells(c, cols[i], st)).unwrap_or_default()).collect();
        let height = wrapped.iter().map(|c| c.len()).max().unwrap_or(1).max(1);
        for k in 0..height {
            let mut row: Row = vec![Span::styled("│", border)];
            for (i, cell) in wrapped.iter().enumerate() {
                let empty = Vec::new();
                let cells = cell.get(k).unwrap_or(&empty);
                let free = cols[i].saturating_sub(cells_w(cells));
                let (l, rr) = match align.get(i).copied().unwrap_or(Alignment::None) {
                    Alignment::Right => (free, 0),
                    Alignment::Center => (free / 2, free - free / 2),
                    _ => (0, free),
                };
                row.push(pad(1 + l));
                row.extend(spans(cells));
                row.push(pad(1 + rr));
                row.push(Span::styled("│", border));
            }
            out.push(row);
        }
        if ri == 0 && rows.len() > 1 {
            out.push(line("├", "┼", "┤"));
        }
    }
    out.push(line("└", "┴", "┘"));
    out
}

fn front_rows(lines: &[String], w: usize) -> Vec<Row> {
    let t = theme();
    let border = Style::default().fg(t.md_rule);
    let text = Style::default().fg(t.md_front);
    let inner = w.saturating_sub(4).max(1);
    let head = "─ metadata ";
    let mut out: Vec<Row> = vec![vec![Span::styled(format!("┌{head}{}┐", "─".repeat(w.saturating_sub(2 + head.chars().count()))), border)]];
    for l in lines {
        let l = l.replace('\t', "    ");
        let mut cells: Vec<Cell> = Vec::new();
        let mut rows: Vec<Vec<Cell>> = Vec::new();
        let mut width = 0;
        for c in l.chars() {
            if width + cw(c) > inner && !cells.is_empty() {
                rows.push(std::mem::take(&mut cells));
                width = 0;
            }
            cells.push((c, text));
            width += cw(c);
        }
        rows.push(cells);
        for r in rows {
            let free = inner.saturating_sub(cells_w(&r));
            let mut row: Row = vec![Span::styled("│ ", border)];
            row.extend(spans(&r));
            row.push(Span::styled(format!("{} │", " ".repeat(free)), border));
            out.push(row);
        }
    }
    out.push(vec![Span::styled(format!("└{}┘", "─".repeat(w.saturating_sub(2))), border)]);
    out
}

/// Lay the document out for a pane `width` columns wide.
pub fn render(doc: &Doc, width: usize) -> Rendered {
    render_with(doc, width, true)
}

/// Like [`render`]; `diagrams` false shows mermaid blocks as plain source.
pub fn render_with(doc: &Doc, width: usize, diagrams: bool) -> Rendered {
    let w = width.saturating_sub(2).clamp(4, MAX_WIDTH);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut row_src: Vec<usize> = Vec::new();
    let mut push = |rows: Vec<Row>, src: usize, lines: &mut Vec<Line<'static>>| {
        if rows.is_empty() {
            return;
        }
        if !lines.is_empty() {
            lines.push(Line::from(""));
            row_src.push(src);
        }
        for mut r in rows {
            if !r.is_empty() {
                r.insert(0, Span::raw(" "));
            }
            lines.push(Line::from(r));
            row_src.push(src);
        }
    };
    if let Some(f) = doc.front.as_ref().filter(|f| !f.is_empty()) {
        push(front_rows(f, w), 0, &mut lines);
    }
    for n in &doc.nodes {
        push(block_rows(&n.block, w, 0, Style::default(), diagrams), n.line, &mut lines);
    }
    Rendered { lines, row_src }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::theme;

    fn plain(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().trim_end().to_string()
    }

    fn at(src: &str, width: usize) -> Vec<String> {
        render(&parse(src), width).lines.iter().map(plain).collect()
    }

    fn text(src: &str) -> Vec<String> {
        at(src, 80)
    }

    fn lines(src: &str, width: usize) -> Vec<Line<'static>> {
        render(&parse(src), width).lines
    }

    #[test]
    fn headings_have_no_markers_and_rules() {
        let t = text("# Title\n\n## Sub\n\n### Third\n\n#### Four\n\ntext\n");
        assert_eq!(t[0], " Title");
        assert_eq!(t[1], format!(" {}", "═".repeat(78)));
        assert_eq!(t[2], "");
        assert_eq!(t[3], " Sub");
        assert_eq!(t[4], format!(" {}", "─".repeat(78)));
        assert_eq!(t[6], " Third");
        assert_eq!(t[7], "", "blank after H3, no rule");
        assert_eq!(t[8], " Four");
        assert!(t.iter().all(|l| !l.contains('#')));
        let l = lines("# T\n\n## S\n\n### U\n\n#### V\n", 80);
        let fg = |i: usize| l[i].spans[1].style.fg;
        assert_eq!(fg(0), Some(theme().md_h1));
        assert!(l[0].spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(fg(3), Some(theme().md_h2));
        assert_eq!(fg(6), Some(theme().md_h3));
        assert_eq!(fg(8), Some(theme().dim));
    }

    #[test]
    fn content_width_is_capped_at_100_with_a_margin() {
        let t = at("# T\n", 300);
        assert_eq!(t[1].chars().count(), 101);
        let t = at("# T\n", 40);
        assert_eq!(t[1].chars().count(), 39);
    }

    #[test]
    fn front_matter_is_a_metadata_block() {
        let t = text("---\ntitle: Demo\ntags: [a, b]\n---\n\n# Hi\n");
        assert!(t[0].starts_with(" ┌─ metadata ") && t[0].ends_with('┐'));
        assert!(t[1].starts_with(" │ title: Demo") && t[1].ends_with('│'));
        assert!(t[2].contains("tags: [a, b]"));
        assert!(t[3].starts_with(" └") && t[3].ends_with('┘'));
        assert!(t.iter().any(|l| l == " Hi"));
        assert!(!t.iter().any(|l| l.contains("## ") || l.contains("title: Demo") && !l.contains('│')));
        let widths: Vec<usize> = t[..4].iter().map(|l| l.chars().count()).collect();
        assert!(widths.iter().all(|w| *w == widths[0]), "box is rectangular: {widths:?}");
        // only at the very top
        let t = text("text\n\n---\nnot: front\n---\n");
        assert!(!t.iter().any(|l| l.contains("metadata")));
    }

    #[test]
    fn inline_styles() {
        let l = lines("a **b** *c* ~~d~~ `e` [f](https://x.dev) [g](g) ![alt](i.png) <https://y.dev>\n", 100);
        assert_eq!(plain(&l[0]), " a b c d e f (https://x.dev) g [image: alt] https://y.dev");
        let style_of = |ch: &str| l[0].spans.iter().find(|s| s.content.contains(ch)).map(|s| s.style).unwrap();
        assert!(style_of("b").add_modifier.contains(Modifier::BOLD));
        assert!(style_of("c").add_modifier.contains(Modifier::ITALIC));
        assert!(style_of("d").add_modifier.contains(Modifier::CROSSED_OUT));
        assert_eq!(style_of("e").bg, Some(theme().md_code_bg));
        assert_eq!(style_of("f").fg, Some(theme().md_link));
        assert!(style_of("f").add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(style_of("(https://x.dev)").fg, Some(theme().dim));
        assert_eq!(style_of("[image: alt]").fg, Some(theme().dim));
    }

    #[test]
    fn soft_and_hard_breaks_and_html() {
        let t = text("one\ntwo  \nthree\\\nfour\n\nx<br>y <b>bold</b> <!-- gone --> z\n\n<!-- hidden -->\n\n<div>block</div>\n");
        assert_eq!(t[0], " one two");
        assert_eq!(t[1], " three");
        assert_eq!(t[2], " four");
        assert_eq!(t[4], " x");
        assert_eq!(t[5], " y bold  z".replace("  z", " z"));
        assert!(!t.iter().any(|l| l.contains("hidden") || l.contains('<') && !l.contains("<div>")));
        assert!(t.contains(&" <div>block</div>".to_string()));
        let l = lines("<div>block</div>\n", 80);
        assert_eq!(l[0].spans[1].style.fg, Some(theme().dim));
    }

    #[test]
    fn bullets_by_depth_and_hanging_indent() {
        let src = "- one that is long enough to wrap around\n  - nested item that is also long enough to wrap\n    - deep\n- two\n";
        let t = at(src, 24);
        assert_eq!(t[0], " • one that is long");
        assert_eq!(t[1], "   enough to wrap");
        assert_eq!(t[2], "   around");
        assert_eq!(t[3], "   ◦ nested item that");
        assert_eq!(t[4], "     is also long");
        assert_eq!(t[5], "     enough to wrap");
        assert_eq!(t[6], "     ▪ deep");
        assert_eq!(t[7], " • two");
        assert_eq!(t.len(), 8, "tight list: no blank lines");
        // several widths: every continuation row keeps the hanging indent
        for w in [12, 18, 30, 60] {
            let t = at("- alpha beta gamma delta epsilon zeta eta theta iota kappa\n", w);
            assert!(t[0].starts_with(" • "));
            assert!(t[1..].iter().all(|l| l.starts_with("   ") && !l.starts_with("    ")), "{w}: {t:?}");
            assert!(t.iter().all(|l| l.chars().count() <= w.max(6)), "{w}: {t:?}");
        }
    }

    #[test]
    fn ordered_lists_right_align_numbers() {
        let items: String = (8..=11).map(|i| format!("{i}. item\n")).collect();
        let t = text(&items);
        assert_eq!(t[0], "  8. item");
        assert_eq!(t[1], "  9. item");
        assert_eq!(t[2], " 10. item");
        assert_eq!(t[3], " 11. item");
        let t = at("1. alpha beta gamma delta\n2. b\n", 16);
        assert_eq!(t[0], " 1. alpha beta");
        assert_eq!(t[1], "    gamma delta");
    }

    #[test]
    fn task_items() {
        let l = lines("- [x] done\n- [ ] todo\n", 80);
        assert_eq!(plain(&l[0]), " ☑ done");
        assert_eq!(plain(&l[1]), " ☐ todo");
        assert_eq!(l[0].spans[1].style.fg, Some(theme().md_task_done));
        assert_eq!(l[1].spans[1].style.fg, Some(theme().md_task_open));
        assert!(!plain(&l[0]).contains('['));
    }

    #[test]
    fn loose_lists_have_blank_lines_between_items() {
        let t = text("- a\n\n- b\n\n- c\n");
        assert_eq!(t, vec![" • a", "", " • b", "", " • c"]);
        let t = text("- a\n- b\n");
        assert_eq!(t, vec![" • a", " • b"]);
        let t = text("1. a\n\n   more\n2. b\n");
        assert_eq!(t[0], " 1. a");
        assert_eq!(t[1], "");
        assert_eq!(t[2], "    more");
    }

    #[test]
    fn quotes_have_a_bar_on_every_wrapped_row_and_nest() {
        let t = at("> a quote that is long enough to wrap twice or more\n>\n> > nested quote text goes here\n", 20);
        assert!(t.iter().all(|l| l.starts_with(" ▌")), "{t:?}");
        let rows: Vec<&String> = t.iter().filter(|l| !l.is_empty()).collect();
        assert!(rows.len() >= 5, "{t:?}");
        assert!(rows.iter().any(|l| l.starts_with(" ▌ ▌ ")), "nested bars: {t:?}");
        assert!(t.contains(&" ▌".to_string()), "bar continues through the blank line: {t:?}");
        let l = lines("> wise\n", 80);
        assert_eq!(l[0].spans[1].style.fg, Some(theme().md_quote_bar));
        assert_eq!(l[0].spans[2].style.fg, Some(theme().md_quote));
    }

    #[test]
    fn code_block_panel() {
        let l = lines("```rust\nfn main() {\n\tlet x = 1;\n}\n```\n", 40);
        assert_eq!(plain(&l[0]), " rust");
        let panel = &l[1];
        assert_eq!(panel.spans[1].style.bg, Some(theme().md_code_bg));
        let s: String = panel.spans[1..].iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(s.chars().count(), 38, "panel fills the content width");
        assert!(s.starts_with(" fn main() {"));
        assert!(plain(&l[2]).contains("     let x = 1;"), "tab became 4 spaces: {:?}", plain(&l[2]));
        assert_eq!(l.len(), 4);
    }

    #[test]
    fn long_code_lines_wrap_inside_the_panel() {
        let l = lines("```\nabcdefghijklmnopqrstuvwxyz\n```\n", 16);
        // content width 14, panel inner 12
        assert_eq!(l.len(), 3);
        for row in &l {
            assert_eq!(row.spans[1].style.bg, Some(theme().md_code_bg), "continuation keeps the panel");
            let s: String = row.spans[1..].iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(s.chars().count(), 14);
        }
        assert_eq!(plain(&l[0]), "  abcdefghijkl");
        assert_eq!(plain(&l[1]), "  mnopqrstuvwx");
        assert_eq!(plain(&l[2]), "  yz");
    }

    #[test]
    fn tables_have_borders_alignment_and_bold_header() {
        let t = text("| name | n | c |\n|:-----|--:|:-:|\n| ab | 5 | x |\n| abcd | 1234 | yy |\n");
        assert_eq!(
            t,
            vec![
                " ┌──────┬──────┬─────┐",
                " │ name │    n │  c  │",
                " ├──────┼──────┼─────┤",
                " │ ab   │    5 │  x  │",
                " │ abcd │ 1234 │ yy  │",
                " └──────┴──────┴─────┘",
            ]
        );
        let l = lines("| a | b |\n|---|---|\n| 1 | 2 |\n", 80);
        assert!(l[1].spans.iter().any(|s| s.content.contains('a') && s.style.add_modifier.contains(Modifier::BOLD)));
        assert!(!l[3].spans.iter().any(|s| s.style.add_modifier.contains(Modifier::BOLD)));
    }

    #[test]
    fn wide_tables_shrink_widest_columns_and_wrap_cells() {
        let src = "| id | description |\n|---|---|\n| 1 | a rather long description that needs several rows |\n";
        let t = at(src, 30);
        assert!(t.iter().all(|l| l.chars().count() <= 30), "{t:?}");
        assert!(t.len() > 5, "cell wrapped over rows: {t:?}");
        assert!(t[1].contains("id") && t[1].contains("description"));
        assert!(t.iter().any(|l| l.contains("long")) && t.iter().any(|l| l.contains("rows")));
        // the narrow column is untouched
        assert!(t[3].starts_with(" │ 1   │"), "{t:?}");
        // many columns: never below 3
        let t = at("| aaaa | bbbb | cccc |\n|--|--|--|\n| 1 | 2 | 3 |\n", 10);
        assert!(t[0].starts_with(" ┌─────┬─────┬─────┐"), "{t:?}");
    }

    #[test]
    fn horizontal_rule_is_full_width() {
        let t = text("a\n\n---\n\nb\n");
        assert_eq!(t[2], format!(" {}", "─".repeat(78)));
    }

    #[test]
    fn wide_characters_wrap_by_columns() {
        let t = at("你好世界你好世界你好\n", 12);
        assert!(t.len() >= 2);
        for l in &t {
            let w: usize = l.chars().map(cw).sum();
            assert!(w <= 12, "{l:?} is {w} wide");
        }
        assert_eq!(t.concat().replace(' ', ""), "你好世界你好世界你好");
        let t = at("- 你好世界你好世界你好\n", 12);
        assert!(t.iter().skip(1).all(|l| l.starts_with("   ")));
        let t = text("| 名前 | x |\n|---|---|\n| 太郎 | y |\n");
        assert_eq!(t[0].chars().map(cw).sum::<usize>(), t[3].chars().map(cw).sum::<usize>());
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(text("").is_empty());
    }

    #[test]
    fn source_lines_follow_the_blocks() {
        let r = render(&parse("---\na: b\n---\n# T\n\npara\n\n- x\n"), 80);
        assert_eq!(r.row_src.len(), r.lines.len());
        assert!(r.row_src.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(*r.row_src.first().unwrap(), 0);
        assert_eq!(*r.row_src.last().unwrap(), 7);
        let r = render(&parse("# T\n\npara\n"), 80);
        assert_eq!(r.row_src[r.lines.len() - 1], 2);
    }

    fn big_doc() -> String {
        let mut s = String::from("---\ntitle: big\n---\n\n");
        let mut i = 0;
        while s.len() < 256 * 1024 {
            s.push_str(&format!(
                "## Section {i}\n\nSome *emphasis* and `code` plus a [link](https://example.com/{i}) in a paragraph that is long enough to wrap a couple of times when the pane is narrow.\n\n- item one\n- [x] done item\n  - nested **bold** item\n\n> quoted text for section {i}\n\n```rust\nfn f{i}() {{ println!(\"hello\"); }}\n```\n\n| a | b |\n|:--|--:|\n| {i} | {} |\n\n",
                i * 2
            ));
            i += 1;
        }
        s
    }

    #[test]
    fn rerendering_a_256_kb_document_is_fast() {
        let src = big_doc();
        let doc = parse(&src);
        let t0 = std::time::Instant::now();
        for w in [60, 100, 140] {
            let r = render(&doc, w);
            assert!(r.lines.len() > 1000);
        }
        let per = t0.elapsed() / 3;
        assert!(per.as_millis() < 500, "render took {per:?}");
        let t0 = std::time::Instant::now();
        let _ = parse(&src);
        assert!(t0.elapsed().as_millis() < 1500, "parse took {:?}", t0.elapsed());
    }

    #[test]
    fn width_changes_the_wrapping() {
        let src = "- a long list item with many words that will wrap at narrow widths for sure\n";
        assert!(at(src, 100).len() < at(src, 30).len());
    }

    fn md(block: &str) -> String {
        format!("# Doc\n\n```mermaid\n{block}```\n\nafter\n")
    }

    const GOLDEN: &[(&str, &str)] = &[
        ("flow_lr", "flowchart LR\n  A[Checkout] --> B{Tests pass?}\n  B -- yes --> C([Deploy])\n  B -- no --> D[Notify]\n"),
        ("flow_td_subgraph", "graph TD\n  subgraph Build\n    A[Compile] --> B[Link]\n  end\n  B --> C[Package]\n  C --> A\n"),
        ("sequence", "sequenceDiagram\n  participant U as User\n  participant W as wi\n  U->>W: open file\n  W-->>U: preview\n"),
        ("class", "classDiagram\n  class Animal {\n    +String name\n    +eat()\n  }\n  class Dog {\n    +bark()\n  }\n  Animal <|-- Dog\n"),
        ("state", "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Running: start\n  Running --> Idle: stop\n"),
        ("er", "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n  ORDER ||--|{ LINE_ITEM : contains\n"),
    ];

    /// Rendered text of each sample at 80 and 120 columns against `golden/<name>_<width>.txt`.
    /// `UPDATE_GOLDEN=1 cargo test mermaid_golden` rewrites the files.
    #[test]
    fn mermaid_golden_text() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/preview/markdown/golden");
        for (name, block) in GOLDEN {
            for width in [80, 120] {
                let got = at(&md(block), width).join("\n") + "\n";
                let file = dir.join(format!("{name}_{width}.txt"));
                if std::env::var("UPDATE_GOLDEN").is_ok() {
                    std::fs::write(&file, &got).unwrap();
                }
                let want = std::fs::read_to_string(&file).unwrap_or_default();
                assert_eq!(got, want, "{name} at {width} columns differs from {}", file.display());
                assert!(got.contains("╭─ mermaid · "), "{name}: bordered block with a title");
                assert!(!got.contains("not rendered"), "{name}: drawn, not source");
            }
        }
    }

    #[test]
    fn mermaid_block_is_bordered_and_dim() {
        let l = lines(&md("graph LR\n  A --> B\n"), 80);
        let top = l.iter().position(|l| plain(l).contains("mermaid · flowchart")).expect("title row");
        assert_eq!(l[top].spans[1].style.fg, Some(theme().dim));
        assert_eq!(l[top + 1].spans[2].style.fg, None, "diagram text uses the theme text colour");
        let t = at(&md("graph LR\n  A --> B\n"), 80);
        let wid = |s: &String| s.chars().count();
        assert_eq!(wid(&t[top]), wid(&t[top + 1]), "box rows are as wide as the top border");
    }

    #[test]
    fn mermaid_failures_show_source_and_a_note() {
        let note = |src: &str, width: usize| at(&md(src), width);
        let t = note("graph LR\n  A --> B\n", 30);
        assert!(t.iter().any(|l| l.contains("graph LR")), "narrow pane shows source");
        let joined = t.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(joined.contains("mermaid: not rendered (pane under 40 columns)"), "wrapped note: {t:?}");
        let t = note("whatever is this\n", 80);
        assert!(t.iter().any(|l| l.contains("whatever is this")));
        assert!(t.iter().any(|l| l.contains("mermaid: not rendered (unknown diagram type 'whatever')")), "{t:?}");
        let t = note("", 80);
        assert!(t.iter().any(|l| l.contains("mermaid: not rendered (empty diagram)")), "{t:?}");
        let note_line = lines(&md("nonsense\n"), 80).into_iter().find(|l| plain(l).contains("not rendered")).unwrap();
        assert_eq!(note_line.spans[1].style.fg, Some(theme().dim));
        assert!(!t.iter().any(|l| l.contains('╭') && l.contains("mermaid ·")));
    }

    #[test]
    fn mermaid_off_shows_plain_source_without_note() {
        let doc = parse(&md("graph LR\n  A --> B\n"));
        assert!(has_mermaid(&doc));
        let off: Vec<String> = render_with(&doc, 80, false).lines.iter().map(plain).collect();
        assert!(off.iter().any(|l| l.contains("graph LR")));
        assert!(!off.iter().any(|l| l.contains("not rendered") || l.contains('╭')));
        assert!(!has_mermaid(&parse("```rust\nfn main() {}\n```\n")));
    }

    #[test]
    fn mermaid_in_quote_and_list_and_several_blocks() {
        let src = "> ```mermaid\n> graph LR\n>   A --> B\n> ```\n\n- item\n\n  ```mermaid\n  graph TD\n    X --> Y\n  ```\n\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n";
        assert!(has_mermaid(&parse(src)));
        let t = text(src);
        assert_eq!(t.iter().filter(|l| l.contains("╭─ mermaid · ")).count(), 3, "{t:?}");
        assert!(t.iter().any(|l| l.starts_with(" ▌ ") && l.contains("╭─")), "inside the quote");
    }
}
