//! Where audio comes from: the microphone (cpal), or a wav file named by `WICLI_AUDIO_FILE`
//! (the test hook). Both deliver 16 kHz mono chunks through a bounded channel.

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Chunks the channel holds before the oldest audio is dropped (100 ms each: one minute).
const CAPACITY: usize = 600;

pub struct Source {
    pub rx: Receiver<Vec<f32>>,
    /// Held so the microphone stream stays open exactly as long as the Source lives.
    _keep: Option<Box<dyn std::any::Any>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Source {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub enum Recv {
    Chunk(Vec<f32>),
    Empty,
    /// The file ended, or the microphone is gone.
    End,
}

impl Source {
    #[cfg(test)]
    pub fn for_test(rx: Receiver<Vec<f32>>) -> Source {
        Source { rx, _keep: None, stop: Arc::new(AtomicBool::new(false)) }
    }
    pub fn recv(&self, wait: Duration) -> Recv {
        match self.rx.recv_timeout(wait) {
            Ok(c) => Recv::Chunk(c),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Recv::Empty,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Recv::End,
        }
    }
}

/// The hook file, when set.
pub fn test_file() -> Option<std::path::PathBuf> {
    std::env::var_os("WICLI_AUDIO_FILE").filter(|v| !v.is_empty()).map(Into::into)
}

/// Open the right source: the hook file if set, else the microphone.
pub fn open(device: Option<&str>) -> Result<Source, String> {
    match test_file() {
        Some(p) => from_file(&p),
        None => mic(device),
    }
}

/// Feed a wav file as if it were live: 100 ms chunks, paced by `WICLI_AUDIO_SPEED` (default 1 =
/// real time; 0 or a big number goes as fast as the reader keeps up).
pub fn from_file(path: &std::path::Path) -> Result<Source, String> {
    let pcm = crate::wav::read_wav(path)?;
    let speed: f64 = std::env::var("WICLI_AUDIO_SPEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let (tx, rx) = sync_channel::<Vec<f32>>(CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        for c in pcm.chunks(1600) {
            if flag.load(Ordering::Relaxed) {
                return;
            }
            if tx.send(c.to_vec()).is_err() {
                return;
            }
            if speed > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(0.1 / speed));
            }
        }
        // Dropping the sender tells the reader the file is over.
    });
    Ok(Source { rx, _keep: None, stop })
}

fn push(tx: &SyncSender<Vec<f32>>, chunk: Vec<f32>) {
    // A full channel means the reader is far behind: drop the audio rather than grow.
    let _ = tx.try_send(chunk);
}

fn input_device(host: &cpal::Host, wanted: Option<&str>) -> Result<cpal::Device, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    if let Some(w) = wanted.filter(|w| !w.is_empty()) {
        let w = w.to_lowercase();
        let found = host.input_devices().map_err(|e| e.to_string())?.find(|d| {
            d.description().map(|x| x.name().to_lowercase().contains(&w)).unwrap_or(false)
                || d.id().map(|i| i.to_string().to_lowercase().contains(&w)).unwrap_or(false)
        });
        return found.ok_or_else(|| format!("no input device matches '{w}' (see: wi-audio doctor)"));
    }
    host.default_input_device().ok_or_else(|| "no microphone found".to_string())
}

/// Names of the input devices, the default first.
pub fn device_names() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.description().ok()).map(|d| d.name().to_string());
    let mut names: Vec<String> = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.description().ok().map(|x| x.name().to_string())).collect())
        .unwrap_or_default();
    if let Some(d) = default {
        names.retain(|n| *n != d);
        names.insert(0, d);
    }
    names
}

pub fn mic(device: Option<&str>) -> Result<Source, String> {
    use cpal::traits::{DeviceTrait, StreamTrait};
    use cpal::SampleFormat;
    let host = cpal::default_host();
    let dev = input_device(&host, device)?;
    let cfg = dev.default_input_config().map_err(|e| friendly(&e))?;
    let rate = cfg.sample_rate();
    let channels = usize::from(cfg.channels()).max(1);
    let (tx, rx) = sync_channel::<Vec<f32>>(CAPACITY);
    // A dead stream shows up as audio that stops arriving; `listen` reports that.
    let err_fn = move |e: cpal::Error| eprintln!("wi-audio: audio stream error: {e}");
    let mut resampler = crate::wav::Resampler::new(rate, crate::wav::RATE);
    let mut pending: Vec<f32> = Vec::new();
    let mut handle = move |mono: Vec<f32>| {
        resampler.push(&mono, &mut pending);
        while pending.len() >= 1600 {
            let rest = pending.split_off(1600);
            push(&tx, std::mem::replace(&mut pending, rest));
        }
    };
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => dev.build_input_stream(
            cfg.into(),
            move |d: &[f32], _: &_| handle(crate::wav::to_mono(d, channels)),
            err_fn,
            None,
        ),
        SampleFormat::I16 => dev.build_input_stream(
            cfg.into(),
            move |d: &[i16], _: &_| {
                let f: Vec<f32> = d.iter().map(|s| f32::from(*s) / 32768.0).collect();
                handle(crate::wav::to_mono(&f, channels))
            },
            err_fn,
            None,
        ),
        SampleFormat::I32 => dev.build_input_stream(
            cfg.into(),
            move |d: &[i32], _: &_| {
                let f: Vec<f32> = d.iter().map(|s| *s as f32 / 2147483648.0).collect();
                handle(crate::wav::to_mono(&f, channels))
            },
            err_fn,
            None,
        ),
        other => return Err(format!("the microphone uses an unsupported sample format ({other})")),
    }
    .map_err(|e| friendly(&e))?;
    stream.play().map_err(|e| friendly(&e))?;
    Ok(Source { rx, _keep: Some(Box::new(stream)), stop: Arc::new(AtomicBool::new(false)) })
}

/// One-line, actionable messages for audio errors.
pub fn friendly(e: &cpal::Error) -> String {
    match e.kind() {
        cpal::ErrorKind::PermissionDenied => MIC_DENIED.to_string(),
        cpal::ErrorKind::DeviceNotAvailable => "the microphone is not available (unplugged, or in use by another app)".to_string(),
        cpal::ErrorKind::DeviceBusy => "the microphone is busy (another app is using it)".to_string(),
        _ => format!("microphone error: {e}"),
    }
}

pub const MIC_DENIED: &str = "microphone access denied: allow your terminal app in System Settings > Privacy & Security > Microphone, then try again";
