# wi

`wi` is a lightweight terminal workspace for working with AI coding tools (Claude Code, Codex), on your own machine
or on a remote box over SSH. It gives you a good terminal with tabs, plus quick previews and diffs of what the AI
produced. It has no debugger, no LSP and no built-in editor. Nothing is installed on the remote box beyond sshd.

## Install

One line (Linux x86_64 / aarch64, macOS; installs to `~/.local/bin`, never uses sudo):

    curl -fsSL https://raw.githubusercontent.com/fahadsuhaib/wi-cli/main/install.sh | sh

Read the script first if you like; it only downloads the archive and `SHA256SUMS`, checks the checksum and copies `wi`.
`WICLI_INSTALL_DIR` changes the target folder and `WICLI_VERSION=v0.1.0` pins a release.

Manual: download `wi-linux-x86_64.tar.gz`, `wi-linux-aarch64.tar.gz` or `wi-macos-universal.tar.gz` from the
[releases page](https://github.com/fahadsuhaib/wi-cli/releases), check it against `SHA256SUMS`, unpack it and put `wi` on your PATH.
The Linux builds are static and run on any distribution; the macOS build runs on Apple Silicon and Intel.

From source (Rust stable):

    cargo install --path .        # or: cargo build --release  ->  target/release/wi

wi supports macOS and Linux.

## Use

    wi                 # local workspace in the current folder
    wi <folder>        # local workspace in a folder
    wi <box>           # remote box from the config, or a Host from ~/.ssh/config
    wi --help

## Features

- Local and remote shells, tabs with activity markers (`*` printing, `!` went quiet)
- Previews: text, markdown, CSV/TSV, JSON, binary stub; git changes and diffs
- Recent files: every file you preview is remembered per workspace (also across restarts). Press H in a preview, or menu then Space and type "recent", to pick one again; the full path of the shown file is always on the first row of the preview
- Mouse (clicks, wheel, selection with copy on release), scrollback, bracketed paste
- Clickable paths and links: Cmd+click on a Mac, Ctrl+click on Linux (also Alt+click or right-click);
  `file:line` opens a preview at that line, URLs and complex files go to the OS after a confirmation
- Agent workspaces: detects `CLAUDE.md`/`.claude` and `AGENTS.md`/`.codex`, asks once and remembers the command per workspace
- File explorer (Ctrl-Space then X): full-screen tree with sizes, dates, git marks and a detail pane; Enter previews a file, O opens it in `$EDITOR` in a new tab
- Pinned panes: pin any tab beside the active one (menu: search "pin"); ⇄ swaps left/right, ✕ unpins; the layout is remembered per workspace (needs 101+ columns)
- Search in files, find file, find in output; dark and light themes (Y cycles auto, dark, light; auto follows your terminal)
- Update wi from the menu (search "update"): asks before contacting github.com, checks the SHA256, replaces the binary, then offers to restart wi with the same tabs ("Restart wi" does the restart alone)

## Keys

`Ctrl-Space` opens the menu; press the underlined letter or use the arrows and Enter. Esc closes it.

| Key | Action | Key | Action |
|---|---|---|---|
| P | Preview a file | N | New shell tab |
| G | Git changes | F | New tab in folder |
| O | Open path on screen | H | Change folder of this tab |
| D | Download (remote) | T / B | Next / previous tab |
| L | Search in files | C | Close tab |
| K | Find file | S | Scroll |
| U | Find in output | W | Switch pane |
| A | Start / change agent | V | Close preview |
| X | Explorer | | |
| M | Mouse on/off | Y | Theme: cycles auto, dark, light |
| R | Reconnect (remote) | I / Q | Info / quit |

Inside a preview: arrows and PgUp/PgDn scroll, F find, W wrap, M markdown source, R reload, H recent files, Esc back, Q close.

Multi-line input (Shift+Enter) in agent tabs: see [docs/KEYBOARD.md](docs/KEYBOARD.md).

## Configuration

`~/.config/wicli/config.toml` (override with `WICLI_CONFIG`). A small example:

    [defaults]
    theme = "auto"          # auto, dark or light
    preview_side = "right"  # right, left or tab
    preview_autorefresh = true          # reload a visible file preview when the file changes
    preview_refresh_debounce_ms = 5000  # wait this long after the last change before reloading

    [agents.claude]
    command = "claude --continue"

    [[box]]
    name = "dev"
    host = "dev.example.com"
    user = "me"
    identity = "~/.ssh/id_ed25519"

A file preview that is on screen (side pane, preview tab or pinned pane) refreshes by itself when the file
changes on disk: wi checks the modification time and size of the visible file once a second (every two on a
box), waits until it stops changing, then reloads it, keeping your scroll position, markdown view, wrap and
find. A deleted file keeps its last content and shows "file removed". No file watchers, nothing runs on the box,
and nothing is checked while no file preview is visible. `preview_autorefresh = false` turns it off.

Every option is documented in `src/config.rs`.

## Develop and test

`cargo build`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. The full suite, including terminal-driving
scripts and a Docker test box for remote mode, is `dev/run_tests.sh`; see [dev/README.md](dev/README.md).

## License

License: not yet chosen
