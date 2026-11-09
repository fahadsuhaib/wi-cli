//! The speech model (Moonshine Streaming through transcribe.cpp, CPU only).

pub trait Transcriber {
    /// 16 kHz mono samples to raw text.
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String, String>;
}

pub struct Moonshine {
    session: transcribe_cpp::Session,
    // Keeps the weights alive for the session.
    _model: transcribe_cpp::Model,
}

impl Moonshine {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let _ = transcribe_cpp::init_backends_default();
        let opts = transcribe_cpp::ModelOptions { backend: transcribe_cpp::Backend::Cpu, ..Default::default() };
        let model = transcribe_cpp::Model::load_with(path, &opts).map_err(|e| format!("cannot load the speech model {}: {e}", path.display()))?;
        let session = model.session().map_err(|e| format!("cannot start the speech model: {e}"))?;
        Ok(Self { session, _model: model })
    }
}

impl Transcriber for Moonshine {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String, String> {
        // The model wants a little room around very short clips.
        let mut padded;
        let pcm = if pcm.len() < 8000 {
            padded = pcm.to_vec();
            padded.resize(8000, 0.0);
            &padded[..]
        } else {
            pcm
        };
        self.session
            .run(pcm, &transcribe_cpp::RunOptions::default())
            .map(|t| t.text)
            .map_err(|e| format!("transcription failed: {e}"))
    }
}

/// Longest stretch handed to the model in one go (seconds). Longer recordings are cut.
pub const CHUNK_SECONDS: usize = 30;

/// Transcribe a long clip in pieces, preferring to cut at the quietest point near the limit.
pub fn transcribe_long(t: &mut dyn Transcriber, pcm: &[f32]) -> Result<String, String> {
    let max = CHUNK_SECONDS * crate::wav::RATE as usize;
    let mut parts = Vec::new();
    let mut start = 0;
    while start < pcm.len() {
        let mut end = (start + max).min(pcm.len());
        if end < pcm.len() {
            end = quietest_cut(pcm, start + max * 2 / 3, end);
        }
        let text = t.transcribe(&pcm[start..end])?;
        parts.push(crate::text::clean(&text));
        start = end;
    }
    parts.retain(|p| !p.is_empty());
    Ok(parts.join(" "))
}

/// The sample index in `lo..hi` with the lowest short-term energy.
fn quietest_cut(pcm: &[f32], lo: usize, hi: usize) -> usize {
    let win = 800; // 50 ms
    let (mut best, mut best_e) = (hi, f32::MAX);
    let mut i = lo;
    while i + win <= hi {
        let e: f32 = pcm[i..i + win].iter().map(|x| x * x).sum();
        if e < best_e {
            best_e = e;
            best = i + win / 2;
        }
        i += win / 2;
    }
    best
}

#[cfg(test)]
pub struct Fake(pub Vec<String>, pub usize);

#[cfg(test)]
impl Transcriber for Fake {
    fn transcribe(&mut self, _pcm: &[f32]) -> Result<String, String> {
        let t = self.0.get(self.1).cloned().unwrap_or_default();
        self.1 += 1;
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_audio_is_cut_into_chunks_and_joined() {
        let pcm = vec![0.1f32; 16000 * 70];
        let mut f = Fake(vec!["one.".into(), " two.".into(), "three.".into()], 0);
        assert_eq!(transcribe_long(&mut f, &pcm).unwrap(), "one. two. three.");
        assert_eq!(f.1, 3);
    }

    #[test]
    fn cut_lands_in_the_quiet_part() {
        let mut pcm = vec![0.5f32; 16000 * 45];
        for s in &mut pcm[16000 * 26..16000 * 26 + 4000] {
            *s = 0.0;
        }
        let max = CHUNK_SECONDS * 16000;
        let cut = quietest_cut(&pcm, max * 2 / 3, max);
        assert!((16000 * 26..16000 * 26 + 4000).contains(&cut), "cut {cut}");
    }
}
