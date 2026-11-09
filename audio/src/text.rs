//! What the engine says becomes clean text, or nothing when it only heard noise.

/// Tidy engine output: special tags and bracketed sound notes go, whitespace collapses.
pub fn clean(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut depth_angle = 0;
    let mut depth_sq = 0;
    for ch in raw.chars() {
        match ch {
            '<' => depth_angle += 1,
            '>' if depth_angle > 0 => depth_angle -= 1,
            '[' => depth_sq += 1,
            ']' if depth_sq > 0 => depth_sq -= 1,
            _ if depth_angle > 0 || depth_sq > 0 => {}
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words models invent from silence or room noise. A phrase that is only one of these is dropped.
const PHANTOMS: &[&str] = &[
    "thank you",
    "thanks",
    "thanks for watching",
    "thank you for watching",
    "you",
    "bye",
    "goodbye",
    "so",
    "the",
    "uh",
    "um",
    "hmm",
    "mm",
    "mhm",
    "oh",
    "ah",
    "yeah",
    "okay",
    "music",
    "applause",
    "laughter",
];

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '\'')
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when `text` (already cleaned) is not worth pasting: empty, punctuation only, a known
/// phantom phrase on its own, or one word repeated over and over (a decoder loop).
pub fn is_noise(text: &str) -> bool {
    let n = normalized(text);
    if n.is_empty() || PHANTOMS.contains(&n.as_str()) {
        return true;
    }
    let words: Vec<&str> = n.split(' ').collect();
    words.len() >= 6 && words.iter().all(|w| *w == words[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_tags_and_collapses_space() {
        assert_eq!(clean("  <s> Hello   world.\n</s> "), "Hello world.");
        assert_eq!(clean("[BLANK_AUDIO]"), "");
        assert_eq!(clean("open it [music] now"), "open it now");
        assert_eq!(clean("a\tb\u{7}c"), "a b c");
    }

    #[test]
    fn hallucinations_are_dropped() {
        for t in ["Thank you.", "you", "Thanks for watching!", "...", "", "Um,", "Okay."] {
            assert!(is_noise(&clean(t)), "{t:?} should be dropped");
        }
    }

    #[test]
    fn real_phrases_are_kept() {
        for t in ["Thank you for the fix, now run the tests.", "Open Cargo.toml", "yes please", "rebase on main"] {
            assert!(!is_noise(&clean(t)), "{t:?} should be kept");
        }
    }

    #[test]
    fn decoder_loops_are_dropped() {
        assert!(is_noise("the the the the the the the"));
        assert!(!is_noise("no no no"));
    }
}
