//! Key-protocol layer: turns the host terminal's enhanced key reports (kitty keyboard protocol
//! `CSI u`, xterm `modifyOtherKeys` `CSI 27;mods;code ~`) into the legacy bytes a terminal
//! without the protocol would send, so everything downstream (menu, mouse, shells) is unchanged.
//! Shift/Ctrl/Alt+Enter become semantic keys the app maps per tab.

/// Longest partial sequence held across reads.
const MAX_CARRY: usize = 64;

/// What Shift+Enter, Ctrl+Enter and Alt+Enter send to the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnterMap {
    pub shift: &'static [u8],
    pub ctrl: &'static [u8],
    pub alt: &'static [u8],
}

impl EnterMap {
    /// Every Enter variant is a plain Enter (menus, prompts, previews).
    pub const PLAIN: EnterMap = EnterMap { shift: b"\r", ctrl: b"\r", alt: b"\r" };

    /// Mapping for a shell tab. `setting` is `auto`, `newline`, `esc-cr` or `enter`.
    pub fn for_tab(setting: &str, agent: bool) -> EnterMap {
        let pick: &'static [u8] = match setting.trim().to_ascii_lowercase().as_str() {
            "newline" => b"\n",
            "esc-cr" => b"\x1b\r",
            "enter" => b"\r",
            _ if agent => b"\n",
            _ => b"\r",
        };
        EnterMap { shift: pick, ctrl: pick, alt: b"\x1b\r" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// Exact legacy bytes.
    Bytes(Vec<u8>),
    ShiftEnter,
    CtrlEnter,
    AltEnter,
    /// Release/repeat events and combinations legacy terminals cannot express.
    Dropped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ev {
    /// Bytes that are not an enhanced report, untouched.
    Raw(Vec<u8>),
    /// A decoded report, with its original bytes (for programs that use the protocol themselves).
    Key { key: Key, raw: Vec<u8> },
}

#[derive(Default)]
pub struct KeyDecoder {
    carry: Vec<u8>,
    /// An enhanced report (CSI u or modifyOtherKeys) has been received at least once.
    pub enhanced: bool,
}

fn is_param(b: u8) -> bool {
    b.is_ascii_digit() || b == b';' || b == b':'
}

fn esc_prefix(alt: bool, mut v: Vec<u8>) -> Vec<u8> {
    if alt {
        v.insert(0, 0x1b);
    }
    v
}

fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c.to_ascii_lowercase() {
        l @ 'a'..='z' => l as u8 & 0x1f,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// US-layout shifted symbol, for kitty reports that carry no shifted key.
fn us_shift(c: char) -> char {
    match c {
        'a'..='z' => c.to_ascii_uppercase(),
        '1' => '!', '2' => '@', '3' => '#', '4' => '$', '5' => '%', '6' => '^', '7' => '&',
        '8' => '*', '9' => '(', '0' => ')', '-' => '_', '=' => '+', '[' => '{', ']' => '}',
        '\\' => '|', ';' => ':', '\'' => '"', ',' => '<', '.' => '>', '/' => '?', '`' => '~',
        _ => c,
    }
}

fn keypad(code: u32) -> Option<char> {
    Some(match code {
        57399..=57408 => char::from_digit(code - 57399, 10)?,
        57409 => '.',
        57410 => '/',
        57411 => '*',
        57412 => '-',
        57413 => '+',
        57415 => '=',
        57416 => ',',
        _ => return None,
    })
}

/// The key for `code` with modifier value `mods` (1 + bit set: shift 1, alt 2, ctrl 4, others dropped).
fn map_key(code: u32, mods: u32, shifted: Option<u32>, text: &str, kitty: bool) -> Key {
    let m = mods.saturating_sub(1);
    if m & (8 | 16 | 32) != 0 {
        return Key::Dropped;
    }
    let (shift, alt, ctrl) = (m & 1 != 0, m & 2 != 0, m & 4 != 0);
    let bytes = |v: Vec<u8>| Key::Bytes(esc_prefix(alt, v));
    match code {
        13 | 57414 => {
            return if alt {
                Key::AltEnter
            } else if shift {
                Key::ShiftEnter
            } else if ctrl {
                Key::CtrlEnter
            } else {
                Key::Bytes(b"\r".to_vec())
            };
        }
        27 => return bytes(vec![0x1b]),
        9 => return bytes(if shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }),
        8 | 127 => return bytes(vec![if ctrl { 0x08 } else { 0x7f }]),
        _ => {}
    }
    let Some(c) = keypad(code).or_else(|| {
        if (57344..=63743).contains(&code) { None } else { char::from_u32(code) }
    }) else {
        return Key::Dropped;
    };
    if (c as u32) < 32 {
        return bytes(vec![c as u8]);
    }
    if ctrl && let Some(b) = ctrl_byte(c) {
        return bytes(vec![b]);
    }
    let out: String = if !text.is_empty() && !alt && !ctrl {
        text.to_string()
    } else if shift && kitty {
        shifted.and_then(char::from_u32).unwrap_or_else(|| us_shift(c)).to_string()
    } else {
        c.to_string()
    };
    bytes(out.into_bytes())
}

fn parse_u32(s: &str) -> Option<u32> {
    s.parse().ok()
}

/// Decode one CSI whose parameters are `params` and final byte `fin`. None: not ours, leave raw.
fn decode_csi(params: &str, fin: u8) -> Option<Key> {
    let fields: Vec<&str> = params.split(';').collect();
    match fin {
        b'u' => {
            let mut code_parts = fields.first()?.split(':');
            let code = parse_u32(code_parts.next()?)?;
            let shifted = code_parts.next().and_then(parse_u32);
            let mut mod_parts = fields.get(1).copied().unwrap_or("").split(':');
            let mods = mod_parts.next().and_then(parse_u32).unwrap_or(1);
            let event = mod_parts.next().and_then(parse_u32).unwrap_or(1);
            if event >= 2 {
                return Some(Key::Dropped);
            }
            let text: String = fields
                .get(2)
                .map(|t| t.split(':').filter_map(parse_u32).filter_map(char::from_u32).collect())
                .unwrap_or_default();
            Some(map_key(code, mods, shifted, &text, true))
        }
        b'~' if fields.len() == 3 && fields[0] == "27" => {
            let mods = parse_u32(fields[1])?;
            let code = parse_u32(fields[2])?;
            Some(map_key(code, mods, None, "", false))
        }
        // Legacy-style functional keys carrying an event type (`CSI 1;2:3 A`): drop release,
        // strip the event type from the rest.
        b'A'..=b'D' | b'H' | b'F' | b'P' | b'Q' | b'S' | b'~' if params.contains(':') => {
            let mut mp = fields.get(1)?.split(':');
            let mods = mp.next()?;
            let event = mp.next().and_then(parse_u32).unwrap_or(1);
            if event >= 2 {
                return Some(Key::Dropped);
            }
            Some(Key::Bytes(format!("\x1b[{};{}{}", fields[0], mods, fin as char).into_bytes()))
        }
        _ => None,
    }
}

impl KeyDecoder {
    /// True when an unfinished sequence waits for the next read.
    pub fn has_pending(&self) -> bool {
        !self.carry.is_empty()
    }

    /// Give up waiting: the held bytes were real keys.
    pub fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }

    pub fn events(&mut self, data: &[u8]) -> Vec<Ev> {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(data);
        let mut out = Vec::new();
        let (mut start, mut i) = (0, 0);
        while i + 1 < buf.len() {
            if buf[i] != 0x1b || buf[i + 1] != b'[' {
                i += 1;
                continue;
            }
            let mut j = i + 2;
            while j < buf.len() && is_param(buf[j]) {
                j += 1;
            }
            if j == buf.len() {
                if j - i <= MAX_CARRY {
                    self.carry = buf[i..].to_vec();
                    if start < i {
                        out.push(Ev::Raw(buf[start..i].to_vec()));
                    }
                    return out;
                }
                break;
            }
            let params = std::str::from_utf8(&buf[i + 2..j]).unwrap_or("");
            match decode_csi(params, buf[j]) {
                Some(key) => {
                    if start < i {
                        out.push(Ev::Raw(buf[start..i].to_vec()));
                    }
                    if buf[j] == b'u' || buf[j] == b'~' && params.starts_with("27;") {
                        self.enhanced = true;
                    }
                    out.push(Ev::Key { key, raw: buf[i..=j].to_vec() });
                    start = j + 1;
                    i = j + 1;
                }
                None => i = j.max(i + 2),
            }
        }
        if start < buf.len() && buf.last() == Some(&0x1b) && buf.len() - start >= 1 && i + 1 == buf.len() {
            // A trailing Esc may begin a report; wait for the next read (or a flush).
            if start < buf.len() - 1 {
                out.push(Ev::Raw(buf[start..buf.len() - 1].to_vec()));
            }
            self.carry = vec![0x1b];
            return out;
        }
        if start < buf.len() {
            out.push(Ev::Raw(buf[start..].to_vec()));
        }
        out
    }

    /// Decode `data` to bytes ready for the rest of the input path. With `passthrough = Some(menu_key)`
    /// (the tab's program speaks the protocol itself) reports are forwarded untouched, except the
    /// menu key, which stays reachable as its legacy byte.
    pub fn decode(&mut self, data: &[u8], map: &EnterMap, passthrough: Option<u8>) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for ev in self.events(data) {
            match ev {
                Ev::Raw(b) => out.extend(b),
                Ev::Key { key, raw } => {
                    let legacy = match key {
                        Key::Bytes(b) => b,
                        Key::ShiftEnter => map.shift.to_vec(),
                        Key::CtrlEnter => map.ctrl.to_vec(),
                        Key::AltEnter => map.alt.to_vec(),
                        Key::Dropped => Vec::new(),
                    };
                    match passthrough {
                        Some(menu) if legacy != [menu] => out.extend(raw),
                        _ => out.extend(legacy),
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: EnterMap = EnterMap { shift: b"S", ctrl: b"C", alt: b"\x1b\r" };

    fn dec(chunks: &[&[u8]]) -> Vec<u8> {
        let mut d = KeyDecoder::default();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(d.decode(c, &MAP, None));
        }
        out.extend(d.flush());
        out
    }
    fn one(s: &[u8]) -> Vec<u8> {
        dec(&[s])
    }

    #[test] fn shift_enter_kitty() { assert_eq!(one(b"\x1b[13;2u"), b"S"); }
    #[test] fn shift_enter_mok() { assert_eq!(one(b"\x1b[27;2;13~"), b"S"); }
    #[test] fn ctrl_enter_kitty() { assert_eq!(one(b"\x1b[13;5u"), b"C"); }
    #[test] fn ctrl_enter_mok() { assert_eq!(one(b"\x1b[27;5;13~"), b"C"); }
    #[test] fn alt_enter_kitty() { assert_eq!(one(b"\x1b[13;3u"), b"\x1b\r"); }
    #[test] fn alt_enter_mok() { assert_eq!(one(b"\x1b[27;3;13~"), b"\x1b\r"); }
    #[test] fn plain_enter_kitty() { assert_eq!(one(b"\x1b[13u"), b"\r"); assert_eq!(one(b"\x1b[13;1u"), b"\r"); }
    #[test] fn legacy_cr_untouched() { assert_eq!(one(b"\r"), b"\r"); }
    #[test] fn legacy_alt_enter_untouched() { assert_eq!(one(b"\x1b\r"), b"\x1b\r"); }
    #[test] fn kitty_enter_with_lock_bits() { assert_eq!(one(b"\x1b[13;66u"), b"S"); } // caps lock (64) + shift
    #[test] fn ctrl_space_kitty() { assert_eq!(one(b"\x1b[32;5u"), b"\x00"); }
    #[test] fn ctrl_space_mok() { assert_eq!(one(b"\x1b[27;5;32~"), b"\x00"); }
    #[test] fn ctrl_a_kitty() { assert_eq!(one(b"\x1b[97;5u"), b"\x01"); }
    #[test] fn ctrl_c_kitty() { assert_eq!(one(b"\x1b[99;5u"), b"\x03"); }
    #[test] fn ctrl_c_mok() { assert_eq!(one(b"\x1b[27;5;99~"), b"\x03"); }
    #[test] fn ctrl_z_kitty() { assert_eq!(one(b"\x1b[122;5u"), b"\x1a"); }
    #[test] fn ctrl_shift_letter_is_ctrl() { assert_eq!(one(b"\x1b[97;6u"), b"\x01"); assert_eq!(one(b"\x1b[27;6;65~"), b"\x01"); }
    #[test] fn ctrl_brackets() {
        assert_eq!(one(b"\x1b[91;5u"), b"\x1b");
        assert_eq!(one(b"\x1b[92;5u"), b"\x1c");
        assert_eq!(one(b"\x1b[93;5u"), b"\x1d");
        assert_eq!(one(b"\x1b[27;5;93~"), b"\x1d");
    }
    #[test] fn ctrl_caret_underscore_question() {
        assert_eq!(one(b"\x1b[54;5u"), b"\x1e");
        assert_eq!(one(b"\x1b[45;5u"), b"\x1f");
        assert_eq!(one(b"\x1b[47;5u"), b"/"); // Ctrl+/ has no legacy byte: the character itself
        assert_eq!(one(b"\x1b[63;5u"), b"\x7f");
    }
    #[test] fn ctrl_at() { assert_eq!(one(b"\x1b[50;5u"), b"\x00"); }
    #[test] fn ctrl_i_is_tab_byte() { assert_eq!(one(b"\x1b[105;5u"), b"\x09"); }
    #[test] fn ctrl_m_is_cr_byte() { assert_eq!(one(b"\x1b[109;5u"), b"\x0d"); }
    #[test] fn alt_x() { assert_eq!(one(b"\x1b[120;3u"), b"\x1bx"); assert_eq!(one(b"\x1b[27;3;120~"), b"\x1bx"); }
    #[test] fn alt_shift_x() { assert_eq!(one(b"\x1b[120;4u"), b"\x1bX"); }
    #[test] fn ctrl_alt_x() { assert_eq!(one(b"\x1b[120;7u"), b"\x1b\x18"); assert_eq!(one(b"\x1b[27;7;120~"), b"\x1b\x18"); }
    #[test] fn ctrl_alt_space() { assert_eq!(one(b"\x1b[32;7u"), b"\x1b\x00"); }
    #[test] fn esc_plain() { assert_eq!(one(b"\x1b[27u"), b"\x1b"); assert_eq!(one(b"\x1b[27;1u"), b"\x1b"); }
    #[test] fn esc_mok() { assert_eq!(one(b"\x1b[27;1;27~"), b"\x1b"); }
    #[test] fn alt_esc() { assert_eq!(one(b"\x1b[27;3u"), b"\x1b\x1b"); }
    #[test] fn tab_plain() { assert_eq!(one(b"\x1b[9u"), b"\t"); }
    #[test] fn shift_tab() { assert_eq!(one(b"\x1b[9;2u"), b"\x1b[Z"); assert_eq!(one(b"\x1b[27;2;9~"), b"\x1b[Z"); }
    #[test] fn legacy_shift_tab_untouched() { assert_eq!(one(b"\x1b[Z"), b"\x1b[Z"); }
    #[test] fn alt_tab() { assert_eq!(one(b"\x1b[9;3u"), b"\x1b\t"); }
    #[test] fn ctrl_tab() { assert_eq!(one(b"\x1b[9;5u"), b"\t"); }
    #[test] fn backspace() { assert_eq!(one(b"\x1b[127u"), b"\x7f"); assert_eq!(one(b"\x1b[27;1;127~"), b"\x7f"); }
    #[test] fn ctrl_backspace() { assert_eq!(one(b"\x1b[127;5u"), b"\x08"); assert_eq!(one(b"\x1b[27;5;127~"), b"\x08"); }
    #[test] fn alt_backspace() { assert_eq!(one(b"\x1b[127;3u"), b"\x1b\x7f"); }
    #[test] fn shift_letter_uses_shifted_field() { assert_eq!(one(b"\x1b[97:65;2u"), b"A"); }
    #[test] fn shift_letter_without_shifted_field() { assert_eq!(one(b"\x1b[97;2u"), b"A"); }
    #[test] fn shift_digit_us_fallback() { assert_eq!(one(b"\x1b[49;2u"), b"!"); }
    #[test] fn shift_symbol_shifted_field_wins() { assert_eq!(one(b"\x1b[49:34;2u"), b"\""); }
    #[test] fn text_field_wins() { assert_eq!(one(b"\x1b[97;2;65u"), b"A"); assert_eq!(one(b"\x1b[97;1;97u"), b"a"); }
    #[test] fn text_field_multiple_codepoints() { assert_eq!(one(b"\x1b[97;1;104:105u"), b"hi"); }
    #[test] fn text_field_unicode() { assert_eq!(one("\x1b[233;1;233u".as_bytes()), "é".as_bytes()); }
    #[test] fn unicode_code_point() { assert_eq!(one("\x1b[233;3u".as_bytes()), "\u{1b}é".as_bytes()); }
    #[test] fn mok_shift_letter_code_is_already_shifted() { assert_eq!(one(b"\x1b[27;2;65~"), b"A"); }
    #[test] fn mok_shift_symbol() { assert_eq!(one(b"\x1b[27;2;33~"), b"!"); }
    #[test] fn base_layout_field_ignored() { assert_eq!(one(b"\x1b[1089::99;5u"), "\u{441}".as_bytes()); }
    #[test] fn super_dropped() { assert_eq!(one(b"\x1b[97;9u"), b""); }
    #[test] fn hyper_meta_dropped() { assert_eq!(one(b"\x1b[97;17u"), b""); assert_eq!(one(b"\x1b[97;33u"), b""); }
    #[test] fn super_dropped_mok() { assert_eq!(one(b"\x1b[27;9;97~"), b""); }
    #[test] fn release_dropped() { assert_eq!(one(b"\x1b[97;1:3u"), b""); assert_eq!(one(b"\x1b[13;2:3u"), b""); }
    #[test] fn repeat_dropped() { assert_eq!(one(b"\x1b[97;1:2u"), b""); }
    #[test] fn press_event_kept() { assert_eq!(one(b"\x1b[97;1:1u"), b"a"); }
    #[test] fn modifier_keys_dropped() { assert_eq!(one(b"\x1b[57441;2u"), b""); }
    #[test] fn function_keys_pua_dropped() { assert_eq!(one(b"\x1b[57376u"), b""); }
    #[test] fn keypad_digits() { assert_eq!(one(b"\x1b[57400u"), b"1"); assert_eq!(one(b"\x1b[57414u"), b"\r"); }
    #[test] fn keypad_enter_shift() { assert_eq!(one(b"\x1b[57414;2u"), b"S"); }
    #[test] fn arrows_pass_through() { assert_eq!(one(b"\x1b[A\x1b[B\x1b[C\x1b[D"), b"\x1b[A\x1b[B\x1b[C\x1b[D"); }
    #[test] fn modified_arrows_pass_through() { assert_eq!(one(b"\x1b[1;5A"), b"\x1b[1;5A"); assert_eq!(one(b"\x1b[1;2D"), b"\x1b[1;2D"); }
    #[test] fn home_end_pgup_pass_through() { assert_eq!(one(b"\x1b[H\x1b[F\x1b[5~\x1b[6~\x1b[3~"), b"\x1b[H\x1b[F\x1b[5~\x1b[6~\x1b[3~"); }
    #[test] fn f_keys_pass_through() { assert_eq!(one(b"\x1b[15;5~\x1b[1;2P\x1bOP"), b"\x1b[15;5~\x1b[1;2P\x1bOP"); }
    #[test] fn arrow_event_type_stripped() { assert_eq!(one(b"\x1b[1;5:1A"), b"\x1b[1;5A"); assert_eq!(one(b"\x1b[1;5:3A"), b""); }
    #[test] fn tilde_key_release_dropped() { assert_eq!(one(b"\x1b[5;1:3~"), b""); }
    #[test] fn plain_text_untouched() { assert_eq!(one(b"hello world\n"), b"hello world\n"); }
    #[test] fn utf8_untouched() { let s = "héllo 日本語 🎉".as_bytes(); assert_eq!(one(s), s); }
    #[test] fn utf8_bytewise_untouched() { let s = "日本語🎉".as_bytes(); let chunks: Vec<&[u8]> = s.chunks(1).collect(); assert_eq!(dec(&chunks), s); }
    #[test] fn all_bytes_without_esc_untouched() { let v: Vec<u8> = (0u8..=255).filter(|b| *b != 0x1b).collect(); assert_eq!(one(&v), v); }
    #[test] fn mouse_report_untouched() { assert_eq!(one(b"\x1b[<0;10;5M\x1b[<0;10;5m"), b"\x1b[<0;10;5M\x1b[<0;10;5m"); }
    #[test] fn mouse_report_split_untouched() { assert_eq!(dec(&[b"\x1b[<0;1", b"0;5M"]), b"\x1b[<0;10;5M"); }
    #[test] fn paste_markers_untouched() { assert_eq!(one(b"\x1b[200~x\x1b[201~"), b"\x1b[200~x\x1b[201~"); }
    #[test] fn unknown_csi_untouched() { assert_eq!(one(b"\x1b[?1;2c\x1b[12;5R\x1b[I\x1b[O"), b"\x1b[?1;2c\x1b[12;5R\x1b[I\x1b[O"); }
    #[test] fn query_reply_u_untouched() { assert_eq!(one(b"\x1b[?0u"), b"\x1b[?0u"); }
    #[test] fn malformed_untouched() { assert_eq!(one(b"\x1b[u"), b"\x1b[u"); assert_eq!(one(b"\x1b[;;u"), b"\x1b[;;u"); }
    #[test] fn invalid_code_dropped() { assert_eq!(one(b"\x1b[99999999;5u"), b""); }
    #[test] fn lone_esc_held_until_flush() {
        let mut d = KeyDecoder::default();
        assert!(d.decode(b"a\x1b", &MAP, None) == b"a");
        assert!(d.has_pending());
        assert_eq!(d.flush(), b"\x1b");
    }
    #[test] fn lone_esc_untouched() { assert_eq!(one(b"\x1b"), b"\x1b"); assert_eq!(one(b"a\x1bb"), b"a\x1bb"); }
    #[test] fn esc_esc_bracket() { assert_eq!(one(b"\x1b\x1b[A"), b"\x1b\x1b[A"); }
    #[test] fn alt_bracket_untouched() { assert_eq!(one(b"\x1b[x"), b"\x1b[x"); }
    #[test] fn mixed_with_text() { assert_eq!(one(b"ab\x1b[13;2ucd\x1b[99;5uef"), b"abScd\x03ef"); }
    #[test] fn back_to_back() { assert_eq!(one(b"\x1b[13;2u\x1b[13;2u"), b"SS"); }
    #[test] fn split_at_every_position() {
        let full: &[u8] = b"a\x1b[13;2ub\x1b[27;5;99~c\x1b[97:65;2;65ud";
        let want = one(full);
        assert_eq!(want, b"aSb\x03cAd");
        for i in 1..full.len() {
            assert_eq!(dec(&[&full[..i], &full[i..]]), want, "split at {i}");
        }
    }
    #[test] fn bytewise() {
        let full: &[u8] = b"\x1b[13;2u\x1b[27;2;13~x\x1b[A";
        let chunks: Vec<&[u8]> = full.chunks(1).collect();
        assert_eq!(dec(&chunks), b"SSx\x1b[A");
    }
    #[test] fn incomplete_is_held_then_flushed_raw() {
        let mut d = KeyDecoder::default();
        assert!(d.decode(b"\x1b[13;", &MAP, None).is_empty());
        assert!(d.has_pending());
        assert_eq!(d.flush(), b"\x1b[13;");
        assert!(!d.has_pending());
    }
    #[test] fn overlong_incomplete_not_held() {
        let mut d = KeyDecoder::default();
        let long = format!("\x1b[{}", "1;".repeat(80));
        assert_eq!(d.decode(long.as_bytes(), &MAP, None), long.as_bytes());
        assert!(!d.has_pending());
    }
    #[test] fn enhanced_flag_set_by_reports_only() {
        let mut d = KeyDecoder::default();
        d.decode(b"abc\x1b[A\x1b[5~", &MAP, None);
        assert!(!d.enhanced);
        d.decode(b"\x1b[13;2u", &MAP, None);
        assert!(d.enhanced);
        let mut d = KeyDecoder::default();
        d.decode(b"\x1b[27;2;13~", &MAP, None);
        assert!(d.enhanced);
    }
    #[test] fn passthrough_forwards_raw() {
        let mut d = KeyDecoder::default();
        assert_eq!(d.decode(b"\x1b[13;2u", &MAP, Some(0)), b"\x1b[13;2u");
        assert_eq!(d.decode(b"\x1b[99;5u", &MAP, Some(0)), b"\x1b[99;5u");
        assert_eq!(d.decode(b"plain", &MAP, Some(0)), b"plain");
    }
    #[test] fn passthrough_keeps_menu_key() {
        let mut d = KeyDecoder::default();
        assert_eq!(d.decode(b"\x1b[32;5u", &MAP, Some(0)), b"\x00");
        assert_eq!(d.decode(b"\x1b[27;5;32~", &MAP, Some(0)), b"\x00");
        assert_eq!(d.decode(b"\x1b[106;5u", &MAP, Some(10)), b"\x0a");
    }
    #[test] fn events_expose_semantic_keys() {
        let mut d = KeyDecoder::default();
        let ev = d.events(b"\x1b[13;2u\x1b[13;5u\x1b[13;3u");
        let keys: Vec<Key> = ev.into_iter().filter_map(|e| if let Ev::Key { key, .. } = e { Some(key) } else { None }).collect();
        assert_eq!(keys, vec![Key::ShiftEnter, Key::CtrlEnter, Key::AltEnter]);
    }

    #[test]
    fn enter_map_rules() {
        assert_eq!(EnterMap::for_tab("auto", true).shift, b"\n");
        assert_eq!(EnterMap::for_tab("auto", true).ctrl, b"\n");
        assert_eq!(EnterMap::for_tab("auto", false).shift, b"\r");
        assert_eq!(EnterMap::for_tab("auto", false).alt, b"\x1b\r");
        assert_eq!(EnterMap::for_tab("newline", false).shift, b"\n");
        assert_eq!(EnterMap::for_tab("esc-cr", true).shift, b"\x1b\r");
        assert_eq!(EnterMap::for_tab("enter", true).shift, b"\r");
        assert_eq!(EnterMap::for_tab("bogus", true).shift, b"\n");
        assert_eq!(EnterMap::PLAIN.alt, b"\r");
    }
}
