//! Drawing the file explorer (full screen, built from the `tui` kit).

use crate::explorer::{DState, ExplorerState, RowKind, sort_name};
use crate::explorer_data::{GitMark, SortKey, format_mark, format_size, relative_time};
use crate::theme::{Theme, theme};
use crate::tui::bars::{HintBar, StatusLine, TitleBar};
use crate::tui::columns::{Cell, Row};
use crate::tui::panel::{Panel, divider};
use crate::tui::{Tone, fill, put, width};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Paragraph, Widget};

/// Keep the tail of a long path, marking the cut with an ellipsis.
fn left_trim(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max == 0 {
        return s.to_string();
    }
    format!("…{}", s.chars().skip(n - (max - 1)).collect::<String>())
}

/// The folder as shown in the title: `~` for the home folder on this machine.
fn shown_root(ex: &ExplorerState) -> String {
    if !ex.remote
        && let Some(h) = dirs::home_dir()
    {
        let home = h.to_string_lossy().into_owned();
        if let Some(rest) = ex.root.strip_prefix(home.trim_end_matches('/'))
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("~{rest}");
        }
    }
    ex.root.clone()
}

/// Colour of a git mark letter, from the theme (diff colours for add/delete, warn for modified).
pub fn mark_color(t: &Theme, m: GitMark) -> ratatui::style::Color {
    match m {
        GitMark::Modified => t.warn,
        GitMark::Added => t.diff_add,
        GitMark::Deleted | GitMark::Conflict | GitMark::Ignored => t.diff_remove,
        GitMark::Renamed => t.diff_hunk,
        GitMark::Untracked => t.diff_untracked,
    }
}

fn ui_rows(ex: &ExplorerState, t: &Theme) -> Vec<Row> {
    let dim = Style::default().fg(t.dim);
    ex.rows
        .iter()
        .map(|r| match &r.kind {
            RowKind::Node => {
                let name_style = match (r.is_dir, r.label.starts_with('.')) {
                    (true, _) => Style::default().add_modifier(Modifier::BOLD),
                    (false, true) => dim,
                    _ => Style::default(),
                };
                let size = match (r.is_dir, r.size) {
                    (false, Some(s)) => Cell::styled(format_size(s), Style::default().fg(t.size_fg)),
                    _ => Cell::new(""),
                };
                let date = r.mtime.map_or(Cell::new(""), |m| Cell::styled(relative_time(ex.now, m), Style::default().fg(t.date_fg)));
                let mark = match (r.mark, r.dirty) {
                    (Some(m), _) => Cell::styled(format_mark(m).to_string(), Style::default().fg(mark_color(t, m)).add_modifier(Modifier::BOLD)),
                    (None, true) => Cell::styled("•", dim),
                    _ => Cell::new(""),
                };
                Row::new(vec![Cell::styled(r.label.clone(), name_style), size, date, mark]).with_prefix(r.prefix.clone(), Some(Style::default().fg(t.tree_guide)))
            }
            RowKind::Error(_) => Row::new(vec![Cell::styled(r.label.clone(), Style::default().fg(t.hot))]).with_prefix(r.prefix.clone(), Some(dim)),
            _ => Row::new(vec![Cell::styled(r.label.clone(), dim.add_modifier(Modifier::ITALIC))]).with_prefix(r.prefix.clone(), Some(dim)),
        })
        .collect()
}

pub fn draw(f: &mut Frame, ex: &ExplorerState) {
    let t = theme();
    let area = f.area();
    let buf = f.buffer_mut();
    fill(buf, area, Style::default().fg(t.fg).bg(t.bg));
    if area.height < 4 || area.width < 20 {
        return;
    }
    let w = area.width as usize;

    // ---- title bar
    let branch = ex.branch().map(|b| format!(" · {b}")).unwrap_or_default();
    let right = ex.remote.then(|| ex.badge.clone());
    let reserve = 2 + right.as_ref().map_or(0, |r| width(r) + 1);
    let head = "explorer · ";
    let room = w.saturating_sub(reserve + width(head) + width(&branch));
    let mut bar = TitleBar::new(format!("{head}{}{branch}", left_trim(&shown_root(ex), room)));
    if let Some(r) = right {
        bar = bar.right(r);
    }
    bar.render(Rect::new(0, 0, area.width, 1), buf, &t);

    // ---- hint line (or the find prompt)
    let hint_row = Rect::new(0, 1, area.width, 1);
    match &ex.filter {
        Some(fl) => {
            fill(buf, hint_row, Style::default().fg(t.hintline_fg).bg(t.bg));
            let key = Style::default().fg(t.fg).bg(t.bg).add_modifier(Modifier::BOLD);
            let mut x = 1;
            x += put(buf, x, 1, "find: ", w, Style::default().fg(t.hintline_fg).bg(t.bg));
            x += put(buf, x, 1, &fl.text, w.saturating_sub(x as usize), key);
            if fl.editing {
                x += put(buf, x, 1, "▏", 1, key);
                put(buf, x + 2, 1, "Enter selects · Esc clears · Up/Down move", w.saturating_sub(x as usize + 2), Style::default().fg(t.hintline_fg).bg(t.bg));
            } else {
                put(buf, x + 2, 1, "(Esc clears the filter)", w.saturating_sub(x as usize + 2), Style::default().fg(t.hintline_fg).bg(t.bg));
            }
        }
        None => HintBar::new(&[
            ("Up/Dn", "move"),
            ("L/R", "fold"),
            ("Enter", "open"),
            ("Q", "back"),
            ("O", "edit"),
            ("F", "find"),
            ("H", "hidden"),
            ("G", "changed"),
            ("S", "sort"),
            ("T", "tab"),
            ("A", "agent"),
            ("C/P", "path"),
            ("R", "reload"),
            ("J/K", "scroll"),
        ])
        .render(hint_row, buf, &t),
    }

    // ---- status line
    let (items, changed, hidden) = ex.counts;
    let mut st = StatusLine::new(ex.badge.clone()).dim(format!("{items} items")).seg(format!("{changed} changed"), if changed > 0 { Tone::Warn } else { Tone::Dim }).dim(format!("{hidden} hidden"));
    if ex.sort != SortKey::Name || ex.desc {
        st = st.dim(format!("by {}{}", sort_name(ex.sort), if ex.desc { " desc" } else { "" }));
    }
    if ex.changed_only {
        st = st.seg("changed only", Tone::Warn);
    }
    if let Some((m, err)) = &ex.message {
        st = st.seg(m.clone(), if *err { Tone::Hot } else { Tone::Good });
    }
    st.render(Rect::new(0, 2, area.width, 1), buf, &t);

    // ---- left pane
    let lay = &ex.lay;
    let rows = ui_rows(ex, &t);
    let left = lay.left;
    let areas = Panel::new("").footer(ex.list.footer(rows.len(), "up/down wrap · left/right fold")).render(left, buf, &t);
    let list_area = Rect::new(left.x, left.y, left.width, areas.header.height + areas.body.height);
    ex.list.render(&rows, list_area, buf, &t);
    if rows.is_empty() {
        let msg = if ex.changed_only { "no changed files (G shows everything)" } else { "nothing to show" };
        put(buf, left.x + 1, left.y + 2, msg, left.width.saturating_sub(2) as usize, Style::default().fg(t.dim).bg(t.bg));
    }

    // ---- right pane
    if lay.single.is_some() {
        return;
    }
    divider(lay.divider, buf, &t, ex.focus_detail);
    let right = lay.right;
    let name = ex.cur_id().map(|id| id.rsplit('/').next().unwrap_or("").to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "(nothing selected)".into());
    let (pos, total) = (ex.detail_pos(), ex.detail_total());
    let footer = match &ex.detail {
        DState::File(p) => {
            let body = ex.detail_h.saturating_sub(usize::from(p.banner.is_some()));
            format!("lines {}-{} of {} · J/K scroll", (pos + 1).min(total), (pos + body).min(total), total)
        }
        DState::Lines(_) => format!("lines {}-{} of {} · J/K scroll", (pos + 1).min(total), (pos + ex.detail_h).min(total), total),
        _ => String::new(),
    };
    let ra = Panel::new(name).footer(footer).render(right, buf, &t);
    let body = ra.body;
    if body.height == 0 {
        return;
    }
    let src = ex.source_line();
    if let Some(rest) = src.strip_prefix("Source:") {
        let x = put(buf, body.x + 1, body.y, "Source:", body.width as usize, Style::default().fg(t.dim).bg(t.bg));
        put(buf, body.x + 1 + x, body.y, rest, (body.width as usize).saturating_sub(x as usize + 1), Style::default().fg(t.fg).bg(t.bg));
    }
    let mut content = Rect::new(body.x + 1, body.y + 1, body.width.saturating_sub(1), body.height.saturating_sub(1));
    if content.height == 0 || content.width == 0 {
        return;
    }
    let base = Style::default().fg(t.fg).bg(t.bg);
    match &ex.detail {
        DState::Empty => {}
        DState::Loading => {
            put(buf, content.x, content.y, "loading...", content.width as usize, Style::default().fg(t.dim).bg(t.bg).add_modifier(Modifier::ITALIC));
        }
        DState::Error(e) => {
            put(buf, content.x, content.y, &format!("could not read: {e}"), content.width as usize, Style::default().fg(t.hot).bg(t.bg));
        }
        DState::Lines(l) => {
            let v: Vec<_> = l.iter().skip(ex.detail_pos()).take(content.height as usize).cloned().collect();
            Paragraph::new(v).style(base).render(content, buf);
        }
        DState::File(p) => {
            if let Some(b) = p.banner.as_deref() {
                put(buf, content.x, content.y, b, content.width as usize, Style::default().fg(t.banner).bg(t.bg).add_modifier(Modifier::BOLD));
                content = Rect::new(content.x, content.y + 1, content.width, content.height.saturating_sub(1));
            }
            if p.lines.is_empty() {
                put(buf, content.x, content.y, "(empty file)", content.width as usize, Style::default().fg(t.dim).bg(t.bg));
            } else {
                let v = p.visible(content.height as usize, content.width as usize);
                Paragraph::new(v).style(base).render(content, buf);
            }
        }
    }
}
