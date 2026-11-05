//! Naming a tab: the inline "rename tab" prompt's state and the rules for a custom name.
//! Pure state and key handling; drawing is in `ui::draw_rename`.

use crate::ask::AskKey;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Longest custom name kept, in terminal columns.
pub const MAX_COLS: usize = 40;
/// Widest a custom name is drawn in the tab strip (longer ones end in an ellipsis).
pub const STRIP_COLS: usize = 24;

/// Trim, replace control and bidi-override characters, and cut to `MAX_COLS` columns.
/// An empty result means "no custom name".
pub fn clean(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control() && !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')).collect();
    let mut out = String::new();
    let mut used = 0;
    for c in s.trim().chars() {
        let w = c.width().unwrap_or(0);
        if used + w > MAX_COLS {
            break;
        }
        out.push(c);
        used += w;
    }
    out.trim_end().to_string()
}

/// `s` cut to `max` columns with a trailing ellipsis when it does not fit.
pub fn clip(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameOutcome {
    /// New custom name; `None` clears it.
    Save(Option<String>),
    Cancel,
}

/// The rename field for the tab with this id.
#[derive(Clone, Debug)]
pub struct Rename {
    pub tab: usize,
    pub buf: String,
    /// Cursor position in characters.
    pub cur: usize,
}

impl Rename {
    pub fn new(tab: usize, current: &str) -> Rename {
        Rename { tab, buf: current.to_string(), cur: current.chars().count() }
    }

    fn byte(&self, chars: usize) -> usize {
        self.buf.char_indices().nth(chars).map_or(self.buf.len(), |(i, _)| i)
    }

    /// Insert text at the cursor (control characters dropped, length capped).
    pub fn insert(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            if self.buf.width() + c.width().unwrap_or(0) > MAX_COLS {
                break;
            }
            let at = self.byte(self.cur);
            self.buf.insert(at, c);
            self.cur += 1;
        }
    }

    /// Cursor column inside the text.
    pub fn cursor_col(&self) -> usize {
        self.buf.chars().take(self.cur).map(|c| c.width().unwrap_or(0)).sum()
    }

    pub fn key(&mut self, k: AskKey) -> Option<RenameOutcome> {
        let len = self.buf.chars().count();
        match k {
            AskKey::Esc => return Some(RenameOutcome::Cancel),
            AskKey::Enter => {
                let n = clean(&self.buf);
                return Some(RenameOutcome::Save(if n.is_empty() { None } else { Some(n) }));
            }
            AskKey::Left => crate::lineedit::left(&mut self.cur),
            AskKey::Right => crate::lineedit::right(&self.buf, &mut self.cur),
            AskKey::WordLeft => crate::lineedit::word_left(&self.buf, &mut self.cur),
            AskKey::WordRight => crate::lineedit::word_right(&self.buf, &mut self.cur),
            AskKey::DeleteWord => crate::lineedit::delete_word(&mut self.buf, &mut self.cur),
            AskKey::Home | AskKey::Up => self.cur = 0,
            AskKey::End | AskKey::Down => self.cur = len,
            AskKey::Backspace => crate::lineedit::backspace(&mut self.buf, &mut self.cur),
            AskKey::Delete => crate::lineedit::delete(&mut self.buf, self.cur),
            AskKey::Char(c) => self.insert(c.encode_utf8(&mut [0; 4])),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(r: &mut Rename, bytes: &[u8]) -> Option<RenameOutcome> {
        for k in crate::ask::parse(bytes) {
            if let Some(o) = r.key(k) {
                return Some(o);
            }
        }
        None
    }

    #[test]
    fn clean_trims_strips_and_limits() {
        assert_eq!(clean("  build  "), "build");
        assert_eq!(clean("a\x1b[31mb\x07\n"), "a[31mb");
        assert_eq!(clean("a\u{202E}b"), "ab");
        assert_eq!(clean("   "), "");
        assert_eq!(clean(&"x".repeat(100)).chars().count(), MAX_COLS);
        // wide characters count two columns
        assert_eq!(clean(&"字".repeat(30)).width(), MAX_COLS);
    }

    #[test]
    fn clip_adds_ellipsis_only_when_needed() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
        assert_eq!(clip("字字字字", 5), "字字…");
    }

    #[test]
    fn editing_keys_move_the_cursor() {
        let mut r = Rename::new(1, "abc");
        assert_eq!(r.cur, 3);
        feed(&mut r, b"\x1b[D\x1b[D"); // left twice
        feed(&mut r, b"X");
        assert_eq!(r.buf, "aXbc");
        feed(&mut r, b"\x1b[H"); // home
        feed(&mut r, b"\x1b[3~"); // delete
        assert_eq!(r.buf, "Xbc");
        feed(&mut r, b"\x1b[F\x7f"); // end, backspace
        assert_eq!(r.buf, "Xb");
        feed(&mut r, b"\x1b[H\x7f");
        assert_eq!(r.buf, "Xb");
    }

    #[test]
    fn enter_saves_esc_cancels_empty_clears() {
        let mut r = Rename::new(1, "old");
        assert_eq!(feed(&mut r, b"  \r"), Some(RenameOutcome::Save(Some("old".into()))));
        let mut r = Rename::new(1, "old");
        assert_eq!(feed(&mut r, b"\x7f\x7f\x7f\r"), Some(RenameOutcome::Save(None)));
        let mut r = Rename::new(1, "old");
        assert_eq!(feed(&mut r, b"\x1b"), Some(RenameOutcome::Cancel));
    }

    #[test]
    fn paste_is_capped_and_cleaned() {
        let mut r = Rename::new(1, "");
        r.insert(&format!("a\tb{}", "z".repeat(100)));
        assert_eq!(r.buf.width(), MAX_COLS);
        assert!(r.buf.starts_with("ab"));
    }
}
