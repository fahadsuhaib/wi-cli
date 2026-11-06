mod agents;
mod app;
mod ask;
mod clip;
mod compat;
mod config;
mod cwd;
mod exec;
mod explorer;
mod explorer_data;
mod input;
mod keys;
mod lineedit;
mod links;
mod link;
mod mouse;
mod opener;
mod pick;
mod preview;
mod refresh;
mod rename;
mod restart;
mod scroll;
mod search;
mod select;
mod files;
mod history;
mod session;
mod tab;
mod term;
mod theme;
mod transfer;
mod tui;
mod ui;
mod update;
mod workspace;

use anyhow::{Result, bail};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    term::install_panic_hook();
    let code = match real_main().await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("wi: {e:#}");
            1
        }
    };
    // Exit directly: tokio's stdin reader thread would otherwise block shutdown
    // until a key is pressed.
    std::process::exit(code);
}

const HELP: &str = "wi: a terminal workspace for AI coding tools, local or over SSH

usage:
  wi                  local workspace in the current folder
  wi <folder>         local workspace in that folder
  wi <box>            remote workspace, a [[box]] name from the config
  wi <ssh-alias>      remote workspace, a Host from ~/.ssh/config
  wi fetch ...        download files from a box (see the docs)
  wi --fresh [...]    start without reopening the last session's tabs (and forget them)
  wi --version, -V    print the version
  wi --help, -h       print this help

config: ~/.config/wicli/config.toml (override with WICLI_CONFIG)
docs:   https://github.com/fahadsuhaib/wi-cli
";

async fn real_main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("wi {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--help" | "-h") => {
            print!("{HELP}");
            return Ok(());
        }
        Some("__ls") => return explorer_data::ls_cmd().await,
        _ => {}
    }
    let cfg = config::Config::load()?;
    if std::env::args().nth(1).as_deref() == Some("fetch") {
        return fetch_cmd(&cfg).await;
    }
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--fresh").collect();
    let fresh = std::env::args().skip(1).any(|a| a == "--fresh");
    let ws = match args.first().cloned() {
        // `wi` alone: local workspace in the current directory.
        None => workspace::Workspace::Local(std::env::current_dir()?),
        Some(arg) => {
            if let Some(b) = cfg.find_box(&arg) {
                workspace::Workspace::Remote(b.clone())
            } else if let Some(b) = config::Config::ssh_config_box(&arg)? {
                workspace::Workspace::Remote(b)
            } else if std::path::Path::new(&arg).is_dir() {
                workspace::Workspace::Local(std::fs::canonicalize(&arg)?)
            } else {
                bail!("'{arg}' is not a box in {}, a Host in {}, or a directory", config::config_path().display(), config::ssh_config_path().display());
            }
        }
    };

    let backend = ws.connect().await?;
    app::run(&ws, &backend, &cfg, fresh).await
}

/// Pick the theme (config, then saved menu choice, then the terminal's background) once at startup, after the config and the connection are known good (so error messages never
/// follow a terminal query).
pub fn setup_theme(cfg: &config::Config) {
    let saved = agents::State::load().theme;
    let fgbg = std::env::var("COLORFGBG").ok();
    // Unless the config fixes the theme, ask the terminal even when a choice is saved: the answer
    // is what switching the menu back to `auto` uses later.
    let asked = if theme::index_of(&cfg.defaults.theme).is_some() { None } else { term::query_background().map(theme::kind_from_rgb) };
    theme::set_detected(asked);
    let chosen = theme::resolve(&cfg.defaults.theme, saved.as_deref(), || asked, fgbg.as_deref());
    let truecolor = theme::colorterm_is_truecolor(std::env::var("COLORTERM").ok().as_deref());
    theme::init(chosen, truecolor, cfg.defaults.ansi_remap, cfg.defaults.paint_background);
}

/// Dev command: `wi fetch <box> <remote-path> [--yes]` downloads through the opener cache.
async fn fetch_cmd(cfg: &config::Config) -> Result<()> {
    use opener::{FetchOpts, FetchResult};
    let args: Vec<String> = std::env::args().skip(2).collect();
    let yes = args.iter().any(|a| a == "--yes");
    let pos: Vec<&String> = args.iter().filter(|a| *a != "--yes").collect();
    let [box_name, path] = pos[..] else { bail!("usage: wi fetch <box> <remote-path> [--yes]") };
    let b = match cfg.find_box(box_name) {
        Some(b) => b.clone(),
        None => config::Config::ssh_config_box(box_name)?.ok_or_else(|| anyhow::anyhow!("unknown box '{box_name}'"))?,
    };
    let backend = workspace::Workspace::Remote(b).connect().await?;
    let mut opts = FetchOpts::new(box_name);
    opts.confirmed = yes;
    let mut last = std::time::Instant::now();
    let mut progress = |done: u64, total: u64| {
        if last.elapsed() >= std::time::Duration::from_secs(1) {
            last = std::time::Instant::now();
            eprintln!("wi: fetched {done} of {total} bytes");
        }
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    match backend.fetch_to_cache(path, &opts, &mut progress, &cancel).await {
        Ok(FetchResult::Ready(p)) => println!("{}", p.display()),
        Ok(FetchResult::NeedsConfirm { size }) => {
            println!("{size}");
            eprintln!("wi: {path} is {size} bytes; rerun with --yes to download");
            std::process::exit(2);
        }
        Err(e) => bail!("{e}"),
    }
    Ok(())
}
