//! Voice dictation set-up, done entirely from inside wi: finding the helper program (`wi-audio`),
//! installing it, fetching the speech model through it, a first microphone test, and the
//! "Dictation settings" screen. The user never runs the helper by hand; it is an internal part.
//!
//! The pieces that touch the disk or the network run on worker threads or tasks, never on the UI
//! loop, and nothing is contacted before the dialog that names the file, its size and the host got
//! a yes. Downloads use the same curl/wget + SHA256 code as "Update wi".
//!
//! Test hooks: `WICLI_AUDIO_URL` is a base URL (`file:///dir` works with curl) holding the helper
//! archive and `SHA256SUMS` instead of github.com; `WICLI_VOICE_BIN` replaces
//! `~/.local/share/wicli/bin` (the fallback folder when wi's own folder is not writable).

// Voice dictation is macOS only for now: elsewhere nothing reaches this code.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::ui::VoiceView;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

pub const ASSET: &str = "wi-audio-macos-arm64.tar.gz";
const RELEASES: &str = "https://github.com/fahadsuhaib/wi-cli/releases/latest/download";
/// Largest helper archive accepted.
const MAX_ARCHIVE: u64 = 40 * 1024 * 1024;
pub const MODEL_BYTES: u64 = 295_793_568;

pub fn override_url() -> Option<String> {
    std::env::var("WICLI_AUDIO_URL").ok().filter(|s| !s.trim().is_empty()).map(|s| s.trim_end_matches('/').to_string())
}

pub fn base_url() -> String {
    override_url().unwrap_or_else(|| RELEASES.to_string())
}

// ---------------------------------------------------------------- finding the helper

pub fn user_bin_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("WICLI_VOICE_BIN").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".local/share/wicli/bin")
}

fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.parent().map(Path::to_path_buf)
}

fn is_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// The command (program and leading arguments) that runs the helper, if there is one. A
/// `[transcribe] command` the user changed wins; otherwise next to the wi binary, then the
/// fallback folder, then `PATH`.
pub fn locate_in(command: &str, exe_dir: Option<&Path>, user_bin: &Path, path_var: Option<&std::ffi::OsStr>) -> Option<Vec<String>> {
    let words = crate::dictate::split_command(command);
    if words.is_empty() || words != ["wi-audio"] {
        return (!words.is_empty()).then_some(words);
    }
    let mut dirs: Vec<PathBuf> = exe_dir.into_iter().map(Path::to_path_buf).collect();
    dirs.push(user_bin.to_path_buf());
    if let Some(p) = path_var {
        dirs.extend(std::env::split_paths(p));
    }
    dirs.into_iter().map(|d| d.join("wi-audio")).find(|p| is_file(p)).map(|p| vec![p.display().to_string()])
}

/// `WICLI_VOICE_COMMAND` (tests, debugging) names another helper program.
pub fn command() -> String {
    std::env::var("WICLI_VOICE_COMMAND").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "wi-audio".into())
}

pub fn locate() -> Option<Vec<String>> {
    locate_in(&command(), exe_dir().as_deref(), &user_bin_dir(), std::env::var_os("PATH").as_deref())
}

// ---------------------------------------------------------------- what the helper reports

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HelperStatus {
    pub version: String,
    pub speech: bool,
    pub speech_bytes: u64,
    pub speech_total: u64,
    pub vad: bool,
    /// The best speech model has been downloaded (true when an older helper does not say).
    pub best_present: bool,
}

pub fn parse_status(line: &str) -> Option<HelperStatus> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("ev")?.as_str()? != "status" {
        return None;
    }
    let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    Some(HelperStatus {
        version: v.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        speech: b("speech"),
        speech_bytes: n("speech_bytes"),
        speech_total: n("speech_total"),
        vad: b("vad"),
        best_present: v.get("best_present").and_then(|x| x.as_bool()).unwrap_or(true),
    })
}

/// Ask the helper what it has (blocking, 10 s at most).
pub fn query_status(argv: &[String]) -> Result<HelperStatus, String> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .arg("status")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start the voice helper: {e}"))?;
    let mut out = child.stdout.take().ok_or("no output from the voice helper")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        let _ = out.by_ref().take(64 * 1024).read_to_string(&mut s);
        let _ = tx.send(s);
    });
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(text) => {
            let _ = child.wait();
            text.lines().find_map(parse_status).ok_or_else(|| "the voice helper gave an answer wi does not understand (reinstall it from Dictation settings)".to_string())
        }
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("the voice helper did not answer".into())
        }
    }
}

// ---------------------------------------------------------------- installing the helper

fn place(new: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let staged = dir.join(format!(".wi-audio-{}", std::process::id()));
    if let Err(e) = std::fs::copy(new, &staged) {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    let finish = || -> std::io::Result<PathBuf> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        let target = dir.join("wi-audio");
        std::fs::rename(&staged, &target)?;
        Ok(target)
    };
    finish().inspect_err(|_| {
        let _ = std::fs::remove_file(&staged);
    })
}

fn denied(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(30)
}

/// Download the helper archive from `base`, check it against `SHA256SUMS`, and put the binary in
/// the first of `dirs` that is writable. Never uses sudo. Returns where it went.
pub fn install_helper(base: &str, dirs: &[PathBuf], status: &mut dyn FnMut(String)) -> Result<PathBuf, String> {
    use crate::update::{download_to, parse_sums, read_text, scratch_dir, sha256_of, unpack};
    let dir = scratch_dir()?;
    let result = (|| {
        let archive = dir.join(ASSET);
        status("downloading the voice helper ...".into());
        download_to(&format!("{base}/{ASSET}"), &archive, Duration::from_secs(300), MAX_ARCHIVE)?;
        status("checking SHA256 ...".into());
        let sums = dir.join("SHA256SUMS");
        download_to(&format!("{base}/SHA256SUMS"), &sums, Duration::from_secs(30), 256 * 1024)?;
        let want = parse_sums(&read_text(&sums)?, ASSET).ok_or_else(|| format!("{ASSET} is not listed in SHA256SUMS; nothing was installed"))?;
        let got = sha256_of(&archive)?;
        if want != got {
            return Err(format!("checksum mismatch for {ASSET}; nothing was installed"));
        }
        let out = dir.join("out");
        std::fs::create_dir(&out).map_err(|e| e.to_string())?;
        unpack(&archive, &out)?;
        let new = out.join(ASSET.trim_end_matches(".tar.gz")).join("wi-audio");
        if !is_file(&new) {
            return Err("the archive does not contain the voice helper".into());
        }
        let o = Command::new(&new).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| format!("the downloaded helper does not run: {e}"))?;
        if !o.status.success() || !String::from_utf8_lossy(&o.stdout).contains("wi-audio") {
            return Err("the downloaded helper does not run, so it was not installed".into());
        }
        status("installing ...".into());
        let mut last = String::from("no folder to install into");
        for d in dirs {
            match place(&new, d) {
                Ok(p) => return Ok(p),
                Err(e) if denied(&e) => last = format!("{} is not writable", d.display()),
                Err(e) => return Err(format!("could not write {}: {e}", d.display())),
            }
        }
        Err(last)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Where a first install goes: next to the wi binary, else the fallback folder.
pub fn install_dirs() -> Vec<PathBuf> {
    exe_dir().into_iter().chain(std::iter::once(user_bin_dir())).collect()
}

/// A helper that is installed by us (not one found only on `PATH`), for update and uninstall.
pub fn installed_copies() -> Vec<PathBuf> {
    install_dirs().into_iter().map(|d| d.join("wi-audio")).filter(|p| is_file(p)).collect()
}

/// Called by "Update wi" after wi itself was replaced: refresh the helper if we installed one.
pub fn update_helper(base: &str, status: &mut dyn FnMut(String)) -> Option<Result<PathBuf, String>> {
    let dir = installed_copies().first()?.parent()?.to_path_buf();
    Some(install_helper(base, &[dir], status))
}

/// Remove the helper copies we installed. Returns the paths removed.
pub fn uninstall_helper() -> Result<Vec<PathBuf>, String> {
    let mut gone = Vec::new();
    for p in installed_copies() {
        std::fs::remove_file(&p).map_err(|e| format!("could not remove {}: {e}", p.display()))?;
        gone.push(p);
    }
    Ok(gone)
}

/// Remove the voice for good: the models (through the helper) and the helper copies we installed.
pub fn remove_voice(argv: Option<&[String]>) -> Result<String, String> {
    if let Some(a) = argv {
        let _ = Command::new(&a[0]).args(&a[1..]).args(["models", "remove"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    uninstall_helper()?;
    Ok("voice removed".into())
}

// ---------------------------------------------------------------- the first-use state machine

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Install,
    Model,
    /// A better speech model than the installed one exists.
    Upgrade,
    Mic,
}

/// The model the helper downloads by default; a declined upgrade is remembered as `upgrade:<id>`.
pub const BEST_MODEL: &str = "medium";

impl Step {
    pub fn key(self) -> &'static str {
        match self {
            Step::Install => "helper",
            Step::Model => "model",
            Step::Upgrade => "upgrade",
            Step::Mic => "mic",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    /// Show the dialog for this step.
    Ask(Step),
    /// The user said "not now" to this step earlier: say how to do it, do not ask again.
    NotNow(Step),
    /// Everything needed is there (or the optional test was declined): turn dictation on.
    Start,
}

/// What is the next thing missing? `helper`: a helper was found; `status`: its answer (None when
/// it could not be asked, in which case wi just tries to start); `declined`: steps answered
/// "not now" before; `mic_ok`: the microphone test passed once.
pub fn next(helper: bool, status: Option<&HelperStatus>, declined: &[String], mic_ok: bool) -> Next {
    let no = |s: Step| declined.iter().any(|d| d == s.key());
    if !helper {
        return if no(Step::Install) { Next::NotNow(Step::Install) } else { Next::Ask(Step::Install) };
    }
    let Some(st) = status else { return Next::Start };
    if !st.speech {
        return if no(Step::Model) { Next::NotNow(Step::Model) } else { Next::Ask(Step::Model) };
    }
    if !st.best_present && !declined.iter().any(|d| *d == format!("upgrade:{BEST_MODEL}")) {
        return Next::Ask(Step::Upgrade);
    }
    if !mic_ok && !no(Step::Mic) {
        return Next::Ask(Step::Mic);
    }
    Next::Start
}

pub fn not_now_text(step: Step) -> &'static str {
    match step {
        Step::Install => "Dictation needs a small helper; choose Dictation again to install it",
        Step::Model => "Dictation needs a speech model; choose Dictation again to download it",
        Step::Upgrade => "keeping the current speech model; Dictation options, D downloads the better one later",
        Step::Mic => "",
    }
}

// ---------------------------------------------------------------- dialogs

/// Text for a progress bar `width` cells wide.
pub fn bar(done: u64, total: u64, width: usize) -> String {
    let pct = (done * 100).checked_div(total).unwrap_or(0).min(100) as usize;
    let filled = pct * width / 100;
    format!("[{}{}] {pct:>3}%", "#".repeat(filled), "-".repeat(width - filled))
}

fn meter(levels: &VecDeque<f32>) -> String {
    const B: [char; 5] = [' ', '▁', '▂', '▅', '▇'];
    levels.iter().map(|l| B[(l.clamp(0.0, 1.0) * 4.0).round() as usize]).collect()
}

fn mb(n: u64) -> String {
    format!("{:.1} MB", n as f64 / 1_000_000.0)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Ask(Step),
    /// A step is running (Esc cancels).
    Busy(Step),
    /// Heard something in the test recording.
    Heard,
    /// A message; Enter or Esc closes it.
    Info,
    /// Dictation is on: Enter turns it off, O opens the options.
    On,
    Options,
    AskRemove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Yes,
    No,
    Char(char),
    None,
}

#[derive(Clone)]
pub struct Dlg {
    pub kind: Kind,
    pub view: VoiceView,
    levels: VecDeque<f32>,
}

impl Dlg {
    fn new(kind: Kind, title: &str, lines: Vec<String>, footer: &str) -> Dlg {
        Dlg { kind, view: VoiceView { title: title.into(), lines, footer: footer.into(), heading: false }, levels: VecDeque::new() }
    }

    pub fn ask(step: Step, model_bytes: u64) -> Dlg {
        const FOOT: &str = "Enter: yes  ·  Esc: not now";
        let mut d = match step {
            Step::Install => Dlg::new(
                Kind::Ask(step),
                "Voice setup: the helper",
                vec![
                    "Install the small voice helper (about 2 MB)".into(),
                    "It is the part that listens and turns speech into text. It is downloaded from github.com/fahadsuhaib/wi-cli (wi-audio-macos-arm64.tar.gz) and checked with SHA256 before it is used.".into(),
                    "Your audio never leaves this Mac.".into(),
                ],
                FOOT,
            ),
            Step::Model => Dlg::new(
                Kind::Ask(step),
                "Voice setup: the speech model",
                vec![
                    format!("Download the speech model ({})", mb(if model_bytes > 0 { model_bytes } else { MODEL_BYTES })),
                    "It is downloaded once from huggingface.co (moonshine-streaming-medium-Q8_0.gguf), checked with SHA256 and reused every time. It runs only on this Mac, so nothing you say is sent anywhere.".into(),
                    "While dictation is on it uses roughly 600 MB of memory; when it is off it uses none.".into(),
                ],
                FOOT,
            ),
            Step::Upgrade => Dlg::new(
                Kind::Ask(step),
                "Voice update: a better speech model",
                vec![
                    format!("Download the better speech model ({})", mb(if model_bytes > 0 { model_bytes } else { MODEL_BYTES })),
                    "You are using a smaller speech model. The better one makes noticeably fewer mistakes, especially with technical words. It downloads once from huggingface.co (moonshine-streaming-medium-Q8_0.gguf) and is checked with SHA256. The current model stays until you remove voice.".into(),
                    "Nothing you say leaves this Mac.".into(),
                ],
                "Enter: download  ·  Esc: not now, keep the current one",
            ),
            Step::Mic => Dlg::new(
                Kind::Ask(step),
                "Voice setup: the microphone",
                vec![
                    "Test the microphone (3 seconds)".into(),
                    "When the test starts, say a short sentence. You will see the level move as you speak, then what was heard.".into(),
                    "The first time, macOS asks whether your terminal app may use the microphone. Choose Allow.".into(),
                ],
                "Enter: start the test  ·  Esc: skip and turn dictation on",
            ),
        };
        d.view.heading = true;
        d
    }

    pub fn busy(step: Step) -> Dlg {
        let (title, line) = match step {
            Step::Install => ("Installing the voice helper", "Starting the download ..."),
            Step::Model | Step::Upgrade => ("Downloading the speech model", "Starting the download ..."),
            Step::Mic => ("Microphone test", "Speak now ..."),
        };
        Dlg::new(Kind::Busy(step), title, vec![line.into()], "Esc: cancel")
    }

    pub fn status_line(&mut self, text: &str) {
        self.view.lines = vec![text.to_string()];
    }

    pub fn progress(&mut self, what: &str, done: u64, total: u64) {
        self.view.lines = vec![what.to_string(), bar(done, total, 30), format!("{} of {}", mb(done), mb(total))];
    }

    pub fn level(&mut self, v: f32, elapsed: Duration) {
        self.levels.push_back(v);
        while self.levels.len() > 24 {
            self.levels.pop_front();
        }
        self.view.lines = vec![format!("Speak now ...   {:.0} s of 3", elapsed.as_secs_f32().min(3.0)), format!("  {}", meter(&self.levels)), "Say a short sentence, for example: write a short commit message.".into()];
    }

    pub fn heard(text: &str) -> Dlg {
        let line = if text.trim().is_empty() { "The microphone works, but no words were recognised. Try again closer to the microphone.".to_string() } else { format!("heard: {text}") };
        Dlg::new(Kind::Heard, "Microphone works", vec![line, "Looks good. Dictation will type each phrase you say into the focused pane, without pressing Enter.".into()], "Enter: turn dictation on  ·  Esc: close")
    }

    pub fn info(title: &str, lines: Vec<String>) -> Dlg {
        Dlg::new(Kind::Info, title, lines, "Enter or Esc: close")
    }

    pub fn on() -> Dlg {
        Dlg::new(Kind::On, "Dictation is on", vec!["Just talk. Each phrase you say is typed into the focused pane, without pressing Enter.".into(), "Click the mic at the top right, or press Enter here, to turn it off.".into()], "Enter: turn off  ·  O: options  ·  Esc: close")
    }

    pub fn options(auto_send: bool) -> Dlg {
        let a = if auto_send { "A: automatic Enter is ON: Enter is pressed after 3 seconds of quiet (press A to turn off)" } else { "A: automatic Enter is OFF (press A to press Enter by itself after 3 seconds of quiet)" };
        Dlg::new(Kind::Options, "Dictation options", vec![a.into(), "T: test the microphone".into(), "D: download the speech model again (fixes a damaged file, or updates it)".into(), "R: remove voice (deletes the model and helper, frees 300 MB)".into()], "A: auto Enter  ·  T: test  ·  D: download again  ·  R: remove  ·  Esc: back")
    }

    pub fn ask_remove() -> Dlg {
        Dlg::new(Kind::AskRemove, "Remove voice?", vec!["Deletes the speech model and the helper (frees 300 MB). Dictation sets itself up again if you use it later.".into()], "Enter: remove  ·  Esc: keep")
    }

    /// Read one chunk of keyboard input.
    pub fn key(&self, chunk: &[u8], menu_key: u8) -> Key {
        for (i, &b) in chunk.iter().enumerate() {
            match b {
                b'\r' | b'\n' => return Key::Yes,
                0x1b if chunk.get(i + 1).is_none() => return Key::No,
                x if x == menu_key => return Key::No,
                b'y' | b'Y' if !matches!(self.kind, Kind::Options) => return Key::Yes,
                b'n' | b'N' if !matches!(self.kind, Kind::Options) => return Key::No,
                c if c.is_ascii_alphabetic() => return Key::Char(c.to_ascii_lowercase() as char),
                _ => {}
            }
        }
        Key::None
    }
}

/// Friendly one-liner for an error the helper reported while testing or downloading.
pub fn friendly(kind: &str, msg: &str) -> String {
    match kind {
        "mic_denied" | "mic_silent" => "No sound from the microphone. Allow your terminal app in System Settings > Privacy & Security > Microphone, then restart the terminal and try again.".to_string(),
        "no_mic" | "mic" => "No microphone found.".to_string(),
        _ if msg.is_empty() => "Something went wrong.".to_string(),
        _ => msg.to_string(),
    }
}

// ---------------------------------------------------------------- workers (off the UI loop)

/// What a worker reports back to the event loop (tagged with the run it belongs to).
#[derive(Debug)]
pub enum VEv {
    /// The helper was looked for and asked about its models.
    Checked { argv: Option<Vec<String>>, status: Option<Result<HelperStatus, String>> },
    /// A line of progress text.
    Status(String),
    Progress { what: String, done: u64, total: u64 },
    Level(f32),
    Installed(Result<PathBuf, String>),
    ModelDone(Result<(), String>),
    /// The test recording: the text heard, or (kind, message) of an error.
    Heard(Result<String, (String, String)>),
    Removed(Result<String, String>),
}

pub type Tx = tokio::sync::mpsc::UnboundedSender<(u64, VEv)>;

/// Look for the helper and ask it what it has.
pub fn spawn_check(tx: &Tx, gen_id: u64, argv: Option<Vec<String>>) {
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let status = argv.as_ref().map(|a| query_status(a));
        let _ = tx.send((gen_id, VEv::Checked { argv, status }));
    });
}

pub fn spawn_install(tx: &Tx, gen_id: u64) {
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let t2 = tx.clone();
        let mut status = |s: String| {
            let _ = t2.send((gen_id, VEv::Status(s)));
        };
        let r = install_helper(&base_url(), &install_dirs(), &mut status);
        let _ = tx.send((gen_id, VEv::Installed(r)));
    });
}

pub fn spawn_remove(tx: &Tx, gen_id: u64, argv: Option<Vec<String>>) {
    let tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        let _ = tx.send((gen_id, VEv::Removed(remove_voice(argv.as_deref()))));
    });
}

/// Run `<helper> <sub...>` and turn each JSON line it prints into events until it ends. Setting
/// `cancel` kills it.
fn run_events(tx: Tx, gen_id: u64, argv: Vec<String>, sub: &'static [&'static str], cancel: std::sync::Arc<std::sync::atomic::AtomicBool>, map: fn(&serde_json::Value) -> Option<VEv>, on_exit: fn() -> VEv) {
    use std::sync::atomic::Ordering;
    use tokio::io::{AsyncBufReadExt, BufReader};
    tokio::spawn(async move {
        let spawned = tokio::process::Command::new(&argv[0])
            .args(&argv[1..])
            .args(sub)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send((gen_id, VEv::Status(format!("cannot start the voice helper: {e}"))));
                let _ = tx.send((gen_id, on_exit()));
                return;
            }
        };
        let _stdin = child.stdin.take();
        let Some(out) = child.stdout.take() else { return };
        let mut lines = BufReader::new(out).lines();
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.start_kill();
                return;
            }
            match tokio::time::timeout(Duration::from_millis(150), lines.next_line()).await {
                Err(_) => {}
                Ok(Ok(Some(l))) => {
                    if let Some(ev) = serde_json::from_str::<serde_json::Value>(&l).ok().and_then(|v| map(&v)) {
                        let end = matches!(ev, VEv::ModelDone(_) | VEv::Heard(_));
                        let _ = tx.send((gen_id, ev));
                        if end {
                            return;
                        }
                    }
                }
                Ok(_) => {
                    let _ = tx.send((gen_id, on_exit()));
                    return;
                }
            }
        }
    });
}

fn s(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// Download the missing model files through the helper (wi already asked the user).
pub fn spawn_fetch(tx: &Tx, gen_id: u64, argv: Vec<String>, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>, force: bool) {
    run_events(
        tx.clone(),
        gen_id,
        argv,
        if force { &["fetch-models", "--force"] } else { &["fetch-models"] },
        cancel,
        |v| match v.get("ev")?.as_str()? {
            "progress" => Some(VEv::Progress {
                what: if s(v, "what") == "vad" { "Voice detector".into() } else { "Speech model".into() },
                done: v.get("done")?.as_u64()?,
                total: v.get("total")?.as_u64()?,
            }),
            "done" => Some(VEv::ModelDone(Ok(()))),
            "error" => Some(VEv::ModelDone(Err(s(v, "msg")))),
            _ => None,
        },
        || VEv::ModelDone(Err("the download stopped before it finished".into())),
    );
}

/// The 3 second microphone test through the helper.
pub fn spawn_probe(tx: &Tx, gen_id: u64, argv: Vec<String>, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    run_events(
        tx.clone(),
        gen_id,
        argv,
        &["probe"],
        cancel,
        |v| match v.get("ev")?.as_str()? {
            "level" => Some(VEv::Level(v.get("v")?.as_f64()? as f32)),
            "heard" => Some(VEv::Heard(Ok(s(v, "text")))),
            "error" => Some(VEv::Heard(Err((s(v, "kind"), s(v, "msg"))))),
            _ => None,
        },
        || VEv::Heard(Err(("mic".into(), "the microphone test stopped unexpectedly".into()))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(speech: bool) -> HelperStatus {
        HelperStatus { version: "1".into(), speech, speech_bytes: 5, speech_total: MODEL_BYTES, vad: true, best_present: true }
    }

    #[test]
    fn missing_pieces_are_asked_in_order() {
        assert_eq!(next(false, None, &[], false), Next::Ask(Step::Install));
        assert_eq!(next(true, Some(&st(false)), &[], false), Next::Ask(Step::Model));
        assert_eq!(next(true, Some(&st(true)), &[], false), Next::Ask(Step::Mic));
        assert_eq!(next(true, Some(&st(true)), &[], true), Next::Start);
        assert_eq!(next(true, None, &[], false), Next::Start, "an unreadable status does not block starting");
    }

    #[test]
    fn a_better_model_is_offered_once() {
        let d = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let old = HelperStatus { best_present: false, ..st(true) };
        assert_eq!(next(true, Some(&old), &[], true), Next::Ask(Step::Upgrade));
        assert_eq!(next(true, Some(&old), &d(&["upgrade:medium"]), true), Next::Start, "declined: use what we have, no nagging");
        assert_eq!(next(true, Some(&old), &d(&["upgrade:older"]), true), Next::Ask(Step::Upgrade), "a decline is for that model only");
        assert_eq!(next(true, Some(&st(true)), &[], true), Next::Start, "the best model is there: nothing to offer");
        assert_eq!(next(true, Some(&HelperStatus { best_present: false, ..st(false) }), &[], true), Next::Ask(Step::Model), "no model at all asks for the model first");
    }

    #[test]
    fn declined_steps_are_not_asked_again() {
        let d = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(next(false, None, &d(&["helper"]), false), Next::NotNow(Step::Install));
        assert_eq!(next(true, Some(&st(false)), &d(&["model"]), false), Next::NotNow(Step::Model));
        assert_eq!(next(true, Some(&st(true)), &d(&["mic"]), false), Next::Start, "skipping the test still turns it on");
        assert_eq!(next(true, Some(&st(false)), &d(&["helper"]), false), Next::Ask(Step::Model), "each step has its own memory");
        assert!(not_now_text(Step::Install).contains("choose Dictation again"));
    }

    #[test]
    fn status_line_parses() {
        let s = parse_status(r#"{"ev":"status","version":"0.1.0","speech":true,"speech_bytes":10,"speech_total":20,"vad":false}"#).unwrap();
        assert_eq!(s, HelperStatus { version: "0.1.0".into(), speech: true, speech_bytes: 10, speech_total: 20, vad: false, best_present: true });
        assert!(parse_status(r#"{"ev":"level","v":1}"#).is_none() && parse_status("hello").is_none());
    }

    #[test]
    fn locate_order_is_custom_command_then_next_to_wi_then_fallback_then_path() {
        let d = std::env::temp_dir().join(format!("wi-voice-locate-{}", std::process::id()));
        let (a, b, c) = (d.join("exe"), d.join("bin"), d.join("path"));
        for x in [&a, &b, &c] {
            std::fs::create_dir_all(x).unwrap();
        }
        let path = std::env::join_paths([&c]).unwrap();
        assert_eq!(locate_in("wi-audio", Some(&a), &b, Some(&path)), None);
        std::fs::write(c.join("wi-audio"), b"x").unwrap();
        assert_eq!(locate_in("wi-audio", Some(&a), &b, Some(&path)), Some(vec![c.join("wi-audio").display().to_string()]));
        std::fs::write(b.join("wi-audio"), b"x").unwrap();
        assert_eq!(locate_in("wi-audio", Some(&a), &b, Some(&path)), Some(vec![b.join("wi-audio").display().to_string()]));
        std::fs::write(a.join("wi-audio"), b"x").unwrap();
        assert_eq!(locate_in("wi-audio", Some(&a), &b, Some(&path)), Some(vec![a.join("wi-audio").display().to_string()]));
        assert_eq!(locate_in("my-engine --x", Some(&a), &b, Some(&path)), Some(vec!["my-engine".into(), "--x".into()]), "a command the user set wins");
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn fixture(tag: &str, script: &str) -> (PathBuf, String) {
        let d = std::env::temp_dir().join(format!("wi-voice-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let pkg = d.join("pkg/wi-audio-macos-arm64");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("wi-audio"), script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(pkg.join("wi-audio"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let serve = d.join("serve");
        std::fs::create_dir_all(&serve).unwrap();
        let ok = Command::new("tar").arg("-czf").arg(serve.join(ASSET)).arg("-C").arg(d.join("pkg")).arg("wi-audio-macos-arm64").status().unwrap();
        assert!(ok.success());
        let sum = sha256_hex(&serve.join(ASSET));
        std::fs::write(serve.join("SHA256SUMS"), format!("{sum}  {ASSET}\n")).unwrap();
        let base = format!("file://{}", serve.display());
        (d, base)
    }

    fn sha256_hex(p: &Path) -> String {
        crate::update::sha256_of(p).unwrap()
    }

    #[test]
    fn helper_installs_next_to_wi_after_the_checksum_passes() {
        let (d, base) = fixture("ok", "#!/bin/sh\necho wi-audio 9.9.9\n");
        let target = d.join("wi-dir");
        let mut seen = Vec::new();
        let got = install_helper(&base, &[target.clone(), d.join("fallback")], &mut |s| seen.push(s)).unwrap();
        assert_eq!(got, target.join("wi-audio"));
        assert!(is_file(&got) && !d.join("fallback").exists());
        assert!(seen.iter().any(|s| s.contains("SHA256")));
        assert!(std::fs::read_dir(&target).unwrap().all(|e| e.unwrap().file_name() == "wi-audio"), "no staged files left");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_folder_that_cannot_be_written_falls_back_to_the_next() {
        // SAFETY: geteuid has no preconditions. Root can write anywhere, so there is nothing to test.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let (d, base) = fixture("fb", "#!/bin/sh\necho wi-audio 9.9.9\n");
        let locked = d.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        let fallback = d.join("fallback");
        let got = install_helper(&base, &[locked.clone(), fallback.clone()], &mut |_| {}).unwrap();
        assert_eq!(got, fallback.join("wi-audio"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_wrong_checksum_or_a_broken_binary_installs_nothing() {
        let (d, base) = fixture("bad", "#!/bin/sh\necho wi-audio 9.9.9\n");
        let serve = d.join("serve");
        std::fs::write(serve.join("SHA256SUMS"), format!("{}  {ASSET}\n", "0".repeat(64))).unwrap();
        let target = d.join("t");
        let e = install_helper(&base, std::slice::from_ref(&target), &mut |_| {}).unwrap_err();
        assert!(e.contains("checksum"), "{e}");
        assert!(!target.join("wi-audio").exists());
        std::fs::remove_dir_all(&d).unwrap();
        let (d, base) = fixture("broken", "#!/bin/sh\nexit 3\n");
        let e = install_helper(&base, &[d.join("t")], &mut |_| {}).unwrap_err();
        assert!(e.contains("does not run"), "{e}");
        assert!(install_helper("file:///nonexistent-dir", &[d.join("t")], &mut |_| {}).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn progress_bar_text() {
        assert_eq!(bar(0, 100, 10), "[----------]   0%");
        assert_eq!(bar(50, 100, 10), "[#####-----]  50%");
        assert_eq!(bar(5, 0, 10), "[----------]   0%");
        assert_eq!(bar(500, 100, 10), "[##########] 100%");
    }

    #[test]
    fn dialogs_name_file_size_and_host_and_read_keys() {
        let m = Dlg::ask(Step::Model, 295_793_568);
        let text = m.view.lines.join("\n");
        assert!(text.contains("295.8 MB") && text.contains("moonshine-streaming-medium-Q8_0.gguf") && text.contains("huggingface.co"));
        let h = Dlg::ask(Step::Install, 0).view.lines.join("\n");
        assert!(h.contains("wi-audio-macos-arm64.tar.gz") && h.contains("github.com/fahadsuhaib/wi-cli") && h.contains("2 MB"));
        assert_eq!(m.key(b"y", 0), Key::Yes);
        assert_eq!(m.key(b"\r", 0), Key::Yes);
        assert_eq!(m.key(b"N", 0), Key::No);
        assert_eq!(m.key(b"\x1b", 0), Key::No);
        assert_eq!(m.key(b"\x00", 0), Key::No);
        let o = Dlg::options(false);
        assert_eq!(o.key(b"a", 0), Key::Char('a'));
        assert_eq!(o.key(b"t", 0), Key::Char('t'));
        assert_eq!(o.key(b"r", 0), Key::Char('r'));
        assert_eq!(o.key(b"\x1b", 0), Key::No);
        let on = Dlg::on();
        assert_eq!(on.key(b"o", 0), Key::Char('o'));
        assert_eq!(on.key(b"\r", 0), Key::Yes);
        assert!(o.view.lines.len() == 4, "four actions plus Esc: automatic Enter, Test mic, Download again, Remove voice");
        assert!(o.view.lines.join(" ").contains("300 MB"));
    }

    #[test]
    fn user_facing_text_never_tells_the_user_to_run_the_helper() {
        for s in [Step::Install, Step::Model, Step::Upgrade, Step::Mic] {
            assert!(!not_now_text(s).contains("wi-audio"));
            assert!(!Dlg::ask(s, 0).view.lines.join(" ").contains("run wi-audio"));
        }
        assert!(!friendly("mic_denied", "").contains("run wi-audio"));
        assert!(Dlg::heard("hello there").view.lines[0].contains("heard: hello there"));
        assert!(Dlg::heard("").view.lines[0].contains("no words"));
    }
}
