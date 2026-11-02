//! Keys inside a preview tab. The menu key is handled by the caller before this runs.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PvKey {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Wrap,
    Reload,
    /// Git view: next / previous changed file.
    NextFile,
    PrevFile,
    Back,
    /// Open the find prompt.
    Find,
    /// Step to the next / previous find match.
    MatchNext,
    MatchPrev,
}

pub fn parse(buf: &[u8]) -> Vec<PvKey> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        if b == 0x1b {
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
                    (b'A', _) => Some(PvKey::Up),
                    (b'B', _) => Some(PvKey::Down),
                    (b'C', _) => Some(PvKey::Right),
                    (b'D', _) => Some(PvKey::Left),
                    (b'H', _) | (b'~', b"1") | (b'~', b"7") => Some(PvKey::Top),
                    (b'F', _) | (b'~', b"4") | (b'~', b"8") => Some(PvKey::Bottom),
                    (b'~', b"5") => Some(PvKey::PageUp),
                    (b'~', b"6") => Some(PvKey::PageDown),
                    _ => None,
                });
                i = j;
            } else {
                keys.push(PvKey::Back);
            }
        } else {
            match b {
                b'w' | b'W' => keys.push(PvKey::Wrap),
                b'r' | b'R' => keys.push(PvKey::Reload),
                b'n' | b'N' => keys.push(PvKey::NextFile),
                b'p' | b'P' => keys.push(PvKey::PrevFile),
                b'f' | b'F' => keys.push(PvKey::Find),
                b'b' | b'B' => keys.push(PvKey::MatchPrev),
                b'\r' | b'\n' => keys.push(PvKey::MatchNext),
                _ => {}
            }
        }
        i += 1;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_pages_and_letters() {
        assert_eq!(parse(b"\x1b[A\x1b[B\x1b[C\x1b[D"), vec![PvKey::Up, PvKey::Down, PvKey::Right, PvKey::Left]);
        assert_eq!(parse(b"\x1b[5~\x1b[6~\x1b[H\x1b[F"), vec![PvKey::PageUp, PvKey::PageDown, PvKey::Top, PvKey::Bottom]);
        assert_eq!(parse(b"wR"), vec![PvKey::Wrap, PvKey::Reload]);
        assert_eq!(parse(b"nP"), vec![PvKey::NextFile, PvKey::PrevFile]);
        assert_eq!(parse(b"fB\r"), vec![PvKey::Find, PvKey::MatchPrev, PvKey::MatchNext]);
    }

    #[test]
    fn bare_escape_goes_back() {
        assert_eq!(parse(b"\x1b"), vec![PvKey::Back]);
    }
}
