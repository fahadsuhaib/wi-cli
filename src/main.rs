mod agents;
mod app;
mod ask;
mod clip;
mod compat;
mod config;
mod cwd;
mod dictate;
mod dictlog;
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
mod gitdiff;
mod diffview;
mod history;
mod tab;
mod term;
mod theme;
mod tui;
mod ui;
mod update;
mod voice;
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

const HELP: &str = "wi: a terminal workspace for AI coding tools

usage:
  wi                  workspace in the current folder
  wi <folder>         workspace in that folder
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
    let args: Vec<String> = std::env::args().skip(1).filter(|a| a != "--fresh").collect();
    let fresh = std::env::args().skip(1).any(|a| a == "--fresh");
    let ws = match args.first().cloned() {
        // `wi` alone: the current directory.
        None => workspace::Workspace::Local(std::env::current_dir()?),
        Some(arg) => {
            if std::path::Path::new(&arg).is_dir() {
                workspace::Workspace::Local(std::fs::canonicalize(&arg)?)
            } else {
                bail!("'{arg}' is not a directory");
            }
        }
    };

    let backend = ws.connect();
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
