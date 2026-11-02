# wicli roadmap

Status as of 2026-11-02: local + remote shell tab, header/status bar, Ctrl-G menu, vt100 emulator, 30 fps coalesced redraw. Verified with vim, htop, resize, quit. Not yet: tabs, previews, downloads, scrollback, detach.

## Vision (updated 2026-11-02)
wi is a lightweight terminal workspace for working with AI coding tools (Claude Code, Codex, and similar) on a local folder or a remote box. It replaces VS Code for that workflow only: the AI tool does the editing, wi gives you a great terminal, several tabs, and a quick way to look at what the AI produced. It should be able to build itself.

Out of scope on purpose: debugger, language servers, built-in code editor, notebooks. If you need to edit by hand, wi opens `$EDITOR` in a tab.

What the AI workflow needs, in priority order:
1. A faithful terminal: AI TUIs render correctly, paste is safe, scrollback works, nothing steals their keys.
2. Several tabs: one agent per tab, plus tabs for tests/servers. Tab strip shows activity (new output) and when an agent tab goes quiet, so you notice when it finishes or is waiting for you.
3. Reviewing results: preview files the AI wrote (markdown, CSV, JSON, logs, code), and a git diff view of what changed.
4. Remote that costs nothing on the box, with detach/reattach so long agent runs survive a dropped connection.
5. Small conveniences: file explorer, search, copy a file path or diff into the clipboard so you can hand it to the agent.

## Phase A: Dogfood-ready (do first; goal is to run `claude` inside `wi`)
Done when: Claude Code runs in one tab, `cargo test` in another, you can scroll back, paste multi-line text safely, and the menu key never collides.
1. Read `prefix_key` from config (currently hardcoded). Decide the default: Ctrl-G collides with Claude Code's editor key; recommend Ctrl-Space.
2. Tab infrastructure: tab strip, multiple shell tabs (menu: New shell, Next/Previous tab, Close tab). Needed again by previews.
3. Bracketed paste: enable on the host terminal, forward markers when the app asked for them.
4. Scrollback: vt100 scrollback (bounded, e.g. 10k lines), menu entry Scroll with arrows/PgUp/PgDn/Home/End, Esc exits.
5. Terminal compatibility pass against Claude Code: DA/cursor-position query replies, Shift-Enter and modified keys, focus events, OSC title, true colour, wide chars, mouse passthrough. Record real sessions and replay through pyte.
6. Robustness: restore the terminal on panic/SIGTERM/SIGHUP, forward Ctrl-C, shell exit closes only its tab.
7. Repo hygiene: git init, CI-less `cargo test` with unit tests for key parsing, config, left_trim; pty smoke tests in `dev/`.

## Phase A2: Agent-aware workspaces and mouse (added 2026-11-02, from user requests)
1. Agent workspaces: wi recognises Claude Code workspaces (`CLAUDE.md`, `.claude/`) and Codex workspaces (`AGENTS.md`, `.codex/`), local or remote, with a bounded check at startup. Config `[agents.<name>]` holds the user's usual command and flags (for example `claude --continue`). Behaviour per workspace: `ask` (default, first time), `always`, `never`; the answer and the chosen command are remembered in a small state file, changeable from the menu. Starting an agent opens it in its own shell tab.
2. Mouse: click a tab, menu entry or completion; wheel scrolls previews and shell scrollback; events are passed to apps that ask for the mouse (vim, htop). Mouse capture blocks native text selection, so add a menu toggle (Mouse on/off) and document Shift-drag; config `mouse = true|false`.

## Phase A3: Unified look on macOS and Linux (added 2026-11-02)
Goal: the same experience in any terminal app. Fonts belong to the terminal, so wi unifies what it controls.
1. Theme system (user asked for dark and light first): fixed RGB palette with 256-colour fallback; `theme = dark | light | auto` (auto asks the terminal for its background via OSC 11, falling back to COLORFGBG, then dark) plus a menu entry to switch live; a `terminal` mode (use the terminal's own palette) later. All colours in ui, previews, diffs go through the theme.
2. Remap ANSI 0-15 inside shell tabs to the theme palette (config switch, default on) so agent output looks the same everywhere.
3. Glyph set with ASCII fallback for non-UTF-8 / console terminals.
4. `wi --print-terminal-config <app>` for Terminal.app, iTerm2, Ghostty, Kitty, WezTerm, Alacritty, GNOME Terminal (font, size, Option-as-Meta); short docs on Ctrl-Space capture.
5. Linux verified in a container: build, unit tests and pty suite (dev/linux/).
Out of scope for now: a standalone GUI window with a bundled font.

## Phase A4: Search (added 2026-11-02, user request)
1. Search text in files: `rg` (fallback `grep`) via local process or SSH exec, on demand only; capped (~2,000 matches, 10 s) with a visible notice; respects .gitignore, skips .git/node_modules; results list `path:line  snippet` grouped by file with highlighted matches; Enter opens a preview at that line with the match highlighted. Mouse and arrow keys work.
2. Find in shell output: search a tab's scrollback, highlight matches, step through them (find that error from 500 lines ago).
3. Find file by name: fuzzy, narrows as you type; `git ls-files` in repos, capped walk otherwise; Enter opens a preview.
4. Find inside a preview (`F`), with the same match highlighting; menu type-to-filter.
Shared: one results/list widget (also used by prompts), every search is a menu Action, no index, no watchers. Engine (rg parsing, scrollback matching, fuzzy scoring) is independent of the UI and can be built by a separate agent first.

## Phase A5: Clickable paths and links, native apps for complex files (added 2026-11-02, user: "main thing for VS Code parity")
1. Paths in terminal output are clickable: Cmd/Ctrl (or Alt, or plain, per `link_click` config) + click on `src/main.rs:12:5`, `./x.py`, `~/a/b`, `File "x.py", line 3`, `--> src/lib.rs:5:9`, git `a/` `b/` prefixes, etc. opens a preview tab (scrolled to the line). URLs open in the local browser.
2. Keyboard route that always works: menu "Open from screen" labels every path/URL visible in the tab with a letter; press the letter. Needed because many terminals do not pass Cmd+click (or Ctrl+click on macOS, which is a right click) to the program.
3. Relative paths resolve against the tab's working directory (OSC 7 if the shell sends it; the tmux pane path for remote tabs; /proc or lsof for local tabs), then the workspace root. Candidates are checked with a bounded stat before opening.
4. Complex files (images, PDF, Office, archives, audio/video, HTML, other binaries) open in the native app: `open` on macOS, `xdg-open` on Linux. Remote files are fetched over SFTP into `~/.cache/wicli/<box>/` (streamed in 64 KB chunks, `.part` then rename, size+mtime reuse, confirm above 100 MB, Esc cancels, total cache capped) and then opened locally. This is the M3 download engine, built first for this purpose.
5. Config: `link_click = ctrl | alt | cmd | any | none`, `[open] native_ext` / `preview_ext` overrides.
Privacy and safety rules (user requirement, 2026-11-02): nothing leaves wi without an explicit yes.
- A confirmation dialog (Y/N or click) comes before any native open, any URL open, and any download from a remote box. It shows the full sanitised path or URL, the size, and where the file comes from. Setting `confirm_open = always` (default) | `never`.
- Opening is delegated to the OS: only `open` (macOS) / `xdg-open` (Linux) with the path as one argument, never through a shell, never executing the file ourselves.
- Only known-safe document and media types are opened natively; programs and installers (app, command, pkg, dmg, exe, msi, bat, jar, appimage, deb, rpm, ...) and unknown binaries are refused with a message.
- Files fetched from a remote box get the macOS quarantine attribute so Gatekeeper's own checks apply when the OS opens them.
- Previews inside wi are read-only renders of a file the user explicitly clicked; they need no extra confirmation.
Work split: links engine (pure path/URL detection, `src/links.rs`), opener (classification, native open, remote fetch cache, `src/opener.rs`), then one integration pass (mouse modifiers, hint mode, cwd resolution, goto-line).

## Phase A6: Side-by-side preview and per-tab folders (added 2026-11-02, user requests)
1. Side-by-side preview: opening a file from a shell tab (path prompt, link click, labels, git view) shows it in a pane beside the shell, on the right by default (`preview_side = right | left | tab`, `preview_width_pct`), instead of a separate full-screen tab. Falls back to a tab on narrow terminals. Focus moves by click or a menu action; Esc in the preview returns focus to the shell; the divider can be dragged.
2. Per-tab base folder: open a new tab in any folder (menu prompt with Tab completion and recent folders), so several Claude sessions can run in different projects. Existing tab: change folder only when it is idle at a shell prompt (never type into a running agent). Previews, git view, links and agent detection use the tab's folder.

## Phase B: Remote M1 completion
1. tmux-backed detach/reattach (`tmux new -A -s wicli`, tmux status/keys disabled so it is invisible; plain session if tmux missing).
2. ssh-agent auth, passphrase-protected keys, `~/.ssh/config` host aliases (HostName, User, Port, IdentityFile).
3. Latency in header (from keepalive round trip), connection state (reconnecting, lost).
4. Reconnect on network drop; manual Reconnect in menu.
5. Idle benchmark: RSS and CPU with 10 tabs, 100 MB output, against spec budgets.

## Phase C: Previews (M2)
1. Workspace file API shared by both backends: stat, list (5,000 cap), read_prefix(256 KB), complete(path).
2. Path prompt with Tab completion; tab titles resolve symlinks.
3. Router: extension then 4 KB sniff, NUL byte means stub. Stub view with name, size, type.
4. Renderers in order: text/log (wrap toggle, tail), code (syntect, first 2,000 lines), CSV/TSV (RFC 4180, 200 rows, `200 of ~N` banner), JSON (collapsible), markdown (pulldown-cmark).
5. In-tab keys: arrows/PgUp/PgDn/Home/End, F find, W wrap, T tail, Enter expand, Esc back.
6. Quick file finder: menu entry "Go to file" with fuzzy search over the workspace (lazy, bounded).

## Phase C2: AI-workflow extras
1. Tab activity indicators and "went quiet" marker (no notifications daemon; just the tab strip and an optional terminal bell).
2. Git diff view: branch in the header, changed-file list, coloured scrollable diff. Read-only first; stage/commit only if you find yourself wanting it.
3. File explorer sidebar (toggle from the menu): lazy, bounded listing; Enter previews; "copy path" to clipboard.
4. Search in files via `rg`/`grep` on request, results list, Enter previews at the match.
5. Edit action: open the file in `$EDITOR` in a new tab (local or on the box).
6. Split view: agent tab plus a preview or diff side by side.
7. Reopen last tabs per workspace (explicit small state file, no watchers).

## Phase D: Downloads (M3, remote only)
SFTP streaming to `.part` then rename, size/mtime reuse, confirm above 100 MB, progress in status bar, Esc cancels, folder download via tar exec channel, Open local copy. Local mode: Open uses the file directly.

## Phase E: Menu and workflow polish (M4)
Menu search-as-you-type, recent files, per-box config (sensitive patterns, caps), config editing from the menu, themes (light/dark detection).

## Phase F: Extensions
1. Write a design doc first. Constraints: nothing runs on the remote box; extensions are separate local processes speaking JSON over stdio; a crash never takes down the session.
2. Surface: contribute menu actions, previewers for file types, status bar items.
3. Manifest + install directory under `~/.config/wicli/extensions/`; permissions declared per extension.
4. First extension as proof: a previewer for a new format.

## Phase G: Hardening and release (M6)
Benchmarks in CI, error messages, man page, README, release builds for macOS (arm64/x86_64) and Linux, Homebrew tap, `wicli.dev` docs (domain not yet registered; check crates.io `wicli` and GitHub name availability).

## Deferred
Preview links and the link service (M5), Windows client, upload-back of edited downloads.

## Open decisions
- Default menu key (Ctrl-Space recommended).
- Whether to keep `tmux` as the only detach mechanism.
- Extension runtime: subprocess+JSON (recommended) vs WASM.
