//! Search: text in files, files by name, text in shell output. The engines here are pure
//! (parsers, scorer, matcher) so they can be tested without a terminal; `app.rs` wires them up.
//! Nothing is indexed and nothing stays running: every search is one bounded command.

pub mod fuzzy;
pub mod grep;
pub mod list;
pub mod output;

/// Smart case: a needle with an uppercase letter matches exactly, otherwise case is ignored.
pub fn smart_sensitive(needle: &str) -> bool {
    needle.chars().any(|c| c.is_uppercase())
}

fn fold(c: char, sensitive: bool) -> char {
    if sensitive { c } else { c.to_lowercase().next().unwrap_or(c) }
}

/// Every (non-overlapping) occurrence of `needle` in `hay` as `(start, len)` in characters.
/// Smart case. An empty needle matches nothing.
pub fn find_matches(hay: &str, needle: &str) -> Vec<(usize, usize)> {
    let sens = smart_sensitive(needle);
    let n: Vec<char> = needle.chars().map(|c| fold(c, sens)).collect();
    let h: Vec<char> = hay.chars().map(|c| fold(c, sens)).collect();
    find_in_chars(&h, &n)
}

/// Like `find_matches` on text that is already case-folded.
pub fn find_in_chars(h: &[char], n: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if n.is_empty() || h.len() < n.len() {
        return out;
    }
    let mut i = 0;
    while i + n.len() <= h.len() {
        if h[i..i + n.len()] == *n {
            out.push((i, n.len()));
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

/// One decoded key for the search prompts and lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum K {
    Char(char),
    Backspace,
    /// Ctrl-U: clear the line.
    Clear,
    Enter,
    Esc,
    Tab,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Menu,
}

/// Decode raw input into keys. The menu key (and a bare Esc) are reported so the caller can close.
pub fn decode(buf: &[u8], menu_key: u8) -> Vec<K> {
    let mut keys = Vec::new();
    let mut i = 0;
    let mut text: Vec<u8> = Vec::new();
    let flush = |text: &mut Vec<u8>, keys: &mut Vec<K>| {
        if !text.is_empty() {
            keys.extend(String::from_utf8_lossy(text).chars().filter(|c| !c.is_control()).map(K::Char));
            text.clear();
        }
    };
    while i < buf.len() {
        let b = buf[i];
        if b >= 0x20 && b != 0x7f && b != menu_key {
            text.push(b);
            i += 1;
            continue;
        }
        flush(&mut text, &mut keys);
        match b {
            0x1b => {
                if matches!(buf.get(i + 1), Some(b'[') | Some(b'O')) {
                    let mut j = i + 2;
                    while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                        j += 1;
                    }
                    if j >= buf.len() {
                        break;
                    }
                    let params = &buf[i + 2..j];
                    keys.extend(match (buf[j], params) {
                        (b'A', _) => Some(K::Up),
                        (b'B', _) => Some(K::Down),
                        (b'H', _) | (b'~', b"1") | (b'~', b"7") => Some(K::Home),
                        (b'F', _) | (b'~', b"4") | (b'~', b"8") => Some(K::End),
                        (b'~', b"5") => Some(K::PageUp),
                        (b'~', b"6") => Some(K::PageDown),
                        _ => None,
                    });
                    i = j;
                } else {
                    keys.push(K::Esc);
                }
            }
            b'\r' | b'\n' => keys.push(K::Enter),
            b'\t' => keys.push(K::Tab),
            0x7f | 0x08 => keys.push(K::Backspace),
            0x15 => keys.push(K::Clear),
            x if x == menu_key => keys.push(K::Menu),
            _ => {}
        }
        i += 1;
    }
    flush(&mut text, &mut keys);
    keys
}

/// Apply a key to a single-line text field. Returns true when the text changed.
pub fn edit(text: &mut String, k: K) -> bool {
    match k {
        K::Char(c) => {
            text.push(c);
            true
        }
        K::Backspace => text.pop().is_some(),
        K::Clear => !std::mem::take(text).is_empty(),
        _ => false,
    }
}

/// State of the "search in files" prompt: the query and its three options.
#[derive(Clone)]
pub struct GrepPrompt {
    pub query: grep::Query,
}

impl GrepPrompt {
    pub fn new(from: Option<&grep::Query>) -> GrepPrompt {
        GrepPrompt { query: from.cloned().unwrap_or_else(|| grep::Query::new("")) }
    }

    /// Apply a key; returns true when the prompt should run the search.
    pub fn key(&mut self, k: K) -> bool {
        match k {
            K::Enter => return !self.query.text.is_empty(),
            K::Tab => self.query.case = self.query.case.next(),
            K::Up => self.query.word = !self.query.word,
            K::Down => self.query.regex = !self.query.regex,
            k => {
                edit(&mut self.query.text, k);
            }
        }
        false
    }
}

/// What a find prompt searches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindTarget {
    Output,
    Preview,
}

#[derive(Clone)]
pub struct FindPrompt {
    pub target: FindTarget,
    pub text: String,
}

/// The fuzzy file finder: the candidate list and the narrowed, ranked view of it.
pub struct Finder {
    pub dir: String,
    pub files: Vec<String>,
    pub notice: Option<String>,
    pub input: String,
    pub hits: Vec<fuzzy::Hit>,
    pub list: list::ListState,
}

impl Finder {
    pub fn new(files: Vec<String>, notice: Option<String>, dir: String) -> Finder {
        let hits = fuzzy::rank("", &files);
        let list = list::ListState::plain(hits.len());
        Finder { dir, files, notice, input: String::new(), hits, list }
    }

    /// Narrow the list after the input changed. Typing more reuses the previous result.
    pub fn refilter(&mut self, previous: &str) {
        let hits = if !previous.trim().is_empty() && self.input.starts_with(previous) {
            let pool: Vec<String> = self.hits.iter().map(|h| self.files[h.idx].clone()).collect();
            let prev: Vec<usize> = self.hits.iter().map(|h| h.idx).collect();
            fuzzy::rank(&self.input, &pool).into_iter().map(|h| fuzzy::Hit { idx: prev[h.idx], score: h.score }).collect()
        } else {
            fuzzy::rank(&self.input, &self.files)
        };
        self.hits = hits;
        self.list.reset(vec![true; self.hits.len()]);
    }

    /// Type into the input (keys or pasted text) and refilter.
    pub fn type_keys(&mut self, keys: &[K]) {
        let before = self.input.clone();
        for &k in keys {
            edit(&mut self.input, k);
        }
        if self.input != before {
            self.refilter(&before);
        }
    }

    pub fn selected(&self) -> Option<&str> {
        self.hits.get(self.list.sel).map(|h| self.files[h.idx].as_str())
    }
}

/// Number with thousands separators: `2,000`.
pub fn thousands(n: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_all_occurrences_smart_case() {
        assert_eq!(find_matches("Foo foo FOO", "foo"), vec![(0, 3), (4, 3), (8, 3)]);
        assert_eq!(find_matches("Foo foo FOO", "Foo"), vec![(0, 3)]);
        assert_eq!(find_matches("aaaa", "aa"), vec![(0, 2), (2, 2)]);
        assert!(find_matches("abc", "").is_empty());
        assert!(find_matches("ab", "abc").is_empty());
        assert_eq!(find_matches("日本語 日本", "日本"), vec![(0, 2), (4, 2)]);
    }

    #[test]
    fn decodes_keys() {
        assert_eq!(decode(b"ab\x7f\r", 0), vec![K::Char('a'), K::Char('b'), K::Backspace, K::Enter]);
        assert_eq!(decode(b"\x1b[A\x1b[B\x1b[5~\x1b[6~\x1b[H\x1b[F\t", 0), vec![K::Up, K::Down, K::PageUp, K::PageDown, K::Home, K::End, K::Tab]);
        assert_eq!(decode(b"\x1b", 0), vec![K::Esc]);
        assert_eq!(decode(b"a\x00", 0), vec![K::Char('a'), K::Menu]);
        assert_eq!(decode("é日".as_bytes(), 0), vec![K::Char('é'), K::Char('日')]);
        assert_eq!(decode(b"\x15", 0), vec![K::Clear]);
    }

    #[test]
    fn edits_text() {
        let mut t = String::new();
        assert!(edit(&mut t, K::Char('a')));
        assert!(edit(&mut t, K::Char('b')));
        assert!(edit(&mut t, K::Backspace));
        assert_eq!(t, "a");
        assert!(edit(&mut t, K::Clear));
        assert!(!edit(&mut t, K::Backspace));
    }

    #[test]
    fn finder_narrows_as_you_type_and_reuses_results() {
        let files: Vec<String> = ["README.md", "src/app.rs", "src/main.rs", "tests/cli.rs"].iter().map(|s| s.to_string()).collect();
        let mut f = Finder::new(files, None, "/d".into());
        assert_eq!(f.hits.len(), 4);
        f.type_keys(&[K::Char('m'), K::Char('n'), K::Char('r'), K::Char('s')]);
        assert_eq!(f.selected(), Some("src/main.rs"));
        assert_eq!(f.hits.len(), 1);
        f.type_keys(&[K::Backspace, K::Backspace, K::Backspace, K::Backspace]);
        assert_eq!(f.hits.len(), 4, "backspacing widens again");
        f.type_keys(&[K::Char('r'), K::Char('s')]);
        let n = f.hits.len();
        f.type_keys(&[K::Char('x')]);
        assert!(f.hits.len() <= n);
        f.type_keys(&[K::Clear]);
        assert_eq!(f.hits.len(), 4);
    }

    #[test]
    fn grep_prompt_toggles() {
        let mut g = GrepPrompt::new(None);
        assert!(!g.key(K::Enter), "an empty query does not run");
        g.key(K::Char('a'));
        g.key(K::Tab);
        g.key(K::Up);
        g.key(K::Down);
        assert_eq!((g.query.text.as_str(), g.query.case, g.query.word, g.query.regex), ("a", grep::Case::Sensitive, true, true));
        assert!(g.key(K::Enter));
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2000), "2,000");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
