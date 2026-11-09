//! Hands-free dictation. The mode is ON or OFF; while ON a `wi-audio listen` child owns the
//! microphone and reports spoken phrases as JSON lines (see `audio/src/listen.rs`). Each finished
//! phrase is pasted into the focused pane, never followed by Enter. wi links no audio code and no
//! model: the child is any program that speaks the same lines (`[transcribe] command`).
//!
//! This file is the logic that does not need the terminal: parsing the child's lines, building
//! its command line, the status indicator, and the queue that holds phrases while the user types
//! or the pane cannot take text. The event loop in `app.rs` does the I/O.

use crate::config::Transcribe;
use std::collections::VecDeque;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc::UnboundedSender;

/// A phrase that finishes within this long after a key waits until typing pauses.
pub const TYPING_GAP: Duration = Duration::from_millis(300);

/// Where to get the engine when it is missing.
pub const INSTALL_HELP: &str = "no wi-audio found: download it from github.com/fahadsuhaib/wi-cli/releases, then run: wi-audio models install";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listen {
    Loading,
    Listening,
    Hearing,
    Transcribing,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    State(Listen),
    Level(f32),
    Final(String),
    Error { kind: String, msg: String },
    Off(String),
    /// The child ended (EOF on its output), with the last line it wrote to stderr.
    Exited(String),
}

/// One line of the child's output as an event; anything that is not a known event is ignored.
pub fn parse_event(line: &str) -> Option<Ev> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match v.get("ev")?.as_str()? {
        "state" => Some(Ev::State(match s("s").as_str() {
            "loading" => Listen::Loading,
            "listening" => Listen::Listening,
            "hearing" => Listen::Hearing,
            "transcribing" => Listen::Transcribing,
            _ => return None,
        })),
        "level" => Some(Ev::Level(v.get("v")?.as_f64()? as f32)),
        "final" => {
            let t = clean_phrase(&s("text"));
            (!t.is_empty()).then_some(Ev::Final(t))
        }
        "error" => Some(Ev::Error { kind: s("kind"), msg: s("msg") }),
        "off" => Some(Ev::Off(s("why"))),
        _ => None,
    }
}

/// Text that is safe to paste: one line, no control characters.
pub fn clean_phrase(t: &str) -> String {
    t.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split a command string into words; double or single quotes keep spaces together.
pub fn split_command(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in cmd.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The full command line: the configured command, `listen`, and the tuning options.
pub fn command_line(cfg: &Transcribe, idle: Duration) -> Vec<String> {
    let mut v = split_command(&cfg.command);
    if v.is_empty() {
        v.push("wi-audio".into());
    }
    v.push("listen".into());
    v.extend(["--end-silence-ms".into(), cfg.end_silence_ms.to_string()]);
    v.extend(["--min-speech-ms".into(), cfg.min_speech_ms.to_string()]);
    v.extend(["--vad-threshold".into(), cfg.vad_threshold.to_string()]);
    // wi switches the mode off itself at `idle`; the child gets a little more as a backstop.
    let backstop = if idle.is_zero() { 0 } else { idle.as_secs() + 10 };
    v.extend(["--idle-secs".into(), backstop.to_string()]);
    if !cfg.input_device.trim().is_empty() {
        v.extend(["--device".into(), cfg.input_device.trim().to_string()]);
    }
    v
}

/// The idle timeout: `WICLI_LISTEN_IDLE_SECS` (tests) beats `listen_idle_minutes`.
pub fn idle_timeout(cfg: &Transcribe) -> Duration {
    std::env::var("WICLI_LISTEN_IDLE_SECS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map(Duration::from_secs_f64)
        .unwrap_or_else(|| Duration::from_secs(u64::from(cfg.listen_idle_minutes) * 60))
}

/// One-line, actionable text for an error event.
pub fn friendly_error(kind: &str, msg: &str) -> String {
    match kind {
        "no_model" => "dictation: no speech model yet. Run: wi-audio models install".to_string(),
        "mic_denied" => "dictation: microphone blocked - allow your terminal in System Settings > Privacy & Security > Microphone".to_string(),
        "mic_silent" => "dictation: no sound from the mic - allow your terminal in System Settings > Privacy & Security > Microphone".to_string(),
        "no_mic" => "dictation: no microphone found".to_string(),
        "mic_stalled" => "dictation: the microphone stopped delivering audio (unplugged, or another app took it). Turn dictation off and on again".to_string(),
        _ if msg.is_empty() => "dictation: the engine reported an error".to_string(),
        _ => format!("dictation: {msg}"),
    }
}

/// Errors after which the child exits and the mode ends.
pub fn is_fatal(kind: &str) -> bool {
    matches!(kind, "no_model" | "mic_denied" | "no_mic" | "mic")
}

/// A recognised phrase waiting for a good moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub text: String,
    pub at: Instant,
}

/// What to do with the oldest waiting phrase now.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Type this (already joined with its leading space) now.
    Paste(String),
    /// Nothing to do before this moment (None: nothing waiting).
    Wait(Option<Instant>),
    /// This many phrases waited too long and were dropped.
    Dropped(usize),
}

/// The paste queue. `can_paste` is whether the focused pane takes text right now; `last_key` is
/// when the user last pressed a key or clicked.
pub fn next_step(q: &mut VecDeque<Held>, now: Instant, last_key: Instant, can_paste: bool, timeout: Duration, leading_space: bool) -> Step {
    let before = q.len();
    q.retain(|h| now.duration_since(h.at) < timeout);
    if q.len() < before {
        return Step::Dropped(before - q.len());
    }
    let Some(front) = q.front() else { return Step::Wait(None) };
    let expires = front.at + timeout;
    if !can_paste {
        return Step::Wait(Some(expires));
    }
    let quiet_at = last_key + TYPING_GAP;
    if now < quiet_at {
        return Step::Wait(Some(quiet_at.min(expires)));
    }
    let h = q.pop_front().expect("front exists");
    Step::Paste(if leading_space { format!(" {}", h.text) } else { h.text })
}

/// Whether the cell left of the cursor is blank (or there is none): then no leading space is added.
pub fn after_space(screen: &vt100::Screen) -> bool {
    let (row, col) = screen.cursor_position();
    if col == 0 {
        return true;
    }
    screen.cell(row, col - 1).is_none_or(|c| !c.has_contents() || c.contents().trim().is_empty())
}

/// The bytes to send to the pane for a phrase.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut v = Vec::with_capacity(text.len() + 12);
    if bracketed {
        v.extend_from_slice(b"\x1b[200~");
    }
    v.extend_from_slice(text.as_bytes());
    if bracketed {
        v.extend_from_slice(b"\x1b[201~");
    }
    v
}

/// The running mode.
pub struct Dictation {
    /// Which run the events belong to (a stale event from a stopped run is ignored).
    pub gen_id: u64,
    pub state: Listen,
    pub levels: VecDeque<f32>,
    pub last_speech: Instant,
    pub idle: Duration,
    pub held: VecDeque<Held>,
    /// When the last phrase went into a pane (the next one then starts with a space).
    pub last_paste: Option<Instant>,
    /// A fatal error text already shown, so the exit does not overwrite it.
    pub shown_error: bool,
    /// Why the engine said it stopped (its `off` event).
    pub off_why: Option<String>,
    _child: Child,
    _stdin: ChildStdin,
}

impl Dictation {
    pub fn spawn(cfg: &Transcribe, gen_id: u64, tx: UnboundedSender<(u64, Ev)>) -> Result<Dictation, String> {
        let idle = idle_timeout(cfg);
        let argv = command_line(cfg, idle);
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound { INSTALL_HELP.to_string() } else { format!("cannot start {}: {e}", argv[0]) }
            })?;
        let stdout = child.stdout.take().ok_or("no output from the engine")?;
        let stderr = child.stderr.take().ok_or("no error output from the engine")?;
        let stdin = child.stdin.take().ok_or("no input to the engine")?;
        let last_err = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let le = last_err.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                if !l.trim().is_empty() {
                    *le.lock().unwrap() = l;
                }
            }
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                if let Some(ev) = parse_event(&l) {
                    let _ = tx.send((gen_id, ev));
                }
            }
            let last = last_err.lock().unwrap().clone();
            let _ = tx.send((gen_id, Ev::Exited(last)));
        });
        Ok(Dictation {
            gen_id,
            state: Listen::Loading,
            levels: VecDeque::new(),
            last_speech: Instant::now(),
            idle,
            held: VecDeque::new(),
            last_paste: None,
            shown_error: false,
            off_why: None,
            _child: child,
            _stdin: stdin,
        })
    }

    pub fn push_level(&mut self, v: f32) {
        self.levels.push_back(v);
        while self.levels.len() > 6 {
            self.levels.pop_front();
        }
    }

    /// When the idle timeout ends the mode (None while a phrase is being heard).
    pub fn idle_deadline(&self) -> Instant {
        self.last_speech + self.idle
    }

    /// The text for the right end of the status line.
    pub fn indicator(&self) -> String {
        const BARS: [char; 5] = [' ', '▁', '▂', '▅', '▇'];
        let meter: String = self.levels.iter().map(|l| BARS[((l.clamp(0.0, 1.0)) * 4.0).round() as usize]).collect();
        let base = match self.state {
            Listen::Loading => "● loading".to_string(),
            Listen::Listening => format!("● listening {meter}"),
            Listen::Hearing => format!("● hearing {meter}"),
            Listen::Transcribing => "● ...".to_string(),
        };
        match self.held.front() {
            Some(h) => {
                let mut t: String = h.text.chars().take(24).collect();
                if h.text.chars().count() > 24 {
                    t.push('…');
                }
                format!("held: \"{t}\"  {}", base.trim_end())
            }
            None => base.trim_end().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(text: &str, at: Instant) -> Held {
        Held { text: text.into(), at }
    }

    #[test]
    fn events_parse() {
        assert_eq!(parse_event(r#"{"ev":"state","s":"hearing"}"#), Some(Ev::State(Listen::Hearing)));
        assert_eq!(parse_event(r#"{"ev":"level","v":0.4}"#), Some(Ev::Level(0.4)));
        assert_eq!(parse_event(r#"{"ev":"final","text":"  open  the\ttab ","ms":9}"#), Some(Ev::Final("open the tab".into())));
        assert_eq!(parse_event(r#"{"ev":"final","text":"   "}"#), None);
        assert_eq!(parse_event(r#"{"ev":"error","kind":"no_mic","msg":"x"}"#), Some(Ev::Error { kind: "no_mic".into(), msg: "x".into() }));
        assert_eq!(parse_event(r#"{"ev":"off","why":"idle"}"#), Some(Ev::Off("idle".into())));
        assert_eq!(parse_event("model loaded"), None);
        assert_eq!(parse_event(r#"{"ev":"mystery"}"#), None);
        assert_eq!(parse_event(r#"{"ev":"state","s":"dancing"}"#), None);
    }

    #[test]
    fn pasted_text_has_no_newlines_or_controls() {
        assert_eq!(clean_phrase("a\nb\r\nc\x1b[31m"), "a b c [31m");
        assert!(!paste_bytes("x", false).contains(&b'\r'));
    }

    #[test]
    fn command_splits_with_quotes() {
        assert_eq!(split_command("wi-audio"), vec!["wi-audio"]);
        assert_eq!(split_command("  python3 \"/my dir/fake.py\" -x 'a b' "), vec!["python3", "/my dir/fake.py", "-x", "a b"]);
        assert_eq!(split_command(""), Vec::<String>::new());
    }

    #[test]
    fn command_line_carries_the_tuning() {
        let cfg = Transcribe { command: "wi-audio --x".into(), input_device: " USB ".into(), ..Transcribe::default() };
        let v = command_line(&cfg, Duration::from_secs(600));
        assert_eq!(&v[..3], ["wi-audio", "--x", "listen"]);
        let at = |k: &str| v[v.iter().position(|a| a == k).unwrap() + 1].clone();
        assert_eq!(at("--end-silence-ms"), "700");
        assert_eq!(at("--min-speech-ms"), "250");
        assert_eq!(at("--idle-secs"), "610");
        assert_eq!(at("--device"), "USB");
        let none = command_line(&Transcribe { command: "  ".into(), ..Transcribe::default() }, Duration::ZERO);
        assert_eq!(none[0], "wi-audio");
        assert_eq!(none[none.iter().position(|a| a == "--idle-secs").unwrap() + 1], "0", "never idle: the engine gets no timeout either");
    }

    #[test]
    fn phrase_is_pasted_when_idle_and_ready() {
        let now = Instant::now();
        let mut q = VecDeque::from([held("hello there", now)]);
        let step = next_step(&mut q, now + Duration::from_secs(1), now - Duration::from_secs(5), true, Duration::from_secs(30), true);
        assert_eq!(step, Step::Paste(" hello there".into()));
        assert!(q.is_empty());
        let mut q = VecDeque::from([held("hello there", now)]);
        assert_eq!(next_step(&mut q, now, now - Duration::from_secs(5), true, Duration::from_secs(30), false), Step::Paste("hello there".into()));
    }

    #[test]
    fn typing_holds_the_phrase_until_a_pause() {
        let now = Instant::now();
        let mut q = VecDeque::from([held("x", now)]);
        let key = now + Duration::from_millis(100);
        match next_step(&mut q, now + Duration::from_millis(200), key, true, Duration::from_secs(30), true) {
            Step::Wait(Some(t)) => assert_eq!(t, key + TYPING_GAP),
            other => panic!("{other:?}"),
        }
        assert_eq!(q.len(), 1);
        assert!(matches!(next_step(&mut q, key + TYPING_GAP, key, true, Duration::from_secs(30), true), Step::Paste(_)));
    }

    #[test]
    fn a_pane_that_cannot_take_text_holds_then_drops_after_the_timeout() {
        let now = Instant::now();
        let mut q = VecDeque::from([held("x", now), held("y", now + Duration::from_secs(10))]);
        let old = now - Duration::from_secs(60);
        assert_eq!(next_step(&mut q, now + Duration::from_secs(5), old, false, Duration::from_secs(30), true), Step::Wait(Some(now + Duration::from_secs(30))));
        assert_eq!(next_step(&mut q, now + Duration::from_secs(31), old, false, Duration::from_secs(30), true), Step::Dropped(1));
        assert_eq!(q.len(), 1);
        // The pane is ready again: the newer phrase goes in.
        assert_eq!(next_step(&mut q, now + Duration::from_secs(32), old, true, Duration::from_secs(30), false), Step::Paste("y".into()));
        assert_eq!(next_step(&mut q, now + Duration::from_secs(33), old, true, Duration::from_secs(30), false), Step::Wait(None));
    }

    #[test]
    fn leading_space_depends_on_the_cell_before_the_cursor() {
        let mut p = vt100::Parser::new(crate::tab::nz(3), crate::tab::nz(20), 0);
        assert!(after_space(p.screen()));
        p.process(b"> ");
        assert!(after_space(p.screen()));
        p.process(b"fix");
        assert!(!after_space(p.screen()));
    }

    #[test]
    fn bracketed_markers_only_when_the_pane_asked() {
        assert_eq!(paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~");
        assert_eq!(paste_bytes("hi", false), b"hi");
    }

    #[test]
    fn errors_are_one_friendly_line() {
        assert!(friendly_error("no_model", "").contains("wi-audio models install"));
        assert!(friendly_error("mic_silent", "").contains("System Settings"));
        assert_eq!(friendly_error("engine", "boom"), "dictation: boom");
        assert!(is_fatal("mic_denied") && !is_fatal("mic_silent"));
    }

    #[test]
    fn indicator_shows_state_meter_and_held_text() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let cfg = Transcribe { command: "sleep 5".into(), ..Transcribe::default() };
            // `sleep 5 listen ...` fails fast, which is fine: only the struct is used here.
            let mut d = Dictation::spawn(&cfg, 1, tx).unwrap();
            assert_eq!(d.indicator(), "● loading");
            d.state = Listen::Listening;
            d.push_level(0.0);
            d.push_level(1.0);
            assert_eq!(d.indicator(), "● listening  ▇");
            d.state = Listen::Transcribing;
            assert_eq!(d.indicator(), "● ...");
            d.held.push_back(held("a very long phrase that does not fit in the status line", Instant::now()));
            assert!(d.indicator().starts_with("held: \"a very long phrase that …\""));
        });
    }

    #[test]
    fn idle_timeout_reads_the_override() {
        let cfg = Transcribe { listen_idle_minutes: 7, ..Transcribe::default() };
        // SAFETY: tests that touch this variable are this one only.
        unsafe { std::env::remove_var("WICLI_LISTEN_IDLE_SECS") };
        assert_eq!(idle_timeout(&cfg), Duration::from_secs(420));
        unsafe { std::env::set_var("WICLI_LISTEN_IDLE_SECS", "1.5") };
        assert_eq!(idle_timeout(&cfg), Duration::from_millis(1500));
        unsafe { std::env::remove_var("WICLI_LISTEN_IDLE_SECS") };
    }
}
