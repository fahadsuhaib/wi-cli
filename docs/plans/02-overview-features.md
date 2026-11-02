# wicli: overview-level developer features

Separate from `01-roadmap.md` (core terminal, previews, remote, search, themes). These are small, glanceable features that make day-to-day work with AI agents (Claude Code, Codex) smoother. Written 2026-11-02 from the user's request ("progress tabs, copy/paste bins, stuff like that").

## Principles
- **Small and glanceable.** One keypress or click from the menu; no new concepts to learn.
- **Nothing resident.** No background daemons, no watchers, no timers unless a visible feature is on screen and needs one. Remote boxes get a command only when the user asks.
- **Explicit over automatic.** Nothing is sent to the agent, copied out, or saved without a keypress. Anything that stores text (clips, notes) is local, small, capped, and can be turned off.
- **Same menu, same keys.** Every feature is an `Action` with a plain letter in the Ctrl-Space menu. No chords, no symbol keys.
- **Local and remote alike.** Work through the `Backend` abstraction; remote uses the existing SSH connection.
- **State lives in small files** under `~/.config/wicli/` (or the workspace's own `.wicli/` for notes the user may want to commit); written atomically.

Size key: **S** about a day, **M** a few days, **L** a week or more. Priority: **P1** first batch, **P2** next, **P3** later.

## 1. Overview and progress

### 1.1 Overview tab ("Home")  P1 · M
A single read-only tab, reachable from the menu, that answers "what is going on in my workspace?" at a glance:
- one row per tab: title, state (`*` printing, `!` went quiet, idle), time since last output, and whether it is an agent tab;
- git: branch, number of changed files, ahead/behind if known (one `git status` call when the tab is opened or reloaded with `R`);
- the remembered agent command for this workspace, and when each agent tab started;
- the last line of output of each tab (what is it doing right now?);
- reachable ports (see 4.1) and the checkpoint summary (see 3.2).
Mouse: click a row to jump to that tab. Rendering reuses the preview tab machinery.

### 1.2 Tab progress and timers  P1 · M
Make the tab strip tell you how long things have run and how far along they are.
- **Elapsed and idle time:** per tab "running 3m12s" and "quiet 45s" (computed from the timestamps wi already keeps; redraw only when the Overview or strip is visible).
- **Progress hints:** recognise common progress output in a tab's latest line (`45%`, `[#####   ]`, `Step 3/10`, `12/80 files`, cargo's `Compiling x (n/m)`) and show it in the tab label, e.g. `build 45%`. Pure pattern matching on already-received bytes; configurable list of regexes.
- **Exit status:** with optional shell integration (OSC 133 prompt markers, one small snippet for bash/zsh/fish), show the last command's exit code and duration per tab, and "ran 8m, failed (1)" when a background tab goes quiet. Works without the snippet, just without exit codes.

### 1.3 Agent state  P2 · M
Beyond "quiet", tell when an agent is **waiting for you** versus **finished**: configurable patterns for prompts such as permission questions or "Do you want to proceed", plus the terminal title the agent sets. Shown as a distinct marker on the tab (`?`). Starts as heuristics and stays opt-in per agent in the config.

### 1.4 Notifications  P1 · S
Optional, off by default: when a background tab goes quiet or an agent starts waiting, ring the terminal bell (exists) and/or send an OS notification (`osascript` on macOS, `notify-send` on Linux; for a remote box the notification is raised by the local wi, so nothing runs there). Per-tab mute from the menu.

### 1.5 Session timeline  P2 · S
A short log per workspace: agent started, tab went quiet, tab exited, reattached. When you reattach to a remote session, a one-screen "while you were away" summary: what finished, what failed, what is waiting. Stored in the state file, capped at the last 200 events.

## 2. Copy, paste and snippets ("bins")

wi cannot see what the terminal app copies with the mouse, so these features work on what wi *does* see: its own screen, previews, search results, and what is pasted through it.

### 2.1 Copy from screen  P1 · M
- Copy the **last output block** (from the last prompt/command to now; uses OSC 133 when available, otherwise the last screenful), the **last N lines**, or the **whole visible screen** of a tab, to the system clipboard.
- Copy from previews and diffs: the **file path**, the **visible text**, the **selected range** (keyboard range: mark start with Enter, extend with arrows, copy with `C`), or the **whole diff**.
- Clipboard route: `pbcopy` on macOS, `wl-copy`/`xclip` on Linux when local, and **OSC 52** when remote or when no tool is found, so it lands on the local clipboard even over SSH. Size capped (default 1 MB) with a notice.

### 2.2 Paste history  P2 · S
A ring of the last 20 things pasted *through wi* (bracketed-paste events wi already sees), viewable in a list with a preview of each; Enter re-pastes into the active tab. **Opt-in** and kept in memory only unless the user turns on persistence (it can contain secrets). Entries matching common secret shapes (API keys, private key headers) are never stored.

### 2.3 Named bins and snippets  P1 · M
Small saved texts you paste often: a standard prompt for the agent ("review this diff for bugs"), a command, a path, a stack of environment exports.
- Menu: Bins list (arrows or mouse), Enter pastes into the active tab with bracketed paste, `S` saves the current clipboard/selection as a new bin, `E` edits, `X` deletes.
- Templates with variables filled from the workspace: `{file}`, `{branch}`, `{dir}`, `{agent}`, `{clipboard}`.
- Stored in `~/.config/wicli/bins.toml`; per-workspace bins in `.wicli/bins.toml` (shareable via git).

### 2.4 Send to agent  P1 · M
The core AI-workflow handoff: from a preview, diff, search result or the Overview, **send** the file path (as `@path`), a code range, or the current diff to the agent tab, with an optional prompt from a bin. Nothing is sent without Enter; the target tab is the agent tab (or a choice if there are several). Uses bracketed paste so multi-line text is delivered intact.

## 3. Reviewing agent work

### 3.1 Changed-files list  P2 · S
A compact list of changed files with +/- counts above the Git diff view (jump to a file with Enter or a click). Builds on the existing Git view.

### 3.2 Checkpoints  P1 · M
"Mark checkpoint" records the current git state (HEAD plus a tree hash of the working directory, using a temporary index so nothing in the repo changes). "Changes since checkpoint" then shows exactly what the agent did in this run, even when it committed along the way. Menu entries: Mark checkpoint, Changes since checkpoint, Clear checkpoint. Stored in the workspace state; no git history is modified.

### 3.3 Run last command / favourites  P2 · S
Re-run the last command in a fresh tab, and pin a few favourite commands (test, build, lint) detected from `Cargo.toml`, `package.json`, `Makefile`, or defined by hand. One key from the menu.

## 4. Environment

### 4.1 Ports and forwarding  P2 · L
List listening ports in the workspace (local: `lsof`/`ss`; remote: `ss` over the existing SSH connection, on request). For a remote box, **forward** a port to the local machine with one keypress (an SSH direct-tcpip channel on the existing connection, no extra process), so the dev server the agent started is reachable in the local browser. Shows active forwards in the Overview.

### 4.2 Resource meter  P3 · S
Optional header segment with load, memory and disk of the box or machine. Off by default; when on, refreshed at most every 10 s by one small command (remote) or a local read, with the timer only running while enabled.

### 4.3 Notes scratchpad  P2 · S
A markdown notes tab per workspace (`.wicli/notes.md`), rendered with the existing markdown preview, with "Add note" (one line appended from a prompt, no editor needed) and "Open in editor" (`$EDITOR` in a tab). Handy for jotting what to tell the agent next.

### 4.4 Restore tabs  P2 · S
Remember which tabs (agent tab, shell tabs, preview paths) were open per workspace and offer to reopen them next time. Explicit state file; no watchers.

## 5. Small quality-of-life items  P3 · S each
- **Bookmarks:** mark a position in a tab's scrollback and jump back to it.
- **Export:** save a tab's scrollback (or the last N lines) to a text/markdown file, for sharing or as agent context.
- **Quick jump:** jump to the tab that last made noise.
- **Tab rename and reorder** from the menu.
- **Per-workspace environment:** extra env vars and a working directory for new tabs.

## Suggested first batch (P1)
1. Overview tab (1.1) and tab progress + timers (1.2).
2. Copy from screen (2.1) and Send to agent (2.4), which together close the loop between reviewing and instructing.
3. Named bins (2.3), since send-to-agent wants prompt templates.
4. Checkpoints (3.2).
5. Notifications (1.4).

## Work split for parallel agents
Independent files, so these can be built in separate worktrees:
- **Clipboard layer + bins + paste history** (`src/clip.rs`, `src/bins.rs`): clipboard backends (pbcopy, wl-copy, xclip, OSC 52), bins file format, template expansion, secret-shape filter. Pure logic plus thin I/O, fully unit-testable.
- **Progress and state detection** (`src/progress.rs`): pattern matching for progress hints and agent-waiting prompts, OSC 133 parser, timers. Pure logic over bytes.
- **Checkpoints and changed-files** (`src/checkpoint.rs`, builds on `src/exec.rs`): git plumbing with a temporary index; tests against a temp repo.
- **Ports and forwarding** (`src/ports.rs`, touches `src/session/mod.rs`): listing and SSH port forwarding.
- **UI integration** (Overview tab, menu items, strip labels, send-to-agent flow): touches `src/app.rs` and `src/ui/mod.rs`; done after the engines land, by one agent, to avoid conflicts.

## Open questions
- Is persisting pasted text ever wanted, or should history stay memory-only? (Default proposed: memory-only.)
- Should notifications be allowed to include output text, or only "tab 2 went quiet"? (Default proposed: no output text.)
- For remote boxes, are port forwards the right size of feature to build in this round, or later?
