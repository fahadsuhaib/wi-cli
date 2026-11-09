//! wi-audio: the voice side of wi. A separate program so that wi itself never links a speech
//! model or an audio library. Everything stays on this machine: no network except the model
//! download you confirm, no telemetry.

mod doctor;
mod engine;
mod listen;
mod models;
mod record;
mod source;
mod text;
mod vad;
mod wav;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn install_signals() {
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        for s in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::signal(s, on_signal as *const () as libc::sighandler_t);
        }
    }
}

const HELP: &str = "wi-audio: local voice input for wi (audio never leaves this machine)

usage:
  wi-audio listen [options]       hands-free: report each spoken phrase as a JSON line (what wi runs)
  wi-audio dictate [options]      record once, print the text on stdout
  wi-audio record [options]       record once into a wav file (default: a private temp file, path printed)
  wi-audio transcribe <file.wav>  print the text of a wav file
  wi-audio doctor [--verify]      what is installed, microphone access, a test recording
  wi-audio models install [--tiny] [--yes]   download the speech model after a confirmation
  wi-audio models list | remove
  wi-audio --version

options:
  --max-seconds N       longest recording (default 120)
  --silence-stop-ms N   stop after this much silence once speech was heard (dictate, record)
  --out FILE            record: where to write the wav
  --device NAME         microphone (part of its name)
  --end-silence-ms N    listen: silence that ends a phrase (default 700)
  --min-speech-ms N     listen: speech that starts a phrase (default 250)
  --vad-threshold X     listen: speech score from 0 to 1 (default 0.5)
  --idle-secs N         listen: stop after this long without speech (default 600, 0 = never)

test hooks: WICLI_AUDIO_FILE=<wav> replaces the microphone, WICLI_AUDIO_SPEED=<x> paces it
(1 = real time, 0 = as fast as possible), WICLI_AUDIO_MODELS=<dir>, WICLI_AUDIO_MODEL=<gguf>.
Stop with Ctrl-C, SIGTERM, or (listen, dictate, record) by closing stdin / sending a line.
";

#[derive(Default)]
struct Args {
    pos: Vec<String>,
    flags: std::collections::HashMap<String, String>,
    switches: Vec<String>,
}

impl Args {
    fn parse(v: &[String]) -> Args {
        let mut a = Args::default();
        let mut i = 0;
        while i < v.len() {
            let s = &v[i];
            if let Some(name) = s.strip_prefix("--") {
                if matches!(name, "yes" | "tiny" | "verify" | "quiet") {
                    a.switches.push(name.to_string());
                } else if i + 1 < v.len() {
                    a.flags.insert(name.to_string(), v[i + 1].clone());
                    i += 1;
                } else {
                    a.switches.push(name.to_string());
                }
            } else {
                a.pos.push(s.clone());
            }
            i += 1;
        }
        a
    }
    fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> T {
        self.flags.get(k).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    fn has(&self, k: &str) -> bool {
        self.switches.iter().any(|s| s == k)
    }
}

/// A line (or end) on stdin means "stop" for the commands that run until told otherwise.
/// `eof_stops`: for `listen`, whose parent (wi) closes our stdin when it turns the mode off or
/// goes away. The one-shot commands ignore EOF so `< /dev/null` and cron do not stop them at once.
fn watch_stdin(eof_stops: bool) {
    std::thread::spawn(move || {
        let mut line = String::new();
        let n = std::io::stdin().read_line(&mut line).unwrap_or(0);
        if n > 0 || eof_stops {
            STOP.store(true, Ordering::Relaxed);
        }
    });
}

fn fail(msg: &str) -> ! {
    eprintln!("wi-audio: {msg}");
    std::process::exit(1);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first().map(String::as_str) else {
        print!("{HELP}");
        return;
    };
    let args = Args::parse(&argv[1..]);
    install_signals();
    match cmd {
        "--version" | "-V" => println!("wi-audio {}", env!("CARGO_PKG_VERSION")),
        "--help" | "-h" | "help" => print!("{HELP}"),
        "listen" => cmd_listen(&args),
        "dictate" => cmd_dictate(&args),
        "record" => cmd_record(&args),
        "transcribe" => cmd_transcribe(&args),
        "doctor" => doctor::run(args.has("verify")),
        "models" => cmd_models(&args),
        other => fail(&format!("unknown command '{other}' (try: wi-audio --help)")),
    }
}

fn load_engine() -> engine::Moonshine {
    let Some(path) = models::speech_model() else {
        fail(&format!("no speech model installed. Install it with: wi-audio models install ({} download from {})", models::mb(models::SMALL.bytes), models::SMALL.host()));
    };
    engine::Moonshine::load(&path).unwrap_or_else(|e| fail(&e))
}

fn cmd_listen(a: &Args) {
    watch_stdin(true);
    let device = a.flags.get("device").cloned();
    let mut out = listen::Stdout;
    use listen::Emit;
    out.emit(serde_json::json!({"ev":"state","s":"loading"}));
    let mut engine = {
        let Some(path) = models::speech_model() else {
            out.emit(serde_json::json!({"ev":"error","kind":"no_model","msg":"no speech model installed: run wi-audio models install"}));
            std::process::exit(2);
        };
        match engine::Moonshine::load(&path) {
            Ok(e) => e,
            Err(e) => {
                out.emit(serde_json::json!({"ev":"error","kind":"engine","msg":e}));
                std::process::exit(2);
            }
        }
    };
    let mut vad: Box<dyn vad::FrameVad> = match models::vad_model().map(|p| vad::SileroVad::load(&p)) {
        Some(Ok(v)) => Box::new(v),
        _ => Box::new(vad::EnergyVad::new()),
    };
    let src = match source::open(device.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            let kind = if e.contains("denied") { "mic_denied" } else if e.contains("no microphone") { "no_mic" } else { "mic" };
            out.emit(serde_json::json!({"ev":"error","kind":kind,"msg":e}));
            std::process::exit(2);
        }
    };
    let opts = listen::Opts {
        end_silence_ms: a.num("end-silence-ms", 700),
        min_speech_ms: a.num("min-speech-ms", 250),
        threshold: a.num("vad-threshold", 0.5),
        idle: Duration::from_secs(a.num("idle-secs", 600)),
        max_phrase_s: 30,
    };
    let why = listen::run(&src, vad.as_mut(), &mut engine, &opts, &STOP, &mut out);
    drop(src);
    out.emit(serde_json::json!({"ev":"off","why":why}));
}

fn limits(a: &Args) -> record::Limits {
    record::Limits { max_seconds: a.num("max-seconds", 120), silence_stop_ms: a.num("silence-stop-ms", 0), meter: !a.has("quiet") }
}

fn open_source(a: &Args) -> source::Source {
    source::open(a.flags.get("device").map(String::as_str)).unwrap_or_else(|e| fail(&e))
}

fn cmd_record(a: &Args) {
    watch_stdin(false);
    let src = open_source(a);
    let rec = record::capture(&src, &limits(a), &STOP);
    drop(src);
    let path = a.flags.get("out").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(format!("wi-audio-{}.wav", std::process::id())));
    wav::write_wav(&path, &rec.pcm).unwrap_or_else(|e| fail(&e));
    eprintln!("recorded {:.1} s ({})", rec.pcm.len() as f32 / 16_000.0, rec.why);
    println!("{}", path.display());
}

fn cmd_dictate(a: &Args) {
    watch_stdin(false);
    let mut engine = load_engine();
    let src = open_source(a);
    let rec = record::capture(&src, &limits(a), &STOP);
    drop(src);
    if rec.peak == 0.0 && source::test_file().is_none() {
        fail(source::MIC_DENIED);
    }
    if rec.pcm.is_empty() {
        fail("nothing was recorded");
    }
    let text = engine::transcribe_long(&mut engine, &rec.pcm).unwrap_or_else(|e| fail(&e));
    if text::is_noise(&text) {
        eprintln!("wi-audio: heard only noise");
        std::process::exit(3);
    }
    println!("{text}");
}

fn cmd_transcribe(a: &Args) {
    let Some(file) = a.pos.first() else { fail("usage: wi-audio transcribe <file.wav>") };
    let pcm = wav::read_wav(std::path::Path::new(file)).unwrap_or_else(|e| fail(&e));
    let mut engine = load_engine();
    let text = engine::transcribe_long(&mut engine, &pcm).unwrap_or_else(|e| fail(&e));
    println!("{text}");
}

fn cmd_models(a: &Args) {
    match a.pos.first().map(String::as_str) {
        Some("install") => {
            let speech = if a.has("tiny") { &models::TINY } else { &models::SMALL };
            for m in [speech, &models::VAD] {
                match models::install(m, a.has("yes")) {
                    Ok(true) => {}
                    Ok(false) => std::process::exit(1),
                    Err(e) => fail(&e),
                }
            }
        }
        Some("list") | None => {
            println!("models folder: {}", models::dir().display());
            for m in models::ALL {
                println!("  {:<6} {:<40} {:>9}  {}", m.id, m.file, models::mb(m.bytes), if m.present() { "installed" } else { "not installed" });
            }
        }
        Some("remove") => match models::remove_all() {
            Ok(n) => println!("removed {n} file(s) from {}", models::dir().display()),
            Err(e) => fail(&e),
        },
        Some(o) => fail(&format!("unknown models command '{o}' (install, list, remove)")),
    }
}
