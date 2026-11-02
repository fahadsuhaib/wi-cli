# wicli (`wi`)

Lightweight terminal workspace for working with AI coding tools (Claude Code, Codex) locally or on a remote box, replacing VS Code for that workflow: great terminal, tabs, quick previews and diffs of what the AI produced. No debugger, LSP or built-in editor. It should be able to build itself. Read `PLAN.md` for the roadmap and `/Users/fahad/Downloads/Devbox terminal client implementation spec.md` for the original spec (names there say "devbox"; we use wicli/`wi`).

## Rules
- Keys must stay simple and memorable: letters, arrows, Enter, Esc, Tab. No symbol keys, no chords. One global menu key opens an arrow-key menu with underlined letters.
- Nothing resident on the remote box beyond sshd. Bounded reads only. No watchers. Redraw capped at fps_cap and only when dirty.
- Every user action is an `Action` in `src/ui/mod.rs` (extensions will plug into this later).
- Local and remote share the `Workspace`/`Shell` interface in `src/workspace.rs`.

## Build and test
- Rust is at `~/.cargo/bin` (add to PATH). `cargo build` produces `target/debug/wi`.
- Config: `~/.config/wicli/config.toml`; override with `WICLI_CONFIG`.
- Test box: `docker build -t devbox-test dev && docker run -d --name devbox-test -p 2222:22 devbox-test` (needs `colima start`). Test key: `dev/id_test`.
- pty tests: `dev/pty_drive.py` (needs a venv with `pyte`). It drives `wi` in local or remote mode and prints the screen.
- Self-hosting: keep a stable copy (`cargo install --path .`) for daily use and test the dev build from `target/debug/wi`, so a bug can't break the terminal you work in.
