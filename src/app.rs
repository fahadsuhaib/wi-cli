//! Event loop: input, resize, shell output from every tab, coalesced redraw.

use crate::input::{InEv, PasteFilter};
use crate::scroll::{self, ScrollKey};
use crate::config::{Config, parse_prefix_key, prefix_label};
use crate::tab::Tab;
use crate::files::complete;
use crate::preview::{Opts, Preview, keys::{self as pvkeys, PvKey}};
use crate::mouse::{self, Kind, MouseEvent, Piece};
use crate::agents::{self, Agent, Decision};
use crate::ui::{self, Action, Ask, MenuItem, Prompt, View, menu_items};
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
    Menu(usize),
    /// Browsing scrollback of the active tab.
    Scroll,
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
}

pub async fn run(ws: &Workspace, backend: &Backend, cfg: &Config) -> Result<()> {
    let Some(menu_key) = parse_prefix_key(&cfg.defaults.prefix_key) else {
        bail!("prefix_key '{}' is not valid; use ctrl-space or ctrl-<letter>", cfg.defaults.prefix_key);
    };
    let menu_label = prefix_label(&cfg.defaults.prefix_key);
    let fps = cfg.defaults.fps_cap;

    let (events_tx, mut events) = mpsc::channel::<Event>(8);
    let (cols, rows) = shell_size(1)?;
    let mut app = App { tabs: Vec::new(), active: 0, next_id: 0, shell_count: 0, scrollback: cfg.defaults.scrollback_lines, quiet: Duration::from_secs(cfg.defaults.quiet_secs.max(1)) };
    open_tab(&mut app, backend, &events_tx, cols, rows).await?;

    let _guard = TermGuard::enter(cfg.defaults.mouse)?;
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
    let wkey = agents::workspace_key(ws);
    let mut splitter = mouse::Splitter::default();
    let mut filter = PasteFilter::default();
    let mut flush_at: Option<Instant> = None;
    // While a paste is in flight: (forward it to the shell, wrap it in markers).
    let mut pasting: Option<(bool, bool)> = None;
    let mut message: Option<String> = None;
    let (name, detail) = ws.title();
    let remote = ws.is_remote();
    let items = menu_items(remote);
    let opts = Opts { csv_rows: cfg.defaults.csv_rows };
    let read_cap = cfg.defaults.preview_read_kb as usize * 1024;
    let frame = Duration::from_secs_f64(1.0 / fps.clamp(1, 120) as f64);
    let mut last_draw = Instant::now() - frame;
    let mut dirty = true;

    // Recognise Claude Code / Codex workspaces: start the remembered agent, or ask the first time.
    let detected = agents::detect(backend).await;
    match agents::decide(&cfg.defaults.agent_start, &detected, agent_state.workspace.get(&wkey)) {
        Decision::Nothing => {}
        Decision::Start(ag) => {
            let cmd = agents::command_for(ag, cfg, &agent_state, &wkey);
            message = Some(match start_agent(&mut app, backend, &events_tx, ag, &cmd).await {
                Ok(()) => format!("started: {cmd}   ({menu_label}, then A, changes it)"),
                Err(e) => format!("could not start {}: {e}", ag.key()),
            });
        }
        Decision::Ask(list) => mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &wkey)),
    }

    loop {
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
                let menu = match mode { Mode::Menu(i) => Some(i), _ => None };
                let scroll = match mode {
                    Mode::Scroll => { let s = app.tabs[app.active].parser.screen(); Some((s.scrollback(), s.scrollback_len())) }
                    _ => None,
                };
                let (pcols, prows) = shell_size(app.tabs.len())?;
                if let Some(p) = app.tabs[app.active].preview.as_mut() {
                    let h = prows as usize - usize::from(p.banner.is_some() && prows > 1);
                    p.layout(pcols, h);
                }
                let strip: Vec<(String, bool, crate::tab::TabState)> = app.tabs.iter().enumerate()
                    .map(|(i, t)| (t.title.clone(), i == app.active, t.state(i == app.active))).collect();
                let view = View {
                    preview: app.tabs[app.active].preview.as_ref(),
                    prompt: match &mode { Mode::Prompt(p) => Some(p), _ => None },
                    ask: match &mode { Mode::Ask(x) => Some(x), _ => None },
                    screen: app.tabs[app.active].parser.screen(),
                    name: &name,
                    detail: &detail,
                    remote,
                    menu_label: &menu_label,
                    tabs: &strip,
                    message: message.as_deref(),
                    menu,
                    scroll,
                };
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
                let in_events = match read {
                    None => filter.flush(),
                    Some(r) => {
                        let n = r?;
                        if n == 0 { return finish(&app, backend).await; }
                        filter.feed(&buf[..n])
                    }
                };
                flush_at = filter.has_pending().then(|| Instant::now() + Duration::from_millis(50));
                for ev in in_events {
                  let bytes = match ev {
                    InEv::Keys(b) => {
                        // Mouse reports are pulled out of the key stream; clicks that mean a key
                        // press (menu entry, completion) are turned into that key.
                        let mut out: Vec<u8> = Vec::new();
                        for piece in splitter.split(&b) {
                            let m = match piece {
                                Piece::Bytes(x) => { out.extend(x); continue; }
                                Piece::Mouse(m) => m,
                            };
                            let (tc, tr) = crossterm::terminal::size()?;
                            let strip_h = u16::from(app.tabs.len() > 1);
                            let main = Rect::new(0, 1 + strip_h, tc, tr.saturating_sub(2 + strip_h).max(1));
                            match mouse_intent(&m, &mode, &app, &items, main) {
                                Intent::None => {}
                                Intent::Synth(bytes) => out.extend(bytes),
                                Intent::SwitchTab(i) => { switch_to(&mut app, i); dirty = true; }
                                Intent::PreviewScroll(n) => {
                                    let page = main.height as usize;
                                    if let Some(p) = app.tabs[app.active].preview.as_mut() {
                                        let key = if n < 0 { PvKey::Up } else { PvKey::Down };
                                        for _ in 0..n.unsigned_abs() { p.navigate(key, page); }
                                    }
                                    dirty = true;
                                }
                                Intent::EnterScroll => {
                                    mode = Mode::Scroll;
                                    out.extend(b"\x1b[A".repeat(3));
                                }
                                Intent::ExitScroll => { mode = Mode::Shell; dirty = true; }
                                Intent::PickCandidate(i) => {
                                    if let Mode::Prompt(pr) = &mut mode
                                        && let Some(c) = pr.shown.get(i).cloned()
                                    {
                                        let dir = crate::files::split_input(&pr.input).0.to_string();
                                        pr.input = format!("{dir}{c}");
                                        pr.error = None;
                                        out.extend(if c.ends_with('/') { b"\t".to_vec() } else { b"\r".to_vec() });
                                    }
                                }
                            }
                        }
                        out
                    }
                    InEv::PasteStart => {
                        let fwd = matches!(mode, Mode::Shell) && !app.tabs[app.active].is_preview();
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
                            if app.tabs[app.active].is_preview() {
                                let rows = shell_size(app.tabs.len())?.1 as usize;
                                let (mut back, mut reload) = (false, false);
                                if let Some(p) = app.tabs[app.active].preview.as_mut() {
                                    let page = rows - usize::from(p.banner.is_some() && rows > 1);
                                    for k in pvkeys::parse(head) {
                                        match k {
                                            PvKey::Back => back = true,
                                            PvKey::Reload => reload = true,
                                            k => { p.navigate(k, page); }
                                        }
                                    }
                                }
                                dirty = true;
                                if back { go_back(&mut app); }
                                if reload {
                                    message = match reload_preview(&mut app, backend, read_cap, &opts).await {
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
                                agent_state.remember(&wkey, ag.key(), &cmd);
                                if let Err(e) = agent_state.save() { message = Some(format!("could not save choice: {e}")); }
                                if let Err(e) = start_agent(&mut app, backend, &events_tx, ag, &cmd).await {
                                    message = Some(format!("could not start {}: {e}", ag.key()));
                                }
                            } else if never {
                                mode = Mode::Shell;
                                agent_state.remember(&wkey, "none", "");
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
                            if complete_now {
                                let dir = crate::files::split_input(&pr.input).0.to_string();
                                match backend.list_dir(&dir).await {
                                    Ok(l) => {
                                        let c = complete(&pr.input, &l.entries);
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
                            if open_now && !pr.input.trim().is_empty() {
                                let path = pr.input.trim().to_string();
                                match open_preview(&mut app, backend, &path, read_cap, &opts).await {
                                    Ok(()) => cancel = true,
                                    Err(e) => pr.error = Some(format!("{path}: {e}")),
                                }
                            }
                            if cancel { mode = Mode::Shell; }
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
                                    Action::Info => message = Some(if remote {
                                        format!("{name} · {detail} · connected · {} tab(s)", app.tabs.len())
                                    } else {
                                        format!("local workspace · {detail} · {} tab(s)", app.tabs.len())
                                    }),
                                    Action::Agent => {
                                        let mut list = agents::detect(backend).await;
                                        if list.is_empty() { list = agents::ALL.to_vec(); }
                                        mode = Mode::Ask(Ask::new(list, cfg, &agent_state, &wkey));
                                    }
                                    Action::Mouse => {
                                        mouse_on = !mouse_on;
                                        let mut o = stdout();
                                        let _ = std::io::Write::write_all(&mut o, if mouse_on { mouse::ENABLE } else { mouse::DISABLE });
                                        let _ = std::io::Write::flush(&mut o);
                                        message = Some(if mouse_on { "mouse on".to_string() } else { "mouse off: select and copy text natively".to_string() });
                                    }
                                    Action::Scroll if app.tabs[app.active].is_preview() => message = Some("scroll a preview with the arrow keys and PgUp/PgDn".into()),
                                    Action::Scroll => mode = Mode::Scroll,
                                    Action::Preview => mode = Mode::Prompt(Prompt::default()),
                                    Action::NewTab => {
                                        let (c, r) = shell_size(app.tabs.len() + 1)?;
                                        let had_strip = app.tabs.len() > 1;
                                        match open_tab(&mut app, backend, &events_tx, c, r).await {
                                            Ok(()) => {
                                                app.active = app.tabs.len() - 1;
                                                if !had_strip { resize_all(&mut app)?; }
                                            }
                                            Err(e) => message = Some(format!("could not open shell: {e}")),
                                        }
                                    }
                                    Action::NextTab => { let n = (app.active + 1) % app.tabs.len(); switch_to(&mut app, n); }
                                    Action::PrevTab => { let n = (app.active + app.tabs.len() - 1) % app.tabs.len(); switch_to(&mut app, n); }
                                    Action::CloseTab => {
                                        let id = app.tabs[app.active].id;
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
                    if app.tabs.get(app.active).is_some_and(|t| t.id == id) && matches!(mode, Mode::Scroll) { mode = Mode::Shell; }
                    if remove_tab(&mut app, id)? { return finish(&app, backend).await; }
                    dirty = true;
                }
                None => return finish(&app, backend).await,
            }
        }
    }
}

async fn open_tab(app: &mut App, backend: &Backend, tx: &mpsc::Sender<Event>, cols: u16, rows: u16) -> Result<()> {
    let shell = backend.open_shell(cols, rows).await?;
    let id = app.next_id;
    app.next_id += 1;
    app.shell_count += 1;
    let handle = spawn_shell(id, shell, tx.clone());
    app.tabs.push(Tab::new(id, format!("shell {}", app.shell_count), handle, cols, rows, app.scrollback));
    Ok(())
}

fn switch_to(app: &mut App, i: usize) {
    app.active = i;
    app.tabs[i].mark_seen();
}

fn resize_all(app: &mut App) -> Result<()> {
    let (c, r) = shell_size(app.tabs.len())?;
    for t in &mut app.tabs {
        t.resize(c, r);
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
async fn open_preview(app: &mut App, backend: &Backend, input: &str, cap: usize, opts: &Opts) -> Result<()> {
    let abs = backend.resolve(input).await?;
    let (bytes, total) = backend.read_prefix(&abs, cap).await?;
    let mut p = Preview::build(&abs, &bytes, total, opts);
    p.from_tab = app.active;
    let id = app.next_id;
    app.next_id += 1;
    app.tabs.push(Tab::new_preview(id, p));
    app.active = app.tabs.len() - 1;
    resize_all(app)?; // the tab strip may have just appeared
    Ok(())
}

/// Re-read the file shown in the active preview tab, keeping scroll position and wrap setting.
async fn reload_preview(app: &mut App, backend: &Backend, cap: usize, opts: &Opts) -> Result<()> {
    let Some(old) = app.tabs[app.active].preview.as_ref() else { return Ok(()) };
    let (path, top, left, wrap, from) = (old.path.clone(), old.top, old.left, old.wrap, old.from_tab);
    let (bytes, total) = backend.read_prefix(&path, cap).await?;
    let mut p = Preview::build(&path, &bytes, total, opts);
    p.top = top;
    p.left = left;
    if p.wrap != wrap {
        p.toggle_wrap();
        p.left = left;
    }
    p.from_tab = from;
    app.tabs[app.active].preview = Some(p);
    Ok(())
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

enum Intent {
    None,
    /// Handle as if these bytes had been typed (or, in a shell tab, send them to the app).
    Synth(Vec<u8>),
    SwitchTab(usize),
    PreviewScroll(isize),
    EnterScroll,
    ExitScroll,
    PickCandidate(usize),
}

/// What a mouse event means right now. Pure so it can be tested without a terminal.
fn mouse_intent(m: &MouseEvent, mode: &Mode, app: &App, items: &[&MenuItem], main: Rect) -> Intent {
    let inside = |r: Rect| m.col >= r.x && m.col < r.x + r.width && m.row >= r.y && m.row < r.y + r.height;
    let up3 = || Intent::Synth(b"\x1b[A".repeat(3));
    let down3 = || Intent::Synth(b"\x1b[B".repeat(3));
    match mode {
        Mode::Menu(_) => {
            let r = ui::menu_rect(main, items.len());
            match m.kind {
                Kind::Down(_) if inside(r) => {
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
        Mode::Ask(_) => Intent::None,
        Mode::Prompt(p) => {
            let r = ui::prompt_rect(main, p);
            match m.kind {
                Kind::Down(_) if inside(r) && ((m.row - r.y) as usize) < p.shown.len() => Intent::PickCandidate((m.row - r.y) as usize),
                _ => Intent::None,
            }
        }
        Mode::Scroll => match m.kind {
            Kind::WheelUp => up3(),
            Kind::WheelDown if app.tabs[app.active].parser.screen().scrollback() == 0 => Intent::ExitScroll,
            Kind::WheelDown => down3(),
            _ => Intent::None,
        },
        Mode::Shell => {
            if app.tabs.len() > 1 && m.row == 1 {
                let strip: Vec<_> = app.tabs.iter().enumerate().map(|(i, t)| (t.title.clone(), i == app.active, t.state(i == app.active))).collect();
                return match m.kind {
                    Kind::Down(mouse::Button::Left) => ui::tab_at(&strip, m.col).map_or(Intent::None, Intent::SwitchTab),
                    _ => Intent::None,
                };
            }
            if !inside(main) {
                return Intent::None;
            }
            let tab = &app.tabs[app.active];
            if tab.is_preview() {
                return match m.kind {
                    Kind::WheelUp => Intent::PreviewScroll(-3),
                    Kind::WheelDown => Intent::PreviewScroll(3),
                    _ => Intent::None,
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
                Some(true) => Intent::Synth(mouse::encode(m, m.col, m.row - main.y, scr.mouse_protocol_encoding() == MouseProtocolEncoding::Sgr)),
                Some(false) => Intent::None,
                None => match m.kind {
                    // Full-screen apps without mouse support get arrow keys, like a terminal's alternate-scroll.
                    Kind::WheelUp if scr.alternate_screen() => up3(),
                    Kind::WheelDown if scr.alternate_screen() => down3(),
                    Kind::WheelUp => Intent::EnterScroll,
                    _ => Intent::None,
                },
            }
        }
    }
}

/// Open a shell tab named after the agent and run the user's command in it. The tab stays a
/// normal shell, so when the agent exits you are left at a prompt.
async fn start_agent(app: &mut App, backend: &Backend, tx: &mpsc::Sender<Event>, agent: Agent, command: &str) -> Result<()> {
    let (c, r) = shell_size(app.tabs.len() + 1)?;
    open_tab(app, backend, tx, c, r).await?;
    let last = app.tabs.len() - 1;
    let tab = &mut app.tabs[last];
    tab.fallback = agent.key().to_string();
    tab.title = agent.key().to_string();
    tab.write(format!("{command}\n").into_bytes());
    app.active = last;
    resize_all(app)
}
