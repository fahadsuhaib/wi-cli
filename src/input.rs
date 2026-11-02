//! Splits raw host input into ordinary keystrokes and bracketed pastes.
//!
//! A paste arrives as `ESC[200~ text ESC[201~` and may be split across reads.
//! Paste text never reaches the menu logic, even if it contains the menu key.

const START: &[u8] = b"\x1b[200~";
const END: &[u8] = b"\x1b[201~";

#[derive(Debug, PartialEq, Eq)]
pub enum InEv {
    /// Ordinary keystrokes (menu handling applies).
    Keys(Vec<u8>),
    PasteStart,
    /// Pasted text, markers removed.
    PasteData(Vec<u8>),
    PasteEnd,
}

#[derive(Default)]
pub struct PasteFilter {
    inside: bool,
    /// Trailing bytes that might be the start of a marker.
    pending: Vec<u8>,
}

/// Length of the longest suffix of `buf` that is a proper prefix of `marker`.
fn partial_suffix(buf: &[u8], marker: &[u8]) -> usize {
    (1..marker.len().min(buf.len() + 1))
        .rev()
        .find(|&n| buf.ends_with(&marker[..n]))
        .unwrap_or(0)
}

fn find(buf: &[u8], marker: &[u8]) -> Option<usize> {
    buf.windows(marker.len()).position(|w| w == marker)
}

impl PasteFilter {
    /// True when held-back bytes (a lone Esc, say) wait for a following read.
    pub fn has_pending(&self) -> bool {
        !self.inside && !self.pending.is_empty()
    }

    pub fn feed(&mut self, data: &[u8]) -> Vec<InEv> {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(data);
        let mut out = Vec::new();
        let mut rest: &[u8] = &buf;
        loop {
            let marker = if self.inside { END } else { START };
            if let Some(pos) = find(rest, marker) {
                self.emit(&mut out, &rest[..pos]);
                out.push(if self.inside { InEv::PasteEnd } else { InEv::PasteStart });
                self.inside = !self.inside;
                rest = &rest[pos + marker.len()..];
            } else {
                let keep = partial_suffix(rest, marker);
                let (now, held) = rest.split_at(rest.len() - keep);
                self.emit(&mut out, now);
                self.pending = held.to_vec();
                return out;
            }
        }
    }

    /// Give up waiting for the rest of a marker: the held bytes were real keys.
    pub fn flush(&mut self) -> Vec<InEv> {
        let held = std::mem::take(&mut self.pending);
        let mut out = Vec::new();
        self.emit(&mut out, &held);
        out
    }

    fn emit(&self, out: &mut Vec<InEv>, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        out.push(if self.inside { InEv::PasteData(bytes.to_vec()) } else { InEv::Keys(bytes.to_vec()) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collapse events into (keys, paste text, balanced markers).
    fn run(chunks: &[&[u8]]) -> (Vec<u8>, Vec<u8>, i32) {
        let mut f = PasteFilter::default();
        let (mut keys, mut paste, mut depth) = (vec![], vec![], 0);
        for c in chunks {
            for e in f.feed(c) {
                match e {
                    InEv::Keys(k) => keys.extend(k),
                    InEv::PasteData(p) => paste.extend(p),
                    InEv::PasteStart => depth += 1,
                    InEv::PasteEnd => depth -= 1,
                }
            }
        }
        (keys, paste, depth)
    }

    #[test]
    fn plain_keys_pass_through() {
        assert_eq!(run(&[b"ls\r"]), (b"ls\r".to_vec(), vec![], 0));
    }

    #[test]
    fn whole_paste() {
        let (k, p, d) = run(&[b"a\x1b[200~x\ny\x1b[201~b"]);
        assert_eq!((k, p, d), (b"ab".to_vec(), b"x\ny".to_vec(), 0));
    }

    #[test]
    fn menu_byte_inside_paste_is_data() {
        let (k, p, _) = run(&[b"\x1b[200~a\x00b\x1b[201~"]);
        assert!(k.is_empty());
        assert_eq!(p, b"a\x00b");
    }

    #[test]
    fn menu_byte_after_paste_is_a_key() {
        let (k, _, _) = run(&[b"\x1b[200~a\x1b[201~\x00"]);
        assert_eq!(k, b"\x00");
    }

    #[test]
    fn markers_split_at_every_position() {
        let full: &[u8] = b"k\x1b[200~hi\x00\nthere\x1b[201~z";
        for i in 1..full.len() {
            let (k, p, d) = run(&[&full[..i], &full[i..]]);
            assert_eq!((k, p, d), (b"kz".to_vec(), b"hi\x00\nthere".to_vec(), 0), "split at {i}");
        }
    }

    #[test]
    fn bytewise_feed() {
        let full: &[u8] = b"\x1b[200~a\x1b[b\x1b[201~";
        let chunks: Vec<&[u8]> = full.chunks(1).collect();
        let (k, p, d) = run(&chunks);
        assert!(k.is_empty());
        assert_eq!(p, b"a\x1b[b");
        assert_eq!(d, 0);
    }

    #[test]
    fn partial_start_is_held_then_flushed_as_keys() {
        let mut f = PasteFilter::default();
        assert!(f.feed(b"\x1b").is_empty());
        assert!(f.has_pending());
        assert_eq!(f.flush(), vec![InEv::Keys(b"\x1b".to_vec())]);
        assert!(!f.has_pending());
    }

    #[test]
    fn false_alarm_prefix_is_released() {
        let (k, p, d) = run(&[b"\x1b[2", b"0x"]);
        assert_eq!((k, p, d), (b"\x1b[20x".to_vec(), vec![], 0));
    }

    #[test]
    fn unterminated_paste_stays_paste() {
        let (k, p, d) = run(&[b"\x1b[200~abc", b"\x00def"]);
        assert!(k.is_empty());
        assert_eq!(p, b"abc\x00def");
        assert_eq!(d, 1);
    }
}
