//! Text search in files: build the command (ripgrep, else grep), run it on the workspace, parse
//! the output into matches grouped by file. Same code locally and over SSH; the command is one
//! bounded `sh` script (`cd dir && rg ... | head -n 2001`) and nothing is left running.

use super::list::ListState;
use crate::exec::sq;
use crate::workspace::Backend;
use regex::Regex;
use std::time::Duration;

pub const MAX_MATCHES: usize = 2000;
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Most matches one file contributes (keeps one huge file from using the whole budget).
const PER_FILE: usize = 500;
/// Longest line text kept per match, in characters.
const KEEP_CHARS: usize = 1000;
pub const FILES_CAP: usize = 50_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Case {
    Smart,
    Sensitive,
    Insensitive,
}

impl Case {
    pub fn next(self) -> Case {
        match self {
            Case::Smart => Case::Sensitive,
            Case::Sensitive => Case::Insensitive,
            Case::Insensitive => Case::Smart,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Case::Smart => "smart",
            Case::Sensitive => "match case",
            Case::Insensitive => "ignore case",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Query {
    pub text: String,
    pub case: Case,
    pub word: bool,
    pub regex: bool,
}

impl Query {
    pub fn new(text: &str) -> Query {
        Query { text: text.to_string(), case: Case::Smart, word: false, regex: false }
    }

    /// Whether upper/lower case matter for this query (smart case resolved).
    pub fn sensitive(&self) -> bool {
        match self.case {
            Case::Sensitive => true,
            Case::Insensitive => false,
            Case::Smart => {
                if self.regex {
                    // `\S`, `\W`, `\D` are escapes, not capitals the user typed.
                    let mut skip = false;
                    self.text.chars().any(|c| {
                        if skip {
                            skip = false;
                            false
                        } else if c == '\\' {
                            skip = true;
                            false
                        } else {
                            c.is_uppercase()
                        }
                    })
                } else {
                    super::smart_sensitive(&self.text)
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Rg,
    Grep,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Rg => "rg",
            Tool::Grep => "grep",
        }
    }
}

/// The shell command (without the `cd`) that lists matches, at most `MAX_MATCHES + 1` lines.
pub fn command(tool: Tool, q: &Query) -> String {
    let per_file = PER_FILE.to_string();
    let mut a: Vec<&str> = match tool {
        Tool::Rg => vec![
            "rg", "--no-config", "--vimgrep", "--no-heading", "--color", "never", "--max-columns", "400", "--max-columns-preview",
            "--max-count", &per_file, "--glob", "'!node_modules'",
        ],
        Tool::Grep => vec!["grep", "-rnIH", "--exclude-dir=.git", "--exclude-dir=node_modules", "-m", &per_file],
    };
    if !q.sensitive() {
        a.push("-i");
    } else if tool == Tool::Rg {
        a.push("-s");
    }
    if !q.regex {
        a.push("-F");
    } else if tool == Tool::Grep {
        a.push("-E");
    }
    if q.word {
        a.push("-w");
    }
    format!("{} -- {} . | head -n {}", a.join(" "), sq(&q.text), MAX_MATCHES + 1)
}

/// The shell command that lists the files under the current folder (capped, one path per line).
pub fn files_command(tool: Tool) -> String {
    let fallback = match tool {
        Tool::Rg => "rg --no-config --files --glob '!node_modules'".to_string(),
        Tool::Grep => "find . -type f -not -path '*/.git/*' -not -path '*/node_modules/*' | sed 's|^\\./||'".to_string(),
    };
    format!(
        "{{ if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then git -c core.quotepath=off ls-files -co --exclude-standard; else {fallback}; fi; }} | head -n {}",
        FILES_CAP + 1
    )
}

/// `rg` exists on this machine? (`WICLI_NO_RG=1` pretends it does not: used by tests and by
/// anyone who wants grep's behaviour.)
pub fn detect_script() -> &'static str {
    "command -v rg >/dev/null 2>&1 && echo rg || echo grep"
}

static TOOL: tokio::sync::OnceCell<Tool> = tokio::sync::OnceCell::const_new();

/// Which search tool the workspace has. Asked once per run of `wi`.
pub async fn detect_tool(backend: &Backend, dir: &str) -> Tool {
    *TOOL
        .get_or_init(|| async {
            if std::env::var("WICLI_NO_RG").is_ok_and(|v| !v.is_empty() && v != "0") {
                return Tool::Grep;
            }
            match backend.run_script(dir, detect_script(), 4096, Duration::from_secs(5)).await {
                Ok(o) if String::from_utf8_lossy(&o.stdout).trim() == "rg" => Tool::Rg,
                _ => Tool::Grep,
            }
        })
        .await
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Match {
    pub path: String,
    pub line: u32,
    /// 1-based column of the first match when the tool reports it (ripgrep), else 0.
    pub col: u32,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Match(Match),
    /// A "binary file matches" notice.
    Binary,
    Skip,
}

fn clean(s: &str) -> String {
    let s: String = s.trim_end_matches('\r').chars().take(KEEP_CHARS).map(|c| if c == '\t' { ' ' } else { c }).collect();
    crate::ui::sanitize(&s)
}

/// Positions of `:<digits>:` after `from`, as (colon index, number, index after the closing colon).
fn num_field(line: &str, from: usize) -> Option<(usize, u32, usize)> {
    let b = line.as_bytes();
    let mut i = from;
    while i < b.len() {
        if b[i] == b':' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1
                && j < b.len()
                && b[j] == b':'
                && let Ok(n) = line[i + 1..j].parse::<u32>()
            {
                return Some((i, n, j + 1));
            }
        }
        i += 1;
    }
    None
}

fn strip_dot(path: &str) -> String {
    path.strip_prefix("./").unwrap_or(path).to_string()
}

/// Parse one line of `rg --vimgrep` (`path:line:col:text`) or `grep -rnH` (`path:line:text`).
/// The path ends at the first `:<number>:` (`:<number>:<number>:` for rg), so paths with colons
/// and Windows drive letters work.
pub fn parse_line(tool: Tool, line: &str) -> Parsed {
    let parsed = match tool {
        Tool::Rg => {
            let mut from = 0;
            let mut found = None;
            while let Some((c1, ln, after)) = num_field(line, from) {
                // The column is the next field: digits then a colon.
                let rest = &line[after..];
                let d = rest.bytes().take_while(u8::is_ascii_digit).count();
                if d > 0 && rest.as_bytes().get(d) == Some(&b':') {
                    found = Some((c1, ln, rest[..d].parse::<u32>().unwrap_or(0), &rest[d + 1..]));
                    break;
                }
                from = c1 + 1;
            }
            found
        }
        Tool::Grep => num_field(line, 0).map(|(c1, ln, after)| (c1, ln, 0, &line[after..])),
    };
    if let Some((c1, ln, col, text)) = parsed
        && c1 > 0
    {
        return Parsed::Match(Match { path: strip_dot(&line[..c1]), line: ln, col, text: clean(text) });
    }
    let l = line.to_ascii_lowercase();
    if l.starts_with("binary file ") || l.contains("binary file matches") {
        Parsed::Binary
    } else {
        Parsed::Skip
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    /// Index into `Results::files`.
    Header(usize),
    /// Index into `Results::matches`.
    Match(usize),
}

pub struct Results {
    pub query: Query,
    /// The folder that was searched (result paths are relative to it).
    pub dir: String,
    pub tool: Tool,
    pub matches: Vec<Match>,
    /// (path, matches in it), in order of appearance.
    pub files: Vec<(String, usize)>,
    pub rows: Vec<Row>,
    /// Why the list may be incomplete (cap, timeout), or an error from the tool.
    pub notice: Option<String>,
    pub error: Option<String>,
    pub binary_skipped: usize,
    pub re: Option<Regex>,
    pub list: ListState,
}

/// Compile the pattern used to highlight matched text in the snippets.
pub fn highlight_regex(q: &Query) -> Option<Regex> {
    if q.text.is_empty() {
        return None;
    }
    let mut p = if q.regex { q.text.clone() } else { regex::escape(&q.text) };
    if q.word {
        p = format!(r"\b(?:{p})\b");
    }
    regex::RegexBuilder::new(&p).case_insensitive(!q.sensitive()).build().ok()
}

/// (start, length) in characters of every match of `re` in `text`.
pub fn highlight_ranges(re: &Regex, text: &str) -> Vec<(usize, usize)> {
    let idx: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let to_char = |b: usize| idx.partition_point(|&i| i < b);
    re.find_iter(text).filter(|m| !m.is_empty()).map(|m| (to_char(m.start()), to_char(m.end()) - to_char(m.start()))).collect()
}

impl Results {
    /// Group parsed matches by file and lay out the list rows.
    pub fn build(query: Query, dir: String, tool: Tool, parsed: Vec<Parsed>, notice: Option<String>) -> Results {
        let mut binary_skipped = 0;
        let mut ms: Vec<Match> = Vec::new();
        for p in parsed {
            match p {
                Parsed::Match(m) => ms.push(m),
                Parsed::Binary => binary_skipped += 1,
                Parsed::Skip => {}
            }
        }
        let mut notice = notice;
        if ms.len() > MAX_MATCHES {
            ms.truncate(MAX_MATCHES);
            notice = Some(format!("stopped at {} matches: narrow the search", super::thousands(MAX_MATCHES)));
        }
        // Keep each file's matches together, files in order of first appearance.
        let mut order: std::collections::HashMap<String, usize> = Default::default();
        for m in &ms {
            let n = order.len();
            order.entry(m.path.clone()).or_insert(n);
        }
        ms.sort_by_key(|m| order[&m.path]);
        let mut files: Vec<(String, usize)> = Vec::new();
        let mut rows = Vec::new();
        for (i, m) in ms.iter().enumerate() {
            if files.last().is_none_or(|(p, _)| *p != m.path) {
                files.push((m.path.clone(), 0));
                rows.push(Row::Header(files.len() - 1));
            }
            files.last_mut().unwrap().1 += 1;
            rows.push(Row::Match(i));
        }
        let list = ListState::new(rows.iter().map(|r| matches!(r, Row::Match(_))).collect());
        let re = highlight_regex(&query);
        Results { query, dir, tool, matches: ms, files, rows, notice, error: None, binary_skipped, re, list }
    }

    pub fn failed(query: Query, dir: String, tool: Tool, error: String) -> Results {
        let mut r = Results::build(query, dir, tool, Vec::new(), None);
        r.error = Some(error);
        r
    }

    /// `37 matches in 6 files`, plus any notice.
    pub fn summary(&self) -> String {
        if let Some(e) = &self.error {
            return format!("search failed: {e}");
        }
        let n = self.matches.len();
        let mut s = if n == 0 {
            "no matches".to_string()
        } else {
            format!("{} match{} in {} file{}", super::thousands(n), if n == 1 { "" } else { "es" }, self.files.len(), if self.files.len() == 1 { "" } else { "s" })
        };
        if let Some(note) = &self.notice {
            s.push_str(&format!(" ({note})"));
        }
        if self.binary_skipped > 0 {
            s.push_str(&format!(" · {} binary file{} skipped", self.binary_skipped, if self.binary_skipped == 1 { "" } else { "s" }));
        }
        s
    }

    /// The match under the selection.
    pub fn selected(&self) -> Option<&Match> {
        match self.rows.get(self.list.sel) {
            Some(Row::Match(i)) => self.matches.get(*i),
            _ => None,
        }
    }
}

/// Parse the raw output of the search command.
pub fn parse_output(tool: Tool, out: &[u8], partial: bool) -> Vec<Parsed> {
    let text = String::from_utf8_lossy(out);
    let mut body: &str = &text;
    if partial && !body.ends_with('\n') {
        // The last line was cut off by the cap or the deadline.
        body = body.rfind('\n').map_or("", |i| &body[..=i]);
    }
    body.lines().map(|l| parse_line(tool, l)).collect()
}

/// Run the search in `dir`. Never fails: a tool error becomes `Results::failed`.
pub async fn search(backend: &Backend, dir: &str, query: Query) -> Results {
    let tool = detect_tool(backend, dir).await;
    let script = command(tool, &query);
    let o = match backend.run_script(dir, &script, crate::exec::OUT_CAP, SEARCH_TIMEOUT).await {
        Ok(o) => o,
        Err(e) => return Results::failed(query, dir.to_string(), tool, format!("{e:#}")),
    };
    let parsed = parse_output(tool, &o.stdout, o.truncated || o.timed_out);
    let has_any = parsed.iter().any(|p| !matches!(p, Parsed::Skip));
    if !has_any && !o.stderr.is_empty() && !o.timed_out {
        let first = o.stderr.lines().next().unwrap_or("error").to_string();
        // grep prints "regex parse error"-like complaints; a missing binary is code 127.
        return Results::failed(query, dir.to_string(), tool, first);
    }
    let notice = if o.timed_out {
        Some(format!("stopped after {} s: partial results", SEARCH_TIMEOUT.as_secs()))
    } else if o.truncated {
        Some("output limit reached: partial results".to_string())
    } else {
        None
    };
    Results::build(query, dir.to_string(), tool, parsed, notice)
}

/// Candidate files for the finder.
pub struct FileList {
    pub files: Vec<String>,
    pub notice: Option<String>,
}

pub fn parse_files(out: &[u8], partial: bool) -> FileList {
    let text = String::from_utf8_lossy(out);
    let mut body: &str = &text;
    if partial && !body.ends_with('\n') {
        body = body.rfind('\n').map_or("", |i| &body[..=i]);
    }
    let mut files: Vec<String> = body.lines().map(|l| l.strip_prefix("./").unwrap_or(l)).filter(|l| !l.is_empty() && !l.chars().any(char::is_control)).map(str::to_string).collect();
    let mut notice = partial.then(|| "file list cut short".to_string());
    if files.len() > FILES_CAP {
        files.truncate(FILES_CAP);
        notice = Some(format!("showing the first {} files", super::thousands(FILES_CAP)));
    }
    FileList { files, notice }
}

pub async fn list_files(backend: &Backend, dir: &str) -> Result<FileList, String> {
    let tool = detect_tool(backend, dir).await;
    let o = backend.run_script(dir, &files_command(tool), 16 * 1024 * 1024, SEARCH_TIMEOUT).await.map_err(|e| format!("{e:#}"))?;
    let l = parse_files(&o.stdout, o.truncated || o.timed_out);
    if l.files.is_empty() && !o.stderr.is_empty() {
        return Err(o.stderr.lines().next().unwrap_or("error").to_string());
    }
    Ok(l)
}

/// Cut a match line to `width` columns for display: leading blanks dropped, the first match kept
/// in view (with `…` marking what was cut). Ranges are (start, length) in characters, and the
/// returned ranges refer to the returned string.
pub fn snippet(text: &str, ranges: &[(usize, usize)], width: usize) -> (String, Vec<(usize, usize)>) {
    let chars: Vec<char> = text.chars().collect();
    let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
    let body = &chars[lead..];
    let shifted: Vec<(usize, usize)> = ranges.iter().filter(|(s, l)| s + l > lead).map(|&(s, l)| if s < lead { (0, l - (lead - s)) } else { (s - lead, l) }).collect();
    let width = width.max(4);
    if body.len() <= width {
        return (body.iter().collect(), shifted);
    }
    let first = shifted.first().map_or(0, |r| r.0);
    let first_len = shifted.first().map_or(0, |r| r.1);
    let start = if first + first_len < width { 0 } else { first.saturating_sub(8) };
    let prefix = usize::from(start > 0);
    let avail = width - prefix;
    let more = body.len() - start > avail;
    let take = if more { avail - 1 } else { avail };
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.extend(&body[start..start + take]);
    if more {
        s.push('…');
    }
    let out: Vec<(usize, usize)> = shifted
        .iter()
        .filter_map(|&(rs, rl)| {
            let (a, b) = (rs.max(start), (rs + rl).min(start + take));
            (b > a).then_some((a - start + prefix, b - a))
        })
        .collect();
    (s, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(t: &str) -> Query {
        Query::new(t)
    }

    fn m(path: &str, line: u32, col: u32, text: &str) -> Parsed {
        Parsed::Match(Match { path: path.into(), line, col, text: text.into() })
    }

    #[test]
    fn rg_command_is_literal_smart_case_and_bounded() {
        let c = command(Tool::Rg, &q("foo"));
        assert_eq!(
            c,
            "rg --no-config --vimgrep --no-heading --color never --max-columns 400 --max-columns-preview --max-count 500 --glob '!node_modules' -i -F -- 'foo' . | head -n 2001"
        );
        assert!(command(Tool::Rg, &q("Foo")).contains(" -s -F -- 'Foo'"));
    }

    #[test]
    fn grep_command_has_the_exclusions() {
        let c = command(Tool::Grep, &q("foo"));
        assert_eq!(c, "grep -rnIH --exclude-dir=.git --exclude-dir=node_modules -m 500 -i -F -- 'foo' . | head -n 2001");
        let mut x = q("a.*b");
        x.regex = true;
        x.word = true;
        x.case = Case::Insensitive;
        assert_eq!(command(Tool::Grep, &x), "grep -rnIH --exclude-dir=.git --exclude-dir=node_modules -m 500 -i -E -w -- 'a.*b' . | head -n 2001");
        x.case = Case::Sensitive;
        assert!(command(Tool::Rg, &x).contains(" -s -w -- 'a.*b' ."));
        assert!(!command(Tool::Rg, &x).contains("-F"));
    }

    #[test]
    fn queries_are_quoted_literally() {
        for t in ["it's", "a b", "$HOME", "`id`", "-rf", "--files", "a;b|c&d", "\"q\"", "$(touch pwned)", "back\\slash", "*.rs", "日本"] {
            for tool in [Tool::Rg, Tool::Grep] {
                let c = command(tool, &q(t));
                assert!(c.contains(&format!("-- {} .", sq(t))), "{c}");
            }
        }
        // The shell sees the query as one literal word (so $, backticks and quotes cannot run).
        for t in ["it's $HOME `id` \"x\" -rf", "$(echo pwned)", "a b"] {
            let script = format!("printf '%s\\n' -- {} .", sq(t));
            let out = std::process::Command::new("sh").arg("-c").arg(script).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), format!("--\n{t}\n.\n"));
        }
    }

    #[test]
    fn smart_case_resolution() {
        assert!(!q("foo").sensitive());
        assert!(q("Foo").sensitive());
        let mut x = q("\\Sfoo");
        x.regex = true;
        assert!(!x.sensitive(), "an escape like \\S is not a capital");
        x.text = "\\SFoo".into();
        assert!(x.sensitive());
        x.case = Case::Insensitive;
        assert!(!x.sensitive());
        assert_eq!(Case::Smart.next().next().next(), Case::Smart);
    }

    #[test]
    fn files_command_prefers_git() {
        let c = files_command(Tool::Rg);
        assert!(c.contains("git -c core.quotepath=off ls-files -co --exclude-standard"));
        assert!(c.contains("rg --no-config --files"));
        assert!(c.ends_with("| head -n 50001"));
        assert!(files_command(Tool::Grep).contains("find . -type f"));
    }

    #[test]
    fn parses_rg_vimgrep() {
        assert_eq!(parse_line(Tool::Rg, "src/main.rs:12:5:    let x = 1;"), m("src/main.rs", 12, 5, "    let x = 1;"));
        assert_eq!(parse_line(Tool::Rg, "./a.txt:1:1:hello"), m("a.txt", 1, 1, "hello"));
        // text that itself looks like a location stays in the text
        assert_eq!(parse_line(Tool::Rg, "a.txt:3:9:see b.rs:10:2:x"), m("a.txt", 3, 9, "see b.rs:10:2:x"));
        assert_eq!(parse_line(Tool::Rg, "a.txt:3:9:"), m("a.txt", 3, 9, ""));
        assert_eq!(parse_line(Tool::Rg, "a.txt:3:9:\ttabbed"), m("a.txt", 3, 9, " tabbed"));
    }

    #[test]
    fn parses_grep_output() {
        assert_eq!(parse_line(Tool::Grep, "./src/lib.rs:7:fn main() {}"), m("src/lib.rs", 7, 0, "fn main() {}"));
        assert_eq!(parse_line(Tool::Grep, "a.txt:3:time 10:30:00 ok"), m("a.txt", 3, 0, "time 10:30:00 ok"));
        assert_eq!(parse_line(Tool::Grep, "a b/c d.txt:3:x"), m("a b/c d.txt", 3, 0, "x"));
    }

    #[test]
    fn odd_paths() {
        assert_eq!(parse_line(Tool::Rg, r"C:\work\app\main.rs:44:3:text"), m(r"C:\work\app\main.rs", 44, 3, "text"));
        assert_eq!(parse_line(Tool::Rg, "C:/work/app.rs:44:3:text"), m("C:/work/app.rs", 44, 3, "text"));
        assert_eq!(parse_line(Tool::Grep, r"C:\work\app\main.rs:44:text"), m(r"C:\work\app\main.rs", 44, 0, "text"));
        assert_eq!(parse_line(Tool::Rg, "weird:name/file.txt:5:1:ok"), m("weird:name/file.txt", 5, 1, "ok"));
        assert_eq!(parse_line(Tool::Grep, "weird:name.txt:5:ok"), m("weird:name.txt", 5, 0, "ok"));
    }

    #[test]
    fn binary_notices_and_noise_are_not_matches() {
        assert_eq!(parse_line(Tool::Grep, "Binary file ./a.bin matches"), Parsed::Binary);
        assert_eq!(parse_line(Tool::Grep, "grep: ./x: binary file matches"), Parsed::Binary);
        assert_eq!(parse_line(Tool::Rg, "a.bin: binary file matches (found \"\\0\" byte around offset 3)"), Parsed::Binary);
        assert_eq!(parse_line(Tool::Rg, ""), Parsed::Skip);
        assert_eq!(parse_line(Tool::Rg, "--"), Parsed::Skip);
        assert_eq!(parse_line(Tool::Grep, ":5:no path"), Parsed::Skip);
    }

    #[test]
    fn control_characters_in_text_are_made_visible() {
        let Parsed::Match(x) = parse_line(Tool::Rg, "a.txt:1:1:evil\x1b[31m red\r") else { panic!() };
        assert_eq!(x.text, "evil·[31m red");
    }

    #[test]
    fn long_lines_are_cut_when_parsed() {
        let long = "x".repeat(5000);
        let Parsed::Match(x) = parse_line(Tool::Rg, &format!("a.txt:1:1:{long}")) else { panic!() };
        assert_eq!(x.text.chars().count(), KEEP_CHARS);
    }

    #[test]
    fn groups_by_file_and_summarises() {
        let parsed = vec![m("a.rs", 1, 1, "x"), m("b.rs", 2, 1, "x"), m("a.rs", 9, 1, "x"), Parsed::Binary, Parsed::Skip];
        let r = Results::build(q("x"), "/d".into(), Tool::Rg, parsed, None);
        assert_eq!(r.summary(), "3 matches in 2 files · 1 binary file skipped");
        assert_eq!(r.files, vec![("a.rs".into(), 2), ("b.rs".into(), 1)]);
        assert_eq!(r.rows.len(), 5, "two headers and three matches");
        assert_eq!(r.binary_skipped, 1);
        assert_eq!(r.list.sel, 1);
        assert_eq!(r.selected().map(|m| (m.path.as_str(), m.line)), Some(("a.rs", 1)));
        assert_eq!(r.matches[1].line, 9, "a.rs matches are together");
        assert_eq!(Results::build(q("x"), "/d".into(), Tool::Rg, vec![m("a", 1, 1, "")], None).summary(), "1 match in 1 file");
        assert_eq!(Results::build(q("x"), "/d".into(), Tool::Rg, vec![], None).summary(), "no matches");
    }

    #[test]
    fn cap_notice_at_2000_matches() {
        let parsed: Vec<Parsed> = (0..2001).map(|i| m("big.txt", i + 1, 1, "x")).collect();
        let r = Results::build(q("x"), "/d".into(), Tool::Rg, parsed, None);
        assert_eq!(r.matches.len(), 2000);
        assert!(r.summary().starts_with("2,000 matches in 1 file (stopped at 2,000 matches"), "{}", r.summary());
        let r = Results::build(q("x"), "/d".into(), Tool::Rg, vec![m("a", 1, 1, "")], Some("stopped after 10 s".into()));
        assert!(r.summary().ends_with("(stopped after 10 s)"));
    }

    #[test]
    fn partial_output_drops_the_cut_line() {
        let p = parse_output(Tool::Rg, b"a.txt:1:1:one\na.txt:2:1:tw", true);
        assert_eq!(p, vec![m("a.txt", 1, 1, "one")]);
        let p = parse_output(Tool::Rg, b"a.txt:1:1:one\na.txt:2:1:two", false);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn file_lists_are_cleaned_and_capped() {
        let l = parse_files(b"./a.rs\nsrc/b.rs\n\nbad\x1bname\n", false);
        assert_eq!(l.files, vec!["a.rs", "src/b.rs"]);
        let big: String = (0..FILES_CAP + 1).map(|i| format!("f{i}\n")).collect();
        let l = parse_files(big.as_bytes(), false);
        assert_eq!(l.files.len(), FILES_CAP);
        assert!(l.notice.unwrap().contains("50,000"));
    }

    #[test]
    fn highlights_literal_regex_and_word() {
        let re = highlight_regex(&q("a.b")).unwrap();
        assert_eq!(highlight_ranges(&re, "xa.b aXb"), vec![(1, 3)], "literal: the dot is a dot");
        let mut x = q("a.b");
        x.regex = true;
        let re = highlight_regex(&x).unwrap();
        assert_eq!(highlight_ranges(&re, "xa.b aXb").len(), 2);
        let mut w = q("cat");
        w.word = true;
        let re = highlight_regex(&w).unwrap();
        assert_eq!(highlight_ranges(&re, "cat concat cat"), vec![(0, 3), (11, 3)]);
        let re = highlight_regex(&q("日本")).unwrap();
        assert_eq!(highlight_ranges(&re, "ab日本c"), vec![(2, 2)], "character offsets, not bytes");
        let mut bad = q("(");
        bad.regex = true;
        assert!(highlight_regex(&bad).is_none());
    }

    #[test]
    fn snippets_fit_the_width_and_keep_the_match_visible() {
        let (s, r) = snippet("    let value = 1;", &[(8, 5)], 40);
        assert_eq!(s, "let value = 1;");
        assert_eq!(r, vec![(4, 5)]);
        let long = format!("{}needle{}", "a".repeat(100), "b".repeat(100));
        let (s, r) = snippet(&long, &[(100, 6)], 30);
        assert_eq!(s.chars().count(), 30);
        assert!(s.starts_with('…') && s.ends_with('…'));
        let (a, l) = r[0];
        assert_eq!(s.chars().skip(a).take(l).collect::<String>(), "needle");
        let (s, r) = snippet(&long, &[(0, 3)], 20);
        assert_eq!(s.chars().count(), 20);
        assert!(!s.starts_with('…') && s.ends_with('…'));
        assert_eq!(r, vec![(0, 3)]);
    }
}
