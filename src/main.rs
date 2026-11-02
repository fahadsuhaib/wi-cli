mod agents;
mod app;
mod compat;
mod config;
mod exec;
mod input;
mod link;
mod mouse;
mod opener;
mod preview;
mod scroll;
mod files;
mod session;
mod tab;
mod term;
mod transfer;
mod ui;
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

async fn real_main() -> Result<()> {
    let cfg = config::Config::load()?;
    if std::env::args().nth(1).as_deref() == Some("fetch") {
        return fetch_cmd(&cfg).await;
    }
    let ws = match std::env::args().nth(1) {
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
    app::run(&ws, &backend, &cfg).await
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
