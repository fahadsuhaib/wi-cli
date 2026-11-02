//! Working directory of a shell tab, so relative paths in its output resolve to the right file.
//! Order: what the shell reported itself (OSC 7), then the shell process (local) or the tmux
//! pane (remote). Looked up only when the user clicks a link.

use crate::workspace::Backend;

pub async fn tab_cwd(backend: &Backend, osc7: Option<&str>, pid: Option<u32>, tmux_session: Option<&str>) -> Option<String> {
    if let Some(p) = osc7 {
        return Some(p.to_string());
    }
    match backend {
        Backend::Local(_) => pid.and_then(local_cwd),
        Backend::Remote(s) => s.tmux_pane_path(tmux_session?).await,
    }
}

/// Is a tab's shell idle at its prompt, or is a program running in it?
#[derive(Debug, PartialEq, Eq)]
pub enum Activity {
    Ready,
    /// A program is running (its name, when known).
    Busy(String),
    /// Cannot tell (so nothing may be typed).
    Unknown,
}

/// Local decision: the pty's foreground process group is the shell's own when it is idle.
pub fn local_idle(foreground: Option<i32>, shell_pid: Option<u32>, running: impl FnOnce(i32) -> Option<String>) -> Activity {
    match (foreground, shell_pid) {
        (Some(fg), Some(pid)) if fg == pid as i32 => Activity::Ready,
        (Some(fg), Some(_)) => Activity::Busy(running(fg).unwrap_or_else(|| "a program".into())),
        _ => Activity::Unknown,
    }
}

/// tmux decision: the pane's foreground command is a shell.
pub fn tmux_idle(command: Option<&str>) -> Activity {
    match command {
        Some(c) if crate::session::is_shell_name(c) => Activity::Ready,
        Some(c) => Activity::Busy(c.trim().to_string()),
        None => Activity::Unknown,
    }
}

/// Name of the process behind a process group id (`ps`; None when it cannot be read).
fn process_name(pgid: i32) -> Option<String> {
    let out = std::process::Command::new("ps").args(["-o", "comm=", "-p", &pgid.to_string()]).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().rsplit('/').next()?.to_string();
    (!name.is_empty()).then_some(name)
}

/// Whether a shell tab is idle at its prompt. Remote shells without tmux cannot be told apart.
pub async fn tab_idle(backend: &Backend, handle: Option<&crate::workspace::ShellHandle>, tmux_session: Option<&str>) -> Activity {
    match backend {
        Backend::Local(_) => match handle {
            Some(h) => local_idle(h.foreground().await, h.pid(), process_name),
            None => Activity::Unknown,
        },
        Backend::Remote(s) => match tmux_session {
            Some(name) => tmux_idle(s.tmux_pane_command(name).await.as_deref()),
            None => Activity::Unknown,
        },
    }
}

#[cfg(target_os = "linux")]
fn local_cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| p.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
fn local_cwd(pid: u32) -> Option<String> {
    let out = std::process::Command::new("lsof").args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]).output().ok()?;
    parse_lsof(&String::from_utf8_lossy(&out.stdout))
}

/// The path from `lsof -Fn` output (lines like `p123`, `fcwd`, `n/Users/me/proj`).
#[allow(dead_code)] // only used on non-Linux
pub fn parse_lsof(s: &str) -> Option<String> {
    s.lines().find_map(|l| l.strip_prefix('n')).filter(|p| p.starts_with('/')).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsof_field_output() {
        assert_eq!(parse_lsof("p123\nfcwd\nn/Users/me/my proj\n").as_deref(), Some("/Users/me/my proj"));
        assert_eq!(parse_lsof("p123\n"), None);
        assert_eq!(parse_lsof("n relative\n"), None);
    }

    #[test]
    fn finds_this_process_directory() {
        let here = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let got = local_cwd(std::process::id()).expect("cwd of our own process");
        assert_eq!(std::fs::canonicalize(got).unwrap(), here);
    }

    #[test]
    fn local_idle_decision() {
        assert_eq!(local_idle(Some(100), Some(100), |_| None), Activity::Ready);
        assert_eq!(local_idle(Some(200), Some(100), |p| Some(format!("prog{p}"))), Activity::Busy("prog200".into()));
        assert_eq!(local_idle(Some(200), Some(100), |_| None), Activity::Busy("a program".into()));
        assert_eq!(local_idle(None, Some(100), |_| None), Activity::Unknown);
        assert_eq!(local_idle(Some(100), None, |_| None), Activity::Unknown);
    }

    #[test]
    fn tmux_idle_decision() {
        for sh in ["bash", "zsh", "sh", "dash", "fish", "ksh", "-bash"] {
            assert_eq!(tmux_idle(Some(sh)), Activity::Ready, "{sh}");
        }
        assert_eq!(tmux_idle(Some("node")), Activity::Busy("node".into()));
        assert_eq!(tmux_idle(Some("claude\n")), Activity::Busy("claude".into()));
        assert_eq!(tmux_idle(Some("sleep")), Activity::Busy("sleep".into()));
        assert_eq!(tmux_idle(None), Activity::Unknown);
    }

    #[tokio::test]
    async fn osc7_wins() {
        let b = Backend::Local(std::path::PathBuf::from("/tmp"));
        assert_eq!(tab_cwd(&b, Some("/a/b"), Some(std::process::id()), None).await.as_deref(), Some("/a/b"));
    }
}
