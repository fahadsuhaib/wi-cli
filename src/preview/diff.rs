//! Git changes view: branch and status summary, the coloured diff against HEAD, untracked files.

use crate::theme::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Raw git output the view is built from.
#[derive(Default, Clone, Debug)]
pub struct Snapshot {
    /// `git status --short --branch`
    pub status: String,
    /// `git diff HEAD` (or staged plus unstaged changes when there is no commit yet)
    pub diff: String,
    /// `git ls-files --others --exclude-standard`
    pub untracked: String,
    /// The diff hit the output cap.
    pub truncated: bool,
}

pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    /// Line index where each changed file (and the untracked list) starts, for N/P.
    pub jumps: Vec<usize>,
}

fn clean(s: &str) -> String {
    s.trim_end_matches('\r')
        .chars()
        .flat_map(|c| match c {
            '\t' => vec![' '; 4],
            c if c.is_control() => vec!['·'],
            c => vec![c],
        })
        .collect()
}

pub fn style_diff_line(line: &str, in_header: bool) -> Style {
    let t = theme();
    let bold = Style::default().fg(t.diff_file).add_modifier(Modifier::BOLD);
    if line.starts_with("diff --git") {
        bold
    } else if line.starts_with("@@") {
        Style::default().fg(t.diff_hunk)
    } else if in_header {
        bold
    } else if line.starts_with('+') {
        Style::default().fg(t.diff_add)
    } else if line.starts_with('-') {
        Style::default().fg(t.diff_remove)
    } else if line.starts_with('\\') {
        Style::default().fg(t.dim)
    } else {
        Style::default()
    }
}

fn status_line(l: &str) -> Line<'static> {
    let chars: Vec<char> = l.chars().collect();
    if chars.len() < 3 {
        return Line::from(clean(l));
    }
    let (x, y): (String, String) = (chars[0].to_string(), chars[1].to_string());
    let rest: String = chars[2..].iter().collect();
    if x == "?" {
        return Line::from(vec![Span::styled(format!("{x}{y}"), Style::default().fg(theme().diff_untracked)), Span::raw(clean(&rest))]);
    }
    Line::from(vec![
        Span::styled(x, Style::default().fg(theme().diff_add)),
        Span::styled(y, Style::default().fg(theme().diff_remove)),
        Span::raw(clean(&rest)),
    ])
}

pub fn render(s: &Snapshot) -> Rendered {
    let bold = Style::default().fg(theme().diff_branch).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme().dim);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut jumps = Vec::new();

    let mut status = s.status.lines();
    let branch = status.next().and_then(|l| l.strip_prefix("## ")).unwrap_or("(unknown)");
    lines.push(Line::from(vec![Span::styled("Branch  ", dim), Span::styled(clean(branch), bold)]));
    let entries: Vec<&str> = status.filter(|l| !l.is_empty()).collect();
    if entries.is_empty() {
        lines.push(Line::from(Span::styled("Working tree clean", dim)));
    } else {
        let untracked = entries.iter().filter(|l| l.starts_with("??")).count();
        lines.push(Line::from(Span::styled(
            format!("{} changed, {} untracked   (left column staged, right column not staged)", entries.len() - untracked, untracked),
            dim,
        )));
        lines.extend(entries.iter().map(|l| status_line(l)));
    }
    lines.push(Line::from(""));

    let mut in_header = false;
    for l in s.diff.lines() {
        if l.starts_with("diff --git") {
            in_header = true;
            jumps.push(lines.len());
        } else if l.starts_with("@@") {
            in_header = false;
        }
        lines.push(Line::from(Span::styled(clean(l), style_diff_line(l, in_header))));
    }
    if s.diff.is_empty() {
        lines.push(Line::from(Span::styled("No changes versus HEAD", dim)));
    }

    let untracked: Vec<&str> = s.untracked.lines().filter(|l| !l.is_empty()).collect();
    if !untracked.is_empty() {
        lines.push(Line::from(""));
        jumps.push(lines.len());
        lines.push(Line::from(Span::styled(format!("Untracked files ({})", untracked.len()), bold)));
        lines.extend(untracked.iter().map(|f| Line::from(Span::styled(format!("  {}", clean(f)), Style::default().fg(theme().diff_untracked)))));
    }
    Rendered { lines, jumps }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn snap() -> Snapshot {
        Snapshot {
            status: "## main...origin/main [ahead 1]\n M a.txt\nA  b.txt\n?? c.txt\n".into(),
            diff: "diff --git a/a.txt b/a.txt\nindex 1..2 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n\\ No newline at end of file\ndiff --git a/b.txt b/b.txt\nnew file mode 100644\n--- /dev/null\n+++ b/b.txt\n@@ -0,0 +1 @@\n+new\n".into(),
            untracked: "c.txt\n".into(),
            truncated: false,
        }
    }

    fn find<'a>(r: &'a Rendered, text: &str) -> &'a Line<'static> {
        r.lines.iter().find(|l| plain(l) == text).unwrap_or_else(|| panic!("no line {text:?}"))
    }

    #[test]
    fn colours_added_removed_hunk_and_headers() {
        let r = render(&snap());
        assert_eq!(find(&r, "+TWO").spans[0].style.fg, Some(theme().diff_add));
        assert_eq!(find(&r, "-two").spans[0].style.fg, Some(theme().diff_remove));
        assert_eq!(find(&r, "@@ -1,2 +1,2 @@").spans[0].style.fg, Some(theme().diff_hunk));
        let h = find(&r, "diff --git a/a.txt b/a.txt");
        assert!(h.spans[0].style.add_modifier.contains(Modifier::BOLD));
        // file header lines are not mistaken for added/removed lines
        let plus = find(&r, "+++ b/a.txt");
        assert_eq!(plus.spans[0].style.fg, Some(theme().diff_file));
        assert!(plus.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(find(&r, "--- a/a.txt").spans[0].style.fg, Some(theme().diff_file));
        assert_eq!(find(&r, " one").spans[0].style.fg, None);
    }

    #[test]
    fn summary_has_branch_and_status() {
        let r = render(&snap());
        assert_eq!(plain(&r.lines[0]), "Branch  main...origin/main [ahead 1]");
        assert!(r.lines.iter().any(|l| plain(l) == " M a.txt"));
        assert!(r.lines.iter().any(|l| plain(l) == "?? c.txt"));
        assert!(plain(&r.lines[1]).starts_with("2 changed, 1 untracked"));
        assert!(r.lines.iter().any(|l| plain(l) == "  c.txt"));
    }

    #[test]
    fn jumps_point_at_each_file_and_untracked_list() {
        let r = render(&snap());
        assert_eq!(r.jumps.len(), 3);
        assert_eq!(plain(&r.lines[r.jumps[0]]), "diff --git a/a.txt b/a.txt");
        assert_eq!(plain(&r.lines[r.jumps[1]]), "diff --git a/b.txt b/b.txt");
        assert_eq!(plain(&r.lines[r.jumps[2]]), "Untracked files (1)");
    }

    #[test]
    fn clean_tree_says_so() {
        let r = render(&Snapshot { status: "## main\n".into(), ..Default::default() });
        assert!(r.lines.iter().any(|l| plain(l) == "Working tree clean"));
        assert!(r.lines.iter().any(|l| plain(l) == "No changes versus HEAD"));
        assert!(r.jumps.is_empty());
    }
}
