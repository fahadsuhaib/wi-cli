mod app;
mod compat;
mod config;
mod input;
mod link;
mod preview;
mod scroll;
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
    let ws = match std::env::args().nth(1) {
        // `wi` alone: local workspace in the current directory.
        None => workspace::Workspace::Local(std::env::current_dir()?),
        Some(arg) => {
            if let Some(b) = cfg.find_box(&arg) {
                workspace::Workspace::Remote(b.clone())
            } else if std::path::Path::new(&arg).is_dir() {
                workspace::Workspace::Local(std::fs::canonicalize(&arg)?)
            } else {
                bail!("'{arg}' is neither a box in {} nor a directory", config::config_path().display());
            }
        }
    };

    let backend = ws.connect().await?;
    app::run(&ws, &backend, &cfg).await
}
