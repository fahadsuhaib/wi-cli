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
    fn cursor_column_counts_wide_characters() {
        assert_eq!(cursor_col("a日b", 2), 3);
        assert_eq!(cursor_col("abc", 0), 0);
    }
}
