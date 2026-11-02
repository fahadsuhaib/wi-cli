//! Scrollback mode: key parsing and offset math.
//! An offset of 0 is the live view; larger values look further back.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScrollKey {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Exit,
}

/// New offset after a key, clamped to `0..=max`. `page` is the page size in lines.
pub fn apply(offset: usize, max: usize, page: usize, key: ScrollKey) -> usize {
    let page = page.max(1);
    match key {
        ScrollKey::Up => (offset + 1).min(max),
        ScrollKey::Down => offset.saturating_sub(1),
        ScrollKey::PageUp => (offset + page).min(max),
        ScrollKey::PageDown => offset.saturating_sub(page),
        ScrollKey::Top => max,
        ScrollKey::Bottom => 0,
        ScrollKey::Exit => offset.min(max),
    }
}

/// Parse raw input into scroll keys. The menu key and a bare Esc both exit.
pub fn parse(buf: &[u8], menu_key: u8) -> Vec<ScrollKey> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        if b == menu_key {
            keys.push(ScrollKey::Exit);
        } else if b == 0x1b {
            if matches!(buf.get(i + 1), Some(b'[') | Some(b'O')) {
                // Skip parameters up to the final byte.
                let mut j = i + 2;
                while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                    j += 1;
                }
                if j >= buf.len() {
                    break;
                }
                let params = &buf[i + 2..j];
                let key = match (buf[j], params) {
                    (b'A', _) => Some(ScrollKey::Up),
                    (b'B', _) => Some(ScrollKey::Down),
                    (b'H', _) | (b'~', b"1") | (b'~', b"7") => Some(ScrollKey::Top),
                    (b'F', _) | (b'~', b"4") | (b'~', b"8") => Some(ScrollKey::Bottom),
                    (b'~', b"5") => Some(ScrollKey::PageUp),
                    (b'~', b"6") => Some(ScrollKey::PageDown),
                    _ => None,
                };
                keys.extend(key);
                i = j;
            } else {
                keys.push(ScrollKey::Exit);
            }
        }
        i += 1;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use ScrollKey::*;

    #[test]
    fn offset_math() {
        assert_eq!(apply(0, 100, 20, Up), 1);
        assert_eq!(apply(100, 100, 20, Up), 100);
        assert_eq!(apply(0, 100, 20, Down), 0);
        assert_eq!(apply(5, 100, 20, Down), 4);
        assert_eq!(apply(90, 100, 20, PageUp), 100);
        assert_eq!(apply(10, 100, 20, PageDown), 0);
        assert_eq!(apply(30, 100, 20, PageDown), 10);
        assert_eq!(apply(3, 100, 20, Top), 100);
        assert_eq!(apply(3, 100, 20, Bottom), 0);
        assert_eq!(apply(3, 0, 20, Up), 0);
        assert_eq!(apply(3, 100, 0, PageUp), 4);
    }

    #[test]
    fn parses_keys() {
        assert_eq!(parse(b"\x1b[A\x1b[B", 0), vec![Up, Down]);
        assert_eq!(parse(b"\x1bOA", 0), vec![Up]);
        assert_eq!(parse(b"\x1b[5~\x1b[6~", 0), vec![PageUp, PageDown]);
        assert_eq!(parse(b"\x1b[H\x1b[F", 0), vec![Top, Bottom]);
        assert_eq!(parse(b"\x1b[1~\x1b[4~", 0), vec![Top, Bottom]);
    }

    #[test]
    fn esc_and_menu_key_exit() {
        assert_eq!(parse(b"\x1b", 0), vec![Exit]);
        assert_eq!(parse(b"\x00", 0), vec![Exit]);
        assert_eq!(parse(b"\x1b[A\x1b", 0), vec![Up, Exit]);
    }

    #[test]
    fn unknown_sequences_and_text_are_ignored() {
        assert!(parse(b"abc\x1b[3~\x1b[1;5C", 0).is_empty());
    }
}
