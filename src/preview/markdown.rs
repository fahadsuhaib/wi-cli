//! Markdown to styled lines with pulldown-cmark. Images become `[image: alt]` placeholders.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use crate::theme::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

struct W {
    lines: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<Option<u64>>,
    quote: usize,
    in_code: bool,
    table: Option<Vec<Vec<String>>>,
    cell: Option<String>,
    link: Vec<String>,
}

impl W {
    fn style(&self) -> Style {
        self.styles.iter().fold(Style::default(), |a, s| a.patch(*s))
    }

    fn push(&mut self, text: impl Into<String>) {
        let st = self.style();
        self.cur.push(Span::styled(text.into(), st));
    }

    fn flush(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        let mut spans = Vec::new();
        if self.quote > 0 {
            spans.push(Span::styled("│ ".repeat(self.quote), Style::default().fg(theme().md_quote)));
        }
        spans.append(&mut self.cur);
        self.lines.push(Line::from(spans));
    }

    fn blank(&mut self) {
        self.flush();
        if self.lines.last().is_some_and(|l| !l.spans.is_empty()) {
            self.lines.push(Line::from(""));
        }
    }
}

fn heading_style(l: HeadingLevel) -> Style {
    let base = Style::default().add_modifier(Modifier::BOLD);
    match l {
        HeadingLevel::H1 => base.fg(theme().md_h1).add_modifier(Modifier::UNDERLINED),
        HeadingLevel::H2 => base.fg(theme().md_h2),
        _ => base.fg(theme().md_h3),
    }
}

pub fn render(src: &str) -> Vec<Line<'static>> {
    let mut w = W { lines: vec![], cur: vec![], styles: vec![], lists: vec![], quote: 0, in_code: false, table: None, cell: None, link: vec![] };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let dim = Style::default().fg(theme().dim);
    let rule = Style::default().fg(theme().md_rule);
    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    w.flush();
                    w.styles.push(heading_style(level));
                    w.push(format!("{} ", "#".repeat(level as usize)));
                }
                Tag::Paragraph => {}
                Tag::BlockQuote(_) => {
                    w.flush();
                    w.quote += 1;
                }
                Tag::CodeBlock(kind) => {
                    w.flush();
                    w.in_code = true;
                    if let CodeBlockKind::Fenced(lang) = kind
                        && !lang.is_empty()
                    {
                        w.lines.push(Line::from(Span::styled(format!("  {lang}"), dim)));
                    }
                }
                Tag::List(start) => w.lists.push(start),
                Tag::Item => {
                    w.flush();
                    let depth = w.lists.len().saturating_sub(1);
                    let marker = match w.lists.last_mut() {
                        Some(Some(n)) => {
                            let m = format!("{n}. ");
                            *n += 1;
                            m
                        }
                        _ => "• ".to_string(),
                    };
                    w.cur.push(Span::raw("  ".repeat(depth)));
                    w.cur.push(Span::styled(marker, Style::default().fg(theme().md_bullet)));
                }
                Tag::Emphasis => w.styles.push(Style::default().add_modifier(Modifier::ITALIC)),
                Tag::Strong => w.styles.push(Style::default().add_modifier(Modifier::BOLD)),
                Tag::Strikethrough => w.styles.push(Style::default().add_modifier(Modifier::CROSSED_OUT)),
                Tag::Link { dest_url, .. } => {
                    w.link.push(dest_url.to_string());
                    w.styles.push(Style::default().fg(theme().md_link).add_modifier(Modifier::UNDERLINED));
                }
                Tag::Image { .. } => w.push("[image: "),
                Tag::Table(_) => w.table = Some(Vec::new()),
                Tag::TableHead | Tag::TableRow => {
                    if let Some(t) = w.table.as_mut() {
                        t.push(Vec::new());
                    }
                }
                Tag::TableCell => w.cell = Some(String::new()),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    w.flush();
                    w.styles.pop();
                    w.blank();
                }
                TagEnd::Paragraph => w.blank(),
                TagEnd::BlockQuote(_) => {
                    w.flush();
                    w.quote = w.quote.saturating_sub(1);
                    w.blank();
                }
                TagEnd::CodeBlock => {
                    w.in_code = false;
                    w.blank();
                }
                TagEnd::List(_) => {
                    w.lists.pop();
                    if w.lists.is_empty() {
                        w.blank();
                    }
                }
                TagEnd::Item => w.flush(),
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    w.styles.pop();
                }
                TagEnd::Link => {
                    w.styles.pop();
                    if let Some(url) = w.link.pop() {
                        w.cur.push(Span::styled(format!(" ({url})"), dim));
                    }
                }
                TagEnd::Image => w.push("]"),
                TagEnd::TableCell => {
                    if let (Some(c), Some(t)) = (w.cell.take(), w.table.as_mut())
                        && let Some(row) = t.last_mut()
                    {
                        row.push(c);
                    }
                }
                TagEnd::Table => {
                    if let Some(rows) = w.table.take() {
                        render_table(&mut w, rows);
                    }
                    w.blank();
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some(c) = w.cell.as_mut() {
                    c.push_str(&t);
                } else if w.in_code {
                    for l in t.trim_end_matches('\n').split('\n') {
                        w.lines.push(Line::from(vec![
                            Span::styled("  │ ", Style::default().fg(theme().md_rule)),
                            Span::styled(l.replace('\t', "    "), Style::default().fg(theme().md_code)),
                        ]));
                    }
                } else {
                    w.push(t.to_string());
                }
            }
            Event::Code(t) => {
                if let Some(c) = w.cell.as_mut() {
                    c.push_str(&t);
                } else {
                    w.cur.push(Span::styled(t.to_string(), Style::default().fg(theme().md_code)));
                }
            }
            Event::SoftBreak => {
                if let Some(c) = w.cell.as_mut() { c.push(' '); } else { w.push(" "); }
            }
            Event::HardBreak => w.flush(),
            Event::Rule => {
                w.flush();
                w.lines.push(Line::from(Span::styled("─".repeat(40), rule)));
                w.blank();
            }
            Event::TaskListMarker(done) => w.cur.push(Span::raw(if done { "[x] " } else { "[ ] " })),
            Event::Html(t) | Event::InlineHtml(t) => w.cur.push(Span::styled(t.trim_end().to_string(), dim)),
            _ => {}
        }
    }
    w.flush();
    while w.lines.last().is_some_and(|l| l.spans.is_empty()) {
        w.lines.pop();
    }
    w.lines
}

fn render_table(w: &mut W, rows: Vec<Vec<String>>) {
    use unicode_width::UnicodeWidthStr;
    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![1; ncols];
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.width());
        }
    }
    w.flush();
    for (ri, r) in rows.iter().enumerate() {
        let mut spans = Vec::new();
        for (i, wd) in widths.iter().enumerate() {
            let c = r.get(i).map(String::as_str).unwrap_or("");
            let st = if ri == 0 { Style::default().fg(theme().table_header).add_modifier(Modifier::BOLD) } else { Style::default() };
            spans.push(Span::styled(format!("{c}{}", " ".repeat(wd.saturating_sub(c.width()))), st));
            if i + 1 < widths.len() {
                spans.push(Span::styled(" │ ", Style::default().fg(theme().table_sep)));
            }
        }
        w.lines.push(Line::from(spans));
        if ri == 0 {
            let rule: Vec<String> = widths.iter().map(|n| "─".repeat(*n)).collect();
            w.lines.push(Line::from(Span::styled(rule.join("─┼─"), Style::default().fg(theme().table_sep))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn text(src: &str) -> Vec<String> {
        render(src).iter().map(plain).collect()
    }

    #[test]
    fn headings_lists_and_code() {
        let t = text("# Title\n\n- one\n- two\n\n1. a\n2. b\n\n```rust\nlet x = 1;\n```\n");
        assert_eq!(t[0], "# Title");
        assert!(t.contains(&"• one".to_string()));
        assert!(t.contains(&"2. b".to_string()));
        assert!(t.iter().any(|l| l.contains("let x = 1;")));
    }

    #[test]
    fn links_show_url_and_images_are_placeholders() {
        let t = text("see [docs](https://x.dev) and ![logo](a.png)\n");
        assert_eq!(t[0], "see docs (https://x.dev) and [image: logo]");
    }

    #[test]
    fn tables_align() {
        let t = text("| a | bb |\n|---|----|\n| 1 | 2 |\n");
        assert_eq!(t[0], "a │ bb");
        assert!(t[1].contains('┼'));
        assert_eq!(t[2], "1 │ 2 ");
    }

    #[test]
    fn quotes_and_tasks() {
        let t = text("> wise\n\n- [x] done\n- [ ] todo\n");
        assert_eq!(t[0], "│ wise");
        assert!(t.iter().any(|l| l.contains("[x] done")));
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(render("").is_empty());
    }
}
