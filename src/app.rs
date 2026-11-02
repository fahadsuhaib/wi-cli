//! Event loop: input, resize, shell output from every tab, coalesced redraw.

use crate::input::{InEv, PasteFilter};
use crate::keys::{EnterMap, KeyDecoder};
use crate::scroll::{self, ScrollKey};
use crate::config::{Config, parse_prefix_key, prefix_label};
use crate::explorer::{self, Act as ExAct, ExJob, ExMouse, ExplorerState};
use crate::tab::{Pane, Side, Split, Tab, pct_for_col, split_main};
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
use crate::ui::{self, Action, Ask, Confirm, Hint, MenuItem, Prompt, PromptKind, View, hint_labels, menu_items_for, sanitize};
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
    Ask(Ask),
    /// A yes/no question before anything is opened or downloaded.
    Confirm(Confirm),
    /// A download is running; Esc cancels it.
    Fetch,
    /// Every path and link on screen is labelled with a letter.
    Hints(HintState),
    Menu(usize),
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
}

/// A search or file listing in flight (dropped = cancelled; the process is killed with it).
type Job<'a> = Pin<Box<dyn Future<Output = JobDone> + 'a>>;

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
    Enter,
    Esc,
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
                match buf.get(i + 2) {
                    Some(b'A') => keys.push(Key::Up),
                    Some(b'B') => keys.push(Key::Down),
                    _ => {}
                }
                i += 3;
                continue;
            }
            0x1b => keys.push(Key::Esc),
            b'\r' | b'\n' => keys.push(Key::Enter),
            c if c == menu_key => keys.push(Key::MenuPressed),
            c if c.is_ascii_graphic() => keys.push(Key::Char((c as char).to_ascii_lowercase())),
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

pub async fn run(ws: &Workspace, backend: &Backend, cfg: &Config) -> Result<()> {
    let Some(menu_key) = parse_prefix_key(&cfg.defaults.prefix_key) else {
        bail!("prefix_key '{}' is not valid; use ctrl-space or ctrl-<letter>", cfg.defaults.prefix_key);
    };
    let menu_label = prefix_label(&cfg.defaults.prefix_key);
    let fps = cfg.defaults.fps_cap;

    let (events_tx, mut events) = mpsc::channel::<Event>(8);
    let (cols, rows) = shell_size(1)?;
    let mut app = App { tabs: Vec::new(), active: 0, next_id: 0, shell_count: 0, scrollback: cfg.defaults.scrollback_lines, quiet: Duration::from_secs(cfg.defaults.quiet_secs.max(1)), sessions: Default::default(), side: Side::parse(&cfg.defaults.preview_side), pct: cfg.defaults.preview_width_pct, div_drag: false, root: backend.resolve("").await.unwrap_or_else(|_| "/".into()), };
    // Reattach to sessions left running on the box; otherwise start one fresh shell.
    let restored = backend.restored_sessions();
    let (cols, rows) = if restored.len() > 1 { shell_size(restored.len())? } else { (cols, rows) };
    for name in &restored {
        open_tab_in(&mut app, backend, &events_tx, cols, rows, Some(name), None).await?;
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
    let mut ask_ctx = AskCtx { key: agents::workspace_key(ws, &app.root), dir: app.root.clone(), tab: None };
    let rkey = agents::recents_key(ws);
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
    let mut last_results: Option<Box<Results>> = None;
    let mut filter = PasteFilter::default();
    let mut flush_at: Option<Instant> = None;
    let mut decoder = KeyDecoder::default();
    // While a paste is in flight: (forward it to the shell, wrap it in markers).
    let mut pasting: Option<(bool, bool)> = None;
    let mut message: Option<String> = None;
    if ws.is_remote() && !backend.has_tmux() {
        message = Some("no tmux on this box (or use_tmux = false): plain shells, quitting ends them".into());
    } else if !restored.is_empty() {
        message = Some(format!("reattached {} running session(s)", restored.len()));
    }
    let (name, base_detail) = ws.title();
    // Header detail: the active tab's folder and its git branch. The branch of a folder is asked
    // once (and again when the Git view opens or reloads); no timers.
    let mut detail = base_detail.clone();
    let remote = ws.is_remote();
    let items = menu_items_for(remote, backend.has_tmux());
    ui::set_tmux_menu(backend.has_tmux());
    let opts = Opts { csv_rows: cfg.defaults.csv_rows };
    let read_cap = cfg.defaults.preview_read_kb as usize * 1024;
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
    match agents::decide(&cfg.defaults.agent_start, &detected, agent_state.workspace.get(&ask_ctx.key)) {
        Decision::Nothing => {}
        Decision::Start(ag) => {
            let cmd = agents::command_for(ag, cfg, &agent_state, &ask_ctx.key);
            message = Some(match start_agent(&mut app, backend, &events_tx, ag, &cmd, &root).await {
                Ok(()) => format!("started: {cmd}   ({menu_label}, then A, changes it)"),
                Err(e) => format!("could not start {}: {e}", ag.key()),
            });
        }
        Decision::Ask(list) => mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &ask_ctx.key)),
    }

    loop {
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
        // Earliest moment a still-printing background tab counts as quiet. No tab streaming
        // means no deadline, so an idle wi has no timers running.
        let quiet_at = app.tabs.iter().enumerate()
            .filter(|(i, t)| *i != app.active && t.activity && !t.quiet)
            .map(|(_, t)| t.last_output + app.quiet)
            .min();
        tokio::select! {
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
            _ = sleep_until(due.into()), if dirty => {
                if std::mem::take(&mut sel_reset) { sel = None; }
                let menu = match mode { Mode::Menu(i) => Some(i), _ => None };
                let scroll = match mode {
                    Mode::Scroll | Mode::OutFind(_) => { let s = app.tabs[app.active].parser.screen(); Some((s.scrollback(), s.scrollback_len())) }
                    _ => None,
                };
                let (pcols, prows) = shell_size(app.tabs.len())?;
                if let Some(p) = app.tabs[app.active].preview.as_mut() {
                    let h = prows as usize - usize::from(p.banner.is_some() && prows > 1);
                    p.layout(pcols, h);
                }
                let (pside, ppct) = (app.side, app.pct);
                if let Some(sp) = split_main(Rect::new(0, 0, pcols, prows), pside, ppct)
                    && let Some(p) = app.tabs[app.active].side.as_mut()
                {
                    let h = sp.side.height as usize - usize::from(p.banner.is_some() && sp.side.height > 1);
                    p.layout(sp.side.width, h);
                }
                let strip: Vec<(String, bool, crate::tab::TabState)> = app.tabs.iter().enumerate()
                    .map(|(i, t)| (t.title.clone(), i == app.active, t.state(i == app.active))).collect();
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
                    Mode::Finder(_) => Some("type to narrow · arrows pick · Enter opens · Esc closes".into()),
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
                let view = View {
                    selection,
                    preview: app.tabs[app.active].preview.as_ref(),
                    side: app.tabs[app.active].side.as_ref(),
                    split: (app.side, app.pct),
                    focus_side: app.tabs[app.active].side_focused(),
                    prompt: match &mode { Mode::Prompt(p) => Some(p), _ => None },
                    ask: match &mode { Mode::Ask(x) => Some(x), _ => None },
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
                };
                if std::mem::take(&mut repaint) {
                    // Not Terminal::clear(): it asks the terminal for the cursor position and wants
                    // the reply on stdin, which wi itself is reading. Clear the screen directly and
                    // start from an empty drawing surface so the next frame paints everything.
                    crossterm::execute!(stdout(), crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?;
                    term = Terminal::new(CrosstermBackend::new(stdout()))?;
                }
                term.draw(|f| ui::draw(f, &view))?;
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
                        if n == 0 { return finish(&app, backend).await; }
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
                            let strip_h = u16::from(app.tabs.len() > 1);
                            let main = Rect::new(0, 1 + strip_h, tc, tr.saturating_sub(2 + strip_h).max(1));
                            if matches!(mode, Mode::Shell | Mode::Scroll) && matches!(m.kind, Kind::Down(_))
                                && let Some(sp) = split_of(&app, &app.tabs[app.active], main)
                            {
                                // Clicking in a pane gives it the keyboard.
                                let t = &mut app.tabs[app.active];
                                if inside(sp.side, m.col, m.row) { t.focus = Pane::Side; dirty = true; }
                                else if inside(sp.shell, m.col, m.row) { t.focus = Pane::Shell; dirty = true; }
                            }
                            match mouse_intent(&m, &mode, &app, &items, main, sel.as_ref().filter(|s| s.dragging).map(|s| s.pane), &link_click) {
                                Intent::None => {}
                                Intent::Synth(bytes) => out.extend(bytes),
                                Intent::SwitchTab(i) => { sel = None; switch_to(&mut app, i); dirty = true; }
                                Intent::ClosePreview => {
                                    let t = &mut app.tabs[app.active];
                                    t.side = None;
                                    t.focus = Pane::Shell;
                                    sel = None;
                                    resize_all(&mut app)?;
                                    dirty = true;
                                }
                                Intent::DividerDown => { app.div_drag = true; sel = None; }
                                Intent::DividerDrag(col) => {
                                    app.pct = pct_for_col(main, app.side, col);
                                    resize_all(&mut app)?;
                                    dirty = true;
                                }
                                Intent::DividerUp => { app.div_drag = false; dirty = true; }
                                Intent::OpenLink(l, pane) => {
                                    sel = None;
                                    if matches!(mode, Mode::Scroll) { mode = Mode::Shell; }
                                    let o = resolve_link(&app, backend, ws, &l, pane, &open_cfg, confirm_needed).await;
                                    apply_outcome(o, &mut app, backend, ws, &mut mode, &mut pending, &mut message, &mut fetching, read_cap, &opts).await;
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
                                    mode = Mode::Scroll;
                                    scroll_by(&mut app, 3);
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
                                            pr.input = c;
                                            pr.sel = None;
                                            out.extend(b"\r");
                                        } else {
                                            let dir = crate::files::split_input(&pr.input).0.to_string();
                                            pr.input = format!("{dir}{c}");
                                            out.extend(if c.ends_with('/') { b"\t".to_vec() } else { b"\r".to_vec() });
                                        }
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
                        let fwd = matches!(mode, Mode::Shell) && !app.tabs[app.active].is_preview() && !app.tabs[app.active].side_focused();
                        let wrap = app.tabs[app.active].parser.screen().bracketed_paste();
                        if fwd && wrap { app.tabs[app.active].write(b"\x1b[200~".to_vec()); }
                        pasting = Some((fwd, wrap));
                        continue;
                    }
                    InEv::PasteData(b) => {
                        if let Mode::Prompt(p) = &mut mode {
                            p.input.push_str(&String::from_utf8_lossy(&b).replace(['\r', '\n'], ""));
                            p.error = None;
                            dirty = true;
                        } else if paste_search(&mut mode, &String::from_utf8_lossy(&b).replace(['\r', '\n'], "")) {
                            dirty = true;
                        } else if let Mode::Ask(x) = &mut mode {
                            if let Some(buf) = x.editing.as_mut() {
                                buf.push_str(&String::from_utf8_lossy(&b).replace(['\r', '\n'], ""));
                                dirty = true;
                            }
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
                            for k in scroll::parse(chunk, menu_key) {
                                if k == ScrollKey::Exit { mode = Mode::Shell; off = 0; break; }
                                off = scroll::apply(off, max, page, k);
                            }
                            t.parser.screen_mut().set_scrollback(off);
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Shell => {
                            let (head, after_menu) = match chunk.iter().position(|&x| x == menu_key) {
                                Some(pos) => (&chunk[..pos], Some(&chunk[pos + 1..])),
                                None => (chunk, None),
                            };
                            let side_focus = app.tabs[app.active].side_focused();
                            if app.tabs[app.active].is_preview() || side_focus {
                                let rows = shell_size(app.tabs.len())?.1 as usize;
                                let (mut back, mut reload, mut find, mut close) = (false, false, false, false);
                                if let Some(p) = focused_slot(&mut app).as_mut() {
                                    let page = rows - usize::from(p.banner.is_some() && rows > 1);
                                    for k in pvkeys::parse(head) {
                                        match k {
                                            PvKey::Back => back = true,
                                            PvKey::Reload => reload = true,
                                            PvKey::Find => find = true,
                                            PvKey::Close => close = true,
                                            k => { p.navigate(k, page); }
                                        }
                                    }
                                    if let Some(l) = p.find_label() { message = Some(l); }
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
                                        if remove_tab(&mut app, id)? { return finish(&app, backend).await; }
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
                                    message = match res {
                                        Ok(()) => Some("reloaded".to_string()),
                                        Err(e) => Some(format!("reload failed: {e}")),
                                    };
                                }
                            } else if !head.is_empty() {
                                app.tabs[app.active].write(head.to_vec());
                            }
                            match after_menu {
                                Some(rest) => {
                                    mode = Mode::Menu(0);
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
                            if open && let Some(f) = fd.selected().map(str::to_string) {
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
                                Some(ExAct::Leave) => mode = Mode::Shell,
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
                                        mode = Mode::Shell;
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
                                            mode = Mode::Shell;
                                        }
                                        Err(e) => ex.message = Some((format!("could not open shell: {e}"), true)),
                                    }
                                }
                                Some(ExAct::Agent(dir)) => {
                                    let mut list = agents::detect(backend, &dir).await;
                                    if list.is_empty() { list = agents::ALL.to_vec(); }
                                    ask_ctx = AskCtx { key: agents::workspace_key(ws, &dir), dir, tab: None };
                                    mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &ask_ctx.key));
                                }
                            }
                            dirty = true;
                            chunk = &[];
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
                                if let Some(p) = pending.take() {
                                    run_pending(p, backend, ws, &mut mode, &mut fetching, &mut message);
                                }
                            } else if no {
                                mode = Mode::Shell;
                                pending = None;
                                message = Some("cancelled".into());
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
                                apply_outcome(o, &mut app, backend, ws, &mut mode, &mut pending, &mut message, &mut fetching, read_cap, &opts).await;
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Ask(ref mut ask) => {
                            // None = still deciding, Some(None) = not now, Some(Some(i)) = start agent i, plus `never`.
                            let (mut start, mut later, mut never) = (None::<usize>, false, false);
                            let mut i = 0;
                            while i < chunk.len() {
                                let b = chunk[i];
                                if let Some(buf) = ask.editing.as_mut() {
                                    match b {
                                        b'\r' | b'\n' => {
                                            let cmd = buf.trim().to_string();
                                            if !cmd.is_empty() { ask.commands[ask.sel] = cmd; start = Some(ask.sel); }
                                            ask.editing = None;
                                            break;
                                        }
                                        0x1b if !matches!(chunk.get(i + 1), Some(b'[') | Some(b'O')) => { ask.editing = None; }
                                        0x1b => { i += 2; while i < chunk.len() && !(0x40..=0x7e).contains(&chunk[i]) { i += 1; } }
                                        0x7f | 0x08 => { buf.pop(); }
                                        0x15 => buf.clear(),
                                        c if c >= 0x20 => {
                                            // keep multi-byte characters whole
                                            let end = (i + 4).min(chunk.len());
                                            let take = chunk[i..end].iter().position(|&x| x < 0x20 || x == 0x7f).unwrap_or(end - i);
                                            buf.push_str(&String::from_utf8_lossy(&chunk[i..i + take]));
                                            i += take - 1;
                                        }
                                        _ => {}
                                    }
                                } else {
                                    match b {
                                        0x1b if matches!(chunk.get(i + 1), Some(b'[') | Some(b'O')) => {
                                            match chunk.get(i + 2) {
                                                Some(b'A') => ask.sel = (ask.sel + ask.agents.len() - 1) % ask.agents.len(),
                                                Some(b'B') => ask.sel = (ask.sel + 1) % ask.agents.len(),
                                                _ => {}
                                            }
                                            i += 2;
                                        }
                                        0x1b => { later = true; break; }
                                        b'\r' | b'\n' | b's' | b'S' => { start = Some(ask.sel); break; }
                                        b'e' | b'E' => { ask.editing = Some(ask.commands[ask.sel].clone()); }
                                        b'n' | b'N' => { later = true; break; }
                                        b'x' | b'X' => { never = true; break; }
                                        d @ b'1'..=b'9' if ((d - b'1') as usize) < ask.agents.len() => { ask.sel = (d - b'1') as usize; }
                                        _ => {}
                                    }
                                }
                                i += 1;
                            }
                            if let Some(idx) = start {
                                let (ag, cmd) = (ask.agents[idx], ask.commands[idx].clone());
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
                                pr.input.push_str(&String::from_utf8_lossy(&std::mem::take(&mut run)));
                                match b {
                                    0x1b => {
                                        if matches!(chunk.get(i + 1), Some(b'[') | Some(b'O')) {
                                            i += 2;
                                            while i < chunk.len() && !(0x40..=0x7e).contains(&chunk[i]) { i += 1; }
                                            match chunk.get(i) {
                                                Some(b'A') => arrows.push(false),
                                                Some(b'B') => arrows.push(true),
                                                _ => {}
                                            }
                                        } else {
                                            cancel = true;
                                        }
                                    }
                                    b'\r' | b'\n' => { open_now = true; break; }
                                    b'\t' => { complete_now = true; break; }
                                    0x7f | 0x08 => { pr.input.pop(); }
                                    0x15 => pr.input.clear(),
                                    x if x == menu_key => cancel = true,
                                    _ => {}
                                }
                                pr.error = None;
                                i += 1;
                            }
                            pr.input.push_str(&String::from_utf8_lossy(&run));
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
                                    pr.input = format!("{}/", c.trim_end_matches('/'));
                                }
                                pr.recent = false;
                                pr.sel = None;
                                let dir = anchor(&base, crate::files::split_input(&pr.input).0);
                                match backend.list_dir(&dir).await {
                                    Ok(l) => {
                                        let c = if pr.kind.is_folder() { complete_dirs(&pr.input, &l.entries) } else { complete(&pr.input, &l.entries) };
                                        pr.input = c.input;
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
                                mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &ask_ctx.key));
                            }
                            dirty = true;
                            chunk = &[];
                        }
                        Mode::Menu(ref mut sel) => {
                            let mut chosen: Option<Action> = None;
                            let mut jump: Option<usize> = None;
                            let mut close = false;
                            for k in parse_keys(chunk, menu_key) {
                                match k {
                                    Key::Up => *sel = (*sel + items.len() - 1) % items.len(),
                                    Key::Down => *sel = (*sel + 1) % items.len(),
                                    Key::Enter => chosen = Some(items[*sel].action),
                                    Key::Esc | Key::MenuPressed => close = true,
                                    Key::Char(c) => {
                                        if let Some(d) = c.to_digit(10).filter(|d| (1..=9).contains(d)) {
                                            jump = Some(d as usize - 1);
                                        } else if let Some(item) = items.iter().find(|m| m.key == c) {
                                            chosen = Some(item.action);
                                        }
                                    }
                                }
                                if chosen.is_some() || jump.is_some() || close { break; }
                            }
                            if let Some(i) = jump {
                                mode = Mode::Shell;
                                if i < app.tabs.len() { switch_to(&mut app, i); }
                            } else if let Some(a) = chosen {
                                mode = Mode::Shell;
                                match a {
                                    Action::Quit => return finish(&app, backend).await,
                                    Action::EndAndQuit => {
                                        backend.end_all_sessions().await;
                                        return finish(&app, backend).await;
                                    }
                                    Action::Info => {
                                        let keys = if decoder.enhanced { "keys: enhanced keyboard active" } else { "keys: standard (Shift+Enter looks like Enter to wi)" };
                                        message = Some(if remote {
                                            format!("{keys} · {name} · {detail} · connected · {} tab(s)", app.tabs.len())
                                        } else {
                                            format!("{keys} · local workspace · {detail} · {} tab(s)", app.tabs.len())
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
                                        mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &ask_ctx.key));
                                    }
                                    Action::Mouse => {
                                        mouse_on = !mouse_on;
                                        let mut o = stdout();
                                        let _ = std::io::Write::write_all(&mut o, if mouse_on { mouse::ENABLE } else { mouse::DISABLE });
                                        let _ = std::io::Write::flush(&mut o);
                                        message = Some(if mouse_on { "mouse on".to_string() } else { "mouse off: select and copy text natively".to_string() });
                                    }
                                    Action::Theme => {
                                        let k = crate::theme::kind().other();
                                        crate::theme::set_kind(k);
                                        agent_state.theme = Some(k.name().to_string());
                                        message = Some(match agent_state.save() {
                                            Ok(()) if crate::theme::Kind::parse(&cfg.defaults.theme).is_some() => format!("theme: {} (config theme = \"{}\" wins on restart)", k.name(), cfg.defaults.theme),
                                            Ok(()) => format!("theme: {}", k.name()),
                                            Err(e) => format!("theme: {} (could not save: {e})", k.name()),
                                        });
                                    }
                                    Action::SwitchPane => {
                                        let main = main_rect(&app)?;
                                        if split_of(&app, &app.tabs[app.active], main).is_some() {
                                            let t = &mut app.tabs[app.active];
                                            t.focus = if t.focus == Pane::Side { Pane::Shell } else { Pane::Side };
                                        } else {
                                            message = Some("no preview pane open".into());
                                        }
                                    }
                                    Action::ClosePreview => {
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
                                    Action::Explorer => {
                                        let (ec, er) = crossterm::terminal::size()?;
                                        let mut ex = ExplorerState::new(active_dir(&app), remote.then(|| name.clone()), explorer::Settings { csv_rows: opts.csv_rows, read_cap, open_cfg: open_cfg.clone(), pct: ex_pct });
                                        ex.prepare(ec, er);
                                        mode = Mode::Explorer(Box::new(ex));
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
                                        if remove_tab(&mut app, id)? { return finish(&app, backend).await; }
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
            _ = sigterm.recv() => return finish(&app, backend).await,
            _ = sighup.recv() => return finish(&app, backend).await,
            _ = sigint.recv() => return finish(&app, backend).await,
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
                        if i != app.active { app.tabs[i].note_background_output(); }
                        dirty = true;
                    }
                }
                Some(Event::Exited(_)) if backend.is_lost() => {
                    message = Some("connection lost - open the menu and quit".into());
                    dirty = true;
                }
                Some(Event::Exited(id)) => {
                    if app.tabs.get(app.active).is_some_and(|t| t.id == id) && matches!(mode, Mode::Scroll | Mode::OutFind(_)) { mode = Mode::Shell; }
                    if remove_tab(&mut app, id)? { return finish(&app, backend).await; }
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

fn switch_to(app: &mut App, i: usize) {
    app.active = i;
    app.tabs[i].mark_seen();
}

fn resize_all(app: &mut App) -> Result<()> {
    refresh_labels(app);
    let (c, r) = shell_size(app.tabs.len())?;
    let split = split_main(Rect::new(0, 0, c, r), app.side, app.pct);
    for t in &mut app.tabs {
        // The shell is narrower while a side preview fits beside it; without room the pane hides.
        let shell_c = match (&t.side, &split) {
            (Some(_), Some(sp)) => sp.shell.width,
            _ => c,
        };
        if t.side.is_some() && split.is_none() { t.focus = Pane::Shell; }
        t.resize(shell_c, r);
    }
    Ok(())
}

/// Remove a tab by id. Returns true when no tabs are left.
fn remove_tab(app: &mut App, id: usize) -> Result<bool> {
    let Some(i) = app.tabs.iter().position(|t| t.id == id) else { return Ok(app.tabs.is_empty()) };
    app.tabs.remove(i);
    if app.tabs.is_empty() {
        return Ok(true);
    }
    if app.active > i || app.active >= app.tabs.len() {
        app.active = app.active.saturating_sub(1).min(app.tabs.len() - 1);
    }
    app.tabs[app.active].mark_seen();
    resize_all(app)?;
    Ok(false)
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
    if app.tabs[app.active].is_preview() || app.side == Side::Tab {
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

/// The preview shown in a pane of the active tab (a preview tab's own preview counts as `Shell`).
fn pane_preview(app: &App, pane: Pane) -> Option<&Preview> {
    let t = &app.tabs[app.active];
    match pane {
        Pane::Side => t.side.as_ref(),
        Pane::Shell => t.preview.as_ref(),
    }
}

fn pane_preview_mut(app: &mut App, pane: Pane) -> Option<&mut Preview> {
    let t = &mut app.tabs[app.active];
    match pane {
        Pane::Side => t.side.as_mut(),
        Pane::Shell => t.preview.as_mut(),
    }
}

fn inside(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// The pane rectangles of a tab with a side preview, when the window has room for both.
fn split_of(app: &App, tab: &Tab, main: Rect) -> Option<Split> {
    tab.side.as_ref().and_then(|_| split_main(main, app.side, app.pct))
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
    PreviewScroll(isize, Pane),
    /// Mouse wheel in scrollback: positive looks further back, negative returns toward the live view.
    ScrollWheel(isize),
    EnterScroll,
    PickCandidate(usize),
    /// A click on a row of the results list or the file finder (row within the list area).
    ListPick(usize),
}

/// What a mouse event means right now. Pure so it can be tested without a terminal.
/// `selecting` is the pane of a selection being dragged, if any.
fn mouse_intent(m: &MouseEvent, mode: &Mode, app: &App, items: &[&MenuItem], main: Rect, selecting: Option<Pane>, link_click: &str) -> Intent {
    let hit = |r: Rect| inside(r, m.col, m.row);
    match mode {
        Mode::Menu(_) => {
            let r = ui::menu_rect(main, items.len());
            match m.kind {
                Kind::Down(_) if hit(r) => {
                    // Rows 1..=n inside the border are the entries.
                    let idx = m.row.saturating_sub(r.y + 1) as usize;
                    match items.get(idx) {
                        Some(it) if m.row > r.y => Intent::Synth(vec![it.key as u8]),
                        _ => Intent::None,
                    }
                }
                Kind::Down(_) => Intent::Synth(vec![0x1b]),
                Kind::WheelUp => Intent::Synth(b"\x1b[A".to_vec()),
                Kind::WheelDown => Intent::Synth(b"\x1b[B".to_vec()),
                _ => Intent::None,
            }
        }
        Mode::Ask(_) | Mode::Confirm(_) | Mode::Fetch | Mode::Hints(_) | Mode::FindPrompt(_) | Mode::Working(_) | Mode::Explorer(_) => Intent::None,
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
                let strip: Vec<_> = app.tabs.iter().enumerate().map(|(i, t)| (t.title.clone(), i == app.active, t.state(i == app.active))).collect();
                return match m.kind {
                    Kind::Down(mouse::Button::Left) => ui::tab_at(&strip, m.col).map_or(Intent::None, Intent::SwitchTab),
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
        if matches!(m.kind, Kind::Down(Button::Left)) && sp.on_close(m.col, m.row) {
            return Intent::ClosePreview;
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
    let banner = pane_preview(app, pane).is_some_and(|p| p.banner.is_some()) && area.height > 1;
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
            let banner = p.banner.is_some() && area.height > 1;
            let h = (area.height - u16::from(banner)) as usize;
            let rows: Vec<Vec<String>> = p.visible(h, area.width as usize).iter().map(|l| crate::preview::row_cells(l, p.gutter, area.width as usize)).collect();
            let wrapped = vec![false; rows.len()];
            (rows, wrapped)
        }
        None => select::screen_rows(app.tabs[app.active].parser.screen()),
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
    let banner = pane_preview(app, pane).is_some_and(|p| p.banner.is_some()) && area.height > 1;
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
    let tab = &app.tabs[app.active];
    if let Some(p) = pane_preview(app, pane) {
        return std::path::Path::new(&p.path).parent().filter(|_| p.path.starts_with('/')).map(|d| d.to_string_lossy().into_owned());
    }
    crate::cwd::tab_cwd(backend, tab.compat.cwd.as_deref(), tab.handle.as_ref().and_then(|h| h.pid()), app.sessions.get(&tab.id).map(String::as_str)).await
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
            let mut found = None;
            let mut is_dir = false;
            for cand in links::resolve_candidates(path, cwd.as_deref(), &root, Some(&home)) {
                match backend.read_prefix(&cand, 4096).await {
                    Ok((head, size)) => {
                        found = Some((cand, head, size));
                        break;
                    }
                    Err(e) => is_dir |= e.to_string().contains("is a directory"),
                }
            }
            let Some((abs, head, size)) = found else {
                return LinkOutcome::Message(if is_dir { format!("{}: is a folder", sanitize(path)) } else { format!("no such file: {}", sanitize(path)) });
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
