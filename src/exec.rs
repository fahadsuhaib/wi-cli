//! Run a short command on the workspace (locally or over the existing SSH connection) with a
//! hard output cap and a timeout. Used for git; nothing is installed or left running.

use crate::preview::diff::Snapshot;
use crate::workspace::Backend;
use anyhow::{Context, Result, bail};
use russh::ChannelMsg;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Output beyond this is dropped and reported as truncated.
pub const OUT_CAP: usize = 1024 * 1024;
pub const TIMEOUT: Duration = Duration::from_secs(10);
const ERR_CAP: usize = 4096;

pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: i32,
    pub truncated: bool,
}

/// Quote one argument for a POSIX shell (single quotes; embedded quotes become `'\''`).
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,@+%".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Quote one argument so a POSIX shell sees exactly `s`: always single quotes, with every
/// embedded `'` written as `'\''`. Nothing inside single quotes is special, so spaces, `$`,
/// backticks, newlines and a leading dash all stay literal (put `--` before it for programs).
pub fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// True when a folder name is safe to type into a shell or pass along: no control characters
/// (a newline or escape typed into a terminal could run commands).
pub fn folder_name_ok(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| c.is_control())
}

pub fn shell_join(args: &[&str]) -> String {
    args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

/// Read up to `cap` bytes; reports whether more data was waiting.
pub async fn read_capped<R: AsyncRead + Unpin>(r: &mut R, cap: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::new();
    r.take(cap as u64 + 1).read_to_end(&mut buf).await?;
    let truncated = buf.len() > cap;
    buf.truncate(cap);
    Ok((buf, truncated))
}

/// Append `data` to `buf` without exceeding `cap`; returns true if something was dropped.
fn push_capped(buf: &mut Vec<u8>, data: &[u8], cap: usize) -> bool {
    let room = cap.saturating_sub(buf.len());
    buf.extend_from_slice(&data[..data.len().min(room)]);
    data.len() > room
}

impl Backend {
    /// Run a program with arguments in the workspace directory. Output is capped at `OUT_CAP`
    /// and the whole call at `TIMEOUT`.
    pub async fn run(&self, args: &[&str]) -> Result<Output> {
        match tokio::time::timeout(TIMEOUT, self.run_inner(args)).await {
            Ok(r) => r,
            Err(_) => bail!("{} timed out after {} s", args.first().unwrap_or(&"command"), TIMEOUT.as_secs()),
        }
    }

    async fn run_inner(&self, args: &[&str]) -> Result<Output> {
        let Some((prog, rest)) = args.split_first() else { bail!("no command") };
        match self {
            Backend::Local(dir) => {
                let mut child = tokio::process::Command::new(prog)
                    .args(rest)
                    .current_dir(dir)
                    .env("GIT_OPTIONAL_LOCKS", "0")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .with_context(|| format!("cannot run {prog}"))?;
                let mut out = child.stdout.take().context("no stdout")?;
                let mut err = child.stderr.take().context("no stderr")?;
                let err_task = tokio::spawn(async move { read_capped(&mut err, ERR_CAP).await.map(|r| r.0).unwrap_or_default() });
                let (bytes, truncated) = read_capped(&mut out, OUT_CAP).await?;
                if truncated {
                    let _ = child.kill().await;
                }
                drop(out);
                let status = child.wait().await?;
                // Anything still holding stderr open (a grandchild) must not hold us up.
                let e = tokio::time::timeout(Duration::from_millis(200), err_task).await.ok().and_then(|r| r.ok()).unwrap_or_default();
                Ok(Output { stdout: bytes, stderr: String::from_utf8_lossy(&e).trim().to_string(), code: status.code().unwrap_or(-1), truncated })
            }
            Backend::Remote(s) => {
                let mut ch = s.handle.channel_open_session().await?;
                ch.exec(true, format!("GIT_OPTIONAL_LOCKS=0 {}", shell_join(args))).await?;
                let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
                let (mut code, mut truncated) = (-1, false);
                while let Some(msg) = ch.wait().await {
                    match msg {
                        ChannelMsg::Data { data } => {
                            if push_capped(&mut stdout, &data, OUT_CAP) {
                                truncated = true;
                                break;
                            }
                        }
                        ChannelMsg::ExtendedData { data, .. } => {
                            push_capped(&mut stderr, &data, ERR_CAP);
                        }
                        ChannelMsg::ExitStatus { exit_status } => code = exit_status as i32,
                        ChannelMsg::Close => break,
                        _ => {}
                    }
                }
                if truncated {
                    let _ = ch.close().await;
                    code = 0;
                }
                Ok(Output { stdout, stderr: String::from_utf8_lossy(&stderr).trim().to_string(), code, truncated })
            }
        }
    }

    async fn git(&self, dir: &str, args: &[&str]) -> Result<Output> {
        let mut full = vec!["git", "-c", "core.quotepath=off", "-C", dir];
        full.extend_from_slice(args);
        self.run(&full).await
    }

    /// Current branch name, or None when this is not a git repository (or git is missing).
    pub async fn git_branch(&self, dir: &str) -> Option<String> {
        let o = self.git(dir, &["branch", "--show-current"]).await.ok()?;
        if o.code != 0 {
            return None;
        }
        let name = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
        // Detached HEAD: show the short commit, or nothing if there is no commit.
        let o = self.git(dir, &["rev-parse", "--short", "HEAD"]).await.ok()?;
        (o.code == 0).then(|| format!("detached {}", String::from_utf8_lossy(&o.stdout).trim()))
    }

    /// Everything the Git changes view shows. Errors are worded for the status bar.
    pub async fn git_snapshot(&self, dir: &str) -> Result<Snapshot> {
        let status = match self.git(dir, &["status", "--short", "--branch"]).await {
            Ok(o) if o.code == 0 => o,
            Ok(o) if o.code == 127 => bail!("git is not installed here"),
            Ok(o) if o.stderr.to_lowercase().contains("not a git repository") => bail!("not a git repository"),
            Ok(o) => bail!("git status failed: {}", o.stderr.lines().next().unwrap_or("unknown error")),
            Err(e) if format!("{e:#}").contains("No such file") => bail!("git is not installed here"),
            Err(e) => bail!("git: {e:#}"),
        };
        let mut truncated = status.truncated;
        let mut diff = self.git(dir, &["diff", "HEAD", "--no-color", "--no-ext-diff"]).await?;
        if diff.code != 0 {
            // No commit yet: staged and unstaged changes separately.
            let staged = self.git(dir, &["diff", "--cached", "--no-color", "--no-ext-diff"]).await?;
            let unstaged = self.git(dir, &["diff", "--no-color", "--no-ext-diff"]).await?;
            diff.stdout = [staged.stdout, unstaged.stdout].concat();
            diff.truncated = staged.truncated || unstaged.truncated;
        }
        truncated |= diff.truncated;
        let untracked = self.git(dir, &["ls-files", "--others", "--exclude-standard"]).await?;
        truncated |= untracked.truncated;
        Ok(Snapshot {
            status: String::from_utf8_lossy(&status.stdout).into_owned(),
            diff: String::from_utf8_lossy(&diff.stdout).into_owned(),
            untracked: String::from_utf8_lossy(&untracked.stdout).into_owned(),
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sq_quotes_everything_literally() {
        assert_eq!(sq("/a/b"), "'/a/b'");
        assert_eq!(sq("a b"), "'a b'");
        assert_eq!(sq("it's"), r"'it'\''s'");
        assert_eq!(sq("$HOME/$(id)"), "'$HOME/$(id)'");
        assert_eq!(sq("`id`"), "'`id`'");
        assert_eq!(sq("a\nb"), "'a\nb'");
        assert_eq!(sq("-rf"), "'-rf'");
        assert_eq!(sq(""), "''");
        assert_eq!(sq("a;b|c&d\"e\\f"), "'a;b|c&d\"e\\f'");
    }

    #[test]
    fn sq_round_trips_through_a_real_shell() {
        for s in ["a b", "it's", "''", "$(echo pwned)", "`echo pwned`", "x\ny", "-n", "a\\b", "\"q\"", "; echo pwned"] {
            let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", sq(s))).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), s);
        }
    }

    #[test]
    fn folder_names_with_control_characters_are_refused() {
        assert!(folder_name_ok("/home/dev/my proj"));
        assert!(!folder_name_ok("/a\nb"));
        assert!(!folder_name_ok("/a\x1b[31m"));
        assert!(!folder_name_ok(""));
    }

    #[test]
    fn quotes_safely() {
        assert_eq!(shell_quote("status"), "status");
        assert_eq!(shell_quote("--short"), "--short");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert_eq!(shell_quote("a;b|c&d`e`"), "'a;b|c&d`e`'");
        assert_eq!(shell_quote("x\ny"), "'x\ny'");
        assert_eq!(shell_join(&["git", "-C", "/my dir", "log"]), "git -C '/my dir' log");
    }

    #[tokio::test]
    async fn read_capped_truncates_and_reports() {
        let data = [b'x'; 100];
        let (b, t) = read_capped(&mut &data[..], 10).await.unwrap();
        assert_eq!((b.len(), t), (10, true));
        let (b, t) = read_capped(&mut &data[..], 100).await.unwrap();
        assert_eq!((b.len(), t), (100, false));
        let (b, t) = read_capped(&mut &data[..], 500).await.unwrap();
        assert_eq!((b.len(), t), (100, false));
    }

    #[test]
    fn push_capped_stops_at_cap() {
        let mut b = Vec::new();
        assert!(!push_capped(&mut b, b"abcd", 6));
        assert!(push_capped(&mut b, b"efgh", 6));
        assert_eq!(b, b"abcdef");
        assert!(push_capped(&mut b, b"z", 6));
        assert_eq!(b.len(), 6);
    }

    #[tokio::test]
    async fn local_run_caps_output_and_reports_exit_code() {
        let b = Backend::Local(std::env::temp_dir());
        let o = b.run(&["sh", "-c", "yes | head -c 3000000"]).await.unwrap();
        assert!(o.truncated);
        assert_eq!(o.stdout.len(), OUT_CAP);
        let o = b.run(&["sh", "-c", "echo hi; exit 3"]).await.unwrap();
        assert_eq!((o.stdout.as_slice(), o.code, o.truncated), (&b"hi\n"[..], 3, false));
    }
}
