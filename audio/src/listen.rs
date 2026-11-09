//! `wi-audio listen`: hands-free listening. Audio flows from the source into a frame loop; the
//! VAD finds phrases; each finished phrase is transcribed and reported as a JSON line.
//!
//! Events on stdout (one JSON object per line):
//!   {"ev":"state","s":"loading"|"listening"|"hearing"|"transcribing"}
//!   {"ev":"level","v":0.4}
//!   {"ev":"final","text":"...","ms":420}
//!   {"ev":"error","kind":"...","msg":"..."}
//!   {"ev":"off","why":"idle"|"end of audio"|"stopped"}

use crate::engine::Transcriber;
use crate::source::{Recv, Source};
use crate::vad::{FRAME, FrameVad, Segmenter, Seg};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct Opts {
    pub end_silence_ms: u32,
    pub min_speech_ms: u32,
    pub threshold: f32,
    /// Stop after this long without speech (0 = never).
    pub idle: Duration,
    /// A phrase is cut after this many seconds of continuous speech.
    pub max_phrase_s: u32,
}

impl Default for Opts {
    fn default() -> Self {
        Self { end_silence_ms: 700, min_speech_ms: 250, threshold: 0.5, idle: Duration::from_secs(600), max_phrase_s: 30 }
    }
}

pub trait Emit {
    fn emit(&mut self, v: serde_json::Value);
}

/// Prints each event as one flushed line on stdout.
pub struct Stdout;

impl Emit for Stdout {
    fn emit(&mut self, v: serde_json::Value) {
        use std::io::Write;
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{v}");
        let _ = o.flush();
    }
}

/// Frames kept before the detected start so the first word is not clipped.
const PRE_ROLL: usize = 10;
const NO_AUDIO_SECS: u64 = 3;
const DEAD_MIC_SECS: u64 = 2;

pub fn run(
    src: &Source,
    vad: &mut dyn FrameVad,
    engine: &mut dyn Transcriber,
    opts: &Opts,
    stop: &AtomicBool,
    out: &mut dyn Emit,
) -> &'static str {
    let mut seg = Segmenter::new(opts.threshold, opts.min_speech_ms, opts.end_silence_ms);
    let mut buf: Vec<f32> = Vec::new();
    let mut pre: VecDeque<Vec<f32>> = VecDeque::new();
    let mut phrase: Vec<f32> = Vec::new();
    let max_samples = opts.max_phrase_s as usize * 16_000;
    let mut last_speech = Instant::now();
    let mut last_audio = Instant::now();
    let mut peak_all = 0.0f32;
    let started = Instant::now();
    let mut silent_warned = false;
    let mut level_acc = 0.0f32;
    let mut level_frames = 0;
    out.emit(serde_json::json!({"ev":"state","s":"listening"}));
    loop {
        if stop.load(Ordering::Relaxed) {
            return "stopped";
        }
        if !opts.idle.is_zero() && last_speech.elapsed() >= opts.idle && !seg.in_phrase {
            return "idle";
        }
        match src.recv(Duration::from_millis(100)) {
            Recv::End => break,
            Recv::Empty => {
                if last_audio.elapsed() > Duration::from_secs(DEAD_MIC_SECS) && crate::source::test_file().is_none() && !silent_warned {
                    silent_warned = true;
                    out.emit(serde_json::json!({"ev":"error","kind":"mic_stalled","msg":"the microphone stopped delivering audio"}));
                }
                continue;
            }
            Recv::Chunk(c) => {
                last_audio = Instant::now();
                peak_all = c.iter().fold(peak_all, |p, x| p.max(x.abs()));
                buf.extend(c);
            }
        }
        if !silent_warned && peak_all == 0.0 && started.elapsed() > Duration::from_secs(NO_AUDIO_SECS) && crate::source::test_file().is_none() {
            silent_warned = true;
            out.emit(serde_json::json!({"ev":"error","kind":"mic_silent","msg":"no sound from the microphone: allow your terminal app in System Settings > Privacy & Security > Microphone"}));
        }
        while buf.len() >= FRAME {
            let frame: Vec<f32> = buf.drain(..FRAME).collect();
            level_acc = level_acc.max(crate::vad::level(&frame));
            level_frames += 1;
            if level_frames >= 3 {
                out.emit(serde_json::json!({"ev":"level","v":(level_acc * 100.0).round() / 100.0}));
                level_acc = 0.0;
                level_frames = 0;
            }
            let score = vad.score(&frame);
            let was_in = seg.in_phrase;
            let ev = seg.push(score);
            if ev == Some(Seg::Start) {
                last_speech = Instant::now();
                out.emit(serde_json::json!({"ev":"state","s":"hearing"}));
                for f in pre.drain(..) {
                    phrase.extend(f);
                }
            }
            if seg.in_phrase || ev == Some(Seg::End) {
                phrase.extend_from_slice(&frame);
                if score >= opts.threshold {
                    last_speech = Instant::now();
                }
            } else {
                pre.push_back(frame);
                if pre.len() > PRE_ROLL {
                    pre.pop_front();
                }
            }
            let forced = was_in && seg.in_phrase && phrase.len() >= max_samples;
            if ev == Some(Seg::End) || forced {
                if forced {
                    seg.reset();
                }
                finish(&mut phrase, engine, out);
                out.emit(serde_json::json!({"ev":"state","s":"listening"}));
                last_speech = Instant::now();
            }
        }
    }
    // The audio is over (a file): a phrase still open is finished first.
    if !phrase.is_empty() {
        finish(&mut phrase, engine, out);
    }
    "end of audio"
}

fn finish(phrase: &mut Vec<f32>, engine: &mut dyn Transcriber, out: &mut dyn Emit) {
    let pcm = std::mem::take(phrase);
    if pcm.len() < FRAME * 4 {
        return;
    }
    out.emit(serde_json::json!({"ev":"state","s":"transcribing"}));
    let t = Instant::now();
    match engine.transcribe(&pcm) {
        Ok(raw) => {
            let text = crate::text::clean(&raw);
            if text.is_empty() || crate::text::is_noise(&text) {
                return;
            }
            out.emit(serde_json::json!({"ev":"final","text":text,"ms":t.elapsed().as_millis() as u64,"audio_ms":(pcm.len() as u64 * 1000 / 16_000)}));
        }
        Err(e) => out.emit(serde_json::json!({"ev":"error","kind":"engine","msg":e})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Fake;
    use crate::vad::EnergyVad;

    struct Collect(Vec<serde_json::Value>);
    impl Emit for Collect {
        fn emit(&mut self, v: serde_json::Value) {
            self.0.push(v);
        }
    }

    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * 16000.0) as usize).map(|i| ((i as f32) * 0.3).sin() * 0.3).collect()
    }

    fn feed(parts: &[Vec<f32>]) -> Source {
        let (tx, rx) = std::sync::mpsc::sync_channel(10_000);
        for p in parts {
            for c in p.chunks(1600) {
                tx.send(c.to_vec()).unwrap();
            }
        }
        Source::for_test(rx)
    }

    fn finals(ev: &[serde_json::Value]) -> Vec<String> {
        ev.iter().filter(|e| e["ev"] == "final").map(|e| e["text"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn three_phrases_make_three_finals() {
        let sil = vec![0.0; 16000];
        let src = feed(&[sil.clone(), tone(1.5), sil.clone(), tone(2.0), sil.clone(), tone(1.0), sil.clone()]);
        let mut eng = Fake(vec!["first phrase.".into(), "second phrase.".into(), "third.".into()], 0);
        let mut out = Collect(vec![]);
        let why = run(&src, &mut EnergyVad::new(), &mut eng, &Opts::default(), &AtomicBool::new(false), &mut out);
        assert_eq!(why, "end of audio");
        assert_eq!(finals(&out.0), vec!["first phrase.", "second phrase.", "third."]);
        let states: Vec<&str> = out.0.iter().filter(|e| e["ev"] == "state").map(|e| e["s"].as_str().unwrap()).collect();
        assert_eq!(states[0], "listening");
        assert!(states.contains(&"hearing") && states.contains(&"transcribing"));
    }

    #[test]
    fn silence_only_emits_no_final() {
        let src = feed(&[vec![0.0; 16000 * 5]]);
        let mut eng = Fake(vec!["should not be asked".into()], 0);
        let mut out = Collect(vec![]);
        run(&src, &mut EnergyVad::new(), &mut eng, &Opts::default(), &AtomicBool::new(false), &mut out);
        assert!(finals(&out.0).is_empty());
        assert_eq!(eng.1, 0, "the engine is never called for silence");
    }

    #[test]
    fn noise_hallucination_is_dropped() {
        let src = feed(&[vec![0.0; 8000], tone(1.0), vec![0.0; 16000]]);
        let mut eng = Fake(vec!["Thank you.".into()], 0);
        let mut out = Collect(vec![]);
        run(&src, &mut EnergyVad::new(), &mut eng, &Opts::default(), &AtomicBool::new(false), &mut out);
        assert!(finals(&out.0).is_empty());
    }

    #[test]
    fn long_speech_is_cut_at_max_phrase() {
        let src = feed(&[vec![0.0; 8000], tone(7.0), vec![0.0; 16000]]);
        let mut eng = Fake(vec!["part one".into(), "part two".into()], 0);
        let mut out = Collect(vec![]);
        let o = Opts { max_phrase_s: 3, ..Opts::default() };
        run(&src, &mut EnergyVad::new(), &mut eng, &o, &AtomicBool::new(false), &mut out);
        assert!(finals(&out.0).len() >= 2, "{:?}", finals(&out.0));
    }

    #[test]
    fn stop_flag_ends_the_loop() {
        let src = feed(&[tone(1.0)]);
        let mut eng = Fake(vec![], 0);
        let mut out = Collect(vec![]);
        let why = run(&src, &mut EnergyVad::new(), &mut eng, &Opts::default(), &AtomicBool::new(true), &mut out);
        assert_eq!(why, "stopped");
    }
}
