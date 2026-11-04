//! The "start an agent" popup: a list of start options plus an inline command field.
//! Pure state and key handling; drawing is in `ui::draw_ask`.

use crate::agents::{self, Agent, StartOption};
use std::cell::Cell;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AskKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
    Esc,
    Backspace,
    Delete,
    Tab,
    Char(char),
}

/// What a key press decided.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AskOutcome {
    Start(Agent, String),
    Later,
    Never,
}

/// The text field while a command is being typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub buf: String,
    /// Cursor position in characters.
    pub cur: usize,
    /// Agent the command is started as (Tab changes it).
    pub agent: Agent,
}

impl Edit {
    fn new(agent: Agent, text: &str) -> Edit {
        Edit { buf: text.to_string(), cur: text.chars().count(), agent }
    }

    pub fn insert(&mut self, s: &str) {
        crate::lineedit::insert(&mut self.buf, &mut self.cur, s);
    }
}

#[derive(Clone)]
pub struct Ask {
    /// Folder name shown in the title.
    pub folder: String,
    pub options: Vec<StartOption>,
    /// Selected row: the options, then Other command, Not now, Never here.
    pub sel: usize,
    /// First visible row, kept by the drawing code.
    pub scroll: Cell<usize>,
    /// Some while the command field has the keyboard.
    pub edit: Option<Edit>,
    pub error: Option<String>,
}

impl Ask {
    pub fn new(detected: Vec<Agent>, cfg: &crate::config::Config, state: &agents::State, key: &str, dir: &str) -> Ask {
        let folder = dir.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("/").to_string();
        Ask { folder, options: agents::start_options(&detected, cfg, state, key), sel: 0, scroll: Cell::new(0), edit: None, error: None }
    }

    pub fn rows(&self) -> usize {
        self.options.len() + 3
    }

    pub fn other_row(&self) -> usize {
        self.options.len()
    }

    /// Agent of the selected row, else the first option's.
    fn agent_here(&self) -> Agent {
        self.options.get(self.sel).or(self.options.first()).map_or(Agent::Claude, |o| o.agent)
    }

    fn begin_edit(&mut self) {
        let text = self.options.get(self.sel).map(|o| o.command.clone()).unwrap_or_default();
        self.edit = Some(Edit::new(self.agent_here(), &text));
        self.error = None;
    }

    pub fn paste(&mut self, s: &str) {
        if let Some(e) = self.edit.as_mut() {
            e.insert(s);
            self.error = None;
        }
    }

    pub fn key(&mut self, k: AskKey) -> Option<AskOutcome> {
        if self.edit.is_some() { self.key_edit(k) } else { self.key_list(k) }
    }

    fn key_list(&mut self, k: AskKey) -> Option<AskOutcome> {
        let n = self.rows();
        let other = self.other_row();
        match k {
            AskKey::Up => self.sel = (self.sel + n - 1) % n,
            AskKey::Down => self.sel = (self.sel + 1) % n,
            AskKey::Home => self.sel = 0,
            AskKey::End => self.sel = n - 1,
            AskKey::Esc => return Some(AskOutcome::Later),
            AskKey::Enter => {
                return match self.sel {
                    s if s < other => Some(AskOutcome::Start(self.options[s].agent, self.options[s].command.clone())),
                    s if s == other => {
                        self.begin_edit();
                        None
                    }
                    s if s == other + 1 => Some(AskOutcome::Later),
                    _ => Some(AskOutcome::Never),
                };
            }
            AskKey::Char(c) => match c.to_ascii_lowercase() {
                'e' => self.begin_edit(),
                'o' => {
                    self.sel = other;
                    self.begin_edit();
                }
                'n' => return Some(AskOutcome::Later),
                'x' => return Some(AskOutcome::Never),
                _ => {}
            },
            _ => {}
        }
        None
    }

    fn key_edit(&mut self, k: AskKey) -> Option<AskOutcome> {
        let e = self.edit.as_mut()?;
        let len = e.buf.chars().count();
        match k {
            AskKey::Esc => {
                self.edit = None;
                self.error = None;
                return None;
            }
            AskKey::Enter => {
                let cmd = e.buf.trim().to_string();
                if cmd.is_empty() {
                    self.error = Some("Type a command first (Esc goes back to the list)".into());
                    return None;
                }
                return Some(AskOutcome::Start(e.agent, cmd));
            }
            AskKey::Left => crate::lineedit::left(&mut e.cur),
            AskKey::Right => crate::lineedit::right(&e.buf, &mut e.cur),
            AskKey::Home | AskKey::Up => e.cur = 0,
            AskKey::End | AskKey::Down => e.cur = len,
            AskKey::Backspace => crate::lineedit::backspace(&mut e.buf, &mut e.cur),
            AskKey::Delete => crate::lineedit::delete(&mut e.buf, e.cur),
            AskKey::Tab => {
                let i = agents::ALL.iter().position(|a| *a == e.agent).unwrap_or(0);
                e.agent = agents::ALL[(i + 1) % agents::ALL.len()];
            }
            AskKey::Char(c) => e.insert(c.encode_utf8(&mut [0; 4])),
        }
        self.error = None;
        None
    }
}

/// Split raw terminal input into keys. A lone Esc is a key; escape sequences are decoded.
pub fn parse(buf: &[u8]) -> Vec<AskKey> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        match b {
            0x1b if matches!(buf.get(i + 1), Some(b'[') | Some(b'O')) => {
                let mut j = i + 2;
                while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                    j += 1;
                }
                let first = buf.get(i + 2).copied();
                match buf.get(j) {
                    Some(b'A') => keys.push(AskKey::Up),
                    Some(b'B') => keys.push(AskKey::Down),
                    Some(b'C') => keys.push(AskKey::Right),
                    Some(b'D') => keys.push(AskKey::Left),
                    Some(b'H') => keys.push(AskKey::Home),
                    Some(b'F') => keys.push(AskKey::End),
                    Some(b'~') => match first {
                        Some(b'1' | b'7') => keys.push(AskKey::Home),
                        Some(b'4' | b'8') => keys.push(AskKey::End),
                        Some(b'3') => keys.push(AskKey::Delete),
                        _ => {}
                    },
                    _ => {}
                }
                i = j + 1;
                continue;
            }
            0x1b => keys.push(AskKey::Esc),
            b'\r' | b'\n' => keys.push(AskKey::Enter),
            0x7f | 0x08 => keys.push(AskKey::Backspace),
            b'\t' => keys.push(AskKey::Tab),
            c if c >= 0x20 => {
                let end = (i + 4).min(buf.len());
                let take = buf[i..end].iter().position(|&x| x < 0x20 || x == 0x7f).unwrap_or(end - i);
                let s = String::from_utf8_lossy(&buf[i..i + take]);
                // a 4-byte window may cut a following character; only the first one counts here
                if let Some(c) = s.chars().next() {
                    keys.push(AskKey::Char(c));
                    i += c.len_utf8();
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn ask() -> Ask {
        let mut st = agents::State::default();
        st.remember("k", "claude", "claude --continue");
        Ask::new(vec![Agent::Claude], &Config::default(), &st, "k", "/w/proj/")
    }

    fn feed(a: &mut Ask, bytes: &[u8]) -> Option<AskOutcome> {
        for k in parse(bytes) {
            if let Some(o) = a.key(k) {
                return Some(o);
            }
        }
        None
    }

    #[test]
    fn enter_starts_selected_and_down_moves() {
        let mut a = ask();
        assert_eq!(a.folder, "proj");
        assert_eq!(feed(&mut a, b"\x1b[B\r"), Some(AskOutcome::Start(Agent::Claude, "claude".into())));
        let mut a = ask();
        assert_eq!(feed(&mut a, b"\r"), Some(AskOutcome::Start(Agent::Claude, "claude --continue".into())));
    }

    #[test]
    fn wraps_and_special_rows() {
        let mut a = ask();
        assert_eq!(feed(&mut a, b"\x1b[A\r"), Some(AskOutcome::Never));
        let mut a = ask();
        assert_eq!(feed(&mut a, b"\x1b[A\x1b[A\r"), Some(AskOutcome::Later));
        assert_eq!(feed(&mut ask(), b"n"), Some(AskOutcome::Later));
        assert_eq!(feed(&mut ask(), b"X"), Some(AskOutcome::Never));
        assert_eq!(parse(b"\x1b"), vec![AskKey::Esc]);
        assert_eq!(feed(&mut ask(), b"\x1b"), Some(AskOutcome::Later));
    }

    #[test]
    fn other_command_edits_and_starts() {
        let mut a = ask();
        assert_eq!(feed(&mut a, b"o"), None);
        assert_eq!(a.edit.as_ref().unwrap().buf, "");
        assert_eq!(feed(&mut a, b"\r"), None);
        assert!(a.error.is_some() && a.edit.is_some(), "empty command refused");
        feed(&mut a, b"my-cc -x");
        assert!(a.error.is_none());
        assert_eq!(a.edit.as_ref().unwrap().cur, 8);
        assert_eq!(feed(&mut a, b"\r"), Some(AskOutcome::Start(Agent::Claude, "my-cc -x".into())));
    }

    #[test]
    fn e_edits_the_selected_command_with_cursor_keys() {
        let mut a = ask();
        feed(&mut a, b"e");
        assert_eq!(a.edit.as_ref().unwrap().buf, "claude --continue");
        feed(&mut a, b"\x1b[H"); // home
        feed(&mut a, b"\x1b[C\x1b[C"); // right x2
        feed(&mut a, b"\x1b[3~"); // delete 'a'
        feed(&mut a, b"\x7f"); // backspace 'c'
        assert_eq!(a.edit.as_ref().unwrap().buf, "cude --continue");
        assert_eq!(a.edit.as_ref().unwrap().cur, 1);
        a.paste("X\ny");
        assert_eq!(a.edit.as_ref().unwrap().buf, "cXyude --continue");
        feed(&mut a, b"\x1b[F");
        feed(&mut a, "é".as_bytes());
        assert!(a.edit.as_ref().unwrap().buf.ends_with("continueé"));
        feed(&mut a, b"\x1b[D\x1b[3~");
        assert!(a.edit.as_ref().unwrap().buf.ends_with("continue"));
    }

    #[test]
    fn esc_in_field_returns_to_list_and_tab_switches_agent() {
        let mut a = ask();
        feed(&mut a, b"o");
        feed(&mut a, b"\t");
        assert_eq!(a.edit.as_ref().unwrap().agent, Agent::Codex);
        assert_eq!(feed(&mut a, b"\x1b"), None);
        assert!(a.edit.is_none());
        assert_eq!(a.sel, a.other_row());
        // list letters type into the field while editing instead of acting
        feed(&mut a, b"o");
        assert_eq!(feed(&mut a, b"nx"), None);
        assert_eq!(a.edit.as_ref().unwrap().buf, "nx");
    }
}
