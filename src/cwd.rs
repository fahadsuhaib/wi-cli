//! Working directory of a shell tab, so relative paths in its output resolve to the right file.
//! Order: what the shell reported itself (OSC 7), then the shell process (local) or the tmux
//! pane (remote). Looked up only when the user clicks a link.

use crate::workspace::Backend;

#[allow(dead_code)] // used by the link integration
pub async fn tab_cwd(backend: &Backend, osc7: Option<&str>, pid: Option<u32>, tmux_session: Option<&str>) -> Option<String> {
    if let Some(p) = osc7 {
        return Some(p.to_string());
    }
    match backend {
        Backend::Local(_) => pid.and_then(local_cwd),
        Backend::Remote(s) => s.tmux_pane_path(tmux_session?).await,
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

    #[tokio::test]
    async fn osc7_wins() {
        let b = Backend::Local(std::path::PathBuf::from("/tmp"));
        assert_eq!(tab_cwd(&b, Some("/a/b"), Some(std::process::id()), None).await.as_deref(), Some("/a/b"));
    }
}
