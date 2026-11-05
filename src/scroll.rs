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

/// What a chunk typed in scroll mode holds: scroll keys up to the first byte that is not one,
/// and where that byte (typing, to be delivered to the tab) starts.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct Scan {
    pub keys: Vec<ScrollKey>,
    pub input_at: Option<usize>,
}

/// Classify raw input: arrows, PgUp/PgDn, Home/End scroll; the menu key and a bare Esc exit (and
/// end the scan); everything else (letters, Enter, Backspace, Tab, Ctrl-letters, Alt-keys, other
/// escape sequences) is input: the scan stops at it and `input_at` says where it starts.
pub fn scan(buf: &[u8], menu_key: u8) -> Scan {
    let mut out = Scan::default();
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        if b == menu_key {
            out.keys.push(ScrollKey::Exit);
            break;
        } else if b == 0x1b {
            if matches!(buf.get(i + 1), Some(b'[') | Some(b'O')) {
                // Skip parameters up to the final byte.
                let mut j = i + 2;
                while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                    j += 1;
                }
                if j >= buf.len() {
                    break; // an incomplete sequence at the end of the read
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
                match key {
                    Some(k) => out.keys.push(k),
                    None => {
                        out.input_at = Some(i);
                        break;
                    }
                }
                i = j;
            } else if matches!(buf.get(i + 1), None | Some(0x1b)) {
                out.keys.push(ScrollKey::Exit);
                break;
            } else {
                out.input_at = Some(i); // Alt+key
                break;
            }
        } else {
            out.input_at = Some(i);
            break;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ScrollKey::*;

    fn parse(b: &[u8], m: u8) -> Vec<ScrollKey> {
        scan(b, m).keys
    }

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
    fn typing_is_input_not_a_scroll_key() {
        assert_eq!(scan(b"a", 0), Scan { keys: vec![], input_at: Some(0) });
        assert_eq!(scan(b"\x1b[A\x1b[Bls\r", 0), Scan { keys: vec![Up, Down], input_at: Some(6) });
        assert_eq!(scan(b"\r", 0).input_at, Some(0));
        assert_eq!(scan(b"\x7f", 0).input_at, Some(0));
        assert_eq!(scan(b"\t", 0).input_at, Some(0));
        assert_eq!(scan(b"\x03", 0).input_at, Some(0));
        assert_eq!(scan(b"\x1b[3~", 0).input_at, Some(0));
        assert_eq!(scan(b"\x1b[1;5C", 0).input_at, Some(0));
        assert_eq!(scan(b"\x1b\r", 0).input_at, Some(0));
        assert_eq!(scan(b"\x1b[5~x", 0), Scan { keys: vec![PageUp], input_at: Some(4) });
    }

    #[test]
    fn exit_ends_the_scan_without_input() {
        assert_eq!(scan(b"\x1b[Aq\x00zz", 0).input_at, Some(3));
        assert_eq!(scan(b"\x00zz", 0), Scan { keys: vec![Exit], input_at: None });
        assert_eq!(scan(b"\x1b", 0), Scan { keys: vec![Exit], input_at: None });
    }

    #[test]
    fn incomplete_sequence_is_dropped() {
        assert_eq!(scan(b"\x1b[", 0), Scan::default());
        assert_eq!(scan(b"\x1b[1;", 0), Scan::default());
    }
}
