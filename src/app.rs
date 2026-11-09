//! Event loop: input, resize, shell output from every tab, coalesced redraw.

use crate::dictate;
use crate::voice;
use crate::input::{InEv, PasteFilter};
use crate::keys::{EnterMap, KeyDecoder};
use crate::scroll::{self, ScrollKey};
use crate::config::{Config, parse_prefix_key, prefix_label};
use std::collections::HashMap;
use crate::explorer::{self, Act as ExAct, ExJob, ExMouse, ExplorerState};
use crate::tab::{Btn, Pane, Side, Split, Tab, button_at, pct_for_col, split_main, split_pinned};
use crate::files::complete;
use crate::preview::{Opts, Preview, keys::{self as pvkeys, PvKey}};
use crate::mouse::{self, Button, Kind, MouseEvent, Piece};
use crate::links::{self, Link, LinkKind};
use crate::opener::{self, FetchOpts, FetchResult, Handling, OpenCfg};
use crate::select::{self, Pos};
use crate::search::{self, K, FindPrompt, FindTarget, Finder, GrepPrompt, grep::{self, Results}, output::OutFind};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::agents::{self, Agent, Decision};
use crate::ui::{self, Action, Ask, Confirm, Hint, MenuItem, Prompt, PromptKind, View, MenuState, hint_labels, menu_items_for, sanitize};
use crate::cwd::Activity;
use crate::files::{anchor, complete_dirs};
use ratatui::layout::Rect;
use crate::workspace::{Backend, Event, Workspace, spawn_shell};
use anyhow::{Result, bail};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::stdout;
use crate::term::{TermGuard, shell_size};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::time::sleep_until;

enum Mode {
    Shell,
    Prompt(Prompt),
    /// Naming a tab (the inline one-line prompt).
    Rename(crate::rename::Rename),
    Ask(Ask),
    /// The theme picker (arrows preview, Enter saves, Esc reverts).
    ThemePick(ui::themepick::ThemePick),
    /// A yes/no question before anything is opened or downloaded.
    Confirm(Confirm),
    /// Dictation set-up or settings (see `voice.rs`).
    Voice(voice::Dlg),
    /// A download is running; Esc cancels it.
    Fetch,
    /// Every path and link on screen is labelled with a letter.
    Hints(HintState),
    Menu(MenuState),
    /// Browsing scrollback of the active tab.
    Scroll,
    /// The "search in files" query prompt.
    GrepPrompt(GrepPrompt),
    /// Search results (kept in `last_results` while closed).
    Results(Box<Results>),
    /// The fuzzy file finder.
    Finder(Box<Finder>),
    /// The find prompt for shell output or a preview.
    FindPrompt(FindPrompt),
    /// Stepping through matches in the active tab's scrollback.
    OutFind(Box<OutFind>),
    /// A search or file listing is running; Esc cancels.
    Working(&'static str),
    /// The full-screen file explorer.
    Explorer(Box<ExplorerState>),
    /// Several places a clicked relative path could mean.
    Pick(crate::pick::Pick),
}

/// A search or file listing in flight (dropped = cancelled; the process is killed with it).
type Job<'a> = Pin<Box<dyn Future<Output = JobDone> + 'a>>;

/// Preview auto-refresh work: first a stat of the visible files, then (for those that settled
/// after a change) a bounded read. One at a time, each with a timeout, never on the UI loop.
type RJob<'a> = Pin<Box<dyn Future<Output = RDone> + 'a>>;

type ReadOut = Result<(Vec<u8>, u64), String>;

enum RDone {
    Stats(Vec<(String, crate::refresh::Obs)>),
    Read(Vec<(String, ReadOut)>),
}

fn start_stats(backend: &Backend, paths: Vec<String>) -> RJob<'_> {
    Box::pin(async move {
        let mut out = Vec::new();
        for p in paths {
            let o = tokio::time::timeout(Duration::from_secs(3), backend.stamp(&p)).await.unwrap_or(crate::refresh::Obs::Failed);
            out.push((p, o));
        }
        RDone::Stats(out)
    })
}

fn start_reads(backend: &Backend, paths: Vec<String>, cap: usize) -> RJob<'_> {
    Box::pin(async move {
        let mut out = Vec::new();
        for p in paths {
            let r = match tokio::time::timeout(Duration::from_secs(10), backend.read_prefix(&p, cap)).await {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err("timed out".to_string()),
            };
            out.push((p, r));
        }
        RDone::Read(out)
    })
}

/// File previews on screen that the auto-refresh watches: the active tab's own and side preview
/// and the pinned partner's. Git views and stubs are left alone.
fn visible_files(app: &App) -> Vec<String> {
    let t = &app.tabs[app.active];
    let mut v: Vec<String> = Vec::new();
    let partner = pin_partner(app).and_then(|pi| app.tabs[pi].preview.as_ref());
    for p in [t.preview.as_ref(), t.side.as_ref(), partner].into_iter().flatten() {
        if !matches!(p.kind, crate::preview::Kind::Diff | crate::preview::Kind::Stub) && !v.contains(&p.path) {
            v.push(p.path.clone());
        }
    }
    v
}

/// Swap freshly read content into every preview of `path`, keeping scroll, wrap, markdown view and find.
fn apply_refresh(app: &mut App, path: &str, bytes: &[u8], total: u64, opts: &Opts) -> bool {
    let mut changed = false;
    for t in app.tabs.iter_mut() {
        for slot in [&mut t.preview, &mut t.side] {
            if let Some(old) = slot.as_ref().filter(|p| p.path == path && p.kind != crate::preview::Kind::Diff) {
                let mut p = Preview::build(path, bytes, total, opts);
                p.carry_over(old);
                *slot = Some(p);
                changed = true;
            }
        }
    }
    changed
}

enum JobDone {
    Grep(Results),
    Files(Result<grep::FileList, String>, String),
}

fn start_search(backend: &Backend, dir: String, q: grep::Query) -> Job<'_> {
    Box::pin(async move { JobDone::Grep(grep::search(backend, &dir, q).await) })
}

fn start_files(backend: &Backend, dir: String) -> Job<'_> {
    Box::pin(async move {
        let r = grep::list_files(backend, &dir).await;
        JobDone::Files(r, dir)
    })
}

/// Paste text into whichever search input is open. Returns true when one was.
fn paste_search(mode: &mut Mode, text: &str) -> bool {
    match mode {
        Mode::GrepPrompt(g) => g.query.text.push_str(text),
        Mode::FindPrompt(f) => f.text.push_str(text),
        Mode::Finder(f) => {
            let keys: Vec<K> = text.chars().map(K::Char).collect();
            f.type_keys(&keys);
        }
        _ => return false,
    }
    true
}

enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Esc,
    Backspace,
    Tab,
    Char(char),
    MenuPressed,
}

fn parse_keys(buf: &[u8], menu_key: u8) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        match b {
            0x1b if buf.get(i + 1) == Some(&b'[') || buf.get(i + 1) == Some(&b'O') => {
                // Skip the parameters up to the final byte.
                let mut j = i + 2;
                while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                    j += 1;
                }
                let params = &buf[(i + 2).min(buf.len())..j.min(buf.len())];
                match buf.get(j) {
                    Some(b'A') => keys.push(Key::Up),
                    Some(b'B') => keys.push(Key::Down),
                    Some(b'H') => keys.push(Key::Home),
                    Some(b'F') => keys.push(Key::End),
                    Some(b'~') => match params.first() {
                        Some(b'1' | b'7') => keys.push(Key::Home),
                        Some(b'4' | b'8') => keys.push(Key::End),
                        Some(b'5') => keys.push(Key::PageUp),
                        Some(b'6') => keys.push(Key::PageDown),
                        _ => {}
                    },
                    _ => {}
                }
                i = j + 1;
                continue;
            }
            0x1b => keys.push(Key::Esc),
            b'\r' | b'\n' => keys.push(Key::Enter),
            0x7f | 0x08 => keys.push(Key::Backspace),
            b'\t' => keys.push(Key::Tab),
            c if c == menu_key => keys.push(Key::MenuPressed),
            c if c.is_ascii_graphic() || c == b' ' => keys.push(Key::Char(c as char)),
            _ => {}
        }
        i += 1;
    }
    keys
}

struct App {
    tabs: Vec<Tab>,
    active: usize,
    next_id: usize,
    shell_count: usize,
    scrollback: usize,
    quiet: Duration,
    /// tmux session behind each shell tab (by tab id), on remote boxes with tmux.
    sessions: std::collections::HashMap<usize, String>,
    /// The workspace's own folder (absolute): where the first tab starts.
    root: String,
    /// Where side previews go and how wide they are (changed by dragging the divider).
    side: Side,
    pct: u16,
    /// The divider is being dragged.
    div_drag: bool,
    /// The pinned tab (by id): always shown in a pane beside the other tab on screen.
    pinned: Option<usize>,
    /// Side of the pinned pane (Left or Right).
    pin_side: Side,
    /// The last tab that had the keyboard other than the pinned one (by id).
    last_main: usize,
    /// Files recently shown in a preview, newest first (saved per workspace).
    history: Vec<crate::history::Entry>,
    hist_dirty: bool,
}

/// Where an agent choice made in the Ask dialog applies.
struct AskCtx {
    /// Key under which the choice is remembered (per folder).
    key: String,
    /// Folder the agent starts in.
    dir: String,
    /// Start in this existing tab (by id); None opens a new tab.
    tab: Option<usize>,
}

pub async fn run(ws: &Workspace, backend: &Backend, cfg: &Config, fresh: bool) -> Result<()> {
    let Some(menu_key) = parse_prefix_key(&cfg.defaults.prefix_key) else {
        bail!("prefix_key '{}' is not valid; use ctrl-space or ctrl-<letter>", cfg.defaults.prefix_key);
    };
    let menu_label = prefix_label(&cfg.defaults.prefix_key);
    let fps = cfg.defaults.fps_cap;

    let (events_tx, mut events) = mpsc::channel::<Event>(8);
    let (cols, rows) = shell_size(1)?;
    let mut app = App { tabs: Vec::new(), active: 0, next_id: 0, shell_count: 0, scrollback: cfg.defaults.scrollback_lines, quiet: Duration::from_secs(cfg.defaults.quiet_secs.max(1)), sessions: Default::default(), side: Side::parse(&cfg.defaults.preview_side), pct: cfg.defaults.preview_width_pct, div_drag: false, pinned: None, pin_side: Side::Right, last_main: 0, history: Vec::new(), hist_dirty: false, root: backend.resolve("").await.unwrap_or_else(|_| "/".into()), };
    // Reattach to sessions left running on the box; otherwise start one fresh shell.
    let restored = backend.restored_sessions();
    let (cols, rows) = if restored.len() > 1 { shell_size(restored.len())? } else { (cols, rows) };
    for name in &restored {
        open_tab_in(&mut app, backend, &events_tx, cols, rows, Some(name), None).await?;
    }
    // A restart (menu "Restart wi", or after an update) left the layout in state.toml: reopen it.
    let mut restore: Option<crate::restart::Snapshot> = None;
    let mut restore_ids: Vec<Option<usize>> = Vec::new();
    if std::env::var_os(crate::restart::MARKER).is_some() {
        // SAFETY: still single-threaded here as far as env access goes; nothing else reads it.
        unsafe { std::env::remove_var(crate::restart::MARKER) };
        let mut st = agents::State::load();
        if let Some(mut sn) = st.restart.take() {
            let _ = st.save();
            if sn.key == agents::workspace_key(ws, &app.root) {
                sn.clamp();
                restore = Some(sn);
            }
        }
    }
    let restore_on = cfg.defaults.restore_tabs && std::env::var("WICLI_RESTORE_TABS").as_deref() != Ok("0");
    let mut reopened = false;
    if restore.is_none() && (restore_on || fresh) {
        // A normal start: reopen what the last quit in this workspace left (or, with --fresh, forget it).
        let mut st = agents::State::load();
        let key = agents::workspace_key(ws, &app.root);
        if fresh {
            if st.sessions.remove(&key).is_some() {
                let _ = st.save();
            }
        } else if let Some(mut sn) = st.sessions.get(&key).cloned()
            && sn.is_fresh(crate::restart::now_secs())
        {
            if !ws.is_remote() {
                sn.prune(crate::restart::local_tab_exists);
            }
            if !sn.tabs.is_empty() {
                sn.clamp();
                restore = Some(sn);
                reopened = true;
            }
        }
    }
    if let Some(sn) = &restore {
        let st = agents::State::load();
        if !sn.side.is_empty() { app.side = Side::parse(&sn.side); }
        if (crate::tab::PCT_MIN..=crate::tab::PCT_MAX).contains(&sn.pct) { app.pct = sn.pct; }
        app.pin_side = if sn.pin_side == "left" { Side::Left } else { Side::Right };
        let ropts = Opts { csv_rows: cfg.defaults.csv_rows };
        let rcap = cfg.defaults.preview_read_kb as usize * 1024;
        let (c, r) = shell_size(sn.tabs.len().max(1))?;
        for t in &sn.tabs {
            let mut id = None;
            if t.kind == "preview" {
                if let Ok((bytes, total)) = backend.read_prefix(&t.path, rcap).await {
                    let p = Preview::build(&t.path, &bytes, total, &ropts);
                    let nid = app.next_id;
                    app.next_id += 1;
                    app.tabs.push(Tab::new_preview(nid, p));
                    id = Some(nid);
                }
            } else if restored.is_empty() {
                // Local shells (and remote ones without tmux) start fresh where they were.
                let dir = if t.cwd.is_empty() { None } else { Some(t.cwd.as_str()) };
                let nid = app.next_id;
                if open_tab_in(&mut app, backend, &events_tx, c, r, None, dir).await.is_ok() {
                    id = Some(nid);
                    if t.kind == "agent" && let Some(ag) = Agent::from_key(&t.agent) {
                        let key = agents::workspace_key(ws, &t.cwd);
                        let cmd = agents::command_for(ag, cfg, &st, &key);
                        let idx = app.tabs.len() - 1;
                        run_agent_in(&mut app, idx, ag, &cmd);
                    }
                }
            }
            if let Some(i) = id.and_then(|id| app.tabs.iter().position(|x| x.id == id)) && !t.name.is_empty() {
                app.tabs[i].set_custom(Some(&t.name));
            }
            restore_ids.push(id);
        }
        // Reattached remote tabs were opened above, before the snapshot was read: names go by order.
        if !restored.is_empty() {
            let named = sn.tabs.iter().filter(|t| t.kind != "preview");
            for (tab, t) in app.tabs.iter_mut().filter(|x| !x.is_preview()).zip(named) {
                if !t.name.is_empty() { tab.set_custom(Some(&t.name)); }
            }
        }
        if let Some(Some(id)) = restore_ids.get(sn.active) && let Some(i) = app.tabs.iter().position(|t| t.id == *id) {
            app.active = i;
        }
        refresh_labels(&mut app);
    }
    if app.tabs.is_empty() {
        open_tab(&mut app, backend, &events_tx, cols, rows).await?;
    }

    // Theme first: it may ask the terminal for its background (OSC 11, ~150 ms at most).
    crate::setup_theme(cfg);
    let _guard = TermGuard::enter(cfg.defaults.mouse)?;
    let keys_on = !cfg.defaults.key_protocol.trim().eq_ignore_ascii_case("off");
    if keys_on { crate::term::enable_key_protocol(); }
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut stdin = tokio::io::stdin();
    let mut winch = signal(SignalKind::window_change())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sighup = signal(SignalKind::hangup())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut buf = [0u8; 8192];
    let mut mode = Mode::Shell;
    let mut mouse_on = cfg.defaults.mouse;
    let mut agent_state = agents::State::load();
    let mut theme_mode = crate::theme::Mode::at_start(&cfg.defaults.theme, agent_state.theme.as_deref());
    // "Update wi" runs on worker threads and reports back here; one at a time.
    let (upd_tx, mut upd_rx) = mpsc::unbounded_channel::<UpdEv>();
    let mut updating = false;
    let mut upd_held: Option<UpdEv> = None;
    let mut ask_ctx = AskCtx { key: agents::workspace_key(ws, &app.root), dir: app.root.clone(), tab: None };
    let rkey = agents::recents_key(ws);
    let pane_key = agents::workspace_key(ws, &app.root);
    app.history = agent_state.files.get(&pane_key).cloned().unwrap_or_default();
    let mut branches: std::collections::HashMap<String, Option<String>> = Default::default();
    let mut detail_key: Option<(usize, String)> = None;
    let mut splitter = mouse::Splitter::default();
    let mut sel: Option<SelState> = None;
    let mut last_click: Option<(Instant, Pos, u8)> = None;
    let open_cfg = cfg.open.clone();
    let link_click = cfg.defaults.link_click.clone();
    let confirm_needed = cfg.defaults.confirm_open != "never";
    let mut sel_reset = false;
    let mut pending: Option<Pending> = None;
    let mut fetching: Option<Fetching<'_>> = None;
    let mut job: Option<Job<'_>> = None;
    // The explorer's reads, one at a time, and its pane split kept for the session.
    let mut ex_job: Option<ExJob<'_>> = None;
    let mut ex_pct: u16 = 40;
    // Explorers left with Esc/Q, by folder, so folds, selection, sort and marks survive.
    let mut saved_ex: HashMap<String, Box<ExplorerState>> = HashMap::new();
    let mut last_results: Option<Box<Results>> = None;
    let mut filter = PasteFilter::default();
    let mut flush_at: Option<Instant> = None;
    let mut decoder = KeyDecoder::default();
    // While a paste is in flight: (forward it to the shell, wrap it in markers).
    let mut pasting: Option<(bool, bool)> = None;
    let mut message: Option<String> = None;
    // Hands-free dictation: the `wi-audio listen` child reports phrases here; one run at a time.
    let (dict_tx, mut dict_rx) = mpsc::unbounded_channel::<(u64, dictate::Ev)>();
    let mut dict: Option<dictate::Dictation> = None;
    let mut dict_gen: u64 = 0;
    let tcfg = cfg.transcribe.clone();
    let tuning = dictate::Tuning::from_env();
    // First-use set-up and settings of dictation: workers report here, tagged with the run.
    let (voice_tx, mut voice_rx) = mpsc::unbounded_channel::<(u64, voice::VEv)>();
    let mut vgen: u64 = 0;
    let mut vargv: Option<Vec<String>> = None;
    let mut vcancel = Arc::new(AtomicBool::new(false));
    let mut vstarted = Instant::now();
    if reopened {
        message = Some(format!("reopened {} tab{} from last session (config: restore_tabs = false to turn off)", app.tabs.len(), if app.tabs.len() == 1 { "" } else { "s" }));
    } else if restore.is_some() {
        message = Some(format!("wi restarted: {} tab(s) back", app.tabs.len()));
    } else if ws.is_remote() && !backend.has_tmux() {
        message = Some("no tmux on this box (or use_tmux = false): plain shells, quitting ends them".into());
    } else if !restored.is_empty() {
        message = Some(format!("reattached {} running session(s)", restored.len()));
    }
    if let Some(note) = crate::theme::unknown_theme_note(&cfg.defaults.theme, agent_state.theme.as_deref()) {
        message = Some(note);
    }
    let (name, base_detail) = ws.title();
    // Header detail: the active tab's folder and its git branch. The branch of a folder is asked
    // once (and again when the Git view opens or reloads); no timers.
    let mut detail = base_detail.clone();
    let remote = ws.is_remote();
    ui::set_theme_mode(theme_mode);
    let mut items = menu_items_for(remote, backend.has_tmux());
    ui::set_tmux_menu(backend.has_tmux());
    let opts = Opts { csv_rows: cfg.defaults.csv_rows };
    let read_cap = cfg.defaults.preview_read_kb as usize * 1024;
    // Auto-refresh of visible file previews: poll state (see refresh.rs).
    let refresh_on = cfg.defaults.preview_autorefresh;
    let refresh_debounce = Duration::from_millis(cfg.defaults.preview_refresh_debounce_ms);
    let refresh_poll = crate::refresh::poll_interval(remote);
    let mut watches: std::collections::HashMap<String, crate::refresh::Watch> = Default::default();
    let mut rjob: Option<RJob<'_>> = None;
    let mut next_poll = Instant::now();
    let mut note: Option<(Instant, String)> = None;
    // One-time "new line" hint: due a moment after an agent tab is first shown, so the host's
    // answer to the key-protocol query has arrived. Never once Alt+Enter has been used.
    let mut hint_at: Option<Instant> = None;
    let mut hint_done = agent_state.newline_hint_seen || !keys_on;
    let frame = Duration::from_secs_f64(1.0 / fps.clamp(1, 120) as f64);
    let mut last_draw = Instant::now() - frame;
    let mut dirty = true;
    // A terminal can throw away what it showed while its tab was hidden. After a pause in input,
    // the next key or click repaints the whole screen once instead of only what changed.
    const IDLE_REPAINT: Duration = Duration::from_millis(1500);
    let mut last_input = Instant::now();
    let mut repaint = false;

    // Recognise Claude Code / Codex workspaces: start the remembered agent, or ask the first time.
    let root = app.root.clone();
    let detected = agents::detect(backend, &root).await;
    let decision = if restore.is_some() { Decision::Nothing } else { agents::decide(&cfg.defaults.agent_start, &detected, agent_state.workspace.get(&ask_ctx.key)) };
    match decision {
        Decision::Nothing => {}
        Decision::Start(ag) => {
            let cmd = agents::command_for(ag, cfg, &agent_state, &ask_ctx.key);
            message = Some(match start_agent(&mut app, backend, &events_tx, ag, &cmd, &root).await {
                Ok(()) => format!("started: {cmd}   ({menu_label}, then A, changes it)"),
                Err(e) => format!("could not start {}: {e}", ag.key()),
            });
        }
        Decision::Ask(list) => mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &ask_ctx.key, &ask_ctx.dir)),
    }

    // The pinned-pane layout remembered for this workspace.
    if let Some(sn) = restore.as_ref().filter(|s| s.pinned.is_some()) {
        let keep = app.tabs[app.active].id;
        if let Some(Some(id)) = sn.pinned.and_then(|p| restore_ids.get(p)) {
            let ps = app.pin_side;
            match pin_tab(&mut app, backend, &events_tx, *id, ps).await {
                Ok(()) => if keep != *id && let Some(i) = app.tabs.iter().position(|t| t.id == keep) { app.active = i; sync_pin(&mut app); },
                Err(e) => message = Some(format!("pinned pane not restored: {e}")),
            }
        }
    } else if restore.is_none() && let Some(l) = agent_state.panes.get(&pane_key).cloned() {
        app.pin_side = if l.side == "left" { Side::Left } else { Side::Right };
        if (crate::tab::PCT_MIN..=crate::tab::PCT_MAX).contains(&l.pct) { app.pct = l.pct; }
        let pinned = match l.kind.as_str() {
            "agent" => app.tabs.iter().find(|t| t.agent.as_deref() == Some(l.target.as_str())).map(|t| t.id),
            "shell" if !l.target.is_empty() => {
                let (c, r) = shell_size(app.tabs.len() + 1)?;
                match open_tab_in(&mut app, backend, &events_tx, c, r, None, Some(&l.target)).await {
                    Ok(()) => app.tabs.last().map(|t| t.id),
                    Err(_) => None,
                }
            }
            "preview" if !l.target.is_empty() => match backend.read_prefix(&l.target, read_cap).await {
                Ok((bytes, total)) => {
                    let p = Preview::build(&l.target, &bytes, total, &opts);
                    let id = app.next_id;
                    app.next_id += 1;
                    app.tabs.push(Tab::new_preview(id, p));
                    Some(id)
                }
                Err(_) => None,
            },
            _ => None,
        };
        if let Some(id) = pinned {
            let keep = app.tabs[app.active].id;
            let ps = app.pin_side;
            match pin_tab(&mut app, backend, &events_tx, id, ps).await {
                Ok(()) => {
                    // Start with the keyboard where it was (the pinned tab was just added at the end).
                    if keep != id && let Some(i) = app.tabs.iter().position(|t| t.id == keep) { app.active = i; sync_pin(&mut app); }
                }
                Err(e) => message = Some(format!("pinned pane not restored: {e}")),
            }
        }
    }

    loop {
        if std::mem::take(&mut app.hist_dirty) {
            agent_state.files.insert(pane_key.clone(), app.history.clone());
            let _ = agent_state.save();
        }
        // An update answer that arrived while another screen was open waits for the shell.
        if matches!(mode, Mode::Shell) && let Some(ev) = upd_held.take() {
            on_update_event(ev, &mut mode, &mut pending, &mut message, &mut app, &mut updating, &opts, remote, backend.has_tmux());
            dirty = true;
        }
        // Dictation: switch off after the idle time, and paste waiting phrases when the focused
        // pane can take them and the user is not mid-keystroke.
        let mut dict_wake: Option<Instant> = None;
        if let Some(d) = dict.as_mut() {
            let now = Instant::now();
            if !d.idle.is_zero() && now >= d.idle_deadline() && matches!(d.state, dictate::Listen::Listening | dictate::Listen::Loading) {
                let secs = d.idle.as_secs_f64();
                let span = if secs >= 60.0 && secs % 60.0 == 0.0 { format!("{} min", secs / 60.0) } else { format!("{secs} s") };
                dict = None;
                message = Some(format!("dictation off: {span} of silence"));
                dirty = true;
            } else {
                let t = &app.tabs[app.active];
                let can = matches!(mode, Mode::Shell) && pasting.is_none() && !t.is_preview() && !t.side_focused();
                let recent = d.last_paste.is_some_and(|p| now.duration_since(p) < Duration::from_millis(1500));
                let lead = tuning.leading_space && !d.skip_space && (recent || !dictate::after_space(t.parser.screen()));
                match dictate::next_step(&mut d.held, now, last_input, can, Duration::from_secs(tuning.held_timeout_s), lead) {
                    dictate::Step::Control(c) => {
                        use dictate::Control;
                        match c {
                            Control::Scratch => match d.last_phrase {
                                Some((tab, n)) if tab == t.id && n > 0 && d.last_paste.is_some_and(|p| last_input <= p) => {
                                    t.write(vec![0x7f; n]);
                                    d.last_phrase = None;
                                    d.pending_send = None;
                                    message = Some("scratched the last phrase: say it again".into());
                                    crate::dictlog::log(&format!("scratch chars={n}"));
                                }
                                _ => {
                                    message = Some("nothing to scratch: only the last phrase, and only before you type".into());
                                    crate::dictlog::log("scratch refused (nothing to remove, or you typed since)");
                                }
                            },
                            Control::NewLine | Control::NewParagraph => {
                                if t.is_agent() {
                                    let n = if c == Control::NewParagraph { 2 } else { 1 };
                                    for _ in 0..n { t.write(b"\x1b\r".to_vec()); }
                                    d.skip_space = true;
                                    d.last_phrase = None;
                                    crate::dictlog::log(&format!("newline x{n}"));
                                } else {
                                    message = Some("new line works in an agent prompt (a plain shell would run the command)".into());
                                }
                            }
                        }
                        snap_live(&mut app);
                        dict_wake = Some(now + Duration::from_millis(150));
                        dirty = true;
                    }
                    dictate::Step::Paste(s) => {
                        let wrap = t.parser.screen().bracketed_paste();
                        let chars = s.chars().count();
                        t.write(dictate::paste_bytes(&s, wrap));
                        d.last_paste = Some(now);
                        d.last_phrase = Some((t.id, chars));
                        d.skip_space = false;
                        d.send_note = "";
                        crate::dictlog::log(&format!("paste tab={} chars={chars} bracketed={wrap} auto_send={}", t.id, agent_state.voice_auto_send));
                        if agent_state.voice_auto_send {
                            d.pending_send = Some(app.tabs[app.active].id);
                        }
                        snap_live(&mut app);
                        dict_wake = Some(now + Duration::from_millis(150));
                        dirty = true;
                    }
                    dictate::Step::Dropped(n) => {
                        message = Some(format!("dictation: dropped {n} phrase{} the pane could not take", if n == 1 { "" } else { "s" }));
                        dirty = true;
                    }
                    dictate::Step::Wait(w) => dict_wake = w,
                }
                // Auto-send: after the quiet, press Enter in the pane the text went to.
                if d.pending_send.is_some() {
                    let t = &app.tabs[app.active];
                    let can = matches!(mode, Mode::Shell) && pasting.is_none() && !t.is_preview() && !t.side_focused();
                    match dictate::send_step(d.pending_send, t.id, can, !d.held.is_empty(), matches!(d.state, dictate::Listen::Hearing), now, d.last_paste, d.last_speech, last_input, dictate::auto_send_delay()) {
                        dictate::Send::Enter => {
                            crate::dictlog::log("auto Enter: sent");
                            t.write(b"\r".to_vec());
                            d.pending_send = None;
                            message = Some("dictation pressed Enter".into());
                            snap_live(&mut app);
                            dirty = true;
                        }
                        dictate::Send::Cancel(why) => {
                            d.pending_send = None;
                            crate::dictlog::log(&format!("auto Enter: cancelled ({why})"));
                            message = Some(format!("auto Enter cancelled: {why}"));
                            dirty = true;
                        }
                        dictate::Send::WaitUntil(w) => {
                            if d.send_note != "armed" {
                                d.send_note = "armed";
                                crate::dictlog::log(&format!("auto Enter: armed, due in {} ms", w.saturating_duration_since(now).as_millis()));
                            }
                            dict_wake = Some(dict_wake.map_or(w, |x| x.min(w)));
                        }
                        dictate::Send::Nothing => {
                            let why = if !d.held.is_empty() { "a phrase is waiting to be typed" } else if matches!(d.state, dictate::Listen::Hearing) { "speech is being heard" } else if !can { "this pane cannot take keys now (a dialog is open, or a preview or side pane has focus)" } else { "waiting" };
                            if d.send_note != why {
                                d.send_note = why;
                                crate::dictlog::log(&format!("auto Enter: holding ({why})"));
                            }
                        }
                    }
                }
                if !d.idle.is_zero() && matches!(d.state, dictate::Listen::Listening | dictate::Listen::Loading) {
                    let idle_at = d.idle_deadline();
                    dict_wake = Some(dict_wake.map_or(idle_at, |w| w.min(idle_at)));
                }
            }
        }
        // Keep the header on the active tab's folder and that folder's branch.
        if let Some(fi) = folder_tab(&app) {
            let (fid, fdir) = (app.tabs[fi].id, app.tabs[fi].dir.as_str());
            if detail_key.as_ref().map(|(i, d)| (*i, d.as_str())) != Some((fid, fdir)) {
                let dir = fdir.to_string();
                let branch = match branches.get(&dir) {
                    Some(b) => b.clone(),
                    None => {
                        let b = tokio::time::timeout(Duration::from_secs(2), backend.git_branch(&dir)).await.ok().flatten();
                        branches.insert(dir.clone(), b.clone());
                        b
                    }
                };
                detail = folder_detail(ws, &base_detail, &dir, branch.as_deref());
                detail_key = Some((fid, dir));
                dirty = true;
            }
        }
        if !matches!(mode, Mode::Explorer(_)) {
            ex_job = None;
        } else if ex_job.is_none() && let Mode::Explorer(ex) = &mut mode && let Some((epoch, req)) = ex.next_req() {
            ex_job = Some(explorer::start(backend, ex.root.clone(), epoch, req, ex.read_cap));
        }
        let due = last_draw + frame;
        // Polling runs only while a file preview is on screen; an idle wi with none has no timer.
        let mut vis = if refresh_on { visible_files(&app) } else { Vec::new() };
        // A diff open in the explorer is watched through the file it shows (no git polling).
        let ex_diff = match &mode { Mode::Explorer(ex) if refresh_on => ex.diff_watch_path(), _ => None };
        if let Some(p) = &ex_diff && !vis.contains(p) {
            vis.push(p.clone());
        }
        if refresh_on && !watches.is_empty() {
            watches.retain(|k, _| ex_diff.as_ref() == Some(k) || app.tabs.iter().any(|t| t.preview.iter().chain(t.side.iter()).any(|p| &p.path == k)));
        }
        if vis.is_empty() { rjob = None; }
        if decoder.alt_enter_seen && !agent_state.newline_hint_seen {
            agent_state.newline_hint_seen = true;
            hint_done = true;
            let _ = agent_state.save();
        }
        if !hint_done && hint_at.is_none() && matches!(mode, Mode::Shell) && app.tabs[app.active].is_agent() {
            hint_at = Some(Instant::now() + Duration::from_millis(1500));
        }
        let hint_wait = if hint_done { None } else { hint_at };
        let note_at = match (note.as_ref().map(|n| n.0), hint_wait) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        // Earliest moment a still-printing background tab counts as quiet. No tab streaming
        // means no deadline, so an idle wi has no timers running.
        let quiet_at = app.tabs.iter().enumerate()
            .filter(|(i, t)| *i != app.active && t.activity && !t.quiet)
            .map(|(_, t)| t.last_output + app.quiet)
            .min();
        tokio::select! {
            _ = sleep_until(dict_wake.unwrap_or(due).into()), if dict_wake.is_some() => { dirty = true; }
            Some((g, ev)) = voice_rx.recv() => {
                use voice::VEv;
                if g == vgen && matches!(mode, Mode::Shell | Mode::Voice(_)) {
                    let recheck = |vgen: &mut u64| {
                        *vgen += 1;
                        voice::spawn_check(&voice_tx, *vgen, voice::locate());
                    };
                    match ev {
                        VEv::Checked { argv, status } => {
                            vargv = argv;
                            let (st, err) = match status {
                                Some(Ok(s)) => (Some(s), None),
                                Some(Err(e)) => (None, Some(e)),
                                None => (None, None),
                            };
                            let vstatus = st;
                            if let Some(e) = err {
                                mode = Mode::Voice(voice::Dlg::info("The voice helper does not answer", vec![e, "Choose Remove voice in the Dictation options, then Dictation again, to reinstall it.".into()]));
                            } else {
                                match voice::next(vargv.is_some(), vstatus.as_ref(), &agent_state.voice_declined, agent_state.voice_mic_ok) {
                                    voice::Next::Ask(step) => mode = Mode::Voice(voice::Dlg::ask(step, vstatus.as_ref().map_or(0, |s| s.speech_total))),
                                    voice::Next::NotNow(step) => message = Some(voice::not_now_text(step).into()),
                                    voice::Next::Start => start_dictation(vargv.as_deref().unwrap_or(&[]), &tcfg, &mut dict_gen, &dict_tx, &mut dict, &mut message),
                                }
                            }
                        }
                        VEv::Status(s) => {
                            if let Mode::Voice(d) = &mut mode { d.status_line(&s); }
                        }
                        VEv::Progress { what, done, total } => {
                            if let Mode::Voice(d) = &mut mode { d.progress(&what, done, total); }
                        }
                        VEv::Level(v) => {
                            if let Mode::Voice(d) = &mut mode { d.level(v, vstarted.elapsed()); }
                        }
                        VEv::Installed(Ok(path)) => {
                            agent_state.voice_declined.retain(|d| d != "helper");
                            let _ = agent_state.save();
                            message = Some(format!("voice helper installed: {}", path.display()));
                            if let Mode::Voice(d) = &mut mode { d.status_line("installed. Checking ..."); }
                            recheck(&mut vgen);
                        }
                        VEv::Installed(Err(e)) => mode = Mode::Voice(voice::Dlg::info("Could not install the voice helper", vec![e])),
                        VEv::ModelDone(Ok(())) => {
                            agent_state.voice_declined.retain(|d| d != "model" && !d.starts_with("upgrade:"));
                            let _ = agent_state.save();
                            message = Some("speech model downloaded".into());
                            if let Mode::Voice(d) = &mut mode { d.status_line("downloaded and checked. Continuing ..."); }
                            recheck(&mut vgen);
                        }
                        VEv::ModelDone(Err(e)) => mode = Mode::Voice(voice::Dlg::info("Could not download the speech model", vec![e])),
                        VEv::Heard(Ok(text)) => mode = Mode::Voice(voice::Dlg::heard(&text)),
                        VEv::Heard(Err((kind, msg))) => mode = Mode::Voice(voice::Dlg::info("Microphone test", voice::friendly(&kind, &msg).split('\n').map(String::from).collect())),
                        VEv::Removed(Ok(msg)) => {
                            message = Some(msg);
                            mode = Mode::Shell;
                        }
                        VEv::Removed(Err(e)) => mode = Mode::Voice(voice::Dlg::info("Could not remove voice", vec![e])),
                    }
                    dirty = true;
                }
            }
            Some((g, ev)) = dict_rx.recv() => {
                use dictate::{Ev, Held, Listen};
                if dict.as_ref().is_some_and(|d| d.gen_id == g) {
                    let before = dict.as_ref().map(|d| (d.button_text(false), d.held_text()));
                    let now = Instant::now();
                    let mut stopped = false;
                    if let Some(d) = dict.as_mut() {
                        match ev {
                            Ev::State(s) => {
                                d.state = s;
                                if matches!(s, Listen::Hearing | Listen::Transcribing) { d.last_speech = now; }
                            }
                            Ev::Level(v) => d.push_level(v),
                            Ev::Final(text) => {
                                d.last_speech = now;
                                let ctl = dictate::control(&text);
                                crate::dictlog::log(&format!("final chars={} control={ctl:?}", text.chars().count()));
                                d.held.push_back(Held { text, at: now, ctl });
                            }
                            Ev::Error { kind, msg } => {
                                message = Some(dictate::friendly_error(&kind, &msg));
                                if dictate::is_fatal(&kind) { d.shown_error = true; }
                                dirty = true;
                            }
                            Ev::Off(why) => d.off_why = Some(why),
                            Ev::Exited(last) => {
                                if let (false, Some(why)) = (d.shown_error, d.off_why.as_deref()) {
                                    message = Some(format!("dictation off: {why}"));
                                } else if !d.shown_error {
                                    message = Some(if last.is_empty() { "dictation stopped".to_string() } else { format!("dictation stopped: {last}") });
                                }
                                stopped = true;
                            }
                        }
                    }
                    if stopped { dict = None; dirty = true; }
                    if dict.as_ref().map(|d| (d.button_text(false), d.held_text())) != before { dirty = true; }
                }
            }
            _ = sleep_until(quiet_at.unwrap_or(due).into()), if quiet_at.is_some() => {
                let now = Instant::now();
                let mut went_quiet = false;
                for (i, t) in app.tabs.iter_mut().enumerate() {
                    if i != app.active && t.activity && !t.quiet && t.last_output + app.quiet <= now {
                        t.quiet = true;
                        went_quiet = true;
                    }
                }
                if went_quiet {
                    if cfg.defaults.bell_on_quiet {
                        use std::io::Write;
                        let _ = stdout().write_all(b"\x07");
                        let _ = stdout().flush();
                    }
                    dirty = true;
                }
            }
            _ = sleep_until(next_poll.into()), if !vis.is_empty() && rjob.is_none() => {
                next_poll = Instant::now() + refresh_poll;
                rjob = Some(start_stats(backend, vis.clone()));
            }
            done = async {
                match rjob.as_mut() {
                    Some(f) => f.as_mut().await,
                    None => std::future::pending().await,
                }
            }, if rjob.is_some() => {
                rjob = None;
                match done {
                    RDone::Stats(v) => {
                        let now = Instant::now();
                        let mut to_read = Vec::new();
                        for (path, obs) in v {
                            let w = watches.entry(path.clone()).or_insert_with(|| crate::refresh::Watch::new(refresh_debounce));
                            match w.observe(now, obs) {
                                crate::refresh::Step::Reload => {
                                    let mut is_diff = false;
                                    if let Mode::Explorer(ex) = &mut mode && ex.diff_watch_path().as_deref() == Some(path.as_str()) {
                                        ex.reload_diff();
                                        is_diff = true;
                                        dirty = true;
                                    }
                                    let shown = app.tabs.iter().any(|t| t.preview.iter().chain(t.side.iter()).any(|p| p.path == path && !matches!(p.kind, crate::preview::Kind::Diff | crate::preview::Kind::Stub)));
                                    if shown || !is_diff { to_read.push(path); }
                                }
                                crate::refresh::Step::Removed => {
                                    message = Some("file removed".into());
                                    note = Some((now + Duration::from_secs(4), "file removed".into()));
                                    dirty = true;
                                }
                                _ => {}
                            }
                        }
                        if !to_read.is_empty() { rjob = Some(start_reads(backend, to_read, read_cap)); }
                    }
                    RDone::Read(v) => {
                        for (path, res) in v {
                            let m = match res {
                                Ok((bytes, total)) => {
                                    if apply_refresh(&mut app, &path, &bytes, total, &opts) { sel = None; }
                                    "refreshed".to_string()
                                }
                                Err(e) => format!("refresh failed: {e}"),
                            };
                            message = Some(m.clone());
                            note = Some((Instant::now() + Duration::from_secs(4), m));
                            dirty = true;
                        }
                    }
                }
            }
            _ = sleep_until(note_at.unwrap_or(due).into()), if note_at.is_some() => {
                if !hint_done && hint_at.is_some_and(|h| h <= Instant::now()) {
                    hint_done = true;
                    if !decoder.enhanced && !decoder.alt_enter_seen && matches!(mode, Mode::Shell) && message.as_deref().is_none_or(|m| m.starts_with("started:")) && app.tabs[app.active].is_agent() {
                        let m = "New line: Alt+Enter (Shift+Enter isn't available in this terminal)".to_string();
                        message = Some(m.clone());
                        note = Some((Instant::now() + Duration::from_secs(6), m));
                        dirty = true;
                    }
                }
                if let Some((t, text)) = note.as_ref() && *t <= Instant::now() && message.as_deref() == Some(text.as_str()) {
                    message = None;
                    dirty = true;
                }
                if note.as_ref().is_some_and(|n| n.0 <= Instant::now()) { note = None; }
            }
            _ = sleep_until(due.into()), if dirty => {
                if std::mem::take(&mut sel_reset) { sel = None; }
                let menu = match &mode { Mode::Menu(st) => Some(st.clone()), _ => None };
                let scroll = match mode {
                    Mode::Scroll | Mode::OutFind(_) => { let s = app.tabs[app.active].parser.screen(); Some((s.scrollback(), s.scrollback_len())) }
                    _ => None,
                };
                let (pcols, prows) = shell_size(app.tabs.len())?;
                if app.pinned.is_some() {
                    // The two tabs on screen follow the pane they are in (cheap when nothing changed).
                    sync_pin(&mut app);
                    resize_all(&mut app)?;
                }
                for (i, r) in displayed(&app, Rect::new(0, 0, pcols, prows)) {
                    if let Some(p) = app.tabs[i].preview.as_mut() {
                        let h = r.height as usize - usize::from(p.has_header() && r.height > 1);
                        p.layout(r.width, h);
                    }
                }
                let (pside, ppct) = (app.side, app.pct);
                if let Some(sp) = split_main(Rect::new(0, 0, pcols, prows), pside, ppct)
                    && let Some(p) = app.tabs[app.active].side.as_mut()
                {
                    let h = sp.side.height as usize - usize::from(p.has_header() && sp.side.height > 1);
                    p.layout(sp.side.width, h);
                }
                let strip = strip_of(&app);
                let pin_split = pin_split(&app, Rect::new(0, 0, pcols, prows));
                let (pin_view, partner) = match (pin_split, partner_idx(&app)) {
                    (Some(_), Some(pi)) => {
                        let pt = &app.tabs[pi];
                        let content = match &pt.preview { Some(p) => ui::Partner::Preview(p), None => ui::Partner::Screen(pt.parser.screen()) };
                        let pinned_active = pinned_idx(&app) == Some(app.active);
                        (Some(ui::PinView { side: app.pin_side, active_is_pinned: pinned_active }), Some(content))
                    }
                    _ => (None, None),
                };
                let selection = sel.as_ref().filter(|s| s.tab == app.tabs[app.active].id).map(|s| (s.pane, select::normalize(s.anchor, s.end)));
                let progress_dlg = fetching.as_ref().map(|f| f.dialog());
                if let Ok(mr) = main_rect(&app) {
                    match &mut mode {
                        Mode::Results(r) => r.list.reveal(ui::search::results_list_rect(mr).height as usize),
                        Mode::Finder(fd) => fd.list.reveal(ui::search::finder_list_rect(mr).height as usize),
                        _ => {}
                    }
                }
                if let Mode::Explorer(ex) = &mut mode {
                    let (ec, er) = crossterm::terminal::size()?;
                    ex.prepare(ec, er);
                }
                let find_hl: Vec<(u16, u16, u16, bool)> = match &mode {
                    Mode::OutFind(f) => f.in_view(app.tabs[app.active].parser.screen().scrollback()),
                    _ => Vec::new(),
                };
                let status_s: Option<String> = match &mode {
                    Mode::Working(what) => Some(format!("{what}... Esc cancels")),
                    Mode::GrepPrompt(_) => Some("type the text · Enter searches · Esc cancels (Tab, Up, Down or a click change the options)".into()),
                    Mode::Results(_) => Some(message.clone().unwrap_or_else(|| "arrows, PgUp/PgDn · Enter opens · S new search · R run again · Esc closes".into())),
                    Mode::Finder(fd) if fd.recent.is_some() => Some(message.clone().unwrap_or_else(|| "newest first · type to narrow · arrows pick · Enter opens · Esc closes".into())),
                    Mode::Finder(_) => Some(message.clone().unwrap_or_else(|| "type to narrow · arrows pick · Enter opens · Esc closes".into())),
                    Mode::FindPrompt(_) => Some("Enter finds · Esc cancels".into()),
                    Mode::OutFind(f) => Some(format!("{}   N older · B newer · Esc leaves", f.label())),
                    _ => None,
                };
                let search_view = match &mode {
                    Mode::GrepPrompt(p) => Some(ui::search::SearchView::Grep(p)),
                    Mode::Results(r) => Some(ui::search::SearchView::Results(r)),
                    Mode::Finder(fd) => Some(ui::search::SearchView::Finder(fd)),
                    Mode::FindPrompt(p) => Some(ui::search::SearchView::Find(p)),
                    _ => None,
                };
                let confirm_ref = match &mode { Mode::Confirm(c) => Some(c), Mode::Fetch => progress_dlg.as_ref(), _ => None };
                let hints_ref = match &mode { Mode::Hints(h) => Some((&h.hints[..], h.typed.as_str())), _ => None };
                let term_w = crossterm::terminal::size().map(|s| s.0).unwrap_or(80);
                let mic_btn = dictate::button(dict.as_ref(), term_w, tcfg.enabled);
                let held_text = dict.as_ref().and_then(|d| d.held_text());
                let view = View {
                    partner,
                    pin: pin_view,
                    selection,
                    preview: app.tabs[app.active].preview.as_ref(),
                    side: app.tabs[app.active].side.as_ref(),
                    split: (app.side, app.pct),
                    focus_side: app.tabs[app.active].side_focused(),
                    prompt: match &mode { Mode::Prompt(p) => Some(p), _ => None },
                    ask: match &mode { Mode::Ask(x) => Some(x), _ => None },
                    rename: match &mode { Mode::Rename(x) => Some(x), _ => None },
                    pick: match &mode { Mode::Pick(x) => Some(x), _ => None },
                    themes: match &mode { Mode::ThemePick(x) => Some(x), _ => None },
                    confirm: confirm_ref,
                    hints: hints_ref,
                    screen: app.tabs[app.active].parser.screen(),
                    name: &name,
                    detail: &detail,
                    remote,
                    menu_label: &menu_label,
                    tabs: &strip,
                    message: message.as_deref(),
                    menu,
                    scroll,
                    search: search_view,
                    find_hl: &find_hl,
                    status: status_s.as_deref(),
                    explorer: match &mode { Mode::Explorer(e) => Some(&**e), _ => None },
                    mic: mic_btn.as_ref().map(|(t, s)| (t.as_str(), *s)),
                    held: held_text.as_deref(),
                    voice: match &mode { Mode::Voice(d) => Some(&d.view), _ => None },
                };
                let full = std::mem::take(&mut repaint);
                if full {
                    // Not Terminal::clear(): it asks the terminal for the cursor position and wants
                    // the reply on stdin, which wi itself is reading. Clear the screen directly and
                    // start from an empty drawing surface so the next frame paints everything.
                    // The clear is only queued and sits inside a synchronized update, so it goes out
                    // together with the frame and the terminal never shows a blank screen in between.
                    crossterm::queue!(stdout(), crossterm::terminal::BeginSynchronizedUpdate, crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?;
                    term = Terminal::new(CrosstermBackend::new(stdout()))?;
                }
                let drawn = term.draw(|f| ui::draw(f, &view));
                if full {
                    crossterm::execute!(stdout(), crossterm::terminal::EndSynchronizedUpdate)?;
                }
                drawn?;
                last_draw = Instant::now();
                dirty = false;
            }
            read = async {
                // A held-back Esc is a real key if nothing follows it soon.
                match flush_at {
                    Some(d) => tokio::select! {
                        r = stdin.read(&mut buf) => Some(r),
                        _ = sleep_until(d.into()) => None,
                    },
                    None => Some(stdin.read(&mut buf).await),
                }
            } => {
                let timed_out = read.is_none();
                let mut in_events = match read {
                    None => {
                        // Held-back bytes were real keys: an unfinished report first, then a lone Esc.
                        let mut v = Vec::new();
                        let held = decoder.flush();
                        if !held.is_empty() { v.push(InEv::Keys(held)); }
                        v.extend(filter.flush());
                        v
                    }
                    Some(r) => {
                        let n = r?;
                        if n == 0 { save_session(&app, backend, ws, restore_on, true).await; return finish(&app, backend).await; }
                        let now = Instant::now();
                        if now.duration_since(last_input) > IDLE_REPAINT {
                            repaint = true;
                            dirty = true;
                        }
                        last_input = now;
                        filter.feed(&buf[..n])
                    }
                };
                if keys_on && !timed_out {
                    // Enhanced key reports become the legacy bytes everything below parses. Paste
                    // text is never decoded. Shift+Enter depends on what the active tab runs.
                    let t = &app.tabs[app.active];
                    let to_shell = matches!(mode, Mode::Shell) && !t.is_preview() && !t.side_focused();
                    let map = if to_shell { EnterMap::for_tab(&cfg.defaults.shift_enter, t.is_agent()) } else { EnterMap::PLAIN };
                    let pass = (to_shell && t.compat.kitty_flags() != 0).then_some(menu_key);
                    let mut decoded = Vec::with_capacity(in_events.len());
                    for ev in in_events {
                        match ev {
                            InEv::Keys(b) => {
                                let d = decoder.decode(&b, &map, pass);
                                if !d.is_empty() { decoded.push(InEv::Keys(d)); }
                            }
                            InEv::PasteStart => {
                                let held = decoder.flush();
                                if !held.is_empty() { decoded.push(InEv::Keys(held)); }
                                decoded.push(InEv::PasteStart);
                            }
                            other => decoded.push(other),
                        }
                    }
                    in_events = decoded;
                }
                flush_at = (filter.has_pending() || decoder.has_pending()).then(|| Instant::now() + Duration::from_millis(50));
                for ev in in_events {
                  let bytes = match ev {
                    InEv::Keys(b) => {
                        // Mouse reports are pulled out of the key stream; clicks that mean a key
                        // press (menu entry, completion) are turned into that key.
                        let mut out: Vec<u8> = Vec::new();
                        for piece in splitter.split(&b) {
                            let m = match piece {
                                Piece::Bytes(x) => { if sel.take().is_some() { dirty = true; } out.extend(x); continue; }
                                Piece::Mouse(m) => m,
                            };
                            if let Mode::Explorer(ex) = &mut mode {
                                if ex.mouse(&m) == ExMouse::Enter { out.extend(b"\r"); }
                                ex_pct = ex.pct();
                                dirty = true;
                                continue;
                            }
                            let (tc, tr) = crossterm::terminal::size()?;
                            // A click on the mic button (right end of the top bar): off -> set up / turn on,
                            // on -> turn off. The tab strip is a different row, so tab clicks are untouched.
                            if matches!(mode, Mode::Shell) && matches!(m.kind, Kind::Down(Button::Left)) && m.row == 0
                                && let Some((txt, _)) = dictate::button(dict.as_ref(), tc, tcfg.enabled)
                            {
                                let (x, w) = ui::mic_span(&txt, tc);
                                if m.col >= x && m.col < x + w {
                                    if dict.take().is_some() {
                                        message = Some("dictation off".into());
                                    } else {
                                        vgen += 1;
                                        voice::spawn_check(&voice_tx, vgen, voice::locate());
                                    }
                                    dirty = true;
                                    continue;
                                }
                            }
                            let strip_h = u16::from(app.tabs.len() > 1);
                            let main = Rect::new(0, 1 + strip_h, tc, tr.saturating_sub(2 + strip_h).max(1));
                            if matches!(mode, Mode::Shell | Mode::Scroll) && matches!(m.kind, Kind::Down(_) | Kind::WheelUp | Kind::WheelDown)
                                && let Some(sp) = split_of(&app, &app.tabs[app.active], main)
                            {
                                if let Some(pi) = pin_partner(&app) {
                                    // Pinned panes: the pane under the pointer becomes the active tab.
                                    let on_btn = button_at(pinned_rect(&app, &sp), m.col, m.row).is_some();
                                    if inside(sp.side, m.col, m.row) && !on_btn {
                                        if matches!(mode, Mode::Scroll) { mode = Mode::Shell; }
                                        sel = None;
                                        switch_to(&mut app, pi);
                                        dirty = true;
                                    }
                                } else if matches!(m.kind, Kind::Down(_)) {
                                    // Clicking in a pane gives it the keyboard.
                                    let t = &mut app.tabs[app.active];
                                    if inside(sp.side, m.col, m.row) { t.focus = Pane::Side; dirty = true; }
                                    else if inside(sp.shell, m.col, m.row) { t.focus = Pane::Shell; dirty = true; }
                                }
                            }
                            match mouse_intent(&m, &mode, &app, &items, main, sel.as_ref().filter(|s| s.dragging).map(|s| s.pane), &link_click) {
                                Intent::None => {}
                                Intent::Synth(bytes) => out.extend(bytes),
                                Intent::SwitchTab(i) => { sel = None; switch_to(&mut app, i); dirty = true; }
                                Intent::RenameTab(i) => {
                                    sel = None;
                                    let t = &app.tabs[i];
                                    mode = Mode::Rename(crate::rename::Rename::new(t.id, t.custom.as_deref().unwrap_or(&t.title)));
                                    dirty = true;
                                }
                                Intent::ClosePreview => {
                                    unsplit(&mut app);
                                    sel = None;
                                    resize_all(&mut app)?;
                                    save_layout(&app, &mut agent_state, &pane_key);
                                    dirty = true;
                                }
                                Intent::SwapSides => {
                                    swap_sides(&mut app);
                                    sel = None;
                                    resize_all(&mut app)?;
                                    save_layout(&app, &mut agent_state, &pane_key);
                                    dirty = true;
                                }
                                Intent::DividerDown => { app.div_drag = true; sel = None; }
                                Intent::DividerDrag(col) => {
                                    app.pct = pct_for_col(main, if app.pinned.is_some() { app.pin_side } else { app.side }, col);
                                    resize_all(&mut app)?;
                                    dirty = true;
                                }
                                Intent::DividerUp => { app.div_drag = false; save_layout(&app, &mut agent_state, &pane_key); dirty = true; }
                                Intent::OpenLink(l, pane) => {
                                    sel = None;
                                    if matches!(mode, Mode::Scroll) { mode = Mode::Shell; }
                                    let o = resolve_link(&app, backend, ws, &l, pane, &open_cfg, confirm_needed).await;
                                    if let LinkOutcome::Explorer(dir) = o {
                                        mode = Mode::Explorer(explorer_for(dir, &mut saved_ex, remote.then(|| name.clone()), &opts, read_cap, &open_cfg, ex_pct)?);
                                    } else {
                                        apply_outcome(o, &mut app, backend, ws, &mut mode, &mut pending, &mut message, &mut fetching, read_cap, &opts).await;
                                    }
                                    dirty = true;
                                }
                                Intent::Select(phase, pos, pane) => {
                                    let tab_id = app.tabs[app.active].id;
                                    let area = pane_rect(&app, main, pane);
                                    let (rows, wrapped) = area_rows(&app, area, pane);
                                    let width = rows.first().map_or(0, |r| r.len()) as u16;
                                    match phase {
                                        Phase::Down => {
                                            let now = Instant::now();
                                            let clicks = match last_click {
                                                Some((t, p, n)) if p == pos && now.duration_since(t) < Duration::from_millis(400) => n % 3 + 1,
                                                _ => 1,
                                            };
                                            last_click = Some((now, pos, clicks));
                                            let (a, b) = match clicks {
                                                2 => {
                                                    let (c1, c2) = rows.get(pos.0 as usize).map_or((pos.1, pos.1), |r| select::word_bounds(r, pos.1 as usize));
                                                    ((pos.0, c1), (pos.0, c2))
                                                }
                                                3 => ((pos.0, 0), (pos.0, width.saturating_sub(1))),
                                                _ => (pos, pos),
                                            };
                                            sel = Some(SelState { tab: tab_id, pane, anchor: a, end: b, dragging: true, clicks });
                                        }
                                        Phase::Drag => {
                                            if let Some(s) = sel.as_mut().filter(|s| s.dragging && s.clicks == 1) { s.end = pos; }
                                        }
                                        Phase::Up => {
                                            if let Some(s) = sel.as_mut().filter(|s| s.dragging) {
                                                s.dragging = false;
                                                if s.clicks == 1 { s.end = pos; }
                                                let range = select::normalize(s.anchor, s.end);
                                                if s.clicks >= 2 || range.0 != range.1 {
                                                    let text = select::join_rows(range, &rows, &wrapped, 0);
                                                    message = Some(match crate::clip::copy(&text) {
                                                        Ok(how) => format!("copied {} characters to the {how}", text.chars().count()),
                                                        Err(e) => format!("could not copy: {e}"),
                                                    });
                                                } else {
                                                    sel = None;
                                                }
                                            }
                                        }
                                    }
                                    dirty = true;
                                }
                                Intent::PreviewScroll(n, pane) => {
                                    sel = None;
                                    let page = main.height as usize;
                                    if let Some(p) = pane_preview_mut(&mut app, pane) {
                                        let key = if n < 0 { PvKey::Up } else { PvKey::Down };
                                        for _ in 0..n.unsigned_abs() { p.navigate(key, page); }
                                    }
                                    dirty = true;
                                }
                                Intent::EnterScroll => {
                                    sel = None;
                                    // No history to browse: stay live (nothing to get stuck in).
                                    if !scroll_by(&mut app, 3) { mode = Mode::Scroll; }
                                    dirty = true;
                                }
                                Intent::ScrollWheel(n) => {
                                    sel = None;
                                    // Applied right away, so a burst of wheel events in one read sees the
                                    // position left by the previous one.
                                    if scroll_by(&mut app, n) && n < 0 { mode = Mode::Shell; }
                                    dirty = true;
                                }
                                Intent::PickCandidate(i) => {
                                    if let Mode::Prompt(pr) = &mut mode
                                        && let Some(c) = pr.shown.get(i).cloned()
                                    {
                                        pr.error = None;
                                        if pr.kind.is_folder() && pr.recent {
                                            // A recent folder: take it as typed and confirm.
                                            pr.set_input(c);
                                            pr.sel = None;
                                            out.extend(b"\r");
                                        } else {
                                            let dir = crate::files::split_input(&pr.input).0.to_string();
                                            pr.set_input(format!("{dir}{c}"));
                                            out.extend(if c.ends_with('/') { b"\t".to_vec() } else { b"\r".to_vec() });
                                        }
                                    }
                                }
                                Intent::MenuPick(i) => {
                                    if let Mode::Menu(st) = &mut mode {
                                        let entries = ui::menu_entries(&ui::menu_rows(&items, &st.filter));
                                        if let Some(pos) = entries.iter().position(|&e| e == i) {
                                            st.sel = pos;
                                            out.extend(b"\r");
                                        }
                                    }
                                }
                                Intent::MenuFilter => {
                                    if let Mode::Menu(st) = &mut mode { st.filtering = true; dirty = true; }
                                }
                                Intent::MenuClose => { mode = Mode::Shell; dirty = true; }
                                Intent::MenuWheel(d) => {
                                    if let Mode::Menu(st) = &mut mode {
                                        let n = ui::menu_entries(&ui::menu_rows(&items, &st.filter)).len();
                                        st.sel = (st.sel.min(n.saturating_sub(1)) as isize + d).clamp(0, n.saturating_sub(1) as isize) as usize;
                                        st.top = ui::menu_layout(main, &items, st).top;
                                        dirty = true;
                                    }
                                }
                                Intent::ListPick(row) => {
                                    let picked = match &mut mode {
                                        Mode::Results(r) => {
                                            let h = ui::search::results_list_rect(main).height as usize;
                                            r.list.hit(row, h).map(|i| r.list.select(i, h))
                                        }
                                        Mode::Finder(fd) => {
                                            let h = ui::search::finder_list_rect(main).height as usize;
                                            fd.list.hit(row, h).map(|i| fd.list.select(i, h))
                                        }
                                        _ => None,
                                    };
                                    if picked.is_some() {
                                        out.extend(b"\r");
                                        dirty = true;
                                    }
                                }
                            }
                        }
                        out
                    }
                    InEv::PasteStart => {
                        if matches!(mode, Mode::Scroll) { mode = Mode::Shell; dirty = true; }
                        if matches!(mode, Mode::Shell) && !app.tabs[app.active].is_preview() && !app.tabs[app.active].side_focused() { snap_live(&mut app); }
                        let fwd = matches!(mode, Mode::Shell) && !app.tabs[app.active].is_preview() && !app.tabs[app.active].side_focused();
                        let wrap = app.tabs[app.active].parser.screen().bracketed_paste();
                        if fwd && wrap { app.tabs[app.active].write(b"\x1b[200~".to_vec()); }
                        pasting = Some((fwd, wrap));
                        continue;
                    }
                    InEv::PasteData(b) => {
                        if let Mode::Prompt(p) = &mut mode {
                            p.insert(&String::from_utf8_lossy(&b));
                            if p.recent { p.recent = false; p.sel = None; p.shown.clear(); p.total = 0; }
                            p.error = None;
                            dirty = true;
                        } else if paste_search(&mut mode, &String::from_utf8_lossy(&b).replace(['\r', '\n'], "")) {
                            dirty = true;
                        } else if let Mode::Rename(r) = &mut mode {
                            r.insert(&String::from_utf8_lossy(&b));
                            dirty = true;
                        } else if let Mode::Ask(x) = &mut mode {
                            x.paste(&String::from_utf8_lossy(&b).replace(['\r', '\n'], ""));
                            dirty = true;
                        } else if matches!(pasting, Some((true, _))) { app.tabs[app.active].write(b); }
                        continue;
                    }
                    InEv::PasteEnd => {
                        if pasting == Some((true, true)) { app.tabs[app.active].write(b"\x1b[201~".to_vec()); }
                        pasting = None;
                        continue;
                    }
                  };
                let mut chunk = &bytes[..];
                // Keys typed right after the menu key in the same read still belong to the menu.
                while !chunk.is_empty() {
                    match mode {
                        Mode::Scroll => {
                            let t = &mut app.tabs[app.active];
                            let page = (t.parser.screen().size().0.get() as usize).saturating_sub(1);
                            let mut off = t.parser.screen().scrollback();
                            let max = t.parser.screen().scrollback_len();
                            let sc = scroll::scan(chunk, menu_key);
                            let mut exit = false;
                            for k in sc.keys {
                                if k == ScrollKey::Exit { exit = true; break; }
                                off = scroll::apply(off, max, page, k);
                            }
                            if exit { mode = Mode::Shell; off = 0; }
                            dirty = true;
                            match sc.input_at {
                                // Typing while scrolled back: jump to the live bottom and let the
                                // Shell branch deliver this input (and whatever follows it).
                                Some(at) if !exit => {
                                    off = 0;
                                    mode = Mode::Shell;
                                    chunk = &chunk[at..];
                                }
                                _ => chunk = &[],
                            }
                            app.tabs[app.active].parser.screen_mut().set_scrollback(off);
                        }
                        Mode::Shell => {
                            let (head, after_menu) = match chunk.iter().position(|&x| x == menu_key) {
                                Some(pos) => (&chunk[..pos], Some(&chunk[pos + 1..])),
                                None => (chunk, None),
                            };
                            let side_focus = app.tabs[app.active].side_focused();
                            if app.tabs[app.active].is_preview() || side_focus {
                                let rows = shell_size(app.tabs.len())?.1 as usize;
                                let (mut back, mut reload, mut find, mut close, mut hist) = (false, false, false, false, false);
                                if let Some(p) = focused_slot(&mut app).as_mut() {
                                    let page = rows - usize::from(p.has_header() && rows > 1);
                                    for k in pvkeys::parse(head) {
                                        match k {
                                            PvKey::Back => back = true,
                                            PvKey::Reload => reload = true,
                                            PvKey::Find => find = true,
                                            PvKey::Close => close = true,
                                            PvKey::History => hist = true,
                                            k => { p.navigate(k, page); }
                                        }
                                    }
                                    if let Some(l) = p.find_label() { message = Some(l); }
                                }
                                if hist {
                                    let in_place = focused_slot(&mut app).as_ref().is_some_and(|p| p.kind != crate::preview::Kind::Diff);
                                    match recent_finder(&app, backend, in_place).await {
                                        Ok(f) => mode = Mode::Finder(Box::new(f)),
                                        Err(m) => message = Some(m),
                                    }
                                }
                                if find { mode = Mode::FindPrompt(FindPrompt { target: FindTarget::Preview, text: String::new() }); }
                                dirty = true;
                                if back && side_focus { app.tabs[app.active].focus = Pane::Shell; }
                                else if back { go_back(&mut app); }
                                if close {
                                    if side_focus {
                                        let t = &mut app.tabs[app.active];
                                        t.side = None;
                                        t.focus = Pane::Shell;
                                        sel = None;
                                        resize_all(&mut app)?;
                                    } else {
                                        // a preview that is its own tab: close the tab
                                        let id = app.tabs[app.active].id;
                                        if remove_tab(&mut app, id)? {
                    save_session(&app, backend, ws, restore_on, false).await;
                    return finish(&app, backend).await;
                }
                                    }
                                }
                                if reload {
                                    let is_git = focused_slot(&mut app).as_ref().is_some_and(|p| p.kind == crate::preview::Kind::Diff);
                                    let res = if is_git {
                                        let gdir = active_dir(&app);
                                        reload_git(&mut app, backend).await.map(|b| { branches.insert(gdir, Some(b)); detail_key = None; })
                                    } else {
                                        reload_preview(&mut app, backend, read_cap, &opts).await
                                    };
                                    if res.is_ok() && let Some(p) = focused_slot(&mut app).as_ref() { watches.remove(&p.path); }
                                    message = match res {
                                        Ok(()) => Some("reloaded".to_string()),
                                        Err(e) => Some(format!("reload failed: {e}")),
                                    };
                                }
                            } else if !head.is_empty() {
                                // Typing with the view scrolled back: back to the live bottom first.
                                snap_live(&mut app);
                                app.tabs[app.active].write(head.to_vec());
                            }
                            match after_menu {
                                Some(rest) => {
                                    mode = Mode::Menu(MenuState::default());
                                    message = None;
                                    dirty = true;
                                    chunk = rest;
                                }
                                None => chunk = &[],
                            }
                        }
                        Mode::Working(_) => {
                            if search::decode(chunk, menu_key).iter().any(|k| matches!(k, K::Esc | K::Menu)) {
                                job = None;
                                mode = Mode::Shell;
                                message = Some("cancelled".into());
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::GrepPrompt(ref mut gp) => {
                            let (mut cancel, mut start) = (false, None);
                            for k in search::decode(chunk, menu_key) {
                                match k {
                                    K::Esc | K::Menu => {
                                        cancel = true;
                                        break;
                                    }
                                    k => {
                                        if gp.key(k) {
                                            start = Some(gp.query.clone());
                                            break;
                                        }
                                    }
                                }
                            }
                            if cancel {
                                mode = Mode::Shell;
                            } else if let Some(q) = start {
                                mode = Mode::Working("searching");
                                job = Some(start_search(backend, active_dir(&app), q));
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Results(ref mut r) => {
                            let h = ui::search::results_list_rect(main_rect(&app)?).height as usize;
                            let (mut leave, mut open, mut rerun, mut newq) = (false, false, false, false);
                            message = None;
                            for k in search::decode(chunk, menu_key) {
                                match k {
                                    K::Up => r.list.step(-1, h),
                                    K::Down => r.list.step(1, h),
                                    K::PageUp => r.list.page(false, h),
                                    K::PageDown => r.list.page(true, h),
                                    K::Home => r.list.home(h),
                                    K::End => r.list.end(h),
                                    K::Enter => {
                                        open = true;
                                        break;
                                    }
                                    K::Esc | K::Menu => {
                                        leave = true;
                                        break;
                                    }
                                    K::Char('r' | 'R') => {
                                        rerun = true;
                                        break;
                                    }
                                    K::Char('s' | 'S') => {
                                        newq = true;
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            if open && let Some(m) = r.selected().cloned() {
                                let path = anchor(&r.dir, &m.path);
                                match open_preview(&mut app, backend, &path, read_cap, &opts, Some(m.line)).await {
                                    Ok(pl) => {
                                        leave = true;
                                        if pl == Placement::TabNarrow {
                                            message = Some("narrow: opened in a tab".into());
                                        }
                                    }
                                    Err(e) => message = Some(format!("{}: {e}", sanitize(&m.path))),
                                }
                            }
                            if rerun {
                                let (d, q) = (r.dir.clone(), r.query.clone());
                                mode = Mode::Working("searching");
                                job = Some(start_search(backend, d, q));
                            } else if newq {
                                let q = r.query.clone();
                                if let Mode::Results(b) = std::mem::replace(&mut mode, Mode::GrepPrompt(GrepPrompt::new(Some(&q)))) {
                                    last_results = Some(b);
                                }
                            } else if leave && let Mode::Results(b) = std::mem::replace(&mut mode, Mode::Shell) {
                                last_results = Some(b);
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Finder(ref mut fd) => {
                            let h = ui::search::finder_list_rect(main_rect(&app)?).height as usize;
                            let (mut leave, mut open) = (false, false);
                            message = None;
                            for k in search::decode(chunk, menu_key) {
                                match k {
                                    K::Up => fd.list.step(-1, h),
                                    K::Down => fd.list.step(1, h),
                                    K::PageUp => fd.list.page(false, h),
                                    K::PageDown => fd.list.page(true, h),
                                    K::Home => fd.list.home(h),
                                    K::End => fd.list.end(h),
                                    K::Enter => {
                                        open = true;
                                        break;
                                    }
                                    K::Esc | K::Menu => {
                                        leave = true;
                                        break;
                                    }
                                    k @ (K::Char(_) | K::Backspace | K::Clear) => fd.type_keys(&[k]),
                                    _ => {}
                                }
                            }
                            if open && let Some(rm) = fd.recent.as_ref().zip(fd.selected_idx()).map(|(m, i)| m[i].clone()) {
                                if rm.missing {
                                    message = Some(format!("file not found: {}", sanitize(&rm.path)));
                                } else {
                                    let res = if fd.replace {
                                        open_in_place(&mut app, backend, &rm.path, read_cap, &opts).await.map(|()| Placement::Side)
                                    } else {
                                        open_preview(&mut app, backend, &rm.path, read_cap, &opts, None).await
                                    };
                                    match res {
                                        Ok(pl) => {
                                            leave = true;
                                            if pl == Placement::TabNarrow {
                                                message = Some("narrow: opened in a tab".into());
                                            }
                                        }
                                        Err(e) => message = Some(format!("{}: {e}", sanitize(&rm.path))),
                                    }
                                }
                            } else if open && let Some(f) = fd.selected().map(str::to_string) {
                                match open_preview(&mut app, backend, &anchor(&fd.dir, &f), read_cap, &opts, None).await {
                                    Ok(pl) => {
                                        leave = true;
                                        if pl == Placement::TabNarrow {
                                            message = Some("narrow: opened in a tab".into());
                                        }
                                    }
                                    Err(e) => message = Some(format!("{}: {e}", sanitize(&f))),
                                }
                            }
                            if leave {
                                mode = Mode::Shell;
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::FindPrompt(ref mut fp) => {
                            let (mut cancel, mut go) = (false, false);
                            for k in search::decode(chunk, menu_key) {
                                match k {
                                    K::Esc | K::Menu => {
                                        cancel = true;
                                        break;
                                    }
                                    K::Enter => {
                                        go = true;
                                        break;
                                    }
                                    k => {
                                        search::edit(&mut fp.text, k);
                                    }
                                }
                            }
                            if cancel {
                                mode = Mode::Shell;
                            } else if go && !fp.text.is_empty() {
                                let (target, text) = (fp.target, fp.text.clone());
                                mode = Mode::Shell;
                                match target {
                                    FindTarget::Output => {
                                        let t = &mut app.tabs[app.active];
                                        let f = OutFind::new(&mut t.parser, &text);
                                        if f.hits.is_empty() {
                                            message = Some(format!("no match for \"{}\" in the output", sanitize(&text)));
                                        } else {
                                            t.parser.screen_mut().set_scrollback(f.offset());
                                            mode = Mode::OutFind(Box::new(f));
                                        }
                                    }
                                    FindTarget::Preview => {
                                        if let Some(p) = focused_slot(&mut app).as_mut() {
                                            message = Some(if p.set_find(&text) == 0 { format!("no match for \"{}\"", sanitize(&text)) } else { p.find_label().unwrap_or_default() });
                                        }
                                    }
                                }
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::OutFind(ref mut f) => {
                            let mut leave = false;
                            for k in search::decode(chunk, menu_key) {
                                match k {
                                    K::Char('n' | 'N') => f.older(),
                                    K::Char('b' | 'B') => f.newer(),
                                    K::Esc | K::Menu => {
                                        leave = true;
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            if leave {
                                app.tabs[app.active].parser.screen_mut().set_scrollback(0);
                                mode = Mode::Shell;
                            } else {
                                let off = f.offset();
                                app.tabs[app.active].parser.screen_mut().set_scrollback(off);
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Explorer(ref mut ex) => {
                            let mut act = None;
                            for k in explorer::decode(chunk, menu_key) {
                                if let Some(a) = ex.key(k) {
                                    act = Some(a);
                                    break;
                                }
                            }
                            ex_pct = ex.pct();
                            match act {
                                None => {}
                                Some(ExAct::Leave) => {
                                    stash_explorer(&mut saved_ex, &mut mode);
                                }
                                Some(ExAct::Send { rels, root }) => {
                                    let target = if app.tabs[app.active].is_agent() && !app.tabs[app.active].is_preview() {
                                        Some(app.active)
                                    } else {
                                        app.tabs.iter().position(|t| t.is_agent() && !t.is_preview())
                                    };
                                    match target {
                                        None => ex.message = Some(("no agent tab to send to: start one with A (or the menu)".into(), true)),
                                        Some(i) => {
                                            let base = app.tabs[i].dir.trim_end_matches('/').to_string();
                                            let same = base == root.trim_end_matches('/');
                                            let mut text = String::new();
                                            for r in &rels {
                                                text.push('@');
                                                text.push_str(&if same { r.clone() } else { format!("{}/{r}", root.trim_end_matches('/')) });
                                                text.push(' ');
                                            }
                                            app.tabs[i].write(text.into_bytes());
                                            ex.sent();
                                            stash_explorer(&mut saved_ex, &mut mode);
                                            switch_to(&mut app, i);
                                        }
                                    }
                                }
                                Some(ExAct::Copy { text, what }) => {
                                    ex.message = Some(match crate::clip::copy(&text) {
                                        Ok(how) => (format!("copied {what} ({how})"), false),
                                        Err(e) => (format!("could not copy: {e}"), true),
                                    });
                                }
                                Some(ExAct::OpenFile(abs)) => {
                                    // Resolved exactly like a clicked path: text opens in the preview,
                                    // complex files and downloads ask first, programs are refused.
                                    let o = resolve_link(&app, backend, ws, &explorer::path_link(&abs), Pane::Shell, &open_cfg, confirm_needed).await;
                                    if let LinkOutcome::Message(m) = &o {
                                        ex.message = Some((sanitize(m), true));
                                    } else {
                                        stash_explorer(&mut saved_ex, &mut mode);
                                        apply_outcome(o, &mut app, backend, ws, &mut mode, &mut pending, &mut message, &mut fetching, read_cap, &opts).await;
                                    }
                                }
                                Some(act @ (ExAct::Editor { .. } | ExAct::NewTab(_))) => {
                                    let (file, dir) = match act {
                                        ExAct::Editor { file, dir } => (Some(file), dir),
                                        ExAct::NewTab(dir) => (None, dir),
                                        _ => unreachable!(),
                                    };
                                    let (c, r) = shell_size(app.tabs.len() + 1)?;
                                    let had_strip = app.tabs.len() > 1;
                                    match open_tab_in(&mut app, backend, &events_tx, c, r, None, Some(&dir)).await {
                                        Ok(()) => {
                                            app.active = app.tabs.len() - 1;
                                            if let Some(f) = file {
                                                app.tabs[app.active].write(explorer::editor_command(&f).into_bytes());
                                            }
                                            if !had_strip { resize_all(&mut app)?; }
                                            stash_explorer(&mut saved_ex, &mut mode);
                                        }
                                        Err(e) => ex.message = Some((format!("could not open shell: {e}"), true)),
                                    }
                                }
                                Some(ExAct::Agent(dir)) => {
                                    let mut list = agents::detect(backend, &dir).await;
                                    if list.is_empty() { list = agents::ALL.to_vec(); }
                                    ask_ctx = AskCtx { key: agents::workspace_key(ws, &dir), dir, tab: None };
                                    stash_explorer(&mut saved_ex, &mut mode);
                                    mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &ask_ctx.key, &ask_ctx.dir));
                                }
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::ThemePick(ref mut tp) => {
                            use crate::theme::Mode as TM;
                            let (mut commit, mut cancel) = (false, false);
                            for k in parse_keys(chunk, menu_key) {
                                match k {
                                    Key::Up => tp.step(-1),
                                    Key::Down => tp.step(1),
                                    Key::PageUp => tp.step(-5),
                                    Key::PageDown => tp.step(5),
                                    Key::Home => tp.sel = 0,
                                    Key::End => tp.sel = tp.entries().len().saturating_sub(1),
                                    Key::Enter => commit = true,
                                    Key::Esc if !tp.filter.is_empty() => { tp.filter.clear(); tp.sel = 0; }
                                    Key::Esc | Key::MenuPressed => cancel = true,
                                    Key::Backspace => { tp.filter.pop(); tp.sel = 0; }
                                    Key::Char(c) => tp.type_char(c),
                                    _ => {}
                                }
                                if commit || cancel { break; }
                            }
                            let picked = tp.current();
                            let orig = tp.orig;
                            if cancel {
                                crate::theme::set_theme(orig.theme());
                                mode = Mode::Shell;
                            } else if commit && let Some(m) = picked {
                                theme_mode = m;
                                crate::theme::set_theme(m.theme());
                                ui::set_theme_mode(m);
                                items = menu_items_for(remote, backend.has_tmux());
                                mode = Mode::Shell;
                                if m != orig {
                                    agent_state.theme = m.saved();
                                    let cfg_note = match crate::theme::index_of(&cfg.defaults.theme) {
                                        Some(_) => format!(" (config theme = \"{}\" wins on restart)", cfg.defaults.theme.trim()),
                                        None => String::new(),
                                    };
                                    let shows = if m == TM::Auto { format!(" -> {}", m.theme().label) } else { String::new() };
                                    message = Some(match agent_state.save() {
                                        Ok(()) => format!("theme: {}{shows}{cfg_note}", m.name()),
                                        Err(e) => format!("theme: {} (could not save: {e})", m.name()),
                                    });
                                }
                            } else if commit {
                                // Nothing matched the filter: keep the picker open.
                            } else if let Some(m) = picked {
                                // Live preview of the highlighted row.
                                crate::theme::set_theme(m.theme());
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Voice(ref d) => {
                            use voice::{Key, Kind, Step};
                            let (key, kind) = (d.key(chunk, menu_key), d.kind.clone());
                            chunk = &[];
                            dirty = true;
                            let argv = vargv.clone();
                            match (kind, key) {
                                (Kind::Ask(step), Key::Yes) => {
                                    vgen += 1;
                                    vcancel = Arc::new(AtomicBool::new(false));
                                    vstarted = Instant::now();
                                    match (step, argv) {
                                        (Step::Install, _) => voice::spawn_install(&voice_tx, vgen),
                                        (Step::Model | Step::Upgrade, Some(a)) => voice::spawn_fetch(&voice_tx, vgen, a, vcancel.clone(), false),
                                        (Step::Mic, Some(a)) => voice::spawn_probe(&voice_tx, vgen, a, vcancel.clone()),
                                        _ => {}
                                    }
                                    mode = Mode::Voice(voice::Dlg::busy(step));
                                }
                                (Kind::Ask(step), Key::No) => {
                                    mode = Mode::Shell;
                                    if step == Step::Mic {
                                        if !agent_state.voice_declined.iter().any(|d| d == "mic") { agent_state.voice_declined.push("mic".into()); let _ = agent_state.save(); }
                                        start_dictation(vargv.as_deref().unwrap_or(&[]), &tcfg, &mut dict_gen, &dict_tx, &mut dict, &mut message);
                                    } else if step == Step::Upgrade {
                                        // Keep working with the model we have; do not ask about this upgrade again.
                                        let key = format!("upgrade:{}", voice::BEST_MODEL);
                                        if !agent_state.voice_declined.contains(&key) { agent_state.voice_declined.push(key); let _ = agent_state.save(); }
                                        start_dictation(vargv.as_deref().unwrap_or(&[]), &tcfg, &mut dict_gen, &dict_tx, &mut dict, &mut message);
                                        message = Some(voice::not_now_text(step).into());
                                    } else {
                                        if !agent_state.voice_declined.iter().any(|d| d == step.key()) { agent_state.voice_declined.push(step.key().into()); let _ = agent_state.save(); }
                                        message = Some(voice::not_now_text(step).into());
                                    }
                                }
                                (Kind::Busy(_), Key::No) => {
                                    vgen += 1;
                                    vcancel.store(true, Ordering::Relaxed);
                                    mode = Mode::Shell;
                                    message = Some("cancelled".into());
                                }
                                (Kind::Heard, Key::Yes) => {
                                    agent_state.voice_mic_ok = true;
                                    agent_state.voice_declined.retain(|d| d != "mic");
                                    let _ = agent_state.save();
                                    mode = Mode::Shell;
                                    start_dictation(vargv.as_deref().unwrap_or(&[]), &tcfg, &mut dict_gen, &dict_tx, &mut dict, &mut message);
                                }
                                (Kind::Heard | Kind::Info, Key::Yes | Key::No) => mode = Mode::Shell,
                                (Kind::On, Key::Yes) => {
                                    dict = None;
                                    mode = Mode::Shell;
                                    message = Some("dictation off".into());
                                }
                                (Kind::On, Key::Char('o')) => mode = Mode::Voice(voice::Dlg::options(agent_state.voice_auto_send)),
                                (Kind::On, Key::No) => mode = Mode::Shell,
                                (Kind::Options, Key::Char('t')) => {
                                    // The test needs the microphone: dictation stops, and starts again after it.
                                    dict = None;
                                    vargv = voice::locate();
                                    vgen += 1;
                                    vcancel = Arc::new(AtomicBool::new(false));
                                    vstarted = Instant::now();
                                    if let Some(a) = vargv.clone() { voice::spawn_probe(&voice_tx, vgen, a, vcancel.clone()); }
                                    mode = Mode::Voice(voice::Dlg::busy(Step::Mic));
                                }
                                (Kind::Options, Key::Char('a')) => {
                                    agent_state.voice_auto_send = !agent_state.voice_auto_send;
                                    let _ = agent_state.save();
                                    if let Some(d) = dict.as_mut() { d.pending_send = None; }
                                    message = Some(if agent_state.voice_auto_send { "dictation: Enter is pressed after 3 s of quiet".into() } else { "dictation: Enter is yours again".into() });
                                    mode = Mode::Voice(voice::Dlg::options(agent_state.voice_auto_send));
                                }
                                (Kind::Options, Key::Char('d')) => {
                                    // Fetch the current speech model again (the old file is replaced only after the new one checks out).
                                    dict = None;
                                    vargv = voice::locate();
                                    vgen += 1;
                                    vcancel = Arc::new(AtomicBool::new(false));
                                    vstarted = Instant::now();
                                    if let Some(a) = vargv.clone() { voice::spawn_fetch(&voice_tx, vgen, a, vcancel.clone(), true); }
                                    mode = Mode::Voice(voice::Dlg::busy(Step::Model));
                                }
                                (Kind::Options, Key::Char('r')) => mode = Mode::Voice(voice::Dlg::ask_remove()),
                                (Kind::Options, Key::No) => mode = Mode::Voice(voice::Dlg::on()),
                                (Kind::AskRemove, Key::Yes) => {
                                    dict = None;
                                    vgen += 1;
                                    voice::spawn_remove(&voice_tx, vgen, voice::locate());
                                    mode = Mode::Shell;
                                    message = Some("removing voice ...".into());
                                }
                                (Kind::AskRemove, Key::No) => mode = Mode::Voice(voice::Dlg::options(agent_state.voice_auto_send)),
                                _ => {}
                            }
                        }
                        Mode::Confirm(_) => {
                            let (mut yes, mut no) = (false, false);
                            for (i, &b) in chunk.iter().enumerate() {
                                match b {
                                    b'\r' | b'\n' | b'y' | b'Y' => { yes = true; break; }
                                    b'n' | b'N' => { no = true; break; }
                                    0x1b if chunk.get(i + 1).is_none() => { no = true; break; }
                                    x if x == menu_key => { no = true; break; }
                                    _ => {}
                                }
                            }
                            if yes {
                                mode = Mode::Shell;
                                match pending.take() {
                                    Some(Pending::UpdateCheck) => {
                                        updating = true;
                                        message = Some("update: checking github.com ...".into());
                                        let tx = upd_tx.clone();
                                        tokio::task::spawn_blocking(move || {
                                            let _ = tx.send(UpdEv::Checked(crate::update::check(env!("CARGO_PKG_VERSION"))));
                                        });
                                    }
                                    Some(Pending::UpdateApply(rel)) => {
                                        message = Some("update: starting download ...".into());
                                        let tx = upd_tx.clone();
                                        tokio::task::spawn_blocking(move || {
                                            let mut status = |s: String| { let _ = tx.send(UpdEv::Status(format!("update: {s}"))); };
                                            let r = crate::update::install(&rel, &mut status);
                                            let _ = tx.send(UpdEv::Installed(r));
                                        });
                                    }
                                    Some(Pending::Restart { .. }) => {
                                        message = Some(do_restart(&app, backend, ws, &mut agent_state, mouse_on).await);
                                        if keys_on { crate::term::enable_key_protocol(); }
                                        repaint = true;
                                    }
                                    Some(p) => run_pending(p, backend, ws, &mut mode, &mut fetching, &mut message),
                                    None => {}
                                }
                            } else if no {
                                mode = Mode::Shell;
                                if matches!(pending, Some(Pending::UpdateApply(_))) { updating = false; }
                                message = Some(match pending {
                                    Some(Pending::Restart { updated: Some(v) }) => format!("Updated to {v} — restart wi when convenient"),
                                    Some(Pending::Restart { updated: None }) => "restart wi when convenient".to_string(),
                                    _ => "cancelled".to_string(),
                                });
                                pending = None;
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Fetch => {
                            if (chunk == [0x1b] || chunk.contains(&menu_key)) && let Some(f) = &fetching {
                                f.cancel.store(true, Ordering::Relaxed);
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Hints(ref mut hs) => {
                            let (mut cancel, mut chosen) = (false, None::<(Link, Pane)>);
                            let mut i = 0;
                            while i < chunk.len() {
                                let b = chunk[i];
                                match b {
                                    0x1b => {
                                        if matches!(chunk.get(i + 1), Some(b'[') | Some(b'O')) {
                                            i += 2;
                                            while i < chunk.len() && !(0x40..=0x7e).contains(&chunk[i]) { i += 1; }
                                        } else {
                                            cancel = true;
                                        }
                                    }
                                    0x7f | 0x08 => { hs.typed.pop(); }
                                    x if x == menu_key => cancel = true,
                                    c if c.is_ascii_alphabetic() => {
                                        let mut t = hs.typed.clone();
                                        t.push((c as char).to_ascii_lowercase());
                                        if hs.hints.iter().any(|h| h.label.starts_with(&t)) {
                                            hs.typed = t;
                                            if let Some(ix) = hs.hints.iter().position(|h| h.label == hs.typed) {
                                                chosen = Some(hs.links[ix].clone());
                                                break;
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                                i += 1;
                            }
                            if cancel {
                                mode = Mode::Shell;
                            } else if let Some((l, pane)) = chosen {
                                mode = Mode::Shell;
                                let o = resolve_link(&app, backend, ws, &l, pane, &open_cfg, confirm_needed).await;
                                if let LinkOutcome::Explorer(dir) = o {
                                    mode = Mode::Explorer(explorer_for(dir, &mut saved_ex, remote.then(|| name.clone()), &opts, read_cap, &open_cfg, ex_pct)?);
                                } else {
                                    apply_outcome(o, &mut app, backend, ws, &mut mode, &mut pending, &mut message, &mut fetching, read_cap, &opts).await;
                                }
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Pick(ref mut pk) => {
                            let mut outcome = None;
                            for k in crate::ask::parse(chunk) {
                                if let Some(o) = pk.key(k) { outcome = Some(o); break; }
                            }
                            if let Some(o) = outcome {
                                let line = pk.line;
                                mode = Mode::Shell;
                                if let crate::pick::PickOutcome::Choose(abs) = o {
                                    let mut l = explorer::path_link(&abs);
                                    if let links::LinkKind::Path { line: ln, .. } = &mut l.kind { *ln = line; }
                                    let o = resolve_link(&app, backend, ws, &l, Pane::Shell, &open_cfg, confirm_needed).await;
                                    if let LinkOutcome::Explorer(dir) = o {
                                        mode = Mode::Explorer(explorer_for(dir, &mut saved_ex, remote.then(|| name.clone()), &opts, read_cap, &open_cfg, ex_pct)?);
                                    } else {
                                        apply_outcome(o, &mut app, backend, ws, &mut mode, &mut pending, &mut message, &mut fetching, read_cap, &opts).await;
                                    }
                                }
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Rename(ref mut r) => {
                            let mut outcome = None;
                            for k in crate::ask::parse(chunk) {
                                if let Some(o) = r.key(k) { outcome = Some(o); break; }
                            }
                            if let Some(o) = outcome {
                                if let crate::rename::RenameOutcome::Save(name) = o
                                    && let Some(i) = app.tabs.iter().position(|t| t.id == r.tab)
                                {
                                    app.tabs[i].set_custom(name.as_deref());
                                    refresh_labels(&mut app);
                                    message = Some(match name { Some(n) => format!("tab renamed to {n}"), None => "tab name cleared: automatic title".into() });
                                }
                                mode = Mode::Shell;
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Ask(ref mut ask) => {
                            let mut outcome = None;
                            for k in crate::ask::parse(chunk) {
                                if let Some(o) = ask.key(k) { outcome = Some(o); break; }
                            }
                            let (mut start, mut later, mut never) = (None::<(Agent, String)>, false, false);
                            match outcome {
                                Some(crate::ask::AskOutcome::Start(a, c)) => start = Some((a, c)),
                                Some(crate::ask::AskOutcome::Later) => later = true,
                                Some(crate::ask::AskOutcome::Never) => never = true,
                                None => {}
                            }
                            if let Some((ag, cmd)) = start {
                                mode = Mode::Shell;
                                agent_state.remember(&ask_ctx.key, ag.key(), &cmd);
                                if let Err(e) = agent_state.save() { message = Some(format!("could not save choice: {e}")); }
                                let res = match ask_ctx.tab.and_then(|id| app.tabs.iter().position(|t| t.id == id)) {
                                    Some(i) => { run_agent_in(&mut app, i, ag, &cmd); app.active = i; Ok(()) }
                                    None => start_agent(&mut app, backend, &events_tx, ag, &cmd, &ask_ctx.dir).await,
                                };
                                if let Err(e) = res {
                                    message = Some(format!("could not start {}: {e}", ag.key()));
                                }
                            } else if never {
                                mode = Mode::Shell;
                                agent_state.remember(&ask_ctx.key, "none", "");
                                if let Err(e) = agent_state.save() { message = Some(format!("could not save choice: {e}")); }
                                else { message = Some("won't ask here again (menu: Start an agent)".into()); }
                            } else if later {
                                mode = Mode::Shell;
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Prompt(ref mut pr) => {
                            let (mut cancel, mut complete_now, mut open_now) = (false, false, false);
                            let before = pr.input.clone();
                            let mut arrows: Vec<bool> = Vec::new(); // true = down
                            let mut run: Vec<u8> = Vec::new();
                            let mut i = 0;
                            while i < chunk.len() {
                                let b = chunk[i];
                                if b >= 0x20 && b != 0x7f && b != menu_key {
                                    run.push(b);
                                    i += 1;
                                    continue;
                                }
                                pr.insert(&String::from_utf8_lossy(&std::mem::take(&mut run)));
                                match b {
                                    0x1b => {
                                        if matches!(chunk.get(i + 1), Some(b'[') | Some(b'O')) {
                                            let first = chunk.get(i + 2).copied();
                                            let i0 = i;
                                            i += 2;
                                            while i < chunk.len() && !(0x40..=0x7e).contains(&chunk[i]) { i += 1; }
                                            let word = chunk.get(i0 + 2..i).is_some_and(|p| p.starts_with(b"1;3") || p.starts_with(b"1;5"));
                                            match chunk.get(i) {
                                                Some(b'C') if word => crate::lineedit::word_right(&pr.input, &mut pr.cur),
                                                Some(b'D') if word => crate::lineedit::word_left(&pr.input, &mut pr.cur),
                                                Some(b'A') => arrows.push(false),
                                                Some(b'B') => arrows.push(true),
                                                Some(b'C') => crate::lineedit::right(&pr.input, &mut pr.cur),
                                                Some(b'D') => crate::lineedit::left(&mut pr.cur),
                                                Some(b'H') => pr.cur = 0,
                                                Some(b'F') => pr.cur = pr.input.chars().count(),
                                                Some(b'~') => match first {
                                                    Some(b'1' | b'7') => pr.cur = 0,
                                                    Some(b'4' | b'8') => pr.cur = pr.input.chars().count(),
                                                    Some(b'3') => crate::lineedit::delete(&mut pr.input, pr.cur),
                                                    _ => {}
                                                },
                                                _ => {}
                                            }
                                        } else if let Some(&w @ (b'b' | b'f' | 0x7f)) = chunk.get(i + 1) {
                                            // Terminal.app style Alt+Left / Right / Backspace.
                                            match w {
                                                b'b' => crate::lineedit::word_left(&pr.input, &mut pr.cur),
                                                b'f' => crate::lineedit::word_right(&pr.input, &mut pr.cur),
                                                _ => crate::lineedit::delete_word(&mut pr.input, &mut pr.cur),
                                            }
                                            i += 1;
                                        } else {
                                            cancel = true;
                                        }
                                    }
                                    b'\r' | b'\n' => { open_now = true; break; }
                                    b'\t' => { complete_now = true; break; }
                                    0x7f | 0x08 => crate::lineedit::backspace(&mut pr.input, &mut pr.cur),
                                    0x15 => pr.set_input(String::new()),
                                    0x01 => pr.cur = 0,
                                    0x05 => pr.cur = pr.input.chars().count(),
                                    x if x == menu_key => cancel = true,
                                    _ => {}
                                }
                                pr.error = None;
                                i += 1;
                            }
                            pr.insert(&String::from_utf8_lossy(&run));
                            if !run.is_empty() || i > 0 { pr.error = None; }
                            // Editing the folder hides the recent list; arrows move through it while it shows.
                            if pr.recent && pr.input != before { pr.recent = false; pr.sel = None; pr.shown.clear(); pr.total = 0; }
                            if pr.recent {
                                let n = pr.shown.len();
                                for down in arrows {
                                    pr.sel = match (pr.sel, down) {
                                        (None, true) => Some(0),
                                        (None, false) => Some(n - 1),
                                        (Some(s), true) => Some((s + 1) % n),
                                        (Some(s), false) => Some((s + n - 1) % n),
                                    };
                                }
                            }
                            let base = active_dir(&app);
                            if complete_now {
                                if pr.recent && let Some(c) = pr.sel.and_then(|s| pr.shown.get(s)) {
                                    pr.set_input(format!("{}/", c.trim_end_matches('/')));
                                }
                                pr.recent = false;
                                pr.sel = None;
                                let dir = anchor(&base, crate::files::split_input(&pr.input).0);
                                match backend.list_dir(&dir).await {
                                    Ok(l) => {
                                        let c = if pr.kind.is_folder() { complete_dirs(&pr.input, &l.entries) } else { complete(&pr.input, &l.entries) };
                                        pr.set_input(c.input);
                                        pr.shown = c.shown;
                                        pr.total = c.total;
                                        if l.truncated {
                                            pr.error = Some(format!("directory has more than {} entries; completion is partial", crate::files::LIST_CAP));
                                        }
                                    }
                                    Err(e) => pr.error = Some(format!("{e}")),
                                }
                            }
                            let mut ask_req = None;
                            if open_now && pr.kind.is_folder() {
                                let picked = if pr.recent { pr.sel.and_then(|s| pr.shown.get(s)).cloned() } else { None };
                                let text = picked.unwrap_or_else(|| pr.input.trim().to_string());
                                match commit_folder(&mut app, backend, ws, cfg, &events_tx, &mut agent_state, &rkey, pr.kind, &text, &mut message).await {
                                    Ok(req) => { cancel = true; ask_req = req; }
                                    Err(e) => pr.error = Some(e),
                                }
                            } else if open_now && !pr.input.trim().is_empty() {
                                let path = pr.input.trim().to_string();
                                match open_preview(&mut app, backend, &anchor(&base, &path), read_cap, &opts, None).await {
                                    Ok(pl) => {
                                        cancel = true;
                                        if pl == Placement::TabNarrow { message = Some("narrow: opened in a tab".into()); }
                                    }
                                    Err(e) => pr.error = Some(format!("{path}: {e}")),
                                }
                            }
                            if cancel { mode = Mode::Shell; }
                            if let Some((list, ctx)) = ask_req {
                                ask_ctx = ctx;
                                mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &ask_ctx.key, &ask_ctx.dir));
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Menu(ref mut st) => {
                            let mut chosen: Option<Action> = None;
                            let mut jump: Option<usize> = None;
                            let mut close = false;
                            let main = main_rect(&app)?;
                            let mut entries = ui::menu_entries(&ui::menu_rows(&items, &st.filter));
                            let page = ui::menu_layout(main, &items, st).list_h.saturating_sub(2).max(1);
                            for k in parse_keys(chunk, menu_key) {
                                let n = entries.len();
                                match k {
                                    Key::Up if n > 0 => st.sel = (st.sel.min(n - 1) + n - 1) % n,
                                    Key::Down if n > 0 => st.sel = (st.sel.min(n - 1) + 1) % n,
                                    Key::PageUp if n > 0 => st.sel = st.sel.min(n - 1).saturating_sub(page),
                                    Key::PageDown if n > 0 => st.sel = (st.sel.min(n - 1) + page).min(n - 1),
                                    Key::Home => st.sel = 0,
                                    Key::End => st.sel = n.saturating_sub(1),
                                    Key::Enter => {
                                        if let Some(&i) = entries.get(st.sel) { chosen = Some(items[i].action); }
                                    }
                                    Key::Esc if !st.filter.is_empty() => { st.filter.clear(); st.sel = 0; }
                                    Key::Esc if st.filtering => st.filtering = false,
                                    Key::Esc | Key::MenuPressed => close = true,
                                    Key::Backspace if st.filtering => { st.filter.pop(); st.sel = 0; }
                                    Key::Tab if !st.filtering => st.filtering = true,
                                    Key::Char(' ') if !st.filtering => st.filtering = true,
                                    Key::Char(c) if st.filtering => {
                                        if c != ' ' || !st.filter.is_empty() { st.filter.push(c); st.sel = 0; }
                                    }
                                    Key::Char(c) => {
                                        let c = c.to_ascii_lowercase();
                                        if let Some(d) = c.to_digit(10).filter(|d| (1..=9).contains(d)) {
                                            jump = Some(d as usize - 1);
                                        } else if let Some(item) = items.iter().find(|m| m.key == c) {
                                            chosen = Some(item.action);
                                        }
                                    }
                                    _ => {}
                                }
                                entries = ui::menu_entries(&ui::menu_rows(&items, &st.filter));
                                if chosen.is_some() || jump.is_some() || close { break; }
                            }
                            st.top = ui::menu_layout(main, &items, st).top;
                            if let Some(i) = jump {
                                mode = Mode::Shell;
                                if i < app.tabs.len() { switch_to(&mut app, i); }
                            } else if let Some(a) = chosen {
                                mode = Mode::Shell;
                                match a {
                                    Action::Quit => {
                                        save_session(&app, backend, ws, restore_on, true).await;
                                        return finish(&app, backend).await;
                                    }
                                    Action::EndAndQuit => {
                                        backend.end_all_sessions().await;
                                        save_session(&app, backend, ws, restore_on, false).await;
                                        return finish(&app, backend).await;
                                    }
                                    Action::Info => {
                                        let keys = if decoder.enhanced { "keys: enhanced keyboard active · New line: Shift+Enter or Alt+Enter" } else { "keys: standard · New line: Alt+Enter (Shift+Enter looks like Enter in this terminal)" };
                                        let tab = app.tabs[app.active].custom.as_ref().map(|n| format!("tab: {n} · ")).unwrap_or_default();
                                        message = Some(if remote {
                                            format!("{tab}{keys} · {name} · {detail} · connected · {} tab(s)", app.tabs.len())
                                        } else {
                                            format!("{tab}{keys} · local workspace · {detail} · {} tab(s)", app.tabs.len())
                                        });
                                    }
                                    Action::OpenLink => {
                                        let main = main_rect(&app)?;
                                        let mut ls: Vec<(Link, Pane, Rect)> = Vec::new();
                                        for (pane, area) in visible_panes(&app, main) {
                                            let (rows, wrapped) = area_rows(&app, area, pane);
                                            ls.extend(links::all_links(&rows, &wrapped).into_iter().map(|l| (l, pane, area)));
                                        }
                                        ls.truncate(200);
                                        if ls.is_empty() {
                                            message = Some("no file paths or links on screen".into());
                                        } else {
                                            let hints = ls.iter().zip(hint_labels(ls.len())).map(|((l, pane, area), label)| Hint {
                                                label,
                                                row: origin_row(&app, *area, *pane) + l.row,
                                                col: area.x + l.start_col,
                                                width: if l.wrap_rows == 0 { l.end_col - l.start_col + 1 } else { area.width - l.start_col },
                                            }).collect();
                                            let links = ls.into_iter().map(|(l, p, _)| (l, p)).collect();
                                            mode = Mode::Hints(HintState { links, hints, typed: String::new() });
                                        }
                                    }
                                    Action::Agent => {
                                        let dir = active_dir(&app);
                                        let mut list = agents::detect(backend, &dir).await;
                                        if list.is_empty() { list = agents::ALL.to_vec(); }
                                        ask_ctx = AskCtx { key: agents::workspace_key(ws, &dir), dir, tab: None };
                                        mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &ask_ctx.key, &ask_ctx.dir));
                                    }
                                    Action::Mouse => {
                                        mouse_on = !mouse_on;
                                        let mut o = stdout();
                                        let _ = std::io::Write::write_all(&mut o, if mouse_on { mouse::ENABLE } else { mouse::DISABLE });
                                        let _ = std::io::Write::flush(&mut o);
                                        message = Some(if mouse_on { "mouse on".to_string() } else { "mouse off: select and copy text natively".to_string() });
                                    }
                                    Action::RenameTab => {
                                        let t = &app.tabs[app.active];
                                        mode = Mode::Rename(crate::rename::Rename::new(t.id, t.custom.as_deref().unwrap_or(&t.title)));
                                    }
                                    Action::Theme => mode = Mode::ThemePick(ui::themepick::ThemePick::new(theme_mode)),
                                    #[cfg(target_os = "macos")]
                                    Action::Dictation => {
                                        if !tcfg.enabled {
                                            message = Some("dictation is turned off in the config ([transcribe] enabled = false)".into());
                                        } else if dict.is_some() {
                                            mode = Mode::Voice(voice::Dlg::on());
                                        } else {
                                            vgen += 1;
                                            voice::spawn_check(&voice_tx, vgen, voice::locate());
                                        }
                                    }
                                    // Voice dictation is macOS only for now (the menu entry is hidden elsewhere).
                                    #[cfg(not(target_os = "macos"))]
                                    Action::Dictation => {
                                        let _ = (&tcfg, &dict_tx, &mut dict_gen, &mut dict, &voice_tx, &mut vgen, &mut vargv, &mut vcancel, &mut vstarted);
                                        message = Some("Dictation is macOS only for now".into());
                                    }
                                    Action::Restart => {
                                        mode = restart_dialog(&app, remote, backend.has_tmux(), None);
                                        pending = Some(Pending::Restart { updated: None });
                                    }
                                    Action::Update => {
                                        if updating {
                                            message = Some("an update is already running".into());
                                        } else {
                                            let mut lines = vec![
                                                "This contacts github.com to check the latest release of".to_string(),
                                                format!("{} and nothing else. Nothing is sent until you confirm.", crate::update::REPO),
                                                "It updates this computer's wi (and the voice helper, if you installed it), never a remote box. Your tabs stay open.".to_string(),
                                            ];
                                            if let Some(u) = crate::update::override_url() { lines.push(format!("(test source: {u})")); }
                                            mode = Mode::Confirm(Confirm { title: "Update wi: check for a new version?".into(), lines, yes: "Check".into() });
                                            pending = Some(Pending::UpdateCheck);
                                        }
                                    }
                                    Action::SwitchPane => {
                                        let main = main_rect(&app)?;
                                        if let (Some(pi), Some(_)) = (pin_partner(&app), pin_split(&app, main)) {
                                            switch_to(&mut app, pi);
                                            sel_reset = true;
                                        } else if split_of(&app, &app.tabs[app.active], main).is_some() {
                                            let t = &mut app.tabs[app.active];
                                            t.focus = if t.focus == Pane::Side { Pane::Shell } else { Pane::Side };
                                        } else {
                                            message = Some("no preview pane open".into());
                                        }
                                    }
                                    Action::PinRight | Action::PinLeft => {
                                        let side = if matches!(a, Action::PinLeft) { Side::Left } else { Side::Right };
                                        match pin_active(&mut app, backend, &events_tx, side).await {
                                            Ok(()) => {
                                                sel_reset = true;
                                                save_layout(&app, &mut agent_state, &pane_key);
                                                message = Some(format!("pinned {}: menu, then Swap pane sides or Unsplit", if side == Side::Left { "left" } else { "right" }));
                                            }
                                            Err(e) => message = Some(e),
                                        }
                                    }
                                    Action::SwapSides => {
                                        if app.pinned.is_some() || app.tabs[app.active].side.is_some() {
                                            swap_sides(&mut app);
                                            sel_reset = true;
                                            resize_all(&mut app)?;
                                            save_layout(&app, &mut agent_state, &pane_key);
                                        } else {
                                            message = Some("no pane to swap: pin a tab first (menu, Space, \"pin\")".into());
                                        }
                                    }
                                    Action::Unsplit | Action::ClosePreview if app.pinned.is_some() => {
                                        unsplit(&mut app);
                                        sel_reset = true;
                                        resize_all(&mut app)?;
                                        save_layout(&app, &mut agent_state, &pane_key);
                                    }
                                    Action::Unsplit | Action::ClosePreview => {
                                        if app.tabs[app.active].side.is_some() {
                                            let t = &mut app.tabs[app.active];
                                            t.side = None;
                                            t.focus = Pane::Shell;
                                            sel_reset = true;
                                            resize_all(&mut app)?;
                                        } else {
                                            message = Some("no preview pane open".into());
                                        }
                                    }
                                    Action::SearchText => {
                                        mode = match last_results.take() {
                                            Some(r) => Mode::Results(r),
                                            None => Mode::GrepPrompt(GrepPrompt::new(None)),
                                        };
                                    }
                                    Action::FindFile => {
                                        mode = Mode::Working("listing files");
                                        job = Some(start_files(backend, active_dir(&app)));
                                    }
                                    Action::RecentFiles => match recent_finder(&app, backend, false).await {
                                        Ok(f) => mode = Mode::Finder(Box::new(f)),
                                        Err(m) => message = Some(m),
                                    },
                                    Action::Explorer => {
                                        let dir = active_dir(&app);
                                        mode = Mode::Explorer(explorer_for(dir, &mut saved_ex, remote.then(|| name.clone()), &opts, read_cap, &open_cfg, ex_pct)?);
                                    }
                                    Action::FindOutput if app.tabs[app.active].is_preview() => message = Some("find in output works on a shell tab; press F inside a preview to find in it".into()),
                                    Action::FindOutput => mode = Mode::FindPrompt(FindPrompt { target: FindTarget::Output, text: String::new() }),
                                    Action::Scroll if app.tabs[app.active].side_focused() => message = Some("the preview has the keyboard: switch pane (menu W) to scroll the shell".into()),
                                    Action::Scroll if app.tabs[app.active].is_preview() => message = Some("scroll a preview with the arrow keys and PgUp/PgDn".into()),
                                    Action::Scroll => mode = Mode::Scroll,
                                    Action::Git => {
                                        let gdir = active_dir(&app);
                                        match open_git(&mut app, backend).await {
                                            Ok(b) => { branches.insert(gdir, Some(b)); detail_key = None; }
                                            Err(e) => message = Some(format!("Git changes: {e}")),
                                        }
                                    }
                                    Action::Preview => mode = Mode::Prompt(Prompt::default()),
                                    Action::NewTab => {
                                        let (c, r) = shell_size(app.tabs.len() + 1)?;
                                        let had_strip = app.tabs.len() > 1;
                                        let here = active_dir(&app);
                                        match open_tab_in(&mut app, backend, &events_tx, c, r, None, Some(&here)).await {
                                            Ok(()) => {
                                                app.active = app.tabs.len() - 1;
                                                if !had_strip { resize_all(&mut app)?; }
                                            }
                                            Err(e) => message = Some(format!("could not open shell: {e}")),
                                        }
                                    }
                                    Action::NewTabIn => {
                                        let start = active_dir(&app);
                                        mode = Mode::Prompt(Prompt::folder(PromptKind::NewFolder, &start, agent_state.recents(&rkey)));
                                    }
                                    Action::ChangeDir => {
                                        let t = &app.tabs[app.active];
                                        if t.is_preview() {
                                            message = Some("change folder works on a shell tab; switch to one first".into());
                                        } else {
                                            let idle = crate::cwd::tab_idle(backend, t.handle.as_ref(), app.sessions.get(&t.id).map(String::as_str)).await;
                                            match idle {
                                                Activity::Ready => mode = Mode::Prompt(Prompt::folder(PromptKind::ChangeFolder, &t.dir, agent_state.recents(&rkey))),
                                                other => message = Some(busy_message(&other, &menu_label)),
                                            }
                                        }
                                    }
                                    Action::NextTab => { let n = (app.active + 1) % app.tabs.len(); switch_to(&mut app, n); }
                                    Action::PrevTab => { let n = (app.active + app.tabs.len() - 1) % app.tabs.len(); switch_to(&mut app, n); }
                                    Action::CloseTab => {
                                        let id = app.tabs[app.active].id;
                                        if let Some(name) = app.sessions.get(&id) { backend.end_session(name).await; }
                                        app.tabs[app.active].close();
                                        if remove_tab(&mut app, id)? {
                    save_session(&app, backend, ws, restore_on, false).await;
                    return finish(&app, backend).await;
                }
                                    }
                                    other => message = Some(format!("{other:?} arrives in a later milestone")),
                                }
                            } else if close {
                                mode = Mode::Shell;
                            }
                            dirty = true;
                            chunk = &[];
                        }
                    }
                }
                }
            }
            res = async {
                match fetching.as_mut() {
                    Some(f) => f.fut.as_mut().await,
                    None => std::future::pending().await,
                }
            }, if fetching.is_some() => {
                fetching = None;
                if matches!(mode, Mode::Fetch) { mode = Mode::Shell; }
                message = Some(match res {
                    Ok(FetchResult::Ready(p)) => {
                        opener::mark_quarantine(&p);
                        let name = p.file_name().map(|n| sanitize(&n.to_string_lossy())).unwrap_or_default();
                        match native_open_hooked(&p) {
                            Ok(()) => format!("opened {name}"),
                            Err(e) => e,
                        }
                    }
                    Ok(FetchResult::NeedsConfirm { size }) => format!("file is {}; open it again to confirm", crate::preview::human(size)),
                    Err(e) if e == "cancelled" => "download cancelled".to_string(),
                    Err(e) => format!("download failed: {e}"),
                });
                dirty = true;
            }
            done = async {
                match ex_job.as_mut() {
                    Some(f) => f.as_mut().await,
                    None => std::future::pending().await,
                }
            }, if ex_job.is_some() => {
                ex_job = None;
                if let Mode::Explorer(ex) = &mut mode { ex.apply(done); }
                dirty = true;
            }
            done = async {
                match job.as_mut() {
                    Some(f) => f.as_mut().await,
                    None => std::future::pending().await,
                }
            }, if job.is_some() => {
                job = None;
                if matches!(mode, Mode::Working(_)) {
                    mode = match done {
                        JobDone::Grep(r) => Mode::Results(Box::new(r)),
                        JobDone::Files(Ok(l), dir) => Mode::Finder(Box::new(Finder::new(l.files, l.notice, dir))),
                        JobDone::Files(Err(e), _) => {
                            message = Some(format!("find file: {e}"));
                            Mode::Shell
                        }
                    };
                }
                dirty = true;
            }
            _ = tokio::time::sleep(Duration::from_millis(250)), if fetching.is_some() => { dirty = true; }
            Some(ev) = upd_rx.recv() => {
                if matches!(ev, UpdEv::Status(_)) || matches!(mode, Mode::Shell) {
                    on_update_event(ev, &mut mode, &mut pending, &mut message, &mut app, &mut updating, &opts, remote, backend.has_tmux());
                } else {
                    upd_held = Some(ev);
                }
                dirty = true;
            }
            _ = sigterm.recv() => {
                save_session(&app, backend, ws, restore_on, true).await;
                return finish(&app, backend).await;
            }
            _ = sighup.recv() => {
                save_session(&app, backend, ws, restore_on, true).await;
                return finish(&app, backend).await;
            }
            _ = sigint.recv() => {
                save_session(&app, backend, ws, restore_on, true).await;
                return finish(&app, backend).await;
            }
            _ = winch.recv() => {
                resize_all(&mut app)?;
                dirty = true;
            }
            ev = events.recv() => match ev {
                Some(Event::Output(id, data)) => {
                    if let Some(i) = app.tabs.iter().position(|t| t.id == id) {
                        if i == app.active && matches!(mode, Mode::OutFind(_)) {
                            // The rows the matches point at have moved: leave find mode.
                            app.tabs[i].parser.screen_mut().set_scrollback(0);
                            mode = Mode::Shell;
                            message = Some("new output: find closed".into());
                        }
                        app.tabs[i].output(&data);
                        // A program that switched to the alternate screen has no scrollback: the
                        // scroll view would only swallow its keys.
                        if i == app.active && matches!(mode, Mode::Scroll) && app.tabs[i].parser.screen().alternate_screen() {
                            app.tabs[i].parser.screen_mut().set_scrollback(0);
                            mode = Mode::Shell;
                        }
                        if i != app.active && Some(i) != pin_partner(&app) { app.tabs[i].note_background_output(); }
                        dirty = true;
                    }
                }
                Some(Event::Exited(_)) if backend.is_lost() => {
                    message = Some("connection lost - open the menu and quit".into());
                    dirty = true;
                }
                Some(Event::Exited(id)) => {
                    if app.tabs.get(app.active).is_some_and(|t| t.id == id) && matches!(mode, Mode::Scroll | Mode::OutFind(_)) { mode = Mode::Shell; }
                    if remove_tab(&mut app, id)? {
                    save_session(&app, backend, ws, restore_on, false).await;
                    return finish(&app, backend).await;
                }
                    dirty = true;
                }
                None => return finish(&app, backend).await,
            }
        }
    }
}

async fn open_tab(app: &mut App, backend: &Backend, tx: &mpsc::Sender<Event>, cols: u16, rows: u16) -> Result<()> {
    open_tab_in(app, backend, tx, cols, rows, None, None).await
}

/// Open a shell tab; `attach` names an existing tmux session to reattach, `dir` is the folder a
/// new shell starts in (the workspace folder when None).
async fn open_tab_in(app: &mut App, backend: &Backend, tx: &mpsc::Sender<Event>, cols: u16, rows: u16, attach: Option<&str>, dir: Option<&str>) -> Result<()> {
    let (shell, session) = backend.open_session_shell(cols, rows, attach, dir).await?;
    let id = app.next_id;
    // A reattached session keeps whatever folder it is in now.
    let tab_dir = match (dir, attach) {
        (Some(d), _) => d.to_string(),
        (None, Some(name)) => crate::cwd::tab_cwd(backend, None, None, Some(name)).await.unwrap_or_else(|| app.root.clone()),
        (None, None) => app.root.clone(),
    };
    if let Some(s) = session { app.sessions.insert(id, s); }
    app.next_id += 1;
    app.shell_count += 1;
    let handle = spawn_shell(id, shell, tx.clone());
    let mut tab = Tab::new(id, format!("shell {}", app.shell_count), handle, cols, rows, app.scrollback);
    tab.dir = tab_dir;
    tab.number = app.shell_count;
    app.tabs.push(tab);
    refresh_labels(app);
    Ok(())
}

/// Show folder names in the labels once there are several tabs or a tab is not in the
/// workspace's own folder; otherwise keep `shell N`.
fn refresh_labels(app: &mut App) {
    let several = app.tabs.len() > 1;
    let root = app.root.clone();
    for t in app.tabs.iter_mut().filter(|t| !t.is_preview()) {
        t.show_dir = several || t.dir != root;
        t.retitle();
    }
}

/// The shell tab whose folder applies to the active tab: itself, or for a preview the tab it
/// was opened from, else the first shell tab.
fn folder_tab(app: &App) -> Option<usize> {
    let t = app.tabs.get(app.active)?;
    if !t.is_preview() {
        return Some(app.active);
    }
    let from = t.preview.as_ref().map(|p| p.from_tab).unwrap_or(0);
    if app.tabs.get(from).is_some_and(|x| !x.is_preview()) {
        return Some(from);
    }
    app.tabs.iter().position(|t| !t.is_preview())
}

/// Base folder that previews, git, agents and new tabs use for the active tab.
fn active_dir(app: &App) -> String {
    folder_tab(app).map(|i| app.tabs[i].dir.clone()).unwrap_or_else(|| app.root.clone())
}

/// Header text: where the active tab is (and for a remote box, which box), plus the branch.
fn folder_detail(ws: &Workspace, base: &str, dir: &str, branch: Option<&str>) -> String {
    let shown = match ws {
        Workspace::Local(_) => {
            let home = dirs::home_dir().unwrap_or_default().to_string_lossy().into_owned();
            match dir.strip_prefix(home.trim_end_matches('/')) {
                Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
                _ => dir.to_string(),
            }
        }
        Workspace::Remote(_) => format!("{base}  {dir}"),
    };
    match branch {
        Some(b) => header_detail(&shown, b),
        None => shown,
    }
}

fn busy_message(idle: &Activity, menu_label: &str) -> String {
    match idle {
        Activity::Busy(cmd) => format!("can't change folder: {} is running in this tab. Open a new tab in that folder instead ({menu_label}, then F)", sanitize(cmd)),
        _ => format!("can't tell whether this tab is idle, so nothing was typed. Open a new tab in that folder instead ({menu_label}, then F)"),
    }
}

/// What the folder prompt asked for. On success the prompt closes; an agent question for the
/// new folder is returned for the caller to show.
#[allow(clippy::too_many_arguments)]
async fn commit_folder(
    app: &mut App,
    backend: &Backend,
    ws: &Workspace,
    cfg: &Config,
    tx: &mpsc::Sender<Event>,
    state: &mut agents::State,
    rkey: &str,
    kind: PromptKind,
    text: &str,
    message: &mut Option<String>,
) -> Result<Option<(Vec<Agent>, AskCtx)>, String> {
    let base = active_dir(app);
    let dir = backend.resolve_folder(&base, text).await?;
    let mut ask = None;
    match kind {
        PromptKind::ChangeFolder => {
            let t = &app.tabs[app.active];
            if t.is_preview() {
                return Err("change folder works on a shell tab".into());
            }
            // Re-check: a program may have started since the prompt opened.
            match crate::cwd::tab_idle(backend, t.handle.as_ref(), app.sessions.get(&t.id).map(String::as_str)).await {
                Activity::Ready => {}
                other => return Err(busy_message(&other, "menu")),
            }
            // A leading space keeps the command out of history in shells configured for that.
            t.write(format!(" cd -- {}\r", crate::exec::sq(&dir)).into_bytes());
            let t = &mut app.tabs[app.active];
            t.dir = dir.clone();
            refresh_labels(app);
            *message = Some(format!("tab is now in {dir}"));
        }
        _ => {
            let (c, r) = shell_size(app.tabs.len() + 1).map_err(|e| e.to_string())?;
            open_tab_in(app, backend, tx, c, r, None, Some(&dir)).await.map_err(|e| format!("could not open shell: {e}"))?;
            let last = app.tabs.len() - 1;
            app.active = last;
            resize_all(app).map_err(|e| e.to_string())?;
            // Recognise an agent workspace in this folder, like at startup.
            let key = agents::workspace_key(ws, &dir);
            let detected = agents::detect(backend, &dir).await;
            match agents::decide(&cfg.defaults.agent_start, &detected, state.workspace.get(&key)) {
                Decision::Nothing => {}
                Decision::Start(ag) => {
                    let cmd = agents::command_for(ag, cfg, state, &key);
                    run_agent_in(app, last, ag, &cmd);
                    *message = Some(format!("started: {cmd}"));
                }
                Decision::Ask(list) => ask = Some((list, AskCtx { key, dir: dir.clone(), tab: Some(app.tabs[last].id) })),
            }
        }
    }
    state.push_recent(rkey, &dir);
    if let Err(e) = state.save() {
        *message = Some(format!("could not save recent folders: {e}"));
    }
    Ok(ask)
}

/// The explorer for `dir`: the kept one from an earlier visit (refreshed), or a new one.
fn explorer_for(dir: String, saved: &mut HashMap<String, Box<ExplorerState>>, remote: Option<String>, opts: &Opts, read_cap: usize, open_cfg: &OpenCfg, pct: u16) -> Result<Box<ExplorerState>> {
    let (ec, er) = crossterm::terminal::size()?;
    let mut ex = match saved.remove(&dir) {
        Some(mut e) => {
            e.refresh();
            e
        }
        None => Box::new(ExplorerState::new(dir, remote, explorer::Settings { csv_rows: opts.csv_rows, read_cap, open_cfg: open_cfg.clone(), pct })),
    };
    ex.prepare(ec, er);
    Ok(ex)
}

/// Leave the explorer for the shell, keeping its state for the next visit to the same folder.
fn stash_explorer(saved: &mut HashMap<String, Box<ExplorerState>>, mode: &mut Mode) {
    if let Mode::Explorer(e) = std::mem::replace(mode, Mode::Shell) {
        saved.insert(e.root.clone(), e);
    }
}

fn switch_to(app: &mut App, i: usize) {
    app.active = i;
    app.tabs[i].mark_seen();
    sync_pin(app);
    if app.pinned.is_some() {
        let _ = resize_all(app); // the pair on screen may have changed
    }
}

/// Back to one pane: the pinned tab stays a tab; a side preview closes.
fn unsplit(app: &mut App) {
    app.pinned = None;
    app.div_drag = false;
    let t = &mut app.tabs[app.active];
    t.side = None;
    t.focus = Pane::Shell;
}

/// Move the pinned pane (or the side preview) to the other side.
fn swap_sides(app: &mut App) {
    let flip = |s: Side| if s == Side::Left { Side::Right } else { Side::Left };
    if app.pinned.is_some() {
        app.pin_side = flip(app.pin_side);
    } else if app.side != Side::Tab {
        app.side = flip(app.side);
        app.pin_side = app.side;
    }
}

/// Remember the pinned-pane layout for this workspace.
fn save_layout(app: &App, state: &mut agents::State, key: &str) {
    // Only workspaces that use pinned panes get an entry (a side preview's width is not kept).
    if app.pinned.is_none() && !state.panes.contains_key(key) {
        return;
    }
    let (kind, target) = match pinned_idx(app).map(|i| &app.tabs[i]) {
        Some(t) if t.preview.as_ref().is_some_and(|p| p.kind != crate::preview::Kind::Diff) => ("preview", t.preview.as_ref().map(|p| p.path.clone()).unwrap_or_default()),
        Some(t) if t.preview.is_some() => ("", String::new()),
        Some(t) if t.agent.is_some() => ("agent", t.agent.clone().unwrap_or_default()),
        Some(t) => ("shell", t.dir.clone()),
        None => ("", String::new()),
    };
    let layout = agents::PaneLayout {
        side: if app.pin_side == Side::Left { "left" } else { "right" }.into(),
        pct: app.pct,
        kind: kind.into(),
        target,
    };
    if state.panes.get(key) != Some(&layout) {
        state.panes.insert(key.to_string(), layout);
        let _ = state.save();
    }
}

/// Pin tab `id` on `side`. The tab that was active moves on to another tab (a new shell when
/// there is none), so the pinned tab is shown beside it.
async fn pin_tab(app: &mut App, backend: &Backend, tx: &mpsc::Sender<Event>, id: usize, side: Side) -> Result<(), String> {
    let (c, r) = shell_size(app.tabs.len() + 1).map_err(|e| e.to_string())?;
    if split_main(Rect::new(0, 0, c, r), side, app.pct).is_none() {
        return Err(format!("window too narrow for two panes (needs {} columns)", crate::tab::SHELL_MIN + crate::tab::PREVIEW_MIN + 1));
    }
    let Some(pi) = app.tabs.iter().position(|t| t.id == id) else { return Err("that tab is gone".into()) };
    if app.pinned != Some(id) && !app.tabs.iter().any(|t| t.id != id) {
        let dir = if app.tabs[pi].dir.is_empty() { None } else { Some(app.tabs[pi].dir.clone()) };
        open_tab_in(app, backend, tx, c, r, None, dir.as_deref()).await.map_err(|e| format!("could not open shell: {e}"))?;
    }
    for t in &mut app.tabs {
        t.side = None;
        t.focus = Pane::Shell;
    }
    app.pinned = Some(id);
    app.pin_side = side;
    let pi = app.tabs.iter().position(|t| t.id == id).unwrap_or(pi);
    if app.active == pi {
        // Next tab, else the one before.
        let other = (pi + 1..app.tabs.len()).chain((0..pi).rev()).next().unwrap_or(0);
        app.active = other;
    }
    app.last_main = app.tabs[app.active].id;
    app.tabs[app.active].mark_seen();
    resize_all(app).map_err(|e| e.to_string())
}

/// Pin the active tab.
async fn pin_active(app: &mut App, backend: &Backend, tx: &mpsc::Sender<Event>, side: Side) -> Result<(), String> {
    let id = app.tabs[app.active].id;
    pin_tab(app, backend, tx, id, side).await
}

/// Index of the pinned tab.
fn pinned_idx(app: &App) -> Option<usize> {
    let id = app.pinned?;
    app.tabs.iter().position(|t| t.id == id)
}

/// The tab in the other pane while a tab is pinned: the pinned tab beside the active one, or, when
/// the pinned tab is active, the last other tab you were in. Whether it fits on screen is a
/// separate question (`pin_split`).
fn partner_idx(app: &App) -> Option<usize> {
    let pi = pinned_idx(app)?;
    if pi != app.active {
        return Some(pi);
    }
    app.tabs.iter().position(|t| t.id == app.last_main && t.id != app.pinned.unwrap_or(usize::MAX))
        .or_else(|| app.tabs.iter().position(|t| Some(t.id) != app.pinned))
}

/// Remember the other-than-pinned tab that has the keyboard, and mark the tabs on screen as seen.
fn sync_pin(app: &mut App) {
    let Some(pid) = app.pinned else { return };
    let id = app.tabs[app.active].id;
    if id != pid {
        app.last_main = id;
    }
    if let Some(pi) = partner_idx(app) {
        app.tabs[pi].mark_seen();
    }
}

/// The two panes while a tab is pinned and the window has room: `shell` is the active tab's
/// rectangle, `side` the other tab's.
fn pin_split(app: &App, main: Rect) -> Option<Split> {
    let pi = pinned_idx(app)?;
    partner_idx(app)?;
    split_pinned(main, app.pin_side, app.pct, pi == app.active)
}

/// The tabs on screen and the area each fills.
fn displayed(app: &App, main: Rect) -> Vec<(usize, Rect)> {
    match (pin_split(app, main), partner_idx(app)) {
        (Some(sp), Some(pi)) => vec![(app.active, sp.shell), (pi, sp.side)],
        _ => vec![(app.active, main)],
    }
}

/// (label, active, state) per tab for the strip; the tab beside the active one counts as seen.
fn strip_of(app: &App) -> Vec<(String, bool, crate::tab::TabState)> {
    let shown = pin_split(app, main_area(app)).and_then(|_| partner_idx(app));
    app.tabs.iter().enumerate()
        .map(|(i, t)| (t.strip_title(), i == app.active, t.state(i == app.active || Some(i) == shown))).collect()
}

/// `main_rect` without a terminal size: only the proportions that matter for fitting panes.
fn main_area(app: &App) -> Rect {
    let (c, r) = shell_size(app.tabs.len()).unwrap_or((80, 24));
    Rect::new(0, 0, c, r)
}

fn resize_all(app: &mut App) -> Result<()> {
    refresh_labels(app);
    let (c, r) = shell_size(app.tabs.len())?;
    let main = Rect::new(0, 0, c, r);
    let split = split_main(main, app.side, app.pct);
    let pin = pin_split(app, main);
    let partner = partner_idx(app);
    let active = app.active;
    for (i, t) in app.tabs.iter_mut().enumerate() {
        // The shell is narrower while a side preview fits beside it; without room the pane hides.
        let shell_c = match (&pin, &t.side, &split) {
            (Some(sp), _, _) if i == active => sp.shell.width,
            (Some(sp), _, _) if Some(i) == partner => sp.side.width,
            (None, Some(_), Some(sp)) => sp.shell.width,
            _ => c,
        };
        if t.side.is_some() && split.is_none() { t.focus = Pane::Shell; }
        if t.is_preview() || t.parser.screen().size() != (crate::tab::nz(r), crate::tab::nz(shell_c)) {
            t.resize(shell_c, r);
        }
    }
    Ok(())
}

/// Remove a tab by id. Returns true when no tabs are left.
fn remove_tab(app: &mut App, id: usize) -> Result<bool> {
    let Some(i) = app.tabs.iter().position(|t| t.id == id) else { return Ok(app.tabs.is_empty()) };
    app.tabs.remove(i);
    if app.pinned == Some(id) {
        app.pinned = None;
    }
    if app.tabs.is_empty() {
        return Ok(true);
    }
    if app.active > i || app.active >= app.tabs.len() {
        app.active = app.active.saturating_sub(1).min(app.tabs.len() - 1);
    }
    app.tabs[app.active].mark_seen();
    sync_pin(app);
    resize_all(app)?;
    Ok(false)
}

/// At a normal quit: remember the tabs for this workspace (`keep`), or forget them (the last tab
/// was closed, or the sessions were ended). Only this workspace's entry changes; entries older
/// than 30 days go. A lone plain shell saves nothing.
async fn save_session(app: &App, backend: &Backend, ws: &Workspace, enabled: bool, keep: bool) {
    if !enabled {
        return;
    }
    let key = agents::workspace_key(ws, &app.root);
    let snap = if keep { Some(build_snapshot(app, backend, ws).await).filter(|s| !s.is_trivial(&app.root)) } else { None };
    let mut st = agents::State::load();
    let now = crate::restart::now_secs();
    st.sessions.retain(|_, s| s.is_fresh(now));
    match snap {
        Some(s) => st.sessions.insert(key, s),
        None => st.sessions.remove(&key),
    };
    let _ = st.save();
}

async fn finish(app: &App, backend: &Backend) -> Result<()> {
    for t in &app.tabs {
        t.close();
    }
    backend.close().await;
    // Give the shell tasks a moment to kill their children before we exit.
    tokio::time::sleep(Duration::from_millis(150)).await;
    Ok(())
}

/// Open `input` (relative to the workspace) in a new preview tab and switch to it.
async fn open_preview(app: &mut App, backend: &Backend, input: &str, cap: usize, opts: &Opts, line: Option<u32>) -> Result<Placement> {
    let abs = backend.resolve(input).await?;
    let (bytes, total) = backend.read_prefix(&abs, cap).await?;
    let mut p = Preview::build(&abs, &bytes, total, opts);
    if let Some(l) = line {
        p.goto_line(l as usize);
    }
    record_shown(app, &abs);
    p.from_tab = app.active;
    match attach_side(app, p)? {
        Ok(()) => Ok(Placement::Side),
        Err((p, narrow)) => {
            let id = app.next_id;
            app.next_id += 1;
            app.tabs.push(Tab::new_preview(id, p));
            app.active = app.tabs.len() - 1;
            resize_all(app)?; // the tab strip may have just appeared
            Ok(if narrow { Placement::TabNarrow } else { Placement::Tab })
        }
    }
}

/// Remember that `path` was shown in a preview (saved by the main loop).
fn record_shown(app: &mut App, path: &str) {
    crate::history::push(&mut app.history, path, crate::history::now());
    app.hist_dirty = true;
}

/// Show `input` in the preview that has the keyboard, replacing its file (scroll starts at the top).
async fn open_in_place(app: &mut App, backend: &Backend, input: &str, cap: usize, opts: &Opts) -> Result<()> {
    let abs = backend.resolve(input).await?;
    let (bytes, total) = backend.read_prefix(&abs, cap).await?;
    let mut p = Preview::build(&abs, &bytes, total, opts);
    record_shown(app, &abs);
    let own_tab = !app.tabs[app.active].side_focused();
    let active = app.active;
    let slot = focused_slot(app);
    p.from_tab = slot.as_ref().map_or(active, |o| o.from_tab);
    let title = p.title.clone();
    *slot = Some(p);
    if own_tab {
        let t = &mut app.tabs[active];
        t.title = title.clone();
        t.fallback = title;
        t.apply_custom();
    }
    resize_all(app)
}

/// The recent-files list for the Finder screen: relative paths, how long ago, what is shown now,
/// and which files are gone (one bounded stat each, two seconds in all).
async fn recent_finder(app: &App, backend: &Backend, replace: bool) -> Result<Finder, String> {
    if app.history.is_empty() {
        return Err("no recent files yet: files you preview are listed here".into());
    }
    let t = &app.tabs[app.active];
    let current = if t.side_focused() { t.side.as_ref() } else { t.preview.as_ref().or(t.side.as_ref()) }.map(|p| p.path.clone());
    let now = crate::history::now();
    let deadline = Instant::now() + Duration::from_secs(2);
    let (mut files, mut marks) = (Vec::new(), Vec::new());
    for e in &app.history {
        let left = deadline.saturating_duration_since(Instant::now());
        let missing = !left.is_zero() && tokio::time::timeout(left, backend.exists(&e.path)).await == Ok(false);
        files.push(crate::history::display(&e.path, &app.root));
        marks.push(search::RecentMark { path: e.path.clone(), ago: crate::history::ago(now, e.at), current: current.as_deref() == Some(e.path.as_str()), missing });
    }
    let mut f = Finder::recent(files, marks, app.root.clone());
    f.replace = replace;
    Ok(f)
}

/// Where an opened preview ended up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Placement {
    Side,
    Tab,
    /// A tab, because the window is too narrow for a pane.
    TabNarrow,
}

/// Put `p` in the active shell tab's side pane when the setting and the window allow it.
/// Otherwise hand it back, with whether the window was too narrow.
fn attach_side(app: &mut App, p: Preview) -> Result<Result<(), (Preview, bool)>> {
    if app.tabs[app.active].is_preview() || app.side == Side::Tab || app.pinned.is_some() {
        return Ok(Err((p, false)));
    }
    let (c, r) = shell_size(app.tabs.len())?;
    if split_main(Rect::new(0, 0, c, r), app.side, app.pct).is_none() {
        return Ok(Err((p, true)));
    }
    let t = &mut app.tabs[app.active];
    t.side = Some(p);
    t.focus = Pane::Side;
    resize_all(app)?;
    Ok(Ok(()))
}

fn header_detail(base: &str, branch: &str) -> String {
    format!("{base}  ({branch})")
}

fn git_tab(app: &App) -> Option<usize> {
    app.tabs.iter().position(|t| t.preview.as_ref().is_some_and(|p| p.kind == crate::preview::Kind::Diff))
}

/// Open (or refresh and switch to) the Git changes tab. Returns the branch for the header.
async fn open_git(app: &mut App, backend: &Backend) -> Result<String> {
    let dir = active_dir(app);
    let snap = backend.git_snapshot(&dir).await?;
    let branch = backend.git_branch(&dir).await.unwrap_or_else(|| "?".into());
    let mut p = Preview::from_git(&snap);
    p.from_tab = app.active;
    let p = match attach_side(app, p)? {
        Ok(()) => return Ok(branch),
        Err((p, _)) => p,
    };
    let mut p = p;
    if let Some(i) = git_tab(app) {
        // Opened from a shell tab: the view now belongs to that tab's folder.
        let from = if app.tabs[app.active].is_preview() { app.tabs[i].preview.as_ref().map(|o| o.from_tab) } else { Some(app.active) };
        if i != app.active { switch_to(app, i); }
        p.from_tab = from.unwrap_or(p.from_tab);
        app.tabs[i].preview = Some(p);
        return Ok(branch);
    }
    let id = app.next_id;
    app.next_id += 1;
    app.tabs.push(Tab::new_preview(id, p));
    app.active = app.tabs.len() - 1;
    resize_all(app)?;
    Ok(branch)
}

/// Re-run git for the active Git tab, keeping the scroll position.
async fn reload_git(app: &mut App, backend: &Backend) -> Result<String> {
    let dir = active_dir(app);
    let snap = backend.git_snapshot(&dir).await?;
    let branch = backend.git_branch(&dir).await.unwrap_or_else(|| "?".into());
    let Some(old) = focused_slot(app).as_ref() else { return Ok(branch) };
    let (top, left, wrap, from) = (old.top, old.left, old.wrap, old.from_tab);
    let mut p = Preview::from_git(&snap);
    p.top = top;
    p.left = left;
    if p.wrap != wrap { p.toggle_wrap(); p.left = left; }
    p.from_tab = from;
    *focused_slot(app) = Some(p);
    Ok(branch)
}

/// Re-read the file shown in the active preview tab, keeping scroll position and wrap setting.
async fn reload_preview(app: &mut App, backend: &Backend, cap: usize, opts: &Opts) -> Result<()> {
    let Some(old) = focused_slot(app).as_ref() else { return Ok(()) };
    let (path, top, left, wrap, from, target) = (old.path.clone(), old.top, old.left, old.wrap, old.from_tab, old.target_line);
    let md_view = old.md_view();
    let (bytes, total) = backend.read_prefix(&path, cap).await?;
    let mut p = Preview::build(&path, &bytes, total, opts);
    p.restore_md_view(md_view);
    p.top = top;
    p.left = left;
    if p.wrap != wrap {
        p.toggle_wrap();
        p.left = left;
    }
    p.from_tab = from;
    p.target_line = target;
    *focused_slot(app) = Some(p);
    Ok(())
}

/// The preview that has the keyboard: the side pane when it is focused, else the tab's own.
fn focused_slot(app: &mut App) -> &mut Option<Preview> {
    let t = &mut app.tabs[app.active];
    if t.side_focused() { &mut t.side } else { &mut t.preview }
}

/// The tab that fills a pane: the active tab, or beside it the pinned tab (or the other tab).
fn pane_tab(app: &App, pane: Pane) -> &Tab {
    if pane == Pane::Side && let Some(pi) = pin_partner(app) {
        return &app.tabs[pi];
    }
    &app.tabs[app.active]
}

/// The tab in the other pane, while a pinned tab is in use.
fn pin_partner(app: &App) -> Option<usize> {
    app.pinned.and(partner_idx(app))
}

/// The preview shown in a pane of the active tab (a preview tab's own preview counts as `Shell`).
fn pane_preview(app: &App, pane: Pane) -> Option<&Preview> {
    if app.pinned.is_some() {
        return pane_tab(app, pane).preview.as_ref();
    }
    let t = &app.tabs[app.active];
    match pane {
        Pane::Side => t.side.as_ref(),
        Pane::Shell => t.preview.as_ref(),
    }
}

fn pane_preview_mut(app: &mut App, pane: Pane) -> Option<&mut Preview> {
    let idx = if app.pinned.is_some() && pane == Pane::Side { pin_partner(app).unwrap_or(app.active) } else { app.active };
    let t = &mut app.tabs[idx];
    match (app.pinned.is_some(), pane) {
        (false, Pane::Side) => t.side.as_mut(),
        _ => t.preview.as_mut(),
    }
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// The pane rectangles while two panes are showing (a side preview, or a pinned tab), when the
/// window has room for both. `shell` is the active tab's, `side` the other one's.
fn split_of(app: &App, tab: &Tab, main: Rect) -> Option<Split> {
    if app.pinned.is_some() {
        return pin_split(app, main);
    }
    tab.side.as_ref().and_then(|_| split_main(main, app.side, app.pct))
}

/// The pane holding the pinned tab, when two panes show.
fn pinned_rect(app: &App, sp: &Split) -> Rect {
    if app.pinned.is_some() && pinned_idx(app) == Some(app.active) { sp.shell } else { sp.side }
}

/// The area a pane occupies in the active tab.
fn pane_rect(app: &App, main: Rect, pane: Pane) -> Rect {
    match (split_of(app, &app.tabs[app.active], main), pane) {
        (Some(s), Pane::Side) => s.side,
        (Some(s), Pane::Shell) => s.shell,
        (None, _) => main,
    }
}

/// Every pane currently on screen with its area.
fn visible_panes(app: &App, main: Rect) -> Vec<(Pane, Rect)> {
    match split_of(app, &app.tabs[app.active], main) {
        Some(s) => vec![(Pane::Shell, s.shell), (Pane::Side, s.side)],
        None => vec![(Pane::Shell, main)],
    }
}

/// Esc in a preview: return to the tab it was opened from, or the first shell.
fn go_back(app: &mut App) {
    let from = app.tabs[app.active].preview.as_ref().map(|p| p.from_tab).unwrap_or(0);
    let target = if from < app.tabs.len() && from != app.active && !app.tabs[from].is_preview() {
        from
    } else {
        app.tabs.iter().position(|t| !t.is_preview()).unwrap_or(0)
    };
    switch_to(app, target);
}

/// What a confirmed question will do.
enum Pending {
    Url(String),
    OpenLocal(std::path::PathBuf),
    /// Download from the box into the cache, then open.
    Fetch { remote: String },
    /// Update wi: contact GitHub to look for a newer release.
    UpdateCheck,
    /// Update wi: download, verify and install this release.
    UpdateApply(crate::update::Release),
    /// Restart wi in place; `updated` is the version just installed, if any.
    Restart { updated: Option<String> },
}

/// What an update worker thread reports.
enum UpdEv {
    Status(String),
    Checked(Result<crate::update::Checked, String>),
    Installed(Result<crate::update::Installed, String>),
}

/// Start the hands-free mode with the helper found at `prefix`.
fn start_dictation(prefix: &[String], tcfg: &crate::config::Transcribe, gen_id: &mut u64, tx: &mpsc::UnboundedSender<(u64, dictate::Ev)>, dict: &mut Option<dictate::Dictation>, message: &mut Option<String>) {
    *gen_id += 1;
    match dictate::Dictation::spawn(prefix, tcfg, *gen_id, tx.clone()) {
        Ok(d) => {
            *dict = Some(d);
            *message = Some("dictation on: speak, phrases are typed without Enter · menu > Dictation or a click on the mic turns it off".into());
        }
        Err(e) => *message = Some(e),
    }
}

fn restart_dialog(app: &App, remote: bool, tmux: bool, updated: Option<&str>) -> Mode {
    Mode::Confirm(Confirm {
        title: match updated {
            Some(v) => format!("Updated to {v}. Restart wi now?"),
            None => "Restart wi now?".into(),
        },
        lines: vec![crate::restart::summary(app.tabs.len(), remote, tmux), "No: keep running; restart wi when convenient.".into()],
        yes: "Restart".into(),
    })
}

/// The session as it is now, for the restarted wi.
async fn build_snapshot(app: &App, backend: &Backend, ws: &Workspace) -> crate::restart::Snapshot {
    use crate::restart::{SnapTab, Snapshot};
    let where_ = if ws.is_remote() { "remote" } else { "local" };
    let mut tabs = Vec::new();
    for t in &app.tabs {
        if let Some(p) = &t.preview {
            if p.path.starts_with('/') {
                tabs.push(SnapTab { kind: "preview".into(), backend: where_.into(), title: t.title.clone(), name: t.custom.clone().unwrap_or_default(), path: p.path.clone(), ..Default::default() });
            } else {
                tabs.push(SnapTab { kind: "preview".into(), backend: where_.into(), title: t.title.clone(), name: t.custom.clone().unwrap_or_default(), ..Default::default() });
            }
            continue;
        }
        let cwd = crate::cwd::tab_cwd(backend, t.compat.cwd.as_deref(), t.handle.as_ref().and_then(|h| h.pid()), app.sessions.get(&t.id).map(String::as_str)).await.unwrap_or_else(|| t.dir.clone());
        tabs.push(SnapTab {
            kind: if t.agent.is_some() { "agent" } else { "shell" }.into(),
            backend: where_.into(),
            cwd,
            title: t.title.clone(),
            name: t.custom.clone().unwrap_or_default(),
            agent: t.agent.clone().unwrap_or_default(),
            path: String::new(),
        });
    }
    Snapshot {
        key: agents::workspace_key(ws, &app.root),
        active: app.active,
        pinned: app.pinned.and_then(|id| app.tabs.iter().position(|t| t.id == id)),
        pin_side: app.pin_side.name().into(),
        side: app.side.name().into(),
        pct: app.pct,
        tabs,
        saved: crate::restart::now_secs(),
    }
}

/// Save the layout and exec this same file again. Only returns when that failed (the terminal
/// is put back and wi keeps running); the text says why.
async fn do_restart(app: &App, backend: &Backend, ws: &Workspace, state: &mut agents::State, mouse_on: bool) -> String {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(p) => std::path::PathBuf::from(crate::restart::strip_deleted(&p.to_string_lossy())),
        Err(e) => return format!("could not restart: {e}"),
    };
    if !exe.is_file() {
        return format!("could not restart: {} is missing", exe.display());
    }
    state.restart = Some(build_snapshot(app, backend, ws).await);
    if let Err(e) = state.save() {
        state.restart = None;
        return format!("could not restart: cannot save the layout ({e})");
    }
    crate::term::restore_terminal();
    let err = std::process::Command::new(&exe).args(std::env::args_os().skip(1)).env(crate::restart::MARKER, "1").exec();
    state.restart = None;
    let _ = state.save();
    if let Ok(g) = TermGuard::enter(mouse_on) { std::mem::forget(g); }
    format!("could not restart: {err} (wi keeps running)")
}

/// Act on a worker's report: ask the second question, or tell the result.
#[allow(clippy::too_many_arguments)]
fn on_update_event(ev: UpdEv, mode: &mut Mode, pending: &mut Option<Pending>, message: &mut Option<String>, app: &mut App, updating: &mut bool, opts: &Opts, remote: bool, tmux: bool) {
    use crate::update::{Checked, Installed};
    match ev {
        UpdEv::Status(s) => *message = Some(s),
        UpdEv::Checked(Err(e)) => {
            *updating = false;
            *message = Some(format!("update: {e}"));
        }
        UpdEv::Checked(Ok(Checked::UpToDate(v))) => {
            *updating = false;
            *message = Some(format!("wi {v} is up to date"));
        }
        UpdEv::Checked(Ok(Checked::Newer(rel))) => {
            let cur = env!("CARGO_PKG_VERSION");
            let size = rel.size.map(|n| format!("downloads ~{:.1} MB, ", n as f64 / 1_048_576.0)).unwrap_or_else(|| "downloads the release archive, ".into());
            *mode = Mode::Confirm(Confirm {
                title: format!("Update {cur} -> {}?", rel.version),
                lines: vec![format!("{size}verifies SHA256, replaces this binary."), "Open tabs keep running; restart wi afterwards.".into()],
                yes: "Update".into(),
            });
            *pending = Some(Pending::UpdateApply(rel));
        }
        UpdEv::Installed(Err(e)) => {
            *updating = false;
            *message = Some(format!("update failed: {e}"));
        }
        UpdEv::Installed(Ok(Installed::Done(v))) => {
            *updating = false;
            *message = Some(format!("Updated to {v} — restart wi to use it"));
            *mode = restart_dialog(app, remote, tmux, Some(&v));
            *pending = Some(Pending::Restart { updated: Some(v) });
        }
        UpdEv::Installed(Ok(Installed::NeedsSudo { tmp, target, version })) => {
            *updating = false;
            let text = format!(
                "wi {version} is downloaded and verified, but its folder is not writable by you:\n{}\n\nwi does not use sudo. Run this in a shell tab (one command), then restart wi:\n\n{}\n",
                target.parent().unwrap_or(&target).display(),
                crate::update::sudo_command(&tmp, &target)
            );
            let p = Preview::build("wi-update.txt", text.as_bytes(), text.len() as u64, opts);
            let id = app.next_id;
            app.next_id += 1;
            app.tabs.push(Tab::new_preview(id, p));
            app.active = app.tabs.len() - 1;
            sync_pin(app);
            let _ = resize_all(app);
            *message = Some("update: install folder is not writable; the command to run is in the wi-update.txt tab".to_string());
        }
    }
}

struct HintState {
    links: Vec<(Link, Pane)>,
    hints: Vec<Hint>,
    typed: String,
}

/// A running download. The future is polled by the main loop so the screen stays responsive.
struct Fetching<'a> {
    name: String,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    fut: Pin<Box<dyn Future<Output = Result<FetchResult, String>> + 'a>>,
}

impl Fetching<'_> {
    fn dialog(&self) -> Confirm {
        let (d, t) = (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        let pct = (d * 100).checked_div(t).unwrap_or(0);
        let filled = (pct as usize * 30) / 100;
        Confirm {
            title: "Downloading from the box".into(),
            lines: vec![
                self.name.clone(),
                String::new(),
                format!("{} of {}  ({pct}%)", crate::preview::human(d), crate::preview::human(t)),
                format!("[{}{}]", "#".repeat(filled), "-".repeat(30 - filled)),
            ],
            yes: String::new(),
        }
    }
}

/// The result of looking at a clicked or labelled link.
enum LinkOutcome {
    Message(String),
    /// Show this file in a preview tab, at this line.
    Preview(String, Option<u32>),
    /// The link is a folder: open the file explorer rooted there.
    Explorer(String),
    /// Several matches: ask which (path as clicked, candidates, line).
    Pick(String, Vec<String>, Option<u32>),
    /// Ask first.
    Confirm(Confirm, Pending),
    /// Confirmation is switched off in the config: do it now.
    Now(Pending),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Down,
    Drag,
    Up,
}

/// Move the active tab's scrollback view by `n` lines (positive = further back). Returns true
/// when the view is at the live bottom afterwards.
fn scroll_by(app: &mut App, n: isize) -> bool {
    let t = &mut app.tabs[app.active];
    let (cur, max) = (t.parser.screen().scrollback(), t.parser.screen().scrollback_len());
    let new = if n >= 0 { (cur + n as usize).min(max) } else { cur.saturating_sub(n.unsigned_abs()) };
    t.parser.screen_mut().set_scrollback(new);
    new == 0
}

/// Put the active tab's view back at the live bottom (typing needs it). Returns true if it moved.
fn snap_live(app: &mut App) -> bool {
    let t = &mut app.tabs[app.active];
    if t.is_preview() || t.parser.screen().scrollback() == 0 { return false; }
    t.parser.screen_mut().set_scrollback(0);
    true
}

/// An in-progress or finished text selection.
struct SelState {
    tab: usize,
    pane: Pane,
    anchor: Pos,
    end: Pos,
    dragging: bool,
    /// 1 = drag, 2 = word, 3 = line
    clicks: u8,
}

enum Intent {
    /// Click on the preview pane's ✕ button.
    ClosePreview,
    /// Click on the ⇄ button: move the pane to the other side.
    SwapSides,
    None,
    /// Ctrl/Alt+click (or right-click) on a path or URL.
    OpenLink(Link, Pane),
    Select(Phase, Pos, Pane),
    /// Mouse went down on the divider between the panes.
    DividerDown,
    /// Divider drag: the pointer column.
    DividerDrag(u16),
    DividerUp,
    /// Handle as if these bytes had been typed (or, in a shell tab, send them to the app).
    Synth(Vec<u8>),
    SwitchTab(usize),
    /// A click on the active tab (or the second click of a double-click): name it.
    RenameTab(usize),
    PreviewScroll(isize, Pane),
    /// Mouse wheel in scrollback: positive looks further back, negative returns toward the live view.
    ScrollWheel(isize),
    EnterScroll,
    PickCandidate(usize),
    /// A click on a row of the results list or the file finder (row within the list area).
    ListPick(usize),
    /// A click on a menu entry (index into the menu's items).
    MenuPick(usize),
    /// A click on the menu's filter row.
    MenuFilter,
    /// A click outside the menu.
    MenuClose,
    /// Mouse wheel over the menu: move the selection this many entries.
    MenuWheel(isize),
}

/// What a mouse event means right now. Pure so it can be tested without a terminal.
/// `selecting` is the pane of a selection being dragged, if any.
fn mouse_intent(m: &MouseEvent, mode: &Mode, app: &App, items: &[&MenuItem], main: Rect, selecting: Option<Pane>, link_click: &str) -> Intent {
    let hit = |r: Rect| inside(r, m.col, m.row);
    match mode {
        Mode::Menu(st) => {
            let lay = ui::menu_layout(main, items, st);
            match m.kind {
                Kind::Down(_) => match ui::menu_hit(&lay, m.col, m.row) {
                    ui::MenuHit::Item(i) => Intent::MenuPick(i),
                    ui::MenuHit::Filter => Intent::MenuFilter,
                    ui::MenuHit::Inert => Intent::None,
                    ui::MenuHit::Outside => Intent::MenuClose,
                },
                Kind::WheelUp => Intent::MenuWheel(-3),
                Kind::WheelDown => Intent::MenuWheel(3),
                _ => Intent::None,
            }
        }
        Mode::Rename(_) | Mode::Pick(_) | Mode::Ask(_) | Mode::ThemePick(_) | Mode::Confirm(_) | Mode::Voice(_) | Mode::Fetch | Mode::Hints(_) | Mode::FindPrompt(_) | Mode::Working(_) | Mode::Explorer(_) => Intent::None,
        Mode::GrepPrompt(p) => match m.kind {
            Kind::Down(mouse::Button::Left) => ui::search::grep_chip_at(main, p, m.col, m.row).map_or(Intent::None, |k| Intent::Synth(k.to_vec())),
            _ => Intent::None,
        },
        Mode::Results(_) | Mode::Finder(_) => {
            let l = if matches!(mode, Mode::Results(_)) { ui::search::results_list_rect(main) } else { ui::search::finder_list_rect(main) };
            match m.kind {
                Kind::Down(mouse::Button::Left) if hit(l) => Intent::ListPick((m.row - l.y) as usize),
                Kind::WheelUp => Intent::Synth(b"\x1b[A".repeat(3)),
                Kind::WheelDown => Intent::Synth(b"\x1b[B".repeat(3)),
                _ => Intent::None,
            }
        }
        Mode::Prompt(p) => {
            let r = ui::prompt_rect(main, p);
            match m.kind {
                Kind::Down(_) if hit(r) && ((m.row - r.y) as usize) < p.shown.len() => Intent::PickCandidate((m.row - r.y) as usize),
                _ => Intent::None,
            }
        }
        Mode::Scroll | Mode::OutFind(_) => pane_mouse(m, app, main, selecting, link_click, true),
        Mode::Shell => {
            if app.tabs.len() > 1 && m.row == 1 {
                let strip = strip_of(app);
                return match m.kind {
                    Kind::Down(mouse::Button::Left) => ui::tab_at(&strip, m.col).map_or(Intent::None, |i| if i == app.active { Intent::RenameTab(i) } else { Intent::SwitchTab(i) }),
                    _ => Intent::None,
                };
            }
            pane_mouse(m, app, main, selecting, link_click, false)
        }
    }
}

/// A mouse event over the shell, a preview, or the divider between them. Coordinates handed on
/// are relative to the pane the event belongs to. `scrolling`: the shell is browsing scrollback.
fn pane_mouse(m: &MouseEvent, app: &App, main: Rect, selecting: Option<Pane>, link_click: &str, scrolling: bool) -> Intent {
    let up3 = || Intent::Synth(b"\x1b[A".repeat(3));
    let down3 = || Intent::Synth(b"\x1b[B".repeat(3));
    let tab = &app.tabs[app.active];
    let split = split_of(app, tab, main);
    if let Some(sp) = &split {
        if app.div_drag {
            match m.kind {
                Kind::Drag(Button::Left) => return Intent::DividerDrag(m.col),
                Kind::Up(Button::Left) => return Intent::DividerUp,
                _ => {}
            }
        }
        if matches!(m.kind, Kind::Down(Button::Left)) {
            match button_at(pinned_rect(app, sp), m.col, m.row) {
                Some(Btn::Close) => return Intent::ClosePreview,
                Some(Btn::Swap) => return Intent::SwapSides,
                None => {}
            }
        }
        if matches!(m.kind, Kind::Down(Button::Left)) && sp.on_divider(m.col, m.row) {
            return Intent::DividerDown;
        }
    }
    if !scrolling && !inside(main, m.col, m.row) {
        return Intent::None;
    }
    let pane = match (&split, selecting) {
        (Some(_), Some(p)) => p,
        (Some(sp), None) => {
            if inside(sp.side, m.col, m.row) { Pane::Side } else if inside(sp.shell, m.col, m.row) { Pane::Shell } else { return Intent::None }
        }
        (None, _) => Pane::Shell,
    };
    let area = match (&split, pane) {
        (Some(sp), Pane::Side) => sp.side,
        (Some(sp), Pane::Shell) => sp.shell,
        (None, _) => main,
    };
    if app.pinned.is_some() && pane == Pane::Side {
        return Intent::None; // a click there first moves the keyboard (handled before this)
    }
    let sel = selecting.is_some();
    let link_or_select = || link_intent(m, app, area, pane, link_click).unwrap_or_else(|| select_intent(m, app, area, pane, sel));
    if pane_preview(app, pane).is_some() {
        return match m.kind {
            Kind::WheelUp => Intent::PreviewScroll(-3, pane),
            Kind::WheelDown => Intent::PreviewScroll(3, pane),
            _ => link_or_select(),
        };
    }
    if scrolling {
        return match m.kind {
            Kind::WheelUp => Intent::ScrollWheel(3),
            Kind::WheelDown => Intent::ScrollWheel(-3),
            _ => link_or_select(),
        };
    }
    let scr = tab.parser.screen();
    use vt100::{MouseProtocolEncoding, MouseProtocolMode};
    let wanted = match scr.mouse_protocol_mode() {
        MouseProtocolMode::None => None,
        MouseProtocolMode::Press => Some(matches!(m.kind, Kind::Down(_) | Kind::WheelUp | Kind::WheelDown)),
        MouseProtocolMode::PressRelease => Some(!matches!(m.kind, Kind::Drag(_) | Kind::Other)),
        MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion => Some(!matches!(m.kind, Kind::Other)),
    };
    match wanted {
        // The app in this tab asked for the mouse: give it the event at its own coordinates.
        Some(true) => Intent::Synth(mouse::encode(m, m.col.saturating_sub(area.x), m.row.saturating_sub(area.y), scr.mouse_protocol_encoding() == MouseProtocolEncoding::Sgr)),
        Some(false) => Intent::None,
        None => match m.kind {
            // Full-screen apps without mouse support get arrow keys, like a terminal's alternate-scroll.
            Kind::WheelUp if scr.alternate_screen() => up3(),
            Kind::WheelDown if scr.alternate_screen() => down3(),
            Kind::WheelUp => Intent::EnterScroll,
            _ => link_or_select(),
        },
    }
}

/// Left button down/drag/up selects text in `area`. Drag and release keep working outside the
/// area so a selection can be dragged past the edge.
fn select_intent(m: &MouseEvent, app: &App, area: Rect, pane: Pane, selecting: bool) -> Intent {
    let banner = pane_preview(app, pane).is_some_and(|p| p.has_header()) && area.height > 1;
    let origin = area.y + u16::from(banner);
    let height = area.height - u16::from(banner);
    let clamped = (m.row.saturating_sub(origin).min(height.saturating_sub(1)), m.col.saturating_sub(area.x).min(area.width.saturating_sub(1)));
    let in_area = m.row >= origin && m.row < origin + height && inside(Rect::new(area.x, area.y, area.width, area.height), m.col, m.row);
    match m.kind {
        Kind::Down(Button::Left) if in_area => Intent::Select(Phase::Down, clamped, pane),
        Kind::Drag(Button::Left) if selecting => Intent::Select(Phase::Drag, clamped, pane),
        Kind::Up(Button::Left) if selecting => Intent::Select(Phase::Up, clamped, pane),
        _ => Intent::None,
    }
}

/// Cell text of every row in the pane being selected in, and which rows soft-wrap.
fn area_rows(app: &App, area: Rect, pane: Pane) -> (Vec<Vec<String>>, Vec<bool>) {
    match pane_preview(app, pane) {
        Some(p) => {
            let banner = p.has_header() && area.height > 1;
            let h = (area.height - u16::from(banner)) as usize;
            let rows: Vec<Vec<String>> = p.visible(h, area.width as usize).iter().map(|l| crate::preview::row_cells(l, p.gutter, area.width as usize)).collect();
            let wrapped = vec![false; rows.len()];
            (rows, wrapped)
        }
        None => select::screen_rows(pane_tab(app, pane).parser.screen()),
    }
}

/// Open a shell tab in `dir` named after the agent and run the user's command in it. The tab
/// stays a normal shell, so when the agent exits you are left at a prompt.
async fn start_agent(app: &mut App, backend: &Backend, tx: &mpsc::Sender<Event>, agent: Agent, command: &str, dir: &str) -> Result<()> {
    let (c, r) = shell_size(app.tabs.len() + 1)?;
    open_tab_in(app, backend, tx, c, r, None, Some(dir)).await?;
    let last = app.tabs.len() - 1;
    run_agent_in(app, last, agent, command);
    app.active = last;
    resize_all(app)
}

/// Run the agent's command in an existing shell tab (it has just been opened, at a prompt).
fn run_agent_in(app: &mut App, idx: usize, agent: Agent, command: &str) {
    let tab = &mut app.tabs[idx];
    tab.agent = Some(agent.key().to_string());
    tab.retitle();
    tab.write(format!("{command}\n").into_bytes());
}

/// The area the shell (or a preview) is drawn in.
fn main_rect(app: &App) -> Result<Rect> {
    let (tc, tr) = crossterm::terminal::size()?;
    let strip_h = u16::from(app.tabs.len() > 1);
    Ok(Rect::new(0, 1 + strip_h, tc, tr.saturating_sub(2 + strip_h).max(1)))
}

/// First screen row of a pane's content (below the banner of a preview that has one).
fn origin_row(app: &App, area: Rect, pane: Pane) -> u16 {
    let banner = pane_preview(app, pane).is_some_and(|p| p.has_header()) && area.height > 1;
    area.y + u16::from(banner)
}

/// A left click with Ctrl/Alt held (per `link_click`), or a right click, on a path or URL.
fn link_intent(m: &MouseEvent, app: &App, area: Rect, pane: Pane, link_click: &str) -> Option<Intent> {
    let wanted = match m.kind {
        Kind::Down(Button::Left) => mouse::is_link_click(m.code, link_click, mouse::cmd_held()),
        Kind::Down(Button::Right) => link_click != "off",
        _ => false,
    };
    if !wanted {
        return None;
    }
    let origin = origin_row(app, area, pane);
    if m.row < origin || m.row >= area.y + area.height || m.col < area.x || m.col >= area.x + area.width {
        return None;
    }
    let (rows, wrapped) = area_rows(app, area, pane);
    links::link_at(&rows, &wrapped, m.row - origin, m.col - area.x).map(|l| Intent::OpenLink(l, pane))
}

/// Where relative paths in the active tab's output point: a preview's own folder, else the
/// shell's working directory.
async fn link_cwd(app: &App, backend: &Backend, pane: Pane) -> Option<String> {
    let tab = pane_tab(app, pane);
    if let Some(p) = pane_preview(app, pane) {
        return std::path::Path::new(&p.path).parent().filter(|_| p.path.starts_with('/')).map(|d| d.to_string_lossy().into_owned());
    }
    // The pty's foreground process (what is printing) when known, else the shell itself.
    let fg = match tab.handle.as_ref() {
        Some(h) if matches!(backend, Backend::Local(_)) => h.foreground().await.and_then(|p| u32::try_from(p).ok()).filter(|p| *p > 1),
        _ => None,
    };
    let pid = fg.or_else(|| tab.handle.as_ref().and_then(|h| h.pid()));
    crate::cwd::tab_cwd(backend, tab.compat.cwd.as_deref(), pid, app.sessions.get(&tab.id).map(String::as_str)).await
}

/// The first candidate that reads as a file (with its head and size), and the first that is a folder.
async fn probe_candidates(backend: &Backend, cands: &[String]) -> (Option<(String, Vec<u8>, u64)>, Option<String>) {
    let mut dir = None;
    for cand in cands {
        match backend.read_prefix(cand, 4096).await {
            Ok((head, size)) => return (Some((cand.clone(), head, size)), dir),
            Err(e) => {
                if dir.is_none() && e.to_string().contains("is a directory") {
                    dir = Some(cand.clone());
                }
            }
        }
    }
    (None, dir)
}

/// Work out what a link means and what to ask the user, without doing anything yet.
async fn resolve_link(app: &App, backend: &Backend, ws: &Workspace, link: &Link, pane: Pane, open_cfg: &OpenCfg, confirm: bool) -> LinkOutcome {
    match &link.kind {
        LinkKind::Url(u) => {
            let lower = u.to_ascii_lowercase();
            if !(lower.starts_with("http://") || lower.starts_with("https://")) || !opener::url_allowed(u) {
                return LinkOutcome::Message("only http and https links are opened".into());
            }
            let p = Pending::Url(u.clone());
            if !confirm {
                return LinkOutcome::Now(p);
            }
            LinkOutcome::Confirm(
                Confirm {
                    title: "Open link in your browser".into(),
                    lines: vec![u.clone(), String::new(), "Opened by the operating system's default browser.".into()],
                    yes: "Open".into(),
                },
                p,
            )
        }
        LinkKind::Path { path, line, .. } => {
            let remote = matches!(backend, Backend::Remote(_));
            let cwd = link_cwd(app, backend, pane).await;
            let root = backend.resolve("").await.unwrap_or_default();
            let home = if remote { root.clone() } else { dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default() };
            let tab_dir = pane_tab(app, pane).dir.clone();
            // Bases, in order: the shell's own folder (OSC 7 or the OS), the workspace root, the
            // folder the tab was started in.
            let mut cands = links::resolve_candidates(path, cwd.as_deref(), &root, Some(&home));
            for c in links::resolve_candidates(path, Some(&tab_dir), &root, Some(&home)) {
                if !cands.contains(&c) {
                    cands.push(c);
                }
            }
            let (mut found, mut dir) = probe_candidates(backend, &cands).await;
            if found.is_none() && dir.is_none() && !path.starts_with('/') && !path.starts_with('~') {
                // Not where any base says: one bounded search for the same trailing path.
                let mut bases: Vec<String> = Vec::new();
                for b in [root.clone(), tab_dir.clone()] {
                    if !b.is_empty() && !bases.iter().any(|x| b == *x || b.starts_with(&format!("{}/", x.trim_end_matches('/')))) {
                        bases.push(b);
                    }
                }
                let hits = crate::pick::search(backend, &bases, path).await;
                match hits.len() {
                    0 => {}
                    1 => (found, dir) = probe_candidates(backend, &hits).await,
                    _ => return LinkOutcome::Pick(path.clone(), hits, *line),
                }
            }
            let Some((abs, head, size)) = found else {
                if let Some(d) = dir {
                    return LinkOutcome::Explorer(d);
                }
                return LinkOutcome::Message(format!("no such file: {}", sanitize(path)));
            };
            match opener::classify(&abs, &head, size, open_cfg) {
                Handling::Preview => LinkOutcome::Preview(abs, *line),
                Handling::Native => {
                    if let Err(e) = opener::native_safe(&abs) {
                        return LinkOutcome::Message(sanitize(&e));
                    }
                    let (p, dialog) = if remote {
                        let from = match ws {
                            Workspace::Remote(b) => b.name.clone(),
                            Workspace::Local(_) => String::new(),
                        };
                        (
                            Pending::Fetch { remote: abs.clone() },
                            Confirm {
                                title: "Download and open".into(),
                                lines: vec![
                                    format!("File: {abs}"),
                                    format!("Size: {}", crate::preview::human(size)),
                                    format!("From: {from}"),
                                    String::new(),
                                    "It is copied to your computer's cache, then opened by the".into(),
                                    "operating system's default app for this type.".into(),
                                ],
                                yes: "Download and open".into(),
                            },
                        )
                    } else {
                        (
                            Pending::OpenLocal(std::path::PathBuf::from(&abs)),
                            Confirm {
                                title: "Open in the default app".into(),
                                lines: vec![
                                    format!("File: {abs}"),
                                    format!("Size: {}", crate::preview::human(size)),
                                    String::new(),
                                    "Opened by the operating system's default app for this type.".into(),
                                ],
                                yes: "Open".into(),
                            },
                        )
                    };
                    if confirm { LinkOutcome::Confirm(dialog, p) } else { LinkOutcome::Now(p) }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_outcome<'a>(
    o: LinkOutcome,
    app: &mut App,
    backend: &'a Backend,
    ws: &Workspace,
    mode: &mut Mode,
    pending: &mut Option<Pending>,
    message: &mut Option<String>,
    fetching: &mut Option<Fetching<'a>>,
    read_cap: usize,
    opts: &Opts,
) {
    match o {
        LinkOutcome::Message(m) => *message = Some(m),
        LinkOutcome::Explorer(_) => {} // opened by the caller, which owns the explorer state
        LinkOutcome::Pick(path, items, line) => *mode = Mode::Pick(crate::pick::Pick::new(path, items, line)),
        LinkOutcome::Preview(abs, line) => {
            match open_preview(app, backend, &abs, read_cap, opts, line).await {
                Ok(Placement::TabNarrow) => *message = Some("narrow: opened in a tab".into()),
                Ok(_) => {}
                Err(e) => *message = Some(format!("could not open: {e}")),
            }
        }
        LinkOutcome::Confirm(c, p) => {
            *mode = Mode::Confirm(c);
            *pending = Some(p);
        }
        LinkOutcome::Now(p) => run_pending(p, backend, ws, mode, fetching, message),
    }
}

/// Do what the user just agreed to.
fn run_pending<'a>(p: Pending, backend: &'a Backend, ws: &Workspace, mode: &mut Mode, fetching: &mut Option<Fetching<'a>>, message: &mut Option<String>) {
    match p {
        Pending::Url(u) => {
            *message = Some(match open_url_hooked(&u) {
                Ok(()) => "opened link in your browser".to_string(),
                Err(e) => e,
            });
        }
        Pending::OpenLocal(path) => {
            let name = path.file_name().map(|n| sanitize(&n.to_string_lossy())).unwrap_or_default();
            *message = Some(match native_open_hooked(&path) {
                Ok(()) => format!("opened {name}"),
                Err(e) => e,
            });
        }
        Pending::UpdateCheck | Pending::UpdateApply(_) | Pending::Restart { .. } => {} // started by the Confirm handler
        Pending::Fetch { remote } => {
            let box_name = match ws {
                Workspace::Remote(b) => b.name.clone(),
                Workspace::Local(_) => String::new(),
            };
            *fetching = Some(start_fetch(backend, remote, box_name));
            *mode = Mode::Fetch;
        }
    }
}

fn start_fetch(backend: &Backend, remote: String, box_name: String) -> Fetching<'_> {
    let (done, total, cancel) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)));
    let (d, t, c) = (done.clone(), total.clone(), cancel.clone());
    let name = sanitize(remote.rsplit('/').next().unwrap_or(&remote));
    let mut opts = FetchOpts::new(&box_name);
    opts.confirmed = true; // the user already agreed in the dialog, which showed the size
    let fut = Box::pin(async move {
        let mut progress = |dn: u64, tt: u64| {
            d.store(dn, Ordering::Relaxed);
            t.store(tt, Ordering::Relaxed);
        };
        backend.fetch_to_cache(&remote, &opts, &mut progress, &c).await
    });
    Fetching { name, done, total, cancel, fut }
}

fn append_log(log: &str, line: String) -> Result<(), String> {
    use std::io::Write;
    std::fs::OpenOptions::new().create(true).append(true).open(log).and_then(|mut f| f.write_all(line.as_bytes())).map_err(|e| e.to_string())
}

/// Hand a file to the OS. `WICLI_OPEN_LOG=<file>` records the request instead (used by the tests,
/// so they never launch real apps).
fn native_open_hooked(p: &std::path::Path) -> Result<(), String> {
    match std::env::var("WICLI_OPEN_LOG") {
        Ok(log) => append_log(&log, format!("open {}\n", p.display())),
        Err(_) => opener::native_open(p),
    }
}

fn open_url_hooked(u: &str) -> Result<(), String> {
    match std::env::var("WICLI_OPEN_LOG") {
        Ok(log) => append_log(&log, format!("url {u}\n")),
        Err(_) => opener::open_url(u),
    }
}

#[cfg(test)]
mod pin_tests {
    use super::*;

    fn app(n: usize) -> App {
        let tabs = (0..n).map(|i| Tab::new(i, format!("shell {i}"), crate::workspace::ShellHandle::dummy(), 80, 24, 10)).collect();
        App { tabs, active: 0, next_id: n, shell_count: n, scrollback: 10, quiet: Duration::from_secs(5), sessions: Default::default(), root: "/".into(), side: Side::Right, pct: 50, div_drag: false, pinned: None, pin_side: Side::Right, last_main: 0, history: Vec::new(), hist_dirty: false }
    }

    #[test]
    fn partner_is_the_pinned_tab_or_the_last_other_tab() {
        let mut a = app(3);
        assert_eq!(partner_idx(&a), None);
        a.pinned = Some(2);
        a.active = 0;
        assert_eq!(partner_idx(&a), Some(2));
        a.active = 2;
        a.last_main = 1;
        assert_eq!(partner_idx(&a), Some(1), "pinned is active: the other pane shows the last other tab");
        a.last_main = 99;
        assert_eq!(partner_idx(&a), Some(0), "a vanished tab falls back to the first other tab");
        let mut one = app(1);
        one.pinned = Some(0);
        assert_eq!(partner_idx(&one), None, "a lone pinned tab has no pane beside it");
    }

    #[test]
    fn pinned_pane_stays_on_its_side_whoever_is_active() {
        let mut a = app(2);
        a.pinned = Some(1);
        let main = Rect::new(0, 0, 120, 26);
        let beside = pin_split(&a, main).unwrap();
        assert!(beside.side.x > beside.shell.x, "pinned right, active left");
        assert_eq!(pinned_rect(&a, &beside), beside.side);
        a.active = 1;
        a.last_main = 0;
        let pinned_active = pin_split(&a, main).unwrap();
        assert_eq!(pinned_rect(&a, &pinned_active), pinned_active.shell);
        assert_eq!(pinned_active.shell, beside.side, "same rectangle for the pinned tab");
        assert!(pin_split(&a, Rect::new(0, 0, 90, 26)).is_none(), "narrow: one pane");
    }

    #[test]
    fn swap_and_unsplit() {
        let mut a = app(2);
        a.pinned = Some(1);
        swap_sides(&mut a);
        assert_eq!(a.pin_side, Side::Left);
        swap_sides(&mut a);
        assert_eq!(a.pin_side, Side::Right);
        unsplit(&mut a);
        assert_eq!(a.pinned, None);
        assert_eq!(a.tabs.len(), 2, "the pinned tab stays a tab");
    }
}
