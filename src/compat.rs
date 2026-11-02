//! Terminal compatibility: answers terminal queries (DA, DSR, CPR, XTVERSION,
//! DECRQM) that full-screen apps send, and tracks the OSC window title.
//! `vt100::Parser` does neither, so apps like Claude Code would otherwise hang.

/// Longest partial escape sequence we hold across chunks.
const MAX_CARRY: usize = 256;

#[derive(Default)]
pub struct Compat {
    /// Trailing, still-incomplete escape sequence from the previous chunk.
    carry: Vec<u8>,
    /// Latest window title set by the program (OSC 0/2), if any.
    pub title: Option<String>,
}

impl Compat {
    /// Feed shell output into `parser`, returning the bytes to write back to the shell.
    pub fn feed(&mut self, parser: &mut vt100::Parser, data: &[u8]) -> Vec<u8> {
        let mut buf = std::mem::take(&mut self.carry);
        let mut done = buf.len(); // carried bytes were already given to the parser
        buf.extend_from_slice(data);
        let mut reply = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            if buf[i] != 0x1b {
                i += 1;
                continue;
            }
            match buf.get(i + 1) {
                None => break,
                Some(b'[') => match parse_csi(&buf[i + 2..]) {
                    Csi::Incomplete => break,
                    Csi::Done { len, prefix, params, inter, fin } => {
                        let end = i + 2 + len;
                        if is_query(prefix, &params, &inter, fin) {
                            parser.process(&buf[done.min(end)..end]);
                            done = done.max(end);
                            answer(parser, prefix, &params, &inter, fin, &mut reply);
                        }
                        i = end;
                    }
                },
                Some(b']') => match parse_osc(&buf[i + 2..]) {
                    None => break,
                    Some((len, body)) => {
                        self.set_title(&body);
                        i += 2 + len;
                    }
                },
                Some(_) => i += 2,
            }
        }
        if i < buf.len() && buf.len() - i <= MAX_CARRY {
            self.carry = buf[i..].to_vec();
        }
        parser.process(&buf[done.min(buf.len())..]);
        reply
    }

    fn set_title(&mut self, body: &[u8]) {
        let s = String::from_utf8_lossy(body);
        let Some((cmd, text)) = s.split_once(';') else { return };
        if cmd == "0" || cmd == "2" {
            let t: String = text.chars().filter(|c| !c.is_control()).take(20).collect();
            self.title = if t.trim().is_empty() { None } else { Some(t) };
        }
    }
}

enum Csi {
    Incomplete,
    Done { len: usize, prefix: Option<u8>, params: Vec<u8>, inter: Vec<u8>, fin: u8 },
}

/// Parse a CSI body (after `ESC [`). Malformed input yields `Done` with a junk final byte.
fn parse_csi(b: &[u8]) -> Csi {
    let mut k = 0;
    let prefix = match b.first() {
        Some(&p @ (b'?' | b'>' | b'=' | b'<')) => {
            k = 1;
            Some(p)
        }
        _ => None,
    };
    let ps = k;
    while k < b.len() && (0x30..=0x3f).contains(&b[k]) {
        k += 1;
    }
    let params = b[ps..k].to_vec();
    let is = k;
    while k < b.len() && (0x20..=0x2f).contains(&b[k]) {
        k += 1;
    }
    let inter = b[is..k].to_vec();
    match b.get(k) {
        None => Csi::Incomplete,
        Some(&fin) => Csi::Done { len: k + 1, prefix, params, inter, fin },
    }
}

/// Parse an OSC body (after `ESC ]`) up to BEL or ST. Returns (consumed, body).
fn parse_osc(b: &[u8]) -> Option<(usize, Vec<u8>)> {
    for k in 0..b.len() {
        if b[k] == 0x07 {
            return Some((k + 1, b[..k].to_vec()));
        }
        if b[k] == 0x1b {
            return match b.get(k + 1) {
                None => None,
                Some(b'\\') => Some((k + 2, b[..k].to_vec())),
                Some(_) => Some((k, b[..k].to_vec())), // aborted by a new escape
            };
        }
    }
    None
}

fn is_query(prefix: Option<u8>, params: &[u8], inter: &[u8], fin: u8) -> bool {
    match (prefix, fin) {
        (None, b'c') => params.is_empty() || params == b"0",
        (Some(b'>'), b'c') => params.is_empty() || params == b"0",
        (Some(b'>'), b'q') => params.is_empty() || params == b"0",
        (None, b'n') => params == b"5" || params == b"6",
        (Some(b'?'), b'n') => params == b"6",
        (Some(b'?'), b'p') => inter == b"$",
        _ => false,
    }
}

fn answer(parser: &vt100::Parser, prefix: Option<u8>, params: &[u8], _inter: &[u8], fin: u8, out: &mut Vec<u8>) {
    let (row, col) = parser.screen().cursor_position();
    match (prefix, fin) {
        (None, b'c') => out.extend_from_slice(b"\x1b[?62;c"),
        (Some(b'>'), b'c') => out.extend_from_slice(b"\x1b[>0;10;0c"),
        (Some(b'>'), b'q') => out.extend_from_slice(b"\x1bP>|wicli(0.1)\x1b\\"),
        (None, b'n') if params == b"5" => out.extend_from_slice(b"\x1b[0n"),
        (None, b'n') => out.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes()),
        (Some(b'?'), b'n') => out.extend_from_slice(format!("\x1b[?{};{}R", row + 1, col + 1).as_bytes()),
        (Some(b'?'), b'p') => {
            let mode: u32 = String::from_utf8_lossy(params).parse().unwrap_or(0);
            // 0 = unknown, 1 = set, 2 = reset
            let state = match mode {
                2004 if parser.screen().bracketed_paste() => 1,
                2004 => 2,
                _ => 0,
            };
            out.extend_from_slice(format!("\x1b[?{mode};{state}$y").as_bytes());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> (Vec<u8>, Compat, vt100::Parser) {
        let mut p = vt100::Parser::new(crate::tab::nz(24), crate::tab::nz(80), 0);
        let mut c = Compat::default();
        let mut out = Vec::new();
        for ch in chunks {
            out.extend(c.feed(&mut p, ch));
        }
        (out, c, p)
    }

    #[test]
    fn da1() {
        assert_eq!(run(&[b"\x1b[c"]).0, b"\x1b[?62;c");
        assert_eq!(run(&[b"\x1b[0c"]).0, b"\x1b[?62;c");
    }
    #[test]
    fn da2_and_xtversion() {
        assert_eq!(run(&[b"\x1b[>c"]).0, b"\x1b[>0;10;0c");
        assert!(run(&[b"\x1b[>q"]).0.starts_with(b"\x1bP>|wicli"));
    }
    #[test]
    fn dsr_status() {
        assert_eq!(run(&[b"\x1b[5n"]).0, b"\x1b[0n");
    }
    #[test]
    fn cursor_position_after_preceding_bytes() {
        assert_eq!(run(&[b"ab\r\ncde\x1b[6n"]).0, b"\x1b[2;4R");
    }
    #[test]
    fn query_split_across_chunks() {
        let (out, _, p) = run(&[b"hi\x1b[", b"6", b"n!"]);
        assert_eq!(out, b"\x1b[1;3R");
        assert!(p.screen().contents().starts_with("hi!"));
    }
    #[test]
    fn decrqm() {
        assert_eq!(run(&[b"\x1b[?2026$p"]).0, b"\x1b[?2026;0$y");
        assert_eq!(run(&[b"\x1b[?2004$p"]).0, b"\x1b[?2004;2$y");
        assert_eq!(run(&[b"\x1b[?2004h\x1b[?2004$p"]).0, b"\x1b[?2004;1$y");
    }
    #[test]
    fn no_reply_to_plain_output() {
        let (out, _, p) = run(&[b"\x1b[31mred\x1b[0m \x1b[2J\x1b[H x"]);
        assert!(out.is_empty());
        assert!(p.screen().contents().contains('x'));
    }
    #[test]
    fn alt_screen_restores_primary() {
        let (_, _, p) = run(&[b"primary\x1b[?1049h\x1b[2Jalt\x1b[?1049l"]);
        let c = p.screen().contents();
        assert!(c.contains("primary") && !c.contains("alt"));
    }
    #[test]
    fn truecolor_and_wide() {
        let (_, _, p) = run(&[b"\x1b[38;2;1;2;3m\xe6\x97\xa5x"]);
        let s = p.screen();
        assert_eq!(s.cell(0, 0).unwrap().fgcolor(), vt100::Color::Rgb(1, 2, 3));
        assert!(s.cell(0, 0).unwrap().is_wide() && s.cell(0, 1).unwrap().is_wide_continuation());
        assert_eq!(s.cell(0, 2).unwrap().contents(), "x");
    }
    #[test]
    fn osc_title_bel_st_and_split() {
        assert_eq!(run(&[b"\x1b]0;hello\x07"]).1.title.as_deref(), Some("hello"));
        assert_eq!(run(&[b"\x1b]2;wor", b"ld\x1b\\"]).1.title.as_deref(), Some("world"));
        let long = "x".repeat(40);
        let t = run(&[format!("\x1b]0;{long}\x07").as_bytes()]).1.title.unwrap();
        assert_eq!(t.len(), 20);
    }
}
