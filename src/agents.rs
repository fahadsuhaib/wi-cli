//! AI-agent workspaces: detect Claude Code / Codex projects and remember how the user starts them.

use crate::config::{Config, config_path};
use crate::workspace::{Backend, Workspace};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agent {
    Claude,
    Codex,
}

pub const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];

impl Agent {
    pub fn key(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    /// Files or folders at the workspace root that show the agent is used here.
    pub fn markers(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &["CLAUDE.md", ".claude"],
            Agent::Codex => &["AGENTS.md", ".codex"],
        }
    }

    pub fn from_key(k: &str) -> Option<Agent> {
        ALL.into_iter().find(|a| a.key() == k)
    }
}

/// Agents whose markers exist in the folder `dir` (absolute). At most four stat calls.
pub async fn detect(backend: &Backend, dir: &str) -> Vec<Agent> {
    let mut found = Vec::new();
    for a in ALL {
        for m in a.markers() {
            if backend.exists_in(dir, m).await {
                found.push(a);
                break;
            }
        }
    }
    found
}

/// What was chosen for one workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remembered {
    /// `claude`, `codex`, or `none` (never start anything here).
    pub agent: String,
    #[serde(default)]
    pub command: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// Theme chosen from the menu (`dark` or `light`); an explicit config theme overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default)]
    pub workspace: BTreeMap<String, Remembered>,
    /// Last command used per agent, offered as the default in new workspaces.
    #[serde(default)]
    pub last_command: BTreeMap<String, String>,
    /// Folders recently opened as a tab, most recent first, per workspace (`recents_key`).
    #[serde(default)]
    pub recent: BTreeMap<String, Vec<String>>,
}

/// How many recent folders are kept per workspace.
pub const RECENT_MAX: usize = 10;

pub fn state_path() -> PathBuf {
    config_path().with_file_name("state.toml")
}

impl State {
    pub fn load() -> State {
        std::fs::read_to_string(state_path()).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    /// Write atomically so a crash never leaves a half-written file.
    pub fn save(&self) -> std::io::Result<()> {
        let path = state_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }

    /// Record `dir` as the most recently used folder: moved to the front if already there,
    /// list capped at `RECENT_MAX`.
    pub fn push_recent(&mut self, key: &str, dir: &str) {
        let list = self.recent.entry(key.to_string()).or_default();
        list.retain(|d| d != dir);
        list.insert(0, dir.to_string());
        list.truncate(RECENT_MAX);
    }

    pub fn recents(&self, key: &str) -> Vec<String> {
        self.recent.get(key).cloned().unwrap_or_default()
    }

    pub fn remember(&mut self, key: &str, agent: &str, command: &str) {
        self.workspace.insert(key.to_string(), Remembered { agent: agent.to_string(), command: command.to_string() });
        if agent != "none" && !command.is_empty() {
            self.last_command.insert(agent.to_string(), command.to_string());
        }
    }
}

/// Key for remembered agent choices: one per folder, so projects in different folders of the
/// same box (or machine) are remembered separately.
pub fn workspace_key(ws: &Workspace, dir: &str) -> String {
    match ws {
        Workspace::Local(_) => format!("local:{dir}"),
        Workspace::Remote(b) => format!("box:{}:{dir}", b.name),
    }
}

/// Key for the recent-folders list: one per machine / box.
pub fn recents_key(ws: &Workspace) -> String {
    match ws {
        Workspace::Local(_) => "local".to_string(),
        Workspace::Remote(b) => format!("box:{}", b.name),
    }
}

/// The command to offer for an agent: what is remembered here, else what was last used
/// anywhere, else the configured one, else the bare program name.
pub fn command_for(agent: Agent, cfg: &Config, state: &State, key: &str) -> String {
    if let Some(r) = state.workspace.get(key)
        && r.agent == agent.key()
        && !r.command.is_empty()
    {
        return r.command.clone();
    }
    if let Some(c) = cfg.agents.get(agent.key()) {
        return c.command.clone();
    }
    state.last_command.get(agent.key()).cloned().unwrap_or_else(|| agent.key().to_string())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    Start(Agent),
    /// Show the start prompt with these agents.
    Ask(Vec<Agent>),
}

/// Pure startup decision. `policy` is the `agent_start` setting.
pub fn decide(policy: &str, detected: &[Agent], remembered: Option<&Remembered>) -> Decision {
    if detected.is_empty() {
        return Decision::Nothing;
    }
    if let Some(r) = remembered {
        if r.agent == "none" {
            return Decision::Nothing;
        }
        if let Some(a) = Agent::from_key(&r.agent)
            && policy != "never"
        {
            return Decision::Start(a);
        }
    }
    match policy {
        "never" => Decision::Nothing,
        "always" => Decision::Start(detected[0]),
        _ => Decision::Ask(detected.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rem(agent: &str, cmd: &str) -> Remembered {
        Remembered { agent: agent.into(), command: cmd.into() }
    }

    #[test]
    fn nothing_detected_means_nothing() {
        assert_eq!(decide("ask", &[], None), Decision::Nothing);
        assert_eq!(decide("always", &[], Some(&rem("claude", "claude"))), Decision::Nothing);
    }

    #[test]
    fn first_time_asks_unless_configured() {
        assert_eq!(decide("ask", &[Agent::Claude], None), Decision::Ask(vec![Agent::Claude]));
        assert_eq!(decide("always", &[Agent::Codex, Agent::Claude], None), Decision::Start(Agent::Codex));
        assert_eq!(decide("never", &[Agent::Claude], None), Decision::Nothing);
    }

    #[test]
    fn remembered_choice_starts_without_asking() {
        assert_eq!(decide("ask", &[Agent::Claude, Agent::Codex], Some(&rem("codex", "codex"))), Decision::Start(Agent::Codex));
        assert_eq!(decide("ask", &[Agent::Claude], Some(&rem("none", ""))), Decision::Nothing);
        assert_eq!(decide("never", &[Agent::Claude], Some(&rem("claude", "claude"))), Decision::Nothing);
    }

    #[test]
    fn command_precedence() {
        let mut cfg = Config::default();
        let mut st = State::default();
        assert_eq!(command_for(Agent::Claude, &cfg, &st, "k"), "claude");
        st.last_command.insert("claude".into(), "claude --continue".into());
        assert_eq!(command_for(Agent::Claude, &cfg, &st, "k"), "claude --continue");
        cfg.agents.insert("claude".into(), crate::config::AgentCfg { command: "claude --model opus".into() });
        assert_eq!(command_for(Agent::Claude, &cfg, &st, "k"), "claude --model opus");
        st.remember("k", "claude", "claude --resume");
        assert_eq!(command_for(Agent::Claude, &cfg, &st, "k"), "claude --resume");
        assert_eq!(command_for(Agent::Claude, &cfg, &st, "other"), "claude --model opus");
    }

    #[test]
    fn agent_keys_are_per_folder() {
        let l = Workspace::Local(PathBuf::from("/w"));
        assert_eq!(workspace_key(&l, "/w"), "local:/w");
        assert_ne!(workspace_key(&l, "/w/a"), workspace_key(&l, "/w/b"));
        let b = Workspace::Remote(crate::config::BoxConfig { name: "test".into(), host: "h".into(), port: 22, user: "u".into(), identity: None, known_hosts: None, use_tmux: true });
        assert_eq!(workspace_key(&b, "/home/dev/a"), "box:test:/home/dev/a");
        assert_ne!(workspace_key(&b, "/home/dev/a"), workspace_key(&b, "/home/dev/b"));
        assert_eq!(recents_key(&l), "local");
        assert_eq!(recents_key(&b), "box:test");
        let mut st = State::default();
        st.remember(&workspace_key(&l, "/w/a"), "claude", "claude");
        assert!(st.workspace.contains_key("local:/w/a") && !st.workspace.contains_key("local:/w/b"));
    }

    #[test]
    fn recent_folders_are_most_recent_first_deduped_and_capped() {
        let mut st = State::default();
        for i in 0..15 {
            st.push_recent("local", &format!("/p/{i}"));
        }
        let r = st.recents("local");
        assert_eq!(r.len(), RECENT_MAX);
        assert_eq!(r[0], "/p/14");
        assert_eq!(r[9], "/p/5");
        st.push_recent("local", "/p/10");
        let r = st.recents("local");
        assert_eq!(r[0], "/p/10");
        assert_eq!(r.iter().filter(|d| *d == "/p/10").count(), 1);
        assert_eq!(r.len(), RECENT_MAX);
        assert!(st.recents("box:x").is_empty());
        st.push_recent("box:x", "/h");
        assert_eq!(st.recents("local").len(), RECENT_MAX, "lists are per workspace");
        let back: State = toml::from_str(&toml::to_string_pretty(&st).unwrap()).unwrap();
        assert_eq!(back.recents("local"), st.recents("local"));
        assert_eq!(back.recents("box:x"), vec!["/h"]);
        let old: State = toml::from_str("[workspace]\n").unwrap();
        assert!(old.recent.is_empty(), "state files from before still load");
    }

    #[test]
    fn state_round_trips_through_toml() {
        let mut st = State::default();
        st.remember("local:/p", "claude", "claude -c");
        st.remember("box:b", "none", "");
        let text = toml::to_string_pretty(&st).unwrap();
        let back: State = toml::from_str(&text).unwrap();
        assert_eq!(back.workspace["local:/p"], rem("claude", "claude -c"));
        assert_eq!(back.workspace["box:b"].agent, "none");
        assert_eq!(back.last_command["claude"], "claude -c");
    }
}
