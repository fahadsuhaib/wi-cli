//! The model files: where they live, what they are, and how they are downloaded (only after a
//! confirmation that names the file, its size and the host; verified with SHA256).

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct ModelFile {
    /// Short name used on the command line.
    pub id: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub what: &'static str,
}

pub const SMALL: ModelFile = ModelFile {
    id: "small",
    file: "moonshine-streaming-small-Q8_0.gguf",
    url: "https://huggingface.co/handy-computer/moonshine-streaming-small-gguf/resolve/main/moonshine-streaming-small-Q8_0.gguf",
    bytes: 198_506_848,
    sha256: "d03670f69629b649085d0f44a63d97668b4119117cc9611a4e4ad94341713dfc",
    what: "Moonshine Streaming small, English speech to text (MIT licence)",
};

pub const TINY: ModelFile = ModelFile {
    id: "tiny",
    file: "moonshine-streaming-tiny-Q8_0.gguf",
    url: "https://huggingface.co/handy-computer/moonshine-streaming-tiny-gguf/resolve/main/moonshine-streaming-tiny-Q8_0.gguf",
    bytes: 50_462_816,
    sha256: "930e4622ad3a24158b91406c30c977fa6a26b34cb32d6ac3e57cfb23383a869e",
    what: "Moonshine Streaming tiny, a smaller and less accurate speech model (MIT licence)",
};

pub const VAD: ModelFile = ModelFile {
    id: "vad",
    file: "silero-vad-v6.2-F32.gguf",
    url: "https://huggingface.co/handy-computer/silero-vad-v6.2-gguf/resolve/main/silero-vad-v6.2-F32.gguf",
    bytes: 1_241_056,
    sha256: "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6",
    what: "Silero voice activity detection, finds where phrases start and end (MIT licence)",
};

pub const ALL: [&ModelFile; 3] = [&SMALL, &TINY, &VAD];

/// `WICLI_AUDIO_MODELS` (tests, custom places), else the per-user data folder.
pub fn dir() -> PathBuf {
    if let Some(p) = std::env::var_os("WICLI_AUDIO_MODELS") {
        return PathBuf::from(p);
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("wicli").join("models")
}

impl ModelFile {
    pub fn path(&self) -> PathBuf {
        dir().join(self.file)
    }
    /// Present with the right size (the hash is checked at download time and by `doctor --verify`).
    pub fn present(&self) -> bool {
        std::fs::metadata(self.path()).is_ok_and(|m| m.len() == self.bytes)
    }
    pub fn host(&self) -> &str {
        self.url.split('/').nth(2).unwrap_or("")
    }
}

/// The speech model to use: `WICLI_AUDIO_MODEL` (a path), else small, else tiny.
pub fn speech_model() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WICLI_AUDIO_MODEL") {
        return Some(PathBuf::from(p));
    }
    [&SMALL, &TINY].into_iter().find(|m| m.present()).map(|m| m.path())
}

pub fn vad_model() -> Option<PathBuf> {
    VAD.present().then(|| VAD.path())
}

pub fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn have(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok()
}

fn download(url: &str, dest: &Path) -> Result<(), String> {
    let status = if have("curl") {
        Command::new("curl").args(["-fL", "--progress-bar", "-o"]).arg(dest).arg(url).status()
    } else if have("wget") {
        Command::new("wget").args(["-O"]).arg(dest).arg(url).status()
    } else {
        return Err("downloading needs curl or wget, and neither was found".into());
    };
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("download failed ({s})")),
        Err(e) => Err(format!("download failed: {e}")),
    }
}

/// Download one file after asking. `yes` skips the question (the caller already has a
/// confirmation of its own). Returns Ok(false) when the user said no.
pub fn install(m: &ModelFile, yes: bool) -> Result<bool, String> {
    if m.present() {
        println!("{} is already installed ({}).", m.file, m.path().display());
        return Ok(true);
    }
    println!("{}", m.what);
    println!("Download {} ({}) from {} into {}", m.file, mb(m.bytes), m.host(), dir().display());
    println!("Nothing else is sent. The file is checked against SHA256 {}.", m.sha256);
    if !yes {
        print!("Continue? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Not downloaded.");
            return Ok(false);
        }
    }
    std::fs::create_dir_all(dir()).map_err(|e| format!("cannot create {}: {e}", dir().display()))?;
    let part = dir().join(format!("{}.part", m.file));
    download(m.url, &part)?;
    let got = sha256_file(&part)?;
    if got != m.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the downloaded file did not match its SHA256 (got {got}); it was deleted"));
    }
    std::fs::rename(&part, m.path()).map_err(|e| e.to_string())?;
    println!("Installed {}.", m.path().display());
    Ok(true)
}

pub fn remove_all() -> Result<usize, String> {
    let mut n = 0;
    for m in ALL {
        for p in [m.path(), dir().join(format!("{}.part", m.file))] {
            if p.exists() {
                std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                n += 1;
            }
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_64_hex_and_hosts_parse() {
        for m in ALL {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            assert_eq!(m.host(), "huggingface.co");
            assert!(m.url.ends_with(m.file));
        }
    }

    #[test]
    fn sha256_of_a_known_string() {
        let p = std::env::temp_dir().join(format!("wi-audio-sha-{}", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn sizes_print_in_megabytes() {
        assert_eq!(mb(198_506_848), "198.5 MB");
    }
}
