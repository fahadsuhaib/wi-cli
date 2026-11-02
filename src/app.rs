//! Event loop: input, resize, shell output from every tab, coalesced redraw.

use crate::config::{Config, parse_prefix_key, prefix_label};
use crate::tab::Tab;
use crate::ui::{self, Action, View, menu_items};
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
    Menu(usize),
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
}

pub async fn run(ws: &Workspace, backend: &Backend, cfg: &Config) -> Result<()> {
    let Some(menu_key) = parse_prefix_key(&cfg.defaults.prefix_key) else {
        bail!("prefix_key '{}' is not valid; use ctrl-space or ctrl-<letter>", cfg.defaults.prefix_key);
    };
    let menu_label = prefix_label(&cfg.defaults.prefix_key);
    let fps = cfg.defaults.fps_cap;

    let (events_tx, mut events) = mpsc::channel::<Event>(64);
    let (cols, rows) = shell_size(1)?;
    let mut app = App { tabs: Vec::new(), active: 0, next_id: 0, shell_count: 0 };
    open_tab(&mut app, backend, &events_tx, cols, rows).await?;

    let _guard = TermGuard::enter()?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut stdin = tokio::io::stdin();
    let mut winch = signal(SignalKind::window_change())?;
    let mut buf = [0u8; 8192];
    let mut mode = Mode::Shell;
    let mut message: Option<String> = None;
    let (name, detail) = ws.title();
    let remote = ws.is_remote();
    let items = menu_items(remote);
    let frame = Duration::from_secs_f64(1.0 / fps.clamp(1, 120) as f64);
    let mut last_draw = Instant::now() - frame;
    let mut dirty = true;

    loop {
        let due = last_draw + frame;
        tokio::select! {
            _ = sleep_until(due.into()), if dirty => {
                let menu = match mode { Mode::Menu(i) => Some(i), Mode::Shell => None };
                let strip: Vec<(String, bool, bool)> = app.tabs.iter().enumerate()
                    .map(|(i, t)| (t.title.clone(), i == app.active, t.activity)).collect();
                let view = View {
                    screen: app.tabs[app.active].parser.screen(),
                    name: &name,
                    detail: &detail,
                    remote,
                    menu_label: &menu_label,
                    tabs: &strip,
                    message: message.as_deref(),
                    menu,
                };
                term.draw(|f| ui::draw(f, &view))?;
                last_draw = Instant::now();
                dirty = false;
            }
            n = stdin.read(&mut buf) => {
                let n = n?;
                if n == 0 { return finish(&app, backend).await; }
                let mut chunk = &buf[..n];
                // Keys typed right after the menu key in the same read still belong to the menu.
                while !chunk.is_empty() {
                    match mode {
                        Mode::Shell => {
                            if let Some(pos) = chunk.iter().position(|&x| x == menu_key) {
                                if pos > 0 { app.tabs[app.active].handle.write(chunk[..pos].to_vec()); }
                                mode = Mode::Menu(0);
                                message = None;
                                dirty = true;
                                chunk = &chunk[pos + 1..];
                            } else {
                                app.tabs[app.active].handle.write(chunk.to_vec());
                                chunk = &[];
                            }
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
                                        app.tabs[app.active].handle.close();
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
            _ = winch.recv() => {
                resize_all(&mut app)?;
                dirty = true;
            }
            ev = events.recv() => match ev {
                Some(Event::Output(id, data)) => {
                    if let Some(i) = app.tabs.iter().position(|t| t.id == id) {
                        app.tabs[i].output(&data);
                        if i != app.active { app.tabs[i].activity = true; }
                        dirty = true;
                    }
                }
                Some(Event::Exited(id)) => {
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
    app.tabs.push(Tab::new(id, format!("shell {}", app.shell_count), handle, cols, rows));
    Ok(())
}

fn switch_to(app: &mut App, i: usize) {
    app.active = i;
    app.tabs[i].activity = false;
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
    app.tabs[app.active].activity = false;
    resize_all(app)?;
    Ok(false)
}

async fn finish(app: &App, backend: &Backend) -> Result<()> {
    for t in &app.tabs {
        t.handle.close();
    }
    backend.close().await;
    Ok(())
}
