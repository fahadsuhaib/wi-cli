# wi: a terminal workspace for AI coding agents

[![release](https://img.shields.io/github/v/release/fahadsuhaib/wi-cli)](https://github.com/fahadsuhaib/wi-cli/releases)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
![platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux-lightgrey)

Run Claude Code and Codex in tabs, preview and diff what they write, browse your files, and do all of it on your own
machine or on a remote box over SSH, from one fast, lightweight terminal app. Nothing is installed on the server beyond
sshd. It replaces the "VS Code window next to a terminal" workflow for agent work: no debugger, no LSP and no built-in
editor, just the parts you actually use while an agent writes the code.

## Why wi

- **Built for the agent workflow.** Tabs show which agent is printing (`*`) or has gone quiet (`!`). Pin an agent tab
  beside your shell. Previews refresh by themselves when the agent edits a file. The explorer can type `@path`
  mentions straight into the agent's prompt.
- **Remote without installing anything.** `wi my-box` connects over SSH (your `~/.ssh/config` works); tabs survive
  disconnects through tmux if the box has it. Nothing runs on the server except sshd.
- **Small and fast.** One Rust binary (static on Linux, universal on macOS), about 12 MB of memory when idle,
  redraws only when something changes. No Electron, no background services.
- **Simple keys.** One menu key (`Ctrl-Space`) opens a menu with underlined letters; everything else is letters,
  arrows, Enter and Esc.
- **Private by default.** No telemetry. Nothing is opened, downloaded or sent anywhere without your confirmation.

## Quick start

    curl -fsSL https://raw.githubusercontent.com/fahadsuhaib/wi-cli/main/install.sh | sh
    cd your-project
    wi

Press `Ctrl-Space` for the menu. Try `P` to preview a file, `X` for the file explorer, `A` to start Claude Code or
Codex in a tab, and `Y` to pick a theme. For a remote box: `wi my-box`.

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

## All features

- Local and remote shells, tabs with activity markers (`*` printing, `!` went quiet)
- Previews: text, markdown (mermaid blocks drawn as text diagrams: flowchart, sequence, class, state, ER, pie, gantt and more; `D` shows the source), CSV/TSV, JSON, binary stub; git changes and diffs; in the explorer a changed file opens to its own read-only diff (staged and unstaged, hunk jumps, auto-refresh)
- Recent files: every file you preview is remembered per workspace (also across restarts). Press H in a preview, or menu then Space and type "recent", to pick one again; the full path of the shown file is always on the first row of the preview
- Mouse (clicks, wheel, selection with copy on release), scrollback, bracketed paste; the "Preview a file" and folder prompts are real line editors (a visible cursor, Left/Right/Home/End, Backspace/Delete, Ctrl-U, paste inserts at the cursor, Tab completes)
- Clickable paths and links: Cmd+click on a Mac, Ctrl+click on Linux (also Alt+click or right-click);
  `file:line` opens a preview at that line, URLs and complex files go to the OS after a confirmation;
  relative paths resolve against the shell's real folder (OSC 7 if it sends it, else asked from the OS when you click), then the workspace root and the tab's start folder; if none has it, one bounded search (depth 6, 5,000 entries, 4 s) finds a unique match by trailing path, and several matches raise a small picker (Enter chooses, Esc cancels), never a silent guess;
  a folder path (`src/`, `./out`, `~/work/app`, `/srv/app/`, local or on the box) opens the file explorer rooted there, and leaving returns to your tab
- Agent workspaces: detects `CLAUDE.md`/`.claude` and `AGENTS.md`/`.codex`, asks once and remembers the command per workspace
- File explorer (Ctrl-Space then X): full-screen tree with sizes, dates, git marks and a detail pane. Enter shows a file in the detail pane and stays (Enter again, or O, opens it in the preview pane); E opens it in `$EDITOR` in a new tab. Just type to find: any letter that is not a command (and digits) starts the filter, Esc clears it. Space marks files (kept across folds and when you leave), M types the marks, or the selected path, as `@path ` mentions into your agent tab without pressing Enter. S cycles the sort (name, size, modified, newest first); files that changed or appeared since you opened it or last pressed R get a dot. The explorer keeps its folds, selection, sort and marks while wi runs. A file with git changes opens to its diff by itself in the detail pane (read-only, unified, line numbers, added and removed lines tinted in the theme's colours; the header shows the path, staged or unstaged, +added -removed and hunk n/m). With the diff focused (Enter): Up/Down/J/K scroll, N/P next and previous hunk, Tab switches staged and unstaged when a file has both, E opens the editor, M sends `@path`, Esc goes back. D shows the file content instead (D again, the diff). The mark column shows two letters when the staged and unstaged halves differ (`AM`). An open diff refreshes by itself shortly after the file changes on disk (stat poll, no git polling; R reloads after a `git add`).
- Rename a tab (menu: search "rename", click the active tab, or double-click any tab); the name survives "Restart wi"
- Pinned panes: pin any tab beside the active one (menu: search "pin"); ⇄ swaps left/right, ✕ unpins; the layout is remembered per workspace (needs 101+ columns)
- Previews refresh by themselves when the file changes on disk (see Configuration)
- Search in files, find file, find in output
- Themes: `dark`, `light` and fourteen more modelled on VS Code's (see Themes below); Y opens a picker that previews each one live. The chosen theme paints the whole window, shell included, so a light theme looks light on a dark terminal; auto follows your terminal's colours on each start
- Voice dictation, macOS only for now (the mic button in the top bar, or menu: search "dictate"); audio stays on your machine (see Voice below)
- Update wi from the menu (search "update"): asks before contacting github.com, checks the SHA256, replaces the binary, then offers to restart wi with the same tabs ("Restart wi" does the restart alone)

## Keys

`Ctrl-Space` opens the menu; press the underlined letter or use the arrows and Enter. Esc closes it.

New line in an agent tab: Alt+Enter works in every terminal (Shift+Enter only where the terminal reports it; see `docs/KEYBOARD.md`).

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
| M | Mouse on/off | Y | Theme picker |
| R | Reconnect (remote) | I / Q | Info / quit |

Scrolled back (wheel up, or S): arrows, PgUp/PgDn, Home/End scroll, Esc leaves; typing or pasting anything else jumps to the live bottom and goes to the tab.

In a shell or agent tab Alt+Left/Right (also Ctrl+Left/Right) move by word; wi's own text fields accept them and Alt+Backspace.

Inside a preview: arrows and PgUp/PgDn scroll, F find, W wrap, M markdown source, D mermaid diagrams or their source, R reload, H recent files, Esc back, Q close.

## Pinned panes (tabs side by side)

Pin any tab beside the one you are working in, for example a Claude tab on the right while you use a shell.

1. Ctrl-Space, then Space, and type `pin`. Choose "Pin tab right" or "Pin tab left" and press Enter.
2. The tab moves to that side and stays visible and live. You land on your next tab (or a new shell).
3. Click a pane, or press Ctrl-Space then W, to move the keyboard between them. Wheel, selection and links work in both.
4. ⇄ in the pinned pane's corner (or "Swap pane sides") switches left and right. ✕, Ctrl-Space then V, or "Unsplit" unpins; the tab stays as a normal tab.

The terminal must be at least 101 columns wide, otherwise wi shows one pane. The layout is remembered per workspace.
While a tab is pinned, links and previews open as ordinary tabs. The explorer cannot be pinned, and a pinned local shell
starts fresh after a restart.

## Themes

Menu, then Y (or search "theme") opens the theme picker: `Auto`, then the dark themes, then the light ones, each with a small colour swatch and the current one marked. Up and Down preview a theme live on the whole window, Enter applies it and remembers it (in `~/.config/wicli/state.toml`), Esc puts the old one back; typing letters narrows the list. Choosing `Auto` clears the remembered choice, so wi follows your terminal's background again on the next start.

| Dark | Light |
| --- | --- |
| `dark` (wi's own) | `light` (wi's own) |
| `dark-modern`, `dark-plus` | `light-modern`, `light-plus` |
| `monokai`, `abyss`, `kimbie-dark`, `red` | `solarized-light`, `quiet-light` |
| `solarized-dark`, `tomorrow-night-blue` | `high-contrast-light` |
| `high-contrast` | |

The colours come from VS Code's built-in themes (the shell's 16 ANSI colours from each theme's terminal colours), mapped onto wi's roles. Colours that would be hard to read on a theme's background are nudged toward white or black until text reaches a 4.5:1 contrast ratio (7:1 in the two high-contrast themes). `theme = "monokai"` (any name above, case does not matter) in `[defaults]` of the config sets one for good and wins over the picker's saved choice; an unknown name falls back to `auto` with a note in the status line. `paint_background = false` still leaves the terminal's own colours in the shell area.

## Naming a tab

Give a tab your own name: Ctrl-Space, Space, `rename` (Rename tab), or click the tab that is already active in the tab
strip, or double-click any tab. A one-line prompt opens with the current name and a real cursor: left/right/Home/End,
Backspace/Delete and paste work, Enter saves, Esc cancels. An empty name goes back to the automatic title (shell title,
folder, agent). Names are trimmed, limited to 40 columns, and cut with an ellipsis in the strip (the full name is kept).
The name wins over the automatic title and stays when the program changes its own terminal title; it does not affect how
wi treats an agent tab (new-line keys). A single click on a tab that is not active still just switches to it.
Custom names survive "Restart wi" (and the restart after an update); they are also remembered across a normal quit (see Reopening your tabs).

## Reopening your tabs

Quit wi from the menu (or close the terminal) and the next `wi`, `wi <folder>` or `wi <box>` in the same workspace
reopens the same tabs: folders, custom names, the active tab, previews and pinned layout. A status note says so
("reopened 3 tabs from last session"). As with "Restart wi", running programs do not survive: shells start fresh
in their folders, agent tabs start their remembered agent command again, remote tmux tabs reattach. Only folders,
names, agent names and session names are saved (in `state.toml`, one entry per workspace; the last quit wins), never
screen content. A snapshot older than 30 days is ignored, a tab whose folder is gone is skipped, and a workspace with
only one plain shell saves nothing; closing the last tab forgets the session. `wi --fresh` starts with one tab and
forgets the saved ones; `restore_tabs = false` in `[defaults]` turns the whole thing off.

## Menu items without a key

A few items have no letter because they are all taken. Press Ctrl-Space, then Space, and type a few letters:
`pin` (Pin tab right / left, Swap pane sides, Unsplit), `recent` (Recent files), `update` (Update wi),
`restart` (Restart wi), `dictate` (Dictation; also `voice`, `mic`), `rename` (Rename tab; also `title`, `name`, `label`).

## Voice

macOS (Apple Silicon) only for now. Click the `○ mic` at the right end of the top bar, or choose Dictation in the menu (search "dictate"): the first time, wi offers to install a small helper and download a 296 MB speech model, then tests your microphone.
After that every phrase you say is typed into the focused pane without Enter (it presses Enter itself after 3 seconds of quiet; switch that off with the menu entry "Auto-Enter for dictation"); click the mic again to stop, or press `O` in the Dictation menu screen for options (test the microphone, remove voice).
Audio never leaves your computer: no cloud, nothing is recorded to disk, and it turns itself off after 10 minutes of silence. More in [docs/AUDIO.md](docs/AUDIO.md).

## Updating

Ctrl-Space, Space, `update`. wi asks before it contacts github.com, tells you whether a newer release exists, asks again
before downloading, checks the SHA256, and replaces the binary. If the folder is not writable (for example
`/usr/local/bin`) it shows the `sudo install` command instead of running it. Afterwards it offers "Restart wi now?":
your tabs reopen in the same folders; running programs (shells, agents) start fresh, remote tabs reattach.
"Restart wi" does the same without updating.

Multi-line input (Shift+Enter) in agent tabs: see [docs/KEYBOARD.md](docs/KEYBOARD.md).

## Configuration

`~/.config/wicli/config.toml` (override with `WICLI_CONFIG`). A small example:

    [defaults]
    theme = "auto"          # auto or a theme name (see Themes)
    paint_background = true # wi paints the whole window (shell included) in the theme's
                            # colours; false leaves default colours to your terminal
    preview_side = "right"  # right, left or tab
    restore_tabs = true     # reopen last session's tabs at start (wi --fresh skips it once)
    preview_autorefresh = true          # reload a visible file preview when the file changes
    preview_refresh_debounce_ms = 5000  # wait this long after the last change before reloading

    [transcribe]              # voice dictation (macOS), see docs/AUDIO.md
    enabled = true
    idle_minutes = 10         # switches itself off after this long without speech

    [agents.claude]
    command = "claude --continue"                  # your usual command, listed first
    commands = ["claude", "cc", "claude-work"]     # more entry points (optional); same for [agents.codex]

    [[box]]
    name = "dev"
    host = "dev.example.com"
    user = "me"
    identity = "~/.ssh/id_ed25519"

### Starting an agent

In a folder with `CLAUDE.md`, `.claude`, `AGENTS.md` or `.codex`, wi asks once how to start the agent (menu, then A,
"Start / change agent" asks from any folder). The popup is one list, each row an agent and its exact command:
the command used here last time ("last used here"), your `[agents.*]` commands, the ones you used recently
(8 per agent), then plain `claude` and `codex`. Up/Down and Enter start the selected row. `O` (or the "Other
command..." row) opens a boxed text field with a real cursor: type any command (Left/Right/Home/End, Backspace,
Delete, paste), Tab switches which agent it counts as, Enter starts it, Esc goes back to the list. `E` edits the
selected row's command as a starting point. `N` or Esc is "Not now", `X` is "Never here". What you start is
remembered for the folder and skips the question next time.

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

[MIT](LICENSE)
