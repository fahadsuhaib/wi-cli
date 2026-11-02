# wicli (`wi`)

Lightweight terminal workspace for working with AI coding tools (Claude Code, Codex) locally or on a remote box, replacing VS Code for that workflow: great terminal, tabs, quick previews and diffs of what the AI produced. No debugger, LSP or built-in editor. It should be able to build itself. Plans live in `docs/plans/` (numbered): `01-roadmap.md` is the roadmap, `02-overview-features.md` the developer-convenience feature plan (overview tab, progress, clipboard bins, send-to-agent, checkpoints), `03-dashboard.md` (parked), `04-explorer.md` (file explorer); the original spec is `/Users/fahad/Downloads/Devbox terminal client implementation spec.md` for the original spec (names there say "devbox"; we use wicli/`wi`).

## Rules
- Keys must stay simple and memorable: letters, arrows, Enter, Esc, Tab. No symbol keys, no chords. One global menu key opens an arrow-key menu with underlined letters.
- Nothing resident on the remote box beyond sshd. Bounded reads only. No watchers. Redraw capped at fps_cap and only when dirty.
- Every user action is an `Action` in `src/ui/mod.rs` (extensions will plug into this later).
- Local and remote share the `Workspace`/`Shell` interface in `src/workspace.rs`.

## Features in place
Local and remote shells, tabs with activity markers (`*` printing, `!` went quiet), previews (text, markdown, CSV/TSV, JSON, binary stub; `Ctrl-Space` then `P`), mouse (clicks and wheel; menu toggle), scrollback, bracketed paste, and clickable paths and links (Cmd+click on a Mac or Ctrl+click on Linux, also Alt+click or right-click, on a path; `file:line` opens a preview at that line, URLs and complex files go to the OS after a confirmation; `Ctrl-Space` then `O` labels everything on screen), mouse selection with copy on release, and agent workspaces (detects `CLAUDE.md`/`.claude` and `AGENTS.md`/`.codex`, asks once, remembers the command per workspace in `~/.config/wicli/state.toml`; `[agents.claude] command = "..."` in the config sets your usual command).

## Build and test
- Rust is at `~/.cargo/bin` (add to PATH). `cargo build` produces `target/debug/wi`.
- Config: `~/.config/wicli/config.toml`; override with `WICLI_CONFIG`.
- Test box: `docker build -t devbox-test dev && docker run -d --name devbox-test -p 2222:22 devbox-test` (needs `colima start`). Test key: `dev/id_test`.
- Full suite: `dev/run_tests.sh` (build, clippy, unit tests, every `dev/pty_*.py`). pty tests need a venv with `pyte`; see `dev/README.md`. Bench: `dev/bench.py`.
- Self-hosting: keep a stable copy (`cargo install --path .`) for daily use and test the dev build from `target/debug/wi`, so a bug can't break the terminal you work in.
