//! 16 kHz mono audio: reading any wav, writing the 16-bit file engines expect, and resampling.

use std::io::Write;
use std::path::Path;

pub const RATE: u32 = 16_000;

/// Average interleaved channels down to one.
pub fn to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect()
}

/// A resampler that works on a stream: every output sample is the average of the input it
/// covers (a box filter), so speech down to 16 kHz keeps its level and aliasing stays small.
/// Upsampling just repeats samples, which never happens with real microphones.
pub struct Resampler {
    step: f64,
    filled: f64,
    sum: f64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { step: f64::from(from) / f64::from(to), filled: 0.0, sum: 0.0 }
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &x in input {
            let mut left = 1.0f64;
            while left > 1e-9 {
                let take = left.min(self.step - self.filled);
                self.sum += f64::from(x) * take;
                self.filled += take;
                left -= take;
                if self.filled >= self.step - 1e-9 {
                    out.push((self.sum / self.step) as f32);
                    self.sum = 0.0;
                    self.filled = 0.0;
                }
            }
        }
    }
}

/// Resample a whole clip.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    let mut r = Resampler::new(from, to);
    let mut out = Vec::with_capacity((input.len() as u64 * u64::from(to) / u64::from(from)) as usize + 1);
    r.push(input, &mut out);
    out
}

/// Read a wav file (16-bit, 24/32-bit int or float, any rate and channel count) as 16 kHz mono.
pub fn read_wav(path: &Path) -> Result<Vec<f32>, String> {
    let mut rd = hound::WavReader::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let spec = rd.spec();
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => rd.samples::<f32>().collect::<Result<_, _>>().map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            rd.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>().map_err(|e| e.to_string())?
        }
    };
    let mono = to_mono(&raw, usize::from(spec.channels.max(1)));
    Ok(resample(&mono, spec.sample_rate, RATE))
}

/// The 44-byte header of a 16-bit mono 16 kHz PCM wav holding `samples` samples.
pub fn header(samples: usize) -> [u8; 44] {
    let data = (samples * 2) as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&RATE.to_le_bytes());
    h[28..32].copy_from_slice(&(RATE * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data.to_le_bytes());
    h
}

pub fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Write `pcm` as a 16-bit mono 16 kHz wav. The file is created private (0600) because it is a
/// recording of the user's voice.
pub fn write_wav(path: &Path, pcm: &[f32]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut bytes = Vec::with_capacity(44 + pcm.len() * 2);
    bytes.extend_from_slice(&header(pcm.len()));
    for &s in pcm {
        bytes.extend_from_slice(&to_i16(s).to_le_bytes());
    }
    f.write_all(&bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_describes_16k_mono_16bit() {
        let h = header(8000);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 36 + 16000);
        assert_eq!(&h[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(h[20..22].try_into().unwrap()), 1, "PCM");
        assert_eq!(u16::from_le_bytes(h[22..24].try_into().unwrap()), 1, "mono");
        assert_eq!(u32::from_le_bytes(h[24..28].try_into().unwrap()), 16000);
        assert_eq!(u16::from_le_bytes(h[34..36].try_into().unwrap()), 16);
        assert_eq!(&h[36..40], b"data");
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 16000);
    }

    #[test]
    fn written_file_reads_back_private_and_equal() {
        let dir = std::env::temp_dir().join(format!("wi-audio-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.wav");
        let pcm: Vec<f32> = (0..1600).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        write_wav(&p, &pcm).unwrap();
        let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&p).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 44 + 3200);
        let back = read_wav(&p).unwrap();
        assert_eq!(back.len(), pcm.len());
        assert!(back.iter().zip(&pcm).all(|(a, b)| (a - b).abs() < 1e-3));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn resample_48k_to_16k_keeps_length_and_level() {
        let input = vec![0.25f32; 4800];
        let out = resample(&input, 48000, 16000);
        assert_eq!(out.len(), 1600);
        assert!(out.iter().all(|x| (x - 0.25).abs() < 1e-5));
    }

    #[test]
    fn resample_44k1_streaming_matches_batch() {
        let input: Vec<f32> = (0..44100).map(|i| ((i as f32) * 0.01).sin()).collect();
        let whole = resample(&input, 44100, 16000);
        let mut r = Resampler::new(44100, 16000);
        let mut parts = Vec::new();
        for c in input.chunks(441) {
            r.push(c, &mut parts);
        }
        assert_eq!(parts.len(), whole.len());
        assert!(parts.iter().zip(&whole).all(|(a, b)| (a - b).abs() < 1e-4));
        assert!((whole.len() as i64 - 16000).abs() <= 1);
    }

    #[test]
    fn stereo_is_averaged() {
        assert_eq!(to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
    }
}
