//! Golden render tests: a composed explorer-like screen drawn into a Buffer in both themes.

use super::bars::{HintBar, StatusLine, TitleBar};
use super::columns::{Cell, Column, ColumnList, Row, Width};
use super::panel::{Panel, divider};
use super::split::SplitView;
use super::tree::{Glyphs, Node, Tree};
use super::{Tone, put};
use crate::theme::{DARK, LIGHT, Theme};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// (size in bytes, modified text)
type P = (u64, &'static str);

fn tree() -> Tree<P> {
    Tree::new(vec![
        Node::dir("devterm", "devterm", (0, "today")).expanded().with_children(vec![
            Node::dir("devterm/docs", "docs", (0, "Oct 30")).with_children(vec![Node::file("devterm/docs/KEYBOARD.md", "KEYBOARD.md", (2100, "Oct 30"))]),
            Node::dir("devterm/src", "src", (0, "today")).expanded().with_children(vec![
                Node::dir("devterm/src/tui", "tui", (0, "today")),
                Node::file("devterm/src/main.rs", "main.rs", (4300, "today")),
                Node::file("devterm/src/tab.rs", "tab.rs", (21500, "2 min ago")),
                Node::file("devterm/src/theme.rs", "theme.rs", (31200, "today")),
            ]),
            Node::file("devterm/.gitignore", ".gitignore", (12, "Sep 02")),
            Node::file("devterm/Cargo.toml", "Cargo.toml", (610, "Oct 21")),
            Node::file("devterm/CLAUDE.md", "CLAUDE.md", (3900, "Oct 29")),
            Node::file("devterm/PLAN.md", "PLAN.md", (18000, "Oct 28")),
            Node::file("devterm/README.md", "README.md", (1400, "Oct 01")),
        ]),
    ])
}

fn human(n: u64) -> String {
    match n {
        0 => String::new(),
        n if n < 1000 => format!("{n}B"),
        n => format!("{:.1}K", n as f64 / 1000.0),
    }
}

fn rows(t: &Tree<P>, th: &Theme) -> Vec<Row> {
    let g = Glyphs::unicode();
    t.flatten()
        .iter()
        .map(|r| {
            let (size, date) = r.node.payload;
            let guide = Style::default().fg(th.tree_guide);
            let size_style = match r.node.id.as_str() {
                "devterm/src/tab.rs" => Style::default().fg(th.hot),
                "devterm/.gitignore" => Style::default().fg(th.good),
                _ => Style::default().fg(th.size_fg),
            };
            Row::new(vec![Cell::new(r.node.label.clone()), Cell::styled(human(size), size_style), Cell::styled(date, Style::default().fg(th.date_fg))]).with_prefix(g.prefix(r), Some(guide))
        })
        .collect()
}

pub fn screen(w: u16, h: u16, th: &Theme) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
    let area = buf.area;
    super::fill(&mut buf, area, Style::default().fg(th.fg).bg(th.bg));
    TitleBar::new("wi explorer").right("devbox:~/work/devterm").render(Rect::new(0, 0, w, 1), &mut buf, th);
    HintBar::new(&[("Up/Down", "move"), ("Left/Right", "fold"), ("Enter", "open"), ("F", "find"), ("Esc", "close")]).render(Rect::new(0, 1, w, 1), &mut buf, th);
    StatusLine::new("LOCAL").dim("12 files").seg("2 changed", Tone::Warn).seg("1 error", Tone::Hot).seg("synced", Tone::Good).render(Rect::new(0, 2, w, 1), &mut buf, th);
    if h <= 3 {
        return buf;
    }
    let main = Rect::new(0, 3, w, h - 3);
    let lay = SplitView::new(40, 20, 20).layout(main);
    let t = tree();
    let data = rows(&t, th);
    let mut list = ColumnList::new(vec![Column::new("FILES", Width::Fill), Column::new("SIZE", Width::Min(5)).right(), Column::new("MODIFIED", Width::Fixed(9))]);
    list.selected = 4;
    let panel = Panel::new("devterm").right("tree").footer(list.footer(data.len(), "up/down wrap · left/right fold"));
    let areas = panel.render(lay.left, &mut buf, th);
    list.render(&data, areas.body, &mut buf, th);
    if lay.single.is_none() {
        divider(lay.divider, &mut buf, th, false);
        let right = Panel::new("src/main.rs").right("rust");
        let ra = right.render(lay.right, &mut buf, th);
        let lines = ["Source: src/main.rs:1", "", "mod agents;", "mod app;", "fn main() {}"];
        for (i, l) in lines.iter().enumerate() {
            if (i as u16) < ra.body.height {
                put(&mut buf, ra.body.x + 1, ra.body.y + i as u16, l, ra.body.width as usize - 1, Style::default().fg(th.fg).bg(th.bg));
            }
        }
    }
    buf
}

use super::row_text as line;

fn left(b: &Buffer, y: u16) -> String {
    line(b, y).chars().take(40).collect()
}

fn x_of(b: &Buffer, y: u16, needle: &str) -> u16 {
    let l = line(b, y);
    let at = l.find(needle).unwrap_or_else(|| panic!("{needle:?} not in {l:?}"));
    l[..at].chars().count() as u16
}

#[test]
fn golden_text_in_both_themes() {
    for th in [DARK, LIGHT] {
        let b = screen(100, 30, &th);
        let name = th.kind.name();
        assert_eq!(line(&b, 0), " wi explorer                                                                  devbox:~/work/devterm ", "{name}");
        assert_eq!(line(&b, 1).trim_end(), " Up/Down move · Left/Right fold · Enter open · F find · Esc close", "{name}");
        assert_eq!(line(&b, 2).trim_end(), " LOCAL   12 files  2 changed  1 error  synced", "{name}");
        assert_eq!(left(&b, 3), " devterm                           tree ", "{name}");
        assert_eq!(left(&b, 4), "FILES                    SIZE  MODIFIED ", "{name}");
        assert_eq!(left(&b, 5), "▾ devterm                      today    ", "{name}");
        assert_eq!(left(&b, 6), "  ▸ docs                       Oct 30   ", "{name}");
        assert_eq!(left(&b, 7), "  ▾ src                        today    ", "{name}");
        assert_eq!(left(&b, 9), "    · main.rs            4.3K  today    ", "{name}");
        assert_eq!(left(&b, 10), "    · tab.rs            21.5K  2 min ago", "{name}");
        assert_eq!(left(&b, 29), " 5/12 · up/down wrap · left/right fold  ", "{name}");
        assert_eq!(line(&b, 3).chars().nth(40), Some('│'), "{name}");
        assert!(line(&b, 3).trim_end().ends_with("rust"), "{name}");
        assert!(line(&b, 4).contains("Source: src/main.rs:1"), "{name}");
        assert!(line(&b, 8).contains("fn main() {}"), "{name}");
    }
}

#[test]
fn golden_colours_in_both_themes() {
    for th in [DARK, LIGHT] {
        let b = screen(100, 30, &th);
        let name = th.kind.name();
        // Title bar: inverted, full width.
        for x in [0u16, 40, 99] {
            assert_eq!(b[(x, 0)].style().bg, Some(th.title_bg), "{name} title bg at {x}");
        }
        assert_eq!(b[(2, 0)].style().fg, Some(th.title_fg), "{name}");
        // Hint line is dim on the canvas; the status badge is coloured.
        assert_eq!(b[(x_of(&b, 1, "move"), 1)].style().fg, Some(th.hintline_fg), "{name}");
        assert_eq!(b[(2, 2)].style().bg, Some(th.badge_bg), "{name}");
        assert_eq!(b[(2, 2)].style().fg, Some(th.badge_fg), "{name}");
        assert_eq!(b[(x_of(&b, 2, "1 error"), 2)].style().fg, Some(th.hot), "{name}");
        assert_eq!(b[(x_of(&b, 2, "synced"), 2)].style().fg, Some(th.good), "{name}");
        // Panel header and column header rows.
        assert_eq!(b[(1, 3)].style().bg, Some(th.badge_bg), "{name}");
        assert_eq!(b[(1, 4)].style().bg, Some(th.col_header_bg), "{name}");
        assert_eq!(b[(1, 4)].style().fg, Some(th.col_header_fg), "{name}");
        // Selected row (main.rs) is painted across the whole pane width.
        for x in [0u16, 20, 39] {
            assert_eq!(b[(x, 9)].style().bg, Some(th.row_sel_bg), "{name} sel at {x}");
            assert_eq!(b[(x, 9)].style().fg, Some(th.row_sel_fg), "{name} sel fg at {x}");
        }
        assert_eq!(b[(5, 8)].style().bg, Some(th.bg), "{name}");
        // A `hot` size, a `good` size, the guide glyph and the date column.
        assert_eq!(b[(x_of(&b, 10, "21.5K"), 10)].style().fg, Some(th.hot), "{name}");
        assert_eq!(b[(x_of(&b, 12, "12B"), 12)].style().fg, Some(th.good), "{name}");
        assert_eq!(b[(0, 5)].style().fg, Some(th.tree_guide), "{name}");
        assert_eq!(b[(x_of(&b, 11, "today"), 11)].style().fg, Some(th.date_fg), "{name}");
        assert_eq!(b[(x_of(&b, 11, "31.2K"), 11)].style().fg, Some(th.size_fg), "{name}");
        // Footer is dim; the divider uses the idle colour.
        assert_eq!(b[(2, 29)].style().fg, Some(th.dim), "{name}");
        assert_eq!(b[(40, 12)].style().fg, Some(th.divider_idle), "{name}");
    }
}

#[test]
fn composed_screen_fits_without_panics() {
    for th in [DARK, LIGHT] {
        for w in [0u16, 1, 2, 20, 40, 80, 120] {
            for h in [0u16, 1, 3, 5, 30] {
                let b = screen(w, h, &th);
                assert_eq!(b.area.width, w);
            }
        }
    }
}
