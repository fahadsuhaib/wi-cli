//! Pure link detection for terminal screen text: file paths (with line/col)
//! and URLs. No UI, no filesystem, no vt100 types: the input is the screen as
//! one `Vec<String>` per row (one string per column, "" for empty, "\u{0}"
//! for the right half of a wide char) plus a per-row "soft-wraps into the
//! next row" flag.
//!
//! A path match is only a *candidate*: the caller must check that the file
//! exists. Decisions on ambiguous cases:
//! - A path with a leading `/`, `~/`, `./` or `../` is a candidate if it has a
//!   letter (so `/help` is a candidate; the existence check rejects it).
//! - A relative path with a slash (`src/foo`) needs a last component that has a
//!   dot or is a well-known extensionless name (Makefile, LICENSE...). So
//!   `and/or`, `I/O`, `3/4`, `src/ui` are rejected.
//! - A bare name (no slash) needs a known extension or a known extensionless
//!   name (exact case), so `1.2.3`, `e.g.`, `bar.com` are rejected.
//! - Anything with a backslash or a colon in the path part (Windows `C:\x`)
//!   is rejected; emails (`@` without a slash) are rejected.
//! - Bare `www.x.y` URLs are returned with `https://` prepended.
//! - For a link that soft-wraps, `row`/`start_col` are where it starts,
//!   `end_col` is the inclusive end column on the LAST row it spans, and
//!   `wrap_rows` is how many extra rows it covers.
//! - `a/` / `b/` are stripped only for the path token right after `---`/`+++`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    Path {
        path: String,
        line: Option<u32>,
        col: Option<u32>,
    },
    Url(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub kind: LinkKind,
    pub row: u16,
    pub start_col: u16,
    /// Inclusive screen column (on the last row when wrapped).
    pub end_col: u16,
    pub wrap_rows: u16,
}

const EXTS: &[&str] = &[
    "rs",
    "py",
    "js",
    "ts",
    "tsx",
    "jsx",
    "mjs",
    "cjs",
    "go",
    "java",
    "kt",
    "kts",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cxx",
    "hxx",
    "cs",
    "rb",
    "php",
    "swift",
    "sh",
    "bash",
    "zsh",
    "fish",
    "md",
    "mdx",
    "txt",
    "rst",
    "json",
    "jsonl",
    "yaml",
    "yml",
    "toml",
    "csv",
    "tsv",
    "log",
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "sql",
    "xml",
    "lock",
    "ini",
    "cfg",
    "env",
    "conf",
    "dockerfile",
    "makefile",
    "gitignore",
    "lua",
    "scala",
    "dart",
    "vue",
    "svelte",
    "proto",
    "graphql",
    "gradle",
    "tf",
    "ipynb",
    "diff",
    "patch",
    "pdf",
    "png",
    "jpg",
    "jpeg",
    "gif",
    "svg",
    "zip",
    "tar",
    "gz",
];

const EXTLESS: &[&str] = &[
    "Makefile",
    "Dockerfile",
    "LICENSE",
    "README",
    "CHANGELOG",
    "Gemfile",
    "Rakefile",
    "Procfile",
    "Justfile",
    "Jenkinsfile",
];

// ---------------------------------------------------------------- screen

struct Cells {
    chars: Vec<char>,
    /// (row, first col, last col) of each char.
    pos: Vec<(u16, u16, u16)>,
}

fn build(rows: &[Vec<String>], first: usize, last: usize) -> Cells {
    let mut chars = Vec::new();
    let mut pos = Vec::new();
    for (r, cells) in rows.iter().enumerate().take(last + 1).skip(first) {
        for (c, s) in cells.iter().enumerate() {
            if s == "\u{0}" {
                continue;
            }
            if s.is_empty() {
                // A wide char that did not fit leaves the last cell of a
                // wrapped row empty: it is not a space.
                if r < last && c + 1 == cells.len() {
                    continue;
                }
                chars.push(' ');
                pos.push((r as u16, c as u16, c as u16));
                continue;
            }
            let end = if cells.get(c + 1).map(|x| x == "\u{0}").unwrap_or(false) {
                c + 1
            } else {
                c
            };
            for ch in s.chars() {
                let ch = if ch.is_control() || ('\u{2500}'..='\u{257f}').contains(&ch) {
                    ' '
                } else {
                    ch
                };
                chars.push(ch);
                pos.push((r as u16, c as u16, end as u16));
            }
        }
    }
    Cells { chars, pos }
}

fn is_wrapped(wrapped: &[bool], r: usize) -> bool {
    wrapped.get(r).copied().unwrap_or(false)
}

fn line_links(rows: &[Vec<String>], first: usize, last: usize) -> Vec<Link> {
    let cells = build(rows, first, last);
    find_links(&cells.chars)
        .into_iter()
        .map(|(s, e, kind)| {
            let (r0, c0, _) = cells.pos[s];
            let (r1, _, c1) = cells.pos[e];
            Link {
                kind,
                row: r0,
                start_col: c0,
                end_col: c1,
                wrap_rows: r1 - r0,
            }
        })
        .collect()
}

/// Every link on screen in reading order.
pub fn all_links(rows: &[Vec<String>], wrapped: &[bool]) -> Vec<Link> {
    let mut out = Vec::new();
    let mut r = 0;
    while r < rows.len() {
        let first = r;
        while r + 1 < rows.len() && is_wrapped(wrapped, r) {
            r += 1;
        }
        out.extend(line_links(rows, first, r));
        r += 1;
    }
    out
}

/// The link under screen cell (row, col), if any.
pub fn link_at(rows: &[Vec<String>], wrapped: &[bool], row: u16, col: u16) -> Option<Link> {
    let row_u = row as usize;
    if row_u >= rows.len() {
        return None;
    }
    let mut f = row_u;
    while f > 0 && is_wrapped(wrapped, f - 1) {
        f -= 1;
    }
    let mut l = row_u;
    while l + 1 < rows.len() && is_wrapped(wrapped, l) {
        l += 1;
    }
    line_links(rows, f, l)
        .into_iter()
        .find(|k| covers(k, row, col))
}

fn covers(k: &Link, row: u16, col: u16) -> bool {
    let last_row = k.row + k.wrap_rows;
    if row < k.row || row > last_row {
        return false;
    }
    if k.wrap_rows == 0 {
        return col >= k.start_col && col <= k.end_col;
    }
    if row == k.row {
        col >= k.start_col
    } else if row == last_row {
        col <= k.end_col
    } else {
        true
    }
}

// ---------------------------------------------------------------- scanning

type Found = (usize, usize, LinkKind); // start, inclusive end, kind

fn find_links(chars: &[char]) -> Vec<Found> {
    let mut claimed = vec![false; chars.len()];
    let mut out = Vec::new();
    urls(chars, &mut claimed, &mut out);
    py_file(chars, &mut claimed, &mut out);
    quoted(chars, &mut claimed, &mut out);
    tokens(chars, &mut claimed, &mut out);
    // Shell errors read `script.sh: line 11: ...`: the line number follows the path in words.
    for f in out.iter_mut() {
        if let LinkKind::Path { line, .. } = &mut f.2
            && line.is_none()
        {
            *line = line_word(chars, f.1 + 1);
        }
    }
    out.sort_by_key(|f| f.0);
    out
}

/// `: line 11`, `, line 11` or ` line 11` right after a path.
fn line_word(chars: &[char], mut i: usize) -> Option<u32> {
    while i < chars.len() && matches!(chars[i], ':' | ',' | ' ') {
        i += 1;
    }
    if !starts_ci(chars, i, "line") {
        return None;
    }
    i += 4;
    let gap = i;
    while i < chars.len() && chars[i] == ' ' {
        i += 1;
    }
    if i == gap {
        return None;
    }
    let digits = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits {
        return None;
    }
    chars[digits..i].iter().collect::<String>().parse().ok()
}

fn any_claimed(claimed: &[bool], s: usize, e: usize) -> bool {
    claimed[s..=e].iter().any(|&c| c)
}

fn claim(claimed: &mut [bool], s: usize, e: usize) {
    for c in &mut claimed[s..=e] {
        *c = true;
    }
}

fn starts_ci(chars: &[char], i: usize, pat: &str) -> bool {
    let mut k = i;
    for p in pat.chars() {
        match chars.get(k) {
            Some(c) if c.to_ascii_lowercase() == p => k += 1,
            _ => return false,
        }
    }
    true
}

fn count(chars: &[char], c: char) -> usize {
    chars.iter().filter(|&&x| x == c).count()
}

fn urls(chars: &[char], claimed: &mut [bool], out: &mut Vec<Found>) {
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let boundary = i == 0 || !chars[i - 1].is_alphanumeric();
        let scheme = if boundary {
            ["https://", "http://", "file://", "ftp://"]
                .iter()
                .find(|p| starts_ci(chars, i, p))
                .map(|p| p.len())
        } else {
            None
        };
        let www = scheme.is_none()
            && i < n
            && (i == 0 || !matches!(chars[i - 1], '.' | '/' | '@' | '-' | '_'))
            && boundary
            && starts_ci(chars, i, "www.");
        if scheme.is_none() && !www {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n
            && !chars[j].is_whitespace()
            && !matches!(chars[j], '<' | '>' | '"' | '`')
            && !(chars[j] == ']' && chars.get(j + 1) == Some(&'('))
        {
            j += 1;
        }
        let mut e = j;
        while e > i {
            let c = chars[e - 1];
            let s = &chars[i..e];
            if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '\'')
                || (c == ')' && count(s, ')') > count(s, '('))
                || (c == ']' && count(s, ']') > count(s, '['))
                || (c == '}' && count(s, '}') > count(s, '{'))
            {
                e -= 1;
            } else {
                break;
            }
        }
        let ok = match scheme {
            Some(len) => e > i + len && chars[i + len..e].iter().any(|c| c.is_alphanumeric()),
            None => {
                let host: Vec<char> = chars[i..e]
                    .iter()
                    .copied()
                    .take_while(|c| !matches!(c, '/' | ':' | '?' | '#'))
                    .collect();
                let labels: Vec<&[char]> = host.split(|&c| c == '.').collect();
                labels.len() >= 3
                    && labels.iter().all(|l| !l.is_empty())
                    && labels[1].iter().any(|c| c.is_alphabetic())
            }
        };
        if ok && !any_claimed(claimed, i, e - 1) {
            let text: String = chars[i..e].iter().collect();
            let text = if www { format!("https://{text}") } else { text };
            out.push((i, e - 1, LinkKind::Url(text)));
            claim(claimed, i, e - 1);
            i = e;
        } else {
            i += 1;
        }
    }
}

/// Python traceback: `File "x.py", line 3`.
fn py_file(chars: &[char], claimed: &mut [bool], out: &mut Vec<Found>) {
    let n = chars.len();
    let pat: Vec<char> = "File \"".chars().collect();
    let mid: Vec<char> = "\", line ".chars().collect();
    let mut i = 0;
    while i + pat.len() < n {
        if chars[i..].starts_with(&pat) && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            let ps = i + pat.len();
            if let Some(off) = chars[ps..].iter().position(|&c| c == '"') {
                let pe = ps + off;
                let ds = pe + mid.len();
                if chars[pe..].starts_with(&mid) {
                    let mut de = ds;
                    while de < n && chars[de].is_ascii_digit() {
                        de += 1;
                    }
                    let path: String = chars[ps..pe].iter().collect();
                    if de > ds && valid_path(&path) && !any_claimed(claimed, ps, de - 1) {
                        let line: Option<u32> =
                            chars[ds..de].iter().collect::<String>().parse().ok();
                        out.push((
                            ps,
                            de - 1,
                            LinkKind::Path {
                                path,
                                line,
                                col: None,
                            },
                        ));
                        claim(claimed, ps, de - 1);
                        i = de;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
}

/// Paths with spaces, only inside matching quotes.
fn quoted(chars: &[char], claimed: &mut [bool], out: &mut Vec<Found>) {
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let q = chars[i];
        let boundary = i == 0 || matches!(chars[i - 1], ' ' | '(' | '[' | '=' | ':' | ',');
        if (q != '"' && q != '\'') || !boundary {
            i += 1;
            continue;
        }
        let Some(off) = chars[i + 1..].iter().position(|&c| c == q) else {
            i += 1;
            continue;
        };
        let j = i + 1 + off;
        let content = &chars[i + 1..j];
        if j > i + 1
            && content.contains(&' ')
            && !any_claimed(claimed, i + 1, j - 1)
            && let Some(t) = parse_tail(content).filter(|t| content[..t.path_end].contains(&' '))
        {
            out.push((i + 1, i + t.link_end, t.to_kind(content)));
            claim(claimed, i + 1, i + t.link_end);
        }
        i = j + 1;
    }
}

fn tokens(chars: &[char], claimed: &mut [bool], out: &mut Vec<Found>) {
    let n = chars.len();
    let mut i = 0;
    let mut tok_no = 0;
    let mut diff_hdr = false;
    while i < n {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        tok_no += 1;
        if tok_no == 1 {
            let t: String = chars[start..i].iter().collect();
            diff_hdr = t == "---" || t == "+++";
        }
        let mut k = start;
        while k < i {
            if claimed[k] {
                k += 1;
                continue;
            }
            let s = k;
            while k < i && !claimed[k] {
                k += 1;
            }
            token(chars, s, k, diff_hdr && tok_no == 2, claimed, out);
        }
    }
}

fn token(
    chars: &[char],
    mut s: usize,
    e: usize,
    diff_second: bool,
    claimed: &mut [bool],
    out: &mut Vec<Found>,
) {
    // Markdown link target: take what follows the last `](`.
    if let Some(p) = (s..e.saturating_sub(1))
        .rev()
        .find(|&p| chars[p] == ']' && chars[p + 1] == '(')
    {
        s = p + 2;
    }
    while s < e && matches!(chars[s], '(' | '[' | '{' | '<' | '"' | '\'' | '`' | '*') {
        s += 1;
    }
    if s < e
        && chars[s] == '-'
        && let Some(p) = chars[s..e].iter().position(|&c| c == '=')
    {
        s += p + 1;
    }
    if diff_second && e - s > 2 && matches!(chars[s], 'a' | 'b') && chars[s + 1] == '/' {
        s += 2;
    }
    if s >= e {
        return;
    }
    let tok = &chars[s..e];
    if let Some(t) = parse_tail(tok) {
        out.push((s, s + t.link_end - 1, t.to_kind(tok)));
        claim(claimed, s, s + t.link_end - 1);
    }
}

struct Tail {
    path_end: usize,
    link_end: usize,
    line: Option<u32>,
    col: Option<u32>,
}

impl Tail {
    fn to_kind(&self, t: &[char]) -> LinkKind {
        LinkKind::Path {
            path: t[..self.path_end].iter().collect(),
            line: self.line,
            col: self.col,
        }
    }
}

fn digits_before(t: &[char], end: usize) -> usize {
    let mut s = end;
    while s > 0 && t[s - 1].is_ascii_digit() {
        s -= 1;
    }
    s
}

fn num(t: &[char]) -> Option<u32> {
    t.iter().collect::<String>().parse().ok()
}

/// Strip trailing punctuation and a line/col suffix from a token, validating
/// the path part.
fn parse_tail(t: &[char]) -> Option<Tail> {
    let mut end = t.len();
    let mut line = None;
    let mut col = None;
    let mut path_end = None;
    loop {
        while end > 0
            && matches!(
                t[end - 1],
                '.' | ',' | ';' | ':' | '>' | '"' | '\'' | ']' | '}' | '!' | '?' | '`' | '*'
            )
        {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        if t[end - 1] != ')' {
            break;
        }
        // `(LINE)` / `(LINE,COL)`
        if let Some(open) = t[..end - 1].iter().rposition(|&c| c == '(') {
            let inner = &t[open + 1..end - 1];
            let parts: Vec<&[char]> = inner.split(|&c| c == ',').collect();
            let ok = open > 0
                && (parts.len() == 1 || parts.len() == 2)
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.iter().all(|c| c.is_ascii_digit()));
            if ok {
                line = num(parts[0]);
                col = parts.get(1).and_then(|p| num(p));
                if line.is_none() || (parts.len() == 2 && col.is_none()) {
                    return None;
                }
                path_end = Some(open);
                break;
            }
        }
        end -= 1;
    }
    let path_end = match path_end {
        Some(p) => p,
        None => {
            let d1 = digits_before(t, end);
            if d1 < end && d1 > 0 && end - d1 < 10 {
                let n1 = num(&t[d1..end])?;
                match t[d1 - 1] {
                    ':' => {
                        let p = d1 - 1;
                        let d2 = digits_before(t, p);
                        if d2 < p && d2 > 1 && p - d2 < 10 && t[d2 - 1] == ':' {
                            line = Some(num(&t[d2..p])?);
                            col = Some(n1);
                            d2 - 1
                        } else {
                            line = Some(n1);
                            p
                        }
                    }
                    '#' => {
                        line = Some(n1);
                        d1 - 1
                    }
                    'L' if d1 >= 2 && t[d1 - 2] == '#' => {
                        line = Some(n1);
                        d1 - 2
                    }
                    _ => end,
                }
            } else {
                end
            }
        }
    };
    if path_end == 0 {
        return None;
    }
    let path: String = t[..path_end].iter().collect();
    if !valid_path(&path) {
        return None;
    }
    Some(Tail {
        path_end,
        link_end: end,
        line,
        col,
    })
}

fn known_ext(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => {
            let ext = ext.to_ascii_lowercase();
            EXTS.contains(&ext.as_str())
        }
        None => false,
    }
}

fn valid_path(p: &str) -> bool {
    if p.is_empty() || p.len() > 1024 {
        return false;
    }
    if p.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                '\\' | ':'
                    | '<'
                    | '>'
                    | '|'
                    | '*'
                    | '?'
                    | '"'
                    | '`'
                    | '{'
                    | '}'
                    | ','
                    | ';'
                    | '('
                    | ')'
                    | '='
                    | '\''
                    | '$'
                    | '!'
                    | '^'
                    | '&'
            )
    }) {
        return false;
    }
    if p.starts_with("//") || p.starts_with('-') || (p.contains('@') && !p.contains('/')) {
        return false;
    }
    if !p.chars().any(|c| c.is_alphabetic()) || p.ends_with('.') {
        return false;
    }
    let prefixed =
        p.starts_with('/') || p.starts_with("~/") || p.starts_with("./") || p.starts_with("../");
    let last = p.rsplit('/').next().unwrap_or("");
    if last.is_empty() || prefixed {
        return prefixed;
    }
    if EXTLESS.contains(&last) {
        return true;
    }
    if p.contains('/') {
        last.contains('.') && last.chars().any(|c| c.is_alphabetic())
    } else {
        known_ext(last)
    }
}

// ---------------------------------------------------------------- OSC 7

/// `file://host/Users/me/proj` (percent-encoded) -> `/Users/me/proj`.
pub fn parse_osc7(payload: &str) -> Option<String> {
    let head = payload.get(..7)?;
    if !head.eq_ignore_ascii_case("file://") {
        return None;
    }
    let rest = &payload[7..];
    let path = &rest[rest.find('/')?..];
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).ok()?;
    if s.contains('\0') {
        return None;
    }
    Some(s)
}

// ---------------------------------------------------------------- resolve

fn normalise(path: &str) -> String {
    let abs = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if matches!(parts.last(), Some(&p) if p != "..") {
                    parts.pop();
                } else if !abs {
                    parts.push("..");
                }
            }
            c => parts.push(c),
        }
    }
    let j = parts.join("/");
    if abs { format!("/{j}") } else { j }
}

/// Ordered, de-duplicated absolute paths to try for a detected path.
pub fn resolve_candidates(
    link_path: &str,
    tab_cwd: Option<&str>,
    root: &str,
    home: Option<&str>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    if link_path == "~" || link_path.starts_with("~/") {
        if let Some(h) = home {
            push(normalise(&format!("{h}/{}", &link_path[1..])));
        }
    } else if link_path.starts_with('/') {
        push(normalise(link_path));
    } else {
        for base in tab_cwd.into_iter().chain(std::iter::once(root)) {
            push(normalise(&format!("{base}/{link_path}")));
        }
    }
    out
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn row(s: &str) -> Vec<String> {
        s.chars()
            .map(|c| {
                if c == ' ' {
                    String::new()
                } else {
                    c.to_string()
                }
            })
            .collect()
    }

    fn rows_of(lines: &[&str]) -> Vec<Vec<String>> {
        lines.iter().map(|l| row(l)).collect()
    }

    fn links1(text: &str) -> Vec<Link> {
        all_links(&[row(text)], &[false])
    }

    fn kind(text: &str) -> LinkKind {
        let l = links1(text);
        assert_eq!(l.len(), 1, "{text:?} -> {l:?}");
        l[0].kind.clone()
    }

    fn pk(path: &str, line: Option<u32>, col: Option<u32>) -> LinkKind {
        LinkKind::Path {
            path: path.into(),
            line,
            col,
        }
    }

    macro_rules! p {
        ($name:ident, $text:expr, $path:expr, $line:expr, $col:expr) => {
            #[test]
            fn $name() {
                assert_eq!(kind($text), pk($path, $line, $col));
            }
        };
    }
    macro_rules! u {
        ($name:ident, $text:expr, $url:expr) => {
            #[test]
            fn $name() {
                assert_eq!(kind($text), LinkKind::Url($url.into()));
            }
        };
    }
    macro_rules! none {
        ($name:ident, $text:expr) => {
            #[test]
            fn $name() {
                let l = links1($text);
                assert!(l.is_empty(), "{:?} -> {l:?}", $text);
            }
        };
    }

    // paths
    p!(abs_path, "open /a/b.c now", "/a/b.c", None, None);
    p!(home_path, "~/a/b", "~/a/b", None, None);
    p!(dot_slash, "run ./x please", "./x", None, None);
    p!(dot_dot, "see ../x/y", "../x/y", None, None);
    p!(rel_slash, "edit src/foo.rs", "src/foo.rs", None, None);
    p!(bare_ext, "edit main.rs", "main.rs", None, None);
    p!(bare_ext_upper, "see README.MD", "README.MD", None, None);
    p!(bare_extless_name, "see Makefile", "Makefile", None, None);
    p!(
        rel_extless_name,
        "see docs/LICENSE",
        "docs/LICENSE",
        None,
        None
    );
    p!(dotfile, "cat .env", ".env", None, None);
    p!(line_only, "src/main.rs:12", "src/main.rs", Some(12), None);
    p!(
        line_col,
        "src/main.rs:12:5",
        "src/main.rs",
        Some(12),
        Some(5)
    );
    p!(paren_line, "src/a.ts(10)", "src/a.ts", Some(10), None);
    p!(
        paren_line_col,
        "src/a.ts(10,5)",
        "src/a.ts",
        Some(10),
        Some(5)
    );
    p!(hash_line, "src/a.rs#12", "src/a.rs", Some(12), None);
    p!(hash_l_line, "src/a.rs#L12", "src/a.rs", Some(12), None);
    p!(
        rustc,
        "  --> src/lib.rs:5:9",
        "src/lib.rs",
        Some(5),
        Some(9)
    );
    p!(
        pytest,
        "tests/test_a.py:12: AssertionError",
        "tests/test_a.py",
        Some(12),
        None
    );
    p!(
        tsc,
        "src/a.ts(10,5): error TS2322",
        "src/a.ts",
        Some(10),
        Some(5)
    );
    p!(
        go_err,
        "./main.go:10:2: undefined: x",
        "./main.go",
        Some(10),
        Some(2)
    );
    p!(
        trailing_colon_after_line,
        "src/a.rs:7:",
        "src/a.rs",
        Some(7),
        None
    );
    p!(trailing_period, "wrote src/a.rs.", "src/a.rs", None, None);
    p!(
        trailing_comma,
        "files: src/a.rs, other",
        "src/a.rs",
        None,
        None
    );
    p!(in_parens, "(see src/a.rs)", "src/a.rs", None, None);
    p!(in_parens_line, "(src/a.rs:12)", "src/a.rs", Some(12), None);
    p!(in_quotes, "\"src/a.rs\"", "src/a.rs", None, None);
    p!(in_brackets, "[src/a.rs]", "src/a.rs", None, None);
    p!(
        py_traceback,
        "  File \"x.py\", line 3, in <module>",
        "x.py",
        Some(3),
        None
    );
    p!(
        py_traceback_abs,
        "File \"/tmp/a b/x.py\", line 30",
        "/tmp/a b/x.py",
        Some(30),
        None
    );
    p!(diff_minus, "--- a/src/x.rs", "src/x.rs", None, None);
    p!(diff_plus, "+++ b/src/x.rs", "src/x.rs", None, None);
    p!(
        not_diff_prefix_stripped,
        "see a/src/x.rs",
        "a/src/x.rs",
        None,
        None
    );
    p!(
        ls_l,
        "-rw-r--r--  1 me  staff  120 Nov  2 10:00 notes.md",
        "notes.md",
        None,
        None
    );
    p!(
        markdown_link,
        "[the guide](docs/guide.md)",
        "docs/guide.md",
        None,
        None
    );
    p!(
        markdown_link_spaces,
        "[my long text](docs/guide.md) ok",
        "docs/guide.md",
        None,
        None
    );
    p!(
        quoted_spaces_dq,
        "open \"my dir/a b.txt\" now",
        "my dir/a b.txt",
        None,
        None
    );
    p!(
        quoted_spaces_sq,
        "open 'my dir/a b.txt:4' now",
        "my dir/a b.txt",
        Some(4),
        None
    );
    p!(
        flag_equals,
        "--config=conf/app.toml",
        "conf/app.toml",
        None,
        None
    );
    p!(unicode_name, "see src/héllo.rs", "src/héllo.rs", None, None);
    p!(nested_ext_case, "see Foo.TXT", "Foo.TXT", None, None);
    p!(
        abs_no_ext,
        "cd /usr/local/bin",
        "/usr/local/bin",
        None,
        None
    );

    // negatives
    none!(neg_version, "version 1.2.3 released");
    none!(neg_time, "at 12:30 today");
    none!(neg_eg, "e.g. this");
    none!(neg_ie, "i.e. that");
    none!(neg_email, "mail foo@bar.com now");
    none!(neg_ratio, "ratio 3/4 here");
    none!(neg_and_or, "use and/or here");
    none!(neg_io, "I/O bound");
    none!(neg_number, "count 12345");
    none!(neg_word, "hello world");
    none!(neg_rel_dir_no_ext, "see src/ui there");
    none!(neg_windows, "C:\\x\\y.py");
    none!(neg_windows_ext, "open C:\\Users\\me\\a.txt");
    none!(neg_dates, "on 2024/01/02 it");
    none!(neg_unknown_ext, "visit bar.com today");
    none!(neg_dash_flag, "--verbose -x");
    none!(neg_time_pm, "10:30pm");
    none!(neg_only_punct, "... --- ///");

    // urls
    u!(
        url_basic,
        "go https://example.com/a?b=1#c now",
        "https://example.com/a?b=1#c"
    );
    u!(
        url_trailing_dot,
        "see https://example.com/a.",
        "https://example.com/a"
    );
    u!(
        url_trailing_comma_paren,
        "(https://example.com/a), ok",
        "https://example.com/a"
    );
    u!(
        url_wiki_parens,
        "https://en.wikipedia.org/wiki/Foo_(bar)",
        "https://en.wikipedia.org/wiki/Foo_(bar)"
    );
    u!(
        url_wiki_parens_wrapped,
        "(https://en.wikipedia.org/wiki/Foo_(bar))",
        "https://en.wikipedia.org/wiki/Foo_(bar)"
    );
    u!(
        url_http,
        "http://localhost:3000/x",
        "http://localhost:3000/x"
    );
    u!(url_file, "file:///tmp/a.txt", "file:///tmp/a.txt");
    u!(url_ftp, "ftp://host/file.bin", "ftp://host/file.bin");
    u!(
        url_markdown,
        "[x](https://a.example/b)",
        "https://a.example/b"
    );
    u!(url_quoted, "\"https://a.example/b\"", "https://a.example/b");
    u!(
        url_www,
        "visit www.example.com/x",
        "https://www.example.com/x"
    );
    u!(
        url_www_prefixed,
        "visit https://www.example.com",
        "https://www.example.com"
    );
    u!(url_semicolon, "https://a.example/b;", "https://a.example/b");

    #[test]
    fn url_not_also_path() {
        assert_eq!(links1("https://example.com/foo/bar.html").len(), 1);
    }

    #[test]
    fn www_needs_three_labels() {
        assert!(links1("www.x").is_empty());
    }

    #[test]
    fn column_span_simple() {
        let l = &links1("see src/a.rs:12:5 ok")[0];
        assert_eq!((l.row, l.start_col, l.end_col, l.wrap_rows), (0, 4, 16, 0));
    }

    #[test]
    fn path_at_row_start() {
        let l = &links1("a.rs and more")[0];
        assert_eq!((l.start_col, l.end_col), (0, 3));
    }

    #[test]
    fn path_at_row_end() {
        let l = &links1("open src/a.rs")[0];
        assert_eq!((l.start_col, l.end_col), (5, 12));
    }

    #[test]
    fn two_links_one_row() {
        let l = links1("a.rs https://x.org/y b/c.py:3");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].kind, pk("a.rs", None, None));
        assert_eq!(l[1].kind, LinkKind::Url("https://x.org/y".into()));
        assert_eq!(l[2].kind, pk("b/c.py", Some(3), None));
        assert!(l[0].end_col < l[1].start_col && l[1].end_col < l[2].start_col);
    }

    #[test]
    fn link_at_hits_and_misses() {
        let rows = vec![row("see src/a.rs:12 ok")];
        assert!(link_at(&rows, &[false], 0, 3).is_none());
        assert!(link_at(&rows, &[false], 0, 4).is_some());
        assert!(link_at(&rows, &[false], 0, 14).is_some());
        assert!(link_at(&rows, &[false], 0, 15).is_none());
        assert!(link_at(&rows, &[false], 5, 4).is_none());
    }

    #[test]
    fn wide_chars_before_path() {
        // "你好 src/a.rs": wide chars occupy two cells each.
        let mut r: Vec<String> = vec![
            "你".into(),
            "\u{0}".into(),
            "好".into(),
            "\u{0}".into(),
            String::new(),
        ];
        r.extend("src/a.rs".chars().map(|c| c.to_string()));
        let l = all_links(&[r.clone()], &[false]);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].start_col, l[0].end_col), (5, 12));
        assert!(link_at(&[r.clone()], &[false], 0, 12).is_some());
        assert!(link_at(&[r], &[false], 0, 2).is_none());
    }

    #[test]
    fn wide_char_inside_path_end_covers_right_half() {
        let mut r: Vec<String> = "x/".chars().map(|c| c.to_string()).collect();
        r.extend(["文".into(), "\u{0}".into()]);
        r.extend(".rs".chars().map(|c| c.to_string()));
        let l = all_links(&[r], &[false]);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].start_col, l[0].end_col), (0, 6));
    }

    #[test]
    fn wrapped_path_two_rows() {
        let rows = rows_of(&["see src/very/lo", "ng/file.rs:7 ok"]);
        let l = all_links(&rows, &[true, false]);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].kind, pk("src/very/long/file.rs", Some(7), None));
        assert_eq!(
            (l[0].row, l[0].start_col, l[0].end_col, l[0].wrap_rows),
            (0, 4, 11, 1)
        );
        assert!(link_at(&rows, &[true, false], 1, 3).is_some());
        assert!(link_at(&rows, &[true, false], 0, 5).is_some());
        assert!(link_at(&rows, &[true, false], 0, 2).is_none());
        assert!(link_at(&rows, &[true, false], 1, 12).is_none());
    }

    #[test]
    fn wrapped_url_three_rows() {
        let rows = rows_of(&["https://exam", "ple.com/aaaa", "bbbb end"]);
        let l = all_links(&rows, &[true, true, false]);
        assert_eq!(l.len(), 1);
        assert_eq!(
            l[0].kind,
            LinkKind::Url("https://example.com/aaaabbbb".into())
        );
        assert_eq!(l[0].wrap_rows, 2);
        assert!(link_at(&rows, &[true, true, false], 1, 0).is_some());
    }

    #[test]
    fn unwrapped_rows_do_not_join() {
        let rows = rows_of(&["see src/very/lo", "ng/file.rs:7 ok"]);
        let l = all_links(&rows, &[false, false]);
        // Left: "src/very/lo" has no dot; right: "ng/file.rs:7" is its own link.
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].kind, pk("ng/file.rs", Some(7), None));
    }

    #[test]
    fn wrap_flag_missing_is_false() {
        let rows = rows_of(&["a.rs", "b.rs"]);
        assert_eq!(all_links(&rows, &[]).len(), 2);
    }

    #[test]
    fn reading_order_across_rows() {
        let rows = rows_of(&["x a.rs", "y b.rs"]);
        let l = all_links(&rows, &[false, false]);
        assert_eq!((l[0].row, l[1].row), (0, 1));
    }

    #[test]
    fn wide_char_leftover_at_wrap_edge() {
        // Row 0 ends with an empty cell (wide char did not fit); path continues.
        let mut r0: Vec<String> = "ab src/"
            .chars()
            .map(|c| {
                if c == ' ' {
                    String::new()
                } else {
                    c.to_string()
                }
            })
            .collect();
        r0.push(String::new());
        let r1 = row("a.rs");
        let l = all_links(&[r0, r1], &[true, false]);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].kind, pk("src/a.rs", None, None));
    }

    #[test]
    fn box_drawing_is_separator() {
        let l = links1("│src/a.rs│");
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].kind, pk("src/a.rs", None, None));
    }

    #[test]
    fn diff_header_only_second_token() {
        let l = links1("+++ b/src/x.rs");
        assert_eq!(l[0].kind, pk("src/x.rs", None, None));
        assert_eq!(l[0].start_col, 6);
    }

    #[test]
    fn bash_error_line_gives_both_paths_and_the_line_number() {
        let text = "dev/run_tests.sh: line 11: /private/tmp/scratchpad/main/venv/bin/python: No such file or directory";
        let l = links1(text);
        assert_eq!(l.len(), 2, "{l:?}");
        assert_eq!(l[0].kind, LinkKind::Path { path: "dev/run_tests.sh".into(), line: Some(11), col: None });
        assert_eq!(l[0].start_col, 0);
        assert_eq!(l[0].end_col as usize, "dev/run_tests.sh".len() - 1, "the link covers only the path");
        assert_eq!(l[1].kind, LinkKind::Path { path: "/private/tmp/scratchpad/main/venv/bin/python".into(), line: None, col: None });
    }

    #[test]
    fn line_words_after_a_path() {
        assert_eq!(kind("./x.sh: line 3: foo: command not found"), LinkKind::Path { path: "./x.sh".into(), line: Some(3), col: None });
        assert_eq!(kind("src/a.rs, line 40"), LinkKind::Path { path: "src/a.rs".into(), line: Some(40), col: None });
        // an explicit :LINE still wins, and prose without a number is left alone
        assert_eq!(kind("src/a.rs:7: line 9"), LinkKind::Path { path: "src/a.rs".into(), line: Some(7), col: None });
        assert_eq!(kind("src/a.rs: linefeed"), LinkKind::Path { path: "src/a.rs".into(), line: None, col: None });
        assert_eq!(kind("src/a.rs: line x"), LinkKind::Path { path: "src/a.rs".into(), line: None, col: None });
    }

    #[test]
    fn py_traceback_span_includes_line() {
        let l = &links1("File \"x.py\", line 30")[0];
        assert_eq!((l.start_col, l.end_col), (6, 19));
    }

    #[test]
    fn markdown_link_span_is_target_only() {
        let l = &links1("[t](a/b.md)")[0];
        assert_eq!((l.start_col, l.end_col), (4, 9));
    }

    #[test]
    fn huge_line_number_not_a_suffix() {
        assert!(links1("a.rs:99999999999").is_empty());
    }

    // OSC 7
    #[test]
    fn osc7_basic() {
        assert_eq!(
            parse_osc7("file://host/Users/me/proj").as_deref(),
            Some("/Users/me/proj")
        );
    }

    #[test]
    fn osc7_empty_host() {
        assert_eq!(parse_osc7("file:///tmp/x").as_deref(), Some("/tmp/x"));
    }

    #[test]
    fn osc7_percent() {
        assert_eq!(
            parse_osc7("file://h/a%20b/%C3%A9").as_deref(),
            Some("/a b/é")
        );
    }

    #[test]
    fn osc7_rejects_non_file() {
        assert_eq!(parse_osc7("https://host/x"), None);
        assert_eq!(parse_osc7("file://hostonly"), None);
        assert_eq!(parse_osc7(""), None);
    }

    #[test]
    fn osc7_rejects_bad_percent() {
        assert_eq!(parse_osc7("file://h/a%zz"), None);
        assert_eq!(parse_osc7("file://h/a%2"), None);
        assert_eq!(parse_osc7("file://h/a%FF"), None);
    }

    // resolve
    #[test]
    fn resolve_relative_cwd_then_root() {
        assert_eq!(
            resolve_candidates("src/a.rs", Some("/w/sub"), "/w", None),
            vec!["/w/sub/src/a.rs", "/w/src/a.rs"]
        );
    }

    #[test]
    fn resolve_strips_dot_slash() {
        assert_eq!(
            resolve_candidates("./a.rs", Some("/w"), "/w", None),
            vec!["/w/a.rs"]
        );
    }

    #[test]
    fn resolve_dotdot() {
        assert_eq!(
            resolve_candidates("../x/./y.rs", Some("/w/sub"), "/w", None),
            vec!["/w/x/y.rs", "/x/y.rs"]
        );
    }

    #[test]
    fn resolve_dotdot_above_root() {
        assert_eq!(
            resolve_candidates("../../../a", None, "/w", None),
            vec!["/a"]
        );
    }

    #[test]
    fn resolve_absolute_kept() {
        assert_eq!(
            resolve_candidates("/a//b/../c.rs", Some("/w"), "/r", None),
            vec!["/a/c.rs"]
        );
    }

    #[test]
    fn resolve_home() {
        assert_eq!(
            resolve_candidates("~/p/a.rs", Some("/w"), "/r", Some("/home/me")),
            vec!["/home/me/p/a.rs"]
        );
        assert!(resolve_candidates("~/p/a.rs", Some("/w"), "/r", None).is_empty());
    }

    #[test]
    fn resolve_no_cwd() {
        assert_eq!(
            resolve_candidates("a.rs", None, "/r", None),
            vec!["/r/a.rs"]
        );
    }

    #[test]
    fn resolve_dedup() {
        assert_eq!(
            resolve_candidates("a.rs", Some("/r"), "/r", None),
            vec!["/r/a.rs"]
        );
    }

    #[test]
    fn dense_screen_is_fast() {
        let line = "let x = foo(bar) // see src/main.rs:12:5 and https://a.example/b (c.py) hello world 1.2.3 ";
        let rows: Vec<Vec<String>> = (0..50)
            .map(|_| {
                line.chars()
                    .cycle()
                    .take(200)
                    .map(|c| {
                        if c == ' ' {
                            String::new()
                        } else {
                            c.to_string()
                        }
                    })
                    .collect()
            })
            .collect();
        let wrapped = vec![false; 50];
        let t = std::time::Instant::now();
        let l = all_links(&rows, &wrapped);
        let el = t.elapsed();
        assert!(!l.is_empty());
        assert!(el.as_millis() < 20, "took {el:?}");
    }
}
