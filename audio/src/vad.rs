//! Phrase detection: a per-frame speech score (Silero VAD, or an energy gate when the VAD file is
//! missing) and a small state machine that turns scores into "speech started" / "speech ended".

/// Samples per frame (32 ms at 16 kHz): what Silero VAD scores at a time.
pub const FRAME: usize = 512;
pub const FRAME_MS: u32 = 32;

pub trait FrameVad {
    /// Speech probability (0..1) of one frame of `FRAME` samples.
    fn score(&mut self, frame: &[f32]) -> f32;
}

pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Loudness 0..1 for a level meter: -60 dBFS is 0, -10 dBFS is 1.
pub fn level(frame: &[f32]) -> f32 {
    let r = rms(frame);
    if r <= 1e-6 {
        return 0.0;
    }
    ((20.0 * r.log10() + 60.0) / 50.0).clamp(0.0, 1.0)
}

/// Energy gate with an adaptive noise floor: a frame is speech when it is clearly louder than
/// the quiet frames seen so far. Good enough in a quiet room; Silero is far better in noise.
pub struct EnergyVad {
    floor: f32,
}

impl EnergyVad {
    pub fn new() -> Self {
        Self { floor: 0.002 }
    }
}

impl FrameVad for EnergyVad {
    fn score(&mut self, frame: &[f32]) -> f32 {
        let r = rms(frame);
        let on = r > (self.floor * 3.0).max(0.01);
        if !on {
            self.floor = self.floor * 0.95 + r * 0.05;
        }
        if on { 1.0 } else { 0.0 }
    }
}

/// Silero VAD through the same runtime as the speech model.
pub struct SileroVad {
    session: transcribe_cpp::VadSession,
    last: f32,
}

impl SileroVad {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let model = transcribe_cpp::Model::load(path).map_err(|e| format!("cannot load the VAD model: {e}"))?;
        let session = model.vad_session().map_err(|e| format!("cannot start the VAD: {e}"))?;
        Ok(Self { session, last: 0.0 })
    }
}

impl FrameVad for SileroVad {
    fn score(&mut self, frame: &[f32]) -> f32 {
        if let Ok(r) = self.session.stream_feed(frame)
            && let Some(p) = r.probs.last()
        {
            self.last = *p;
        }
        self.last
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Seg {
    /// Enough speech in a row: a phrase has begun.
    Start,
    /// Enough silence after speech: the phrase is over.
    End,
}

/// Turns frame scores into phrase boundaries: a phrase starts after `min_speech` frames of
/// speech in a row and ends after `end_silence` frames without any.
pub struct Segmenter {
    threshold: f32,
    min_speech: u32,
    end_silence: u32,
    speech_run: u32,
    silence_run: u32,
    pub in_phrase: bool,
}

impl Segmenter {
    pub fn new(threshold: f32, min_speech_ms: u32, end_silence_ms: u32) -> Self {
        Self {
            threshold,
            min_speech: (min_speech_ms / FRAME_MS).max(1),
            end_silence: (end_silence_ms / FRAME_MS).max(1),
            speech_run: 0,
            silence_run: 0,
            in_phrase: false,
        }
    }

    pub fn push(&mut self, score: f32) -> Option<Seg> {
        let speech = score >= self.threshold;
        if speech {
            self.silence_run = 0;
            self.speech_run += 1;
            if !self.in_phrase && self.speech_run >= self.min_speech {
                self.in_phrase = true;
                return Some(Seg::Start);
            }
        } else if self.in_phrase {
            self.silence_run += 1;
            if self.silence_run >= self.end_silence {
                self.reset();
                return Some(Seg::End);
            }
        } else {
            self.speech_run = 0;
        }
        None
    }

    /// Back to waiting for speech (after a phrase or a forced cut).
    pub fn reset(&mut self) {
        self.in_phrase = false;
        self.speech_run = 0;
        self.silence_run = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| ((i as f32) * 0.3).sin() * amp).collect()
    }

    #[test]
    fn silence_is_not_speech_and_speech_is() {
        let mut v = EnergyVad::new();
        for _ in 0..20 {
            assert_eq!(v.score(&vec![0.0; FRAME]), 0.0);
        }
        assert_eq!(v.score(&tone(FRAME, 0.3)), 1.0);
    }

    #[test]
    fn energy_gate_ignores_steady_low_noise() {
        let mut v = EnergyVad::new();
        let noise = tone(FRAME, 0.004);
        for _ in 0..50 {
            assert_eq!(v.score(&noise), 0.0);
        }
    }

    #[test]
    fn segmenter_finds_two_phrases() {
        let mut s = Segmenter::new(0.5, 96, 320); // 3 frames to start, 10 to end
        let mut events = Vec::new();
        let script: Vec<f32> = [vec![0.0; 5], vec![1.0; 20], vec![0.0; 4], vec![1.0; 6], vec![0.0; 12], vec![1.0; 2], vec![0.0; 12]]
            .concat();
        for (i, p) in script.into_iter().enumerate() {
            if let Some(e) = s.push(p) {
                events.push((i, e));
            }
        }
        // First phrase starts on its 3rd speech frame; the 4-frame gap is bridged; it ends 10 frames
        // of silence after the second burst (frames 35..=44). The 2-frame blip never starts a phrase.
        assert_eq!(events, vec![(7, Seg::Start), (44, Seg::End)]);
    }

    #[test]
    fn short_blips_never_start_a_phrase() {
        let mut s = Segmenter::new(0.5, 250, 700);
        for _ in 0..10 {
            assert_eq!(s.push(1.0), None); // 7 frames needed, only 1 at a time
            assert_eq!(s.push(0.0), None);
        }
    }

    #[test]
    fn level_meter_range() {
        assert_eq!(level(&vec![0.0; 100]), 0.0);
        assert!(level(&tone(512, 0.5)) > 0.8);
        assert!(level(&tone(512, 0.001)) < 0.3);
    }
}
