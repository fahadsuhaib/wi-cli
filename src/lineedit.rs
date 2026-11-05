//! Editing a one-line text with a cursor (a position counted in characters). Shared by the
//! rename field, the agent command field and the open-file / folder prompt.

use unicode_width::UnicodeWidthChar;

fn byte(buf: &str, chars: usize) -> usize {
    buf.char_indices().nth(chars).map_or(buf.len(), |(i, _)| i)
}

/// Insert `s` at the cursor (control characters, so newlines too, are dropped); the cursor ends after it.
pub fn insert(buf: &mut String, cur: &mut usize, s: &str) {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    let at = byte(buf, *cur);
    buf.insert_str(at, &s);
    *cur += s.chars().count();
}

/// Delete the character before the cursor.
pub fn backspace(buf: &mut String, cur: &mut usize) {
    if *cur > 0 {
        let (a, b) = (byte(buf, *cur - 1), byte(buf, *cur));
        buf.replace_range(a..b, "");
        *cur -= 1;
    }
}

/// Delete the character under the cursor.
pub fn delete(buf: &mut String, cur: usize) {
    if cur < buf.chars().count() {
        let (a, b) = (byte(buf, cur), byte(buf, cur + 1));
        buf.replace_range(a..b, "");
    }
}

pub fn left(cur: &mut usize) {
    *cur = cur.saturating_sub(1);
}

pub fn right(buf: &str, cur: &mut usize) {
    *cur = (*cur + 1).min(buf.chars().count());
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Start of the word before the cursor (skips spaces and punctuation first).
fn word_start(buf: &str, cur: usize) -> usize {
    let cs: Vec<char> = buf.chars().collect();
    let mut i = cur.min(cs.len());
    while i > 0 && !is_word(cs[i - 1]) {
        i -= 1;
    }
    while i > 0 && is_word(cs[i - 1]) {
        i -= 1;
    }
    i
}

/// Move the cursor to the start of the previous word.
pub fn word_left(buf: &str, cur: &mut usize) {
    *cur = word_start(buf, *cur);
}

/// Move the cursor to the end of the next word.
pub fn word_right(buf: &str, cur: &mut usize) {
    let cs: Vec<char> = buf.chars().collect();
    let mut i = (*cur).min(cs.len());
    while i < cs.len() && !is_word(cs[i]) {
        i += 1;
    }
    while i < cs.len() && is_word(cs[i]) {
        i += 1;
    }
    *cur = i;
}

/// Delete the word before the cursor.
pub fn delete_word(buf: &mut String, cur: &mut usize) {
    let a = word_start(buf, *cur);
    let (x, y) = (byte(buf, a), byte(buf, *cur));
    buf.replace_range(x..y, "");
    *cur = a;
}

/// Terminal columns from the start of the text to the cursor.
pub fn cursor_col(buf: &str, cur: usize) -> usize {
    buf.chars().take(cur).map(|c| c.width().unwrap_or(0)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_in_the_middle_and_strip_newlines() {
        let mut b = "abcd".to_string();
        let mut c = 2;
        insert(&mut b, &mut c, "X\nY\r");
        assert_eq!((b.as_str(), c), ("abXYcd", 4));
    }

    #[test]
    fn backspace_delete_and_edges() {
        let mut b = "héllo".to_string();
        let mut c = 2;
        backspace(&mut b, &mut c);
        assert_eq!((b.as_str(), c), ("hllo", 1));
        delete(&mut b, c);
        assert_eq!(b, "hlo");
        c = 0;
        backspace(&mut b, &mut c);
        assert_eq!(b, "hlo");
        delete(&mut b, 3);
        assert_eq!(b, "hlo");
        left(&mut c);
        assert_eq!(c, 0);
        c = 3;
        right(&b, &mut c);
        assert_eq!(c, 3);
    }

    #[test]
    fn word_moves_and_delete_word() {
        let b = "git commit  -m foo".to_string();
        let mut c = b.chars().count();
        word_left(&b, &mut c);
        assert_eq!(c, 15);
        word_left(&b, &mut c);
        assert_eq!(c, 13);
        word_left(&b, &mut c);
        assert_eq!(c, 4);
        c = 0;
        word_right(&b, &mut c);
        assert_eq!(c, 3);
        word_right(&b, &mut c);
        assert_eq!(c, 10);
        let mut b = b;
        c = 10;
        delete_word(&mut b, &mut c);
        assert_eq!((b.as_str(), c), ("git   -m foo", 4));
        c = 0;
        delete_word(&mut b, &mut c);
        assert_eq!(c, 0);
    }

    #[test]
    fn cursor_column_counts_wide_characters() {
        assert_eq!(cursor_col("a日b", 2), 3);
        assert_eq!(cursor_col("abc", 0), 0);
    }
}
