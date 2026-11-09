//! `record` and `dictate`: capture one stretch of audio, stopped by a line on stdin, a signal,
//! the time limit, or (optionally) silence after speech.

use crate::source::{Recv, Source};
use crate::vad::{EnergyVad, FRAME, FrameVad};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct Limits {
    pub max_seconds: u32,
    /// Stop after this much silence once speech was heard (0 = off).
    pub silence_stop_ms: u32,
    pub meter: bool,
}

pub struct Recording {
    pub pcm: Vec<f32>,
    pub why: &'static str,
    pub peak: f32,
}

/// The five-step bar used on stderr: ` ▁▂▅▇`.
pub fn bar(level: f32) -> char {
    const B: [char; 5] = [' ', '▁', '▂', '▅', '▇'];
    B[((level * 4.0).round() as usize).min(4)]
}

pub fn capture(src: &Source, lim: &Limits, stop: &AtomicBool) -> Recording {
    let mut pcm: Vec<f32> = Vec::new();
    let mut vad = EnergyVad::new();
    let (mut heard, mut quiet_frames) = (false, 0u32);
    let max = lim.max_seconds as usize * 16_000;
    let started = Instant::now();
    let mut scanned = 0;
    let mut peak = 0.0f32;
    let why = loop {
        if stop.load(Ordering::Relaxed) {
            break "stopped";
        }
        match src.recv(Duration::from_millis(100)) {
            Recv::End => break "end of audio",
            Recv::Empty => {}
            Recv::Chunk(c) => {
                peak = c.iter().fold(peak, |p, x| p.max(x.abs()));
                pcm.extend(c);
            }
        }
        while pcm.len() >= scanned + FRAME {
            let frame = &pcm[scanned..scanned + FRAME];
            scanned += FRAME;
            if vad.score(frame) > 0.5 {
                heard = true;
                quiet_frames = 0;
            } else {
                quiet_frames += 1;
            }
            if lim.meter && scanned % (FRAME * 3) == 0 {
                eprint!("\r\u{25cf} rec {:02}:{:02} {}  ",started.elapsed().as_secs() / 60, started.elapsed().as_secs() % 60, bar(crate::vad::level(frame)));
            }
        }
        if lim.silence_stop_ms > 0 && heard && quiet_frames * crate::vad::FRAME_MS >= lim.silence_stop_ms {
            break "silence";
        }
        if pcm.len() >= max {
            break "max length";
        }
    };
    if lim.meter {
        eprintln!();
    }
    Recording { pcm, why, peak }
}

/// A short test recording for wi's first-use check: level events while it records. The caller
/// decides what to say about the result.
pub fn probe(src: &Source, seconds: u32, stop: &AtomicBool, out: &mut dyn crate::listen::Emit) -> Recording {
    let end = Instant::now() + Duration::from_secs(u64::from(seconds));
    let mut pcm: Vec<f32> = Vec::new();
    let mut peak = 0.0f32;
    let mut last = Instant::now() - Duration::from_millis(100);
    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
        match src.recv(Duration::from_millis(100)) {
            Recv::End => break,
            Recv::Empty => {}
            Recv::Chunk(c) => {
                peak = c.iter().fold(peak, |p, x| p.max(x.abs()));
                pcm.extend(c);
            }
        }
        if last.elapsed() >= Duration::from_millis(100) && pcm.len() >= FRAME {
            last = Instant::now();
            out.emit(serde_json::json!({"ev":"level","v":crate::vad::level(&pcm[pcm.len() - FRAME..])}));
        }
    }
    Recording { pcm, why: "done", peak }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(parts: Vec<Vec<f32>>) -> Source {
        let (tx, rx) = std::sync::mpsc::sync_channel(10_000);
        for p in parts {
            for c in p.chunks(1600) {
                tx.send(c.to_vec()).unwrap();
            }
        }
        Source::for_test(rx)
    }

    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * 16000.0) as usize).map(|i| ((i as f32) * 0.3).sin() * 0.3).collect()
    }

    #[test]
    fn stops_at_max_seconds() {
        let s = src(vec![tone(10.0)]);
        let r = capture(&s, &Limits { max_seconds: 3, silence_stop_ms: 0, meter: false }, &AtomicBool::new(false));
        assert_eq!(r.why, "max length");
        assert!(r.pcm.len() >= 48_000 && r.pcm.len() < 48_000 + 1600);
    }

    #[test]
    fn stops_after_silence_following_speech() {
        let s = src(vec![vec![0.0; 8000], tone(1.0), vec![0.0; 32000]]);
        let r = capture(&s, &Limits { max_seconds: 30, silence_stop_ms: 1000, meter: false }, &AtomicBool::new(false));
        assert_eq!(r.why, "silence");
        assert!(r.pcm.len() < 8000 + 16000 + 24000, "stopped near 1 s after speech, got {}", r.pcm.len());
    }

    #[test]
    fn leading_silence_alone_does_not_stop() {
        let s = src(vec![vec![0.0; 48000]]);
        let r = capture(&s, &Limits { max_seconds: 30, silence_stop_ms: 500, meter: false }, &AtomicBool::new(false));
        assert_eq!(r.why, "end of audio");
        assert_eq!(r.peak, 0.0);
    }

    #[test]
    fn probe_reports_levels_and_returns_what_it_heard() {
        struct C(Vec<serde_json::Value>);
        impl crate::listen::Emit for C {
            fn emit(&mut self, v: serde_json::Value) {
                self.0.push(v);
            }
        }
        let s = src(vec![tone(0.5)]);
        let mut c = C(vec![]);
        let r = probe(&s, 3, &AtomicBool::new(false), &mut c);
        assert!(r.peak > 0.2 && r.pcm.len() == 8000);
        assert!(!c.0.is_empty() && c.0.iter().all(|e| e["ev"] == "level"));
    }

    #[test]
    fn meter_bar_characters() {
        assert_eq!(bar(0.0), ' ');
        assert_eq!(bar(1.0), '▇');
    }
}
