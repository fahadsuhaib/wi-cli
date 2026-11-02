//! JSON: pretty-printed and coloured. Returns None when the text is not valid JSON.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

pub fn render(body: &str) -> Option<Vec<Line<'static>>> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let pretty = serde_json::to_string_pretty(&v).ok()?;
    Some(pretty.lines().map(colour_line).collect())
}

fn colour_line(line: &str) -> Line<'static> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(chars.len());
            let tok: String = chars[i..end].iter().collect();
            let is_key = chars.get(end) == Some(&':');
            spans.push(Span::styled(tok, Style::default().fg(if is_key { Color::Cyan } else { Color::Green })));
            i = end;
        } else if c.is_ascii_digit() || c == '-' || c.is_ascii_alphabetic() {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '-' | '.' | '+')) {
                j += 1;
            }
            let tok: String = chars[i..j].iter().collect();
            spans.push(Span::styled(tok, Style::default().fg(Color::Yellow)));
            i = j;
        } else {
            let mut j = i;
            while j < chars.len() && !matches!(chars[j], '"' | '-') && !chars[j].is_ascii_alphanumeric() {
                j += 1;
            }
            if j == i {
                j = i + 1;
            }
            spans.push(Span::raw(chars[i..j].iter().collect::<String>()));
            i = j;
        }
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn pretty_prints_and_preserves_text() {
        let lines = render(r#"{"a":1,"b":[true,null],"c":"x y"}"#).unwrap();
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(text[0], "{");
        assert_eq!(text[1], r#"  "a": 1,"#);
        assert!(text.contains(&r#"  "c": "x y""#.to_string()));
    }

    #[test]
    fn invalid_json_is_none() {
        assert!(render("{not json").is_none());
    }
}
