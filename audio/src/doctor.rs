//! `wi-audio doctor`: what works, what is missing, and the one command that fixes it.

use crate::{engine, models, record, source, text};
use engine::Transcriber;
use std::io::{BufRead, IsTerminal, Write};
use std::sync::atomic::AtomicBool;

pub fn run(verify: bool) {
    println!("wi-audio {}", env!("CARGO_PKG_VERSION"));
    println!("Audio stays on this machine. Nothing is sent anywhere; the only download is the model you confirm.\n");
    let mut problems: Vec<String> = Vec::new();

    println!("Models in {}", models::dir().display());
    for m in models::ALL {
        let state = if m.present() { "installed" } else { "not installed" };
        println!("  {:<6} {:<38} {:>9}  {state}", m.id, m.file, models::mb(m.bytes));
        if verify && m.present() {
            match models::sha256_file(&m.path()) {
                Ok(h) if h == m.sha256 => println!("         SHA256 ok"),
                Ok(_) => problems.push(format!("{} does not match its SHA256: delete it and run: wi-audio models install", m.file)),
                Err(e) => problems.push(e),
            }
        }
    }
    let speech = models::speech_model();
    if speech.is_none() {
        problems.push(format!("no speech model: run  wi-audio models install  ({} from {})", models::mb(models::SMALL.bytes), models::SMALL.host()));
    }
    if models::vad_model().is_none() {
        println!("  (no VAD file: phrase detection falls back to a simple loudness gate; wi-audio models install adds it)");
    }

    println!("\nMicrophones");
    let names = source::device_names();
    if names.is_empty() {
        println!("  none found");
        problems.push("no microphone found".into());
    }
    for (i, n) in names.iter().enumerate() {
        println!("  {}{n}", if i == 0 { "* " } else { "  " });
    }

    let can_test = !names.is_empty() && (std::io::stdin().is_terminal() || source::test_file().is_some());
    if can_test {
        println!("\nTest recording: speak for 3 seconds after pressing Enter.");
        if source::test_file().is_none() {
            print!("Press Enter to start (Ctrl-C to skip) ");
            let _ = std::io::stdout().flush();
            let mut l = String::new();
            let _ = std::io::stdin().lock().read_line(&mut l);
        }
        match source::open(None) {
            Err(e) => problems.push(e),
            Ok(src) => {
                let rec = record::capture(&src, &record::Limits { max_seconds: 3, silence_stop_ms: 0, meter: true }, &AtomicBool::new(false));
                if rec.peak == 0.0 && source::test_file().is_none() {
                    problems.push(source::MIC_DENIED.into());
                } else {
                    println!("  heard a peak level of {:.0}%", rec.peak * 100.0);
                    if let Some(p) = &speech {
                        match engine::Moonshine::load(p).and_then(|mut e| e.transcribe(&rec.pcm)) {
                            Ok(t) => {
                                let t = text::clean(&t);
                                println!("  heard: {}", if t.is_empty() { "(nothing)" } else { &t });
                            }
                            Err(e) => problems.push(e),
                        }
                    }
                }
            }
        }
    }

    println!();
    if problems.is_empty() {
        println!("All good. In wi, open the menu (Ctrl-Space) and choose Dictation.");
    } else {
        println!("To fix:");
        for p in &problems {
            println!("  - {p}");
        }
        std::process::exit(1);
    }
}
