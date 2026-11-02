//! Mouse input: SGR reports from the host terminal, split out of the keystroke stream,
//! and re-encoding for apps in a tab that asked for the mouse (vim, htop).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Down(Button),
    Up(Button),
    Drag(Button),
    WheelUp,
    WheelDown,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MouseEvent {
    pub kind: Kind,
    /// 0-based screen position.
    pub col: u16,
    pub row: u16,
    /// The report's button code with modifier and motion bits, kept for forwarding.
    pub code: u16,
}

#[derive(PartialEq, Eq, Debug)]
pub enum Piece {
    Bytes(Vec<u8>),
    Mouse(MouseEvent),
}

/// Turn on button, drag and SGR reporting on the host terminal.
pub const ENABLE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
pub const DISABLE: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";

/// Modifier bits in a mouse report's button code. The Cmd/Super key is not reported by most
/// terminals, so links are opened with Ctrl or Alt held.
pub const MOD_ALT: u16 = 8;
pub const MOD_CTRL: u16 = 16;

/// Whether this click should open a link, per the `link_click` setting:
/// `any` (Ctrl or Alt held, the default), `ctrl`, `alt`, `plain` (an ordinary click on a link),
/// or `off`.
pub fn is_link_click(code: u16, setting: &str) -> bool {
    let (ctrl, alt) = (code & MOD_CTRL != 0, code & MOD_ALT != 0);
    match setting {
        "ctrl" => ctrl,
        "alt" => alt,
        "plain" => true,
        "off" => false,
        _ => ctrl || alt,
    }
}

fn button(code: u16) -> Button {
    match code & 3 {
        0 => Button::Left,
        1 => Button::Middle,
        _ => Button::Right,
    }
}

fn decode(code: u16, col: u16, row: u16, release: bool) -> MouseEvent {
    let kind = if code & 64 != 0 {
        match code & 3 {
            0 => Kind::WheelUp,
            1 => Kind::WheelDown,
            _ => Kind::Other,
        }
    } else if code & 32 != 0 {
        Kind::Drag(button(code))
    } else if release {
        Kind::Up(button(code))
    } else {
        Kind::Down(button(code))
    };
    MouseEvent { kind, col: col.saturating_sub(1), row: row.saturating_sub(1), code }
}

/// Parse `ESC [ < code ; col ; row (M|m)` at the start of `b`. Ok(Some((event, len))) when
/// complete, Ok(None) when it could still become one (needs more bytes), Err when it is not one.
fn parse_sgr(b: &[u8]) -> Result<Option<(MouseEvent, usize)>, ()> {
    // Only hold bytes back once the full `ESC [ <` prefix is seen, so a lone Esc key is never delayed.
    if b.len() < 3 || b[0] != 0x1b || b[1] != b'[' || b[2] != b'<' {
        return Err(());
    }
    let mut nums = [0u16; 3];
    let (mut idx, mut have) = (0usize, false);
    let mut i = 3;
    while i < b.len() {
        match b[i] {
            d @ b'0'..=b'9' => {
                nums[idx] = nums[idx].saturating_mul(10).saturating_add((d - b'0') as u16);
                have = true;
            }
            b';' if have && idx < 2 => {
                idx += 1;
                have = false;
            }
            f @ (b'M' | b'm') if idx == 2 && have => return Ok(Some((decode(nums[0], nums[1], nums[2], f == b'm'), i + 1))),
            _ => return Err(()),
        }
        i += 1;
    }
    Ok(None)
}

/// Splits mouse reports out of input bytes. A report cut in half by a read boundary is held back.
#[derive(Default)]
pub struct Splitter {
    carry: Vec<u8>,
}

impl Splitter {
    pub fn split(&mut self, input: &[u8]) -> Vec<Piece> {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(input);
        let mut out = Vec::new();
        let mut plain: Vec<u8> = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            if buf[i] == 0x1b {
                match parse_sgr(&buf[i..]) {
                    Ok(Some((ev, n))) => {
                        if !plain.is_empty() {
                            out.push(Piece::Bytes(std::mem::take(&mut plain)));
                        }
                        out.push(Piece::Mouse(ev));
                        i += n;
                        continue;
                    }
                    Ok(None) => {
                        self.carry = buf[i..].to_vec();
                        break;
                    }
                    Err(()) => {}
                }
            }
            plain.push(buf[i]);
            i += 1;
        }
        if !plain.is_empty() {
            out.push(Piece::Bytes(plain));
        }
        out
    }
}

/// Encode an event for an app inside a tab, at position (`col`, `row`) in that app's screen (0-based).
pub fn encode(ev: &MouseEvent, col: u16, row: u16, sgr: bool) -> Vec<u8> {
    let release = matches!(ev.kind, Kind::Up(_));
    if sgr {
        let f = if release { 'm' } else { 'M' };
        format!("\x1b[<{};{};{}{}", ev.code, col + 1, row + 1, f).into_bytes()
    } else {
        // Classic encoding: releases are reported as button 3 and positions must fit in a byte.
        let code = if release { 3 | (ev.code & !3) } else { ev.code };
        let (x, y) = (col as u32 + 33, row as u32 + 33);
        if x > 255 || y > 255 {
            return Vec::new();
        }
        vec![0x1b, b'[', b'M', (code as u32 + 32) as u8, x as u8, y as u8]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: Kind, col: u16, row: u16, code: u16) -> Piece {
        Piece::Mouse(MouseEvent { kind, col, row, code })
    }

    #[test]
    fn splits_click_and_release_from_keys() {
        let mut s = Splitter::default();
        let p = s.split(b"ab\x1b[<0;10;5Mcd\x1b[<0;10;5m");
        assert_eq!(
            p,
            vec![
                Piece::Bytes(b"ab".to_vec()),
                ev(Kind::Down(Button::Left), 9, 4, 0),
                Piece::Bytes(b"cd".to_vec()),
                ev(Kind::Up(Button::Left), 9, 4, 0),
            ]
        );
    }

    #[test]
    fn wheel_and_drag_and_modifiers() {
        let mut s = Splitter::default();
        assert_eq!(s.split(b"\x1b[<64;1;1M"), vec![ev(Kind::WheelUp, 0, 0, 64)]);
        assert_eq!(s.split(b"\x1b[<65;3;4M"), vec![ev(Kind::WheelDown, 2, 3, 65)]);
        assert_eq!(s.split(b"\x1b[<32;2;2M"), vec![ev(Kind::Drag(Button::Left), 1, 1, 32)]);
        assert_eq!(s.split(b"\x1b[<2;2;2M"), vec![ev(Kind::Down(Button::Right), 1, 1, 2)]);
    }

    #[test]
    fn report_split_across_reads_is_held_then_joined() {
        let mut s = Splitter::default();
        assert!(s.split(b"x\x1b[<0;1").len() == 1, "only the plain byte comes out");
        let p = s.split(b"2;7M");
        assert_eq!(p, vec![ev(Kind::Down(Button::Left), 11, 6, 0)]);
    }

    #[test]
    fn ordinary_escape_sequences_pass_through() {
        let mut s = Splitter::default();
        assert_eq!(s.split(b"\x1b[A\x1b[5~"), vec![Piece::Bytes(b"\x1b[A\x1b[5~".to_vec())]);
        assert_eq!(s.split(b"\x1b"), vec![Piece::Bytes(vec![0x1b])] , "a lone Esc is a key, not a report prefix");
    }

    #[test]
    fn link_click_modifiers() {
        assert!(is_link_click(16, "any") && is_link_click(8, "any"));
        assert!(!is_link_click(0, "any") && !is_link_click(4, "any"), "shift is the terminal's own selection");
        assert!(is_link_click(16, "ctrl") && !is_link_click(8, "ctrl"));
        assert!(is_link_click(8, "alt") && !is_link_click(16, "alt"));
        assert!(is_link_click(0, "plain"));
        assert!(!is_link_click(16, "off"));
        assert!(is_link_click(16, "anything else"), "unknown values behave like the default");
    }

    #[test]
    fn encodes_for_inner_apps() {
        let e = MouseEvent { kind: Kind::Down(Button::Left), col: 9, row: 4, code: 0 };
        assert_eq!(encode(&e, 2, 3, true), b"\x1b[<0;3;4M".to_vec());
        let up = MouseEvent { kind: Kind::Up(Button::Left), col: 9, row: 4, code: 0 };
        assert_eq!(encode(&up, 2, 3, true), b"\x1b[<0;3;4m".to_vec());
        assert_eq!(encode(&e, 2, 3, false), vec![0x1b, b'[', b'M', 32, 35, 36]);
        assert_eq!(encode(&up, 2, 3, false), vec![0x1b, b'[', b'M', 35, 35, 36]);
        assert!(encode(&e, 300, 3, false).is_empty());
    }
}
