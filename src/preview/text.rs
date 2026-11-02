//! Plain text, logs and source code: line numbers, tabs expanded, control characters made visible.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Returns the lines and the gutter width (characters) so wrapping and scrolling can keep it fixed.
pub fn render(body: &str) -> (Vec<Line<'static>>, usize) {
    let count = body.lines().count().max(1);
    let digits = count.to_string().len();
    let gutter = digits + 2;
    let dim = Style::default().add_modifier(Modifier::DIM);
    let lines = body
        .lines()
        .enumerate()
        .map(|(i, l)| {
            let clean: String = l
                .trim_end_matches('\r')
                .chars()
                .flat_map(|c| match c {
                    '\t' => vec![' '; 4],
                    c if c.is_control() => vec!['·'],
                    c => vec![c],
                })
                .collect();
            Line::from(vec![Span::styled(format!("{:>digits$}  ", i + 1), dim), Span::raw(clean)])
        })
        .collect();
    (lines, gutter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_lines_and_expands_tabs() {
        let (lines, g) = render("a\n\tb\r\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(g, 3);
        let second: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(second, "2      b");
    }
}
