# wicli: git changes and diffs in the explorer

Written 2026-11-06. Goal: stop going to VS Code to see what an agent changed. The explorer already shows git
marks and a changed-only filter; this plan adds the diff itself, staged and unstaged, and a changed-files
view. Nothing is written to the repository: **read-only**. No staging, committing or discarding in the
first version.

**Status:** phase 1 plus the auto-refresh of an open diff is built (parser, staged/unstaged split, read-only viewer in the explorer detail pane, `D` toggle, `dev/pty_gitdiff.py`). The viewer lives in the detail pane, not yet in a separate preview-pane tab. Changes list, side-by-side, ask-the-agent are still to do.

## 1. What exists, what is new

| Already in place (`src/explorer_data.rs`, `src/explorer.rs`) | New in this plan |
| --- | --- |
| `git_marks()`: one shell call, `git status --porcelain=v1 -z`, capped, local and remote | staged vs unstaged split (the `X` and `Y` columns are parsed today, then merged into one mark) |
| `GitMark` (M A D R ? !), dirty folders, branch, repo root | per-file diff view with hunks |
| `g` toggles changed-only | a flat "Changes" list (staged, unstaged, untracked sections) |
| `Mode` preview pane, markdown/CSV/JSON viewers | a diff viewer pane in the same style |

## 2. Reuse, not build

- **Git itself computes everything.** `git diff`, `git diff --cached`, `git show HEAD:path`. No diff
  algorithm in wi. It runs through the same `run_script` path as `git_marks`, so remote works with nothing
  installed (git is on any dev box), output is capped and has a timeout, and `GIT_OPTIONAL_LOCKS=0` keeps wi
  from touching the index.
- **A small unified-diff parser** (about 100 lines, `src/gitdiff.rs`): file headers, `@@` hunks, `+`, `-`,
  context, `\ No newline at end of file`, binary notice, rename headers. Pure function, unit tested.
  (The `patch` crate parses the same format; adopt it only if our parser grows past ~150 lines.)
- **`similar` crate** (pure Rust, no deps) for word-level highlight inside a changed line pair. Optional,
  phase 2.
- **Rejected:** `git2` (libgit2, C build, bigger release matrix), `gix` (large, diff/status still maturing),
  `delta`/`difftastic` (must exist on the remote box; could be an optional local-only renderer later),
  `gitui`/`lazygit`/`tig` (whole apps, not embeddable), `syntect` highlighting (a few MB; later, optional).
- Always pass `--no-ext-diff --no-color --no-textconv` so a user's pager, external diff tool or color config
  cannot change the output we parse. Add `-c core.quotepath=off` and use `-z` where paths are listed.

## 3. Data layer (`src/gitdiff.rs`, extend `src/explorer_data.rs`)

```
struct Change { path, orig_path: Option, staged: Option<GitMark>, unstaged: Option<GitMark> }
struct FileDiff { path, old_path, hunks: Vec<Hunk>, binary: bool, truncated: bool, added: u32, removed: u32 }
struct Hunk { header: String, old_start, new_start, lines: Vec<DiffLine> }
enum DiffLine { Context(String), Add(String), Del(String), NoNewline }
```
Calls (each one shell call, capped, timeout, off the UI loop like the other explorer loads):
1. `changes(dir)`: `git status --porcelain=v1 -z --untracked-files=all` (already run; keep the X/Y pair).
2. `file_diff(path, Staged | Unstaged | Both)`: `git diff [--cached] -U3 -- path`. Untracked file: show the whole
   file as additions (`git diff --no-index /dev/null path`, or read the file with the existing bounded
   reader). Deleted file: the removal diff. Rename: `-M` header plus content hunks.
3. `diff_stat(dir)`: `git diff --numstat -z` for `+12 -3` per file in the Changes list (one call for all).
Caps: diff output at the existing `OUT_CAP`; a "diff too large, showing first N lines" note when truncated;
files over a size limit show the stat only.

## 4. UX

**Explorer (small changes)**
- Marks stay as they are. A second column letter for staged state when it differs: `M` unstaged, `M·`... shown
  as two characters (`MM`, `A `, ` M`) in the right-hand column, dim for staged. Keep colour language.
- `G` (Shift-g is not allowed; use a plain letter): the **Changes view** replaces the tree with a flat list in
  three sections, Staged, Changes, Untracked, each row `M src/app.rs  +12 -3`. Same movement keys as the tree.
  Key choice: the explorer's commands are `Q J K F H G S R C P T A O E M`; `G` is taken by changed-only, so
  `G` cycles **all files, changed only, changes list** (one key, three states, shown in the status line).
- Enter on a changed file in the detail pane shows its diff (not its content) in the right pane; the pane header
  says `diff · unstaged` or `diff · staged`. `D` toggles diff/content for any file (a free letter; check
  against the find filter rules, where any non-command letter starts find).

**Diff viewer** (opens from the detail pane by a second Enter or `O`, like the preview, in the normal preview
pane so it can be pinned beside an agent tab)
- Unified view by default; side-by-side when the pane is 120+ columns wide (`V` toggles). Line numbers for both
  sides, green/red backgrounds from the theme (`Spec` rows already define the palette; add `diff_add`,
  `diff_del`, `diff_hunk` derived from the theme's green/red/blue, blended so all 16 themes stay readable).
- Keys: up/down or `J`/`K` scroll, `N`/`P` next/previous hunk, `Tab` switch staged/unstaged, `C` toggle
  context collapse (folds unchanged runs longer than 7 lines), `E` open the file in the editor at the hunk's
  line, `M` send `@path` to the agent, Esc back.
- Header row with the full path (cut in the middle), `+added -removed`, and hunk position `3/8`.
- Next/previous **file** with left/right (or `[`/`]` is not allowed, so `Q`... decide in review: proposal is
  left/right arrows when the viewer has no horizontal scroll).

**Ask the agent (phase 2):** wi never changes the repository; the agent does. In the diff viewer and the
Changes list, `A` opens a small menu of ready-made requests. Choosing one **types the line into the agent tab
without pressing Enter**, so you can edit it and send it yourself (same mechanism as `M` mentions):
- `Please revert src/app.rs` (whole file)
- `Please revert the change at src/app.rs:120-140` (current hunk, line range from the `@@` header)
- `Please commit the staged changes` / `Please commit all these changes with a clear message`
- `Please explain this change in src/app.rs:120-140`
- `Please resolve the conflict in src/app.rs` (shown only on `!` conflict files)
- `Please review this diff for bugs` (current file, or all changed files from the Changes list)
Paths with spaces are quoted. If no agent tab exists the menu says so and offers to start one (the normal agent
start popup). Each entry is an `Action`. Nothing is sent without the user pressing Enter in the agent tab.

**Agent loop helper (phase 3):** `M` in the Changes list marks files and types all `@path` mentions, which the
explorer already does for marked files.

## 5. Refresh rules (fit the no-watcher rule)

- Load on opening the Changes view or a diff, and on `R`. No background polling of git.
- A diff open in the preview pane reuses `src/refresh.rs`: the 1 s stat poll of the file triggers one
  re-run of `git diff` for that file after the same debounce (`preview_refresh_debounce_ms`). Scroll position
  and hunk are kept by matching the hunk header text.
- Remote: one request at a time, timeouts, same as the preview refresh.

## 6. Edge cases to handle and test

- Not a repo, git missing, bare repo, detached HEAD, shallow clone, submodule entries (show as one changed
  line, no diff), worktrees, a repo root above the explorer root.
- Merge conflicts: show `U` marks and the combined diff (`git diff` default); no resolving.
- Binary files, huge files, very long lines (cut with an ellipsis and horizontal scroll), CRLF (show `^M` dimmed
  or trim at display only), tabs expanded to 4 columns, wide characters and emoji, non-UTF-8 bytes (lossy).
- Paths with spaces, quotes, newlines, leading dashes (always `--` before the path).
- Mode-only changes and symlink changes (`old mode`/`new mode`, `-> target`).
- Unborn branch (no commits yet): staged files diff against the empty tree.
- Privacy: nothing leaves the machine; diff content is never logged or sent anywhere.

## 7. Phases and tests

| Phase | Work | Tests |
| --- | --- | --- |
| 1 | `gitdiff.rs` parser; `file_diff`, staged/unstaged split in `Change`; read-only diff viewer in the preview pane; `D` toggle from the explorer | unit: parser fixtures (add, delete, rename, binary, no-newline, multi-hunk, quoted paths, mode change, empty diff, truncated); pty: `dev/pty_gitdiff.py` builds a temp repo, edits files, opens the viewer, checks rows and hunk jump; remote run against the Docker box |
| 2 | Changes list (`G` three-state), numstat counts, side-by-side, word-level highlight (`similar`), collapse context, `E` at hunk line, `A` ask-the-agent menu | unit: numstat parser, section grouping, side-by-side line pairing; pty: Changes list navigation, open diff from list; unit: request text for file/hunk/conflict, quoting; pty: `A` types the line into the agent tab and does not press Enter |
| 3 | Auto-refresh of an open diff, `M` for all changed files, optional staged/unstaged toggle polish, optional `git log -p`/`blame` for the selected file (read-only history) | pty refresh test (edit file while diff open) |
| later, only if asked | stage/unstage a hunk or file (`git apply --cached`), discard (needs a confirmation), commit message prompt | separate plan; these write to the repo |

Run only affected tests while developing; the full `dev/run_tests.sh` before release.

## 8. Docs to update when built

`CLAUDE.md` Features paragraph, `docs/KEYBOARD.md` (explorer and diff keys), `README.md` feature list and
"All features", `docs/plans/04-explorer.md` (point to this plan), the in-app key hint line for the explorer.

## 9. Open questions (decide before phase 2)

1. Key for the Changes view: a three-state `G`, or a separate letter? (Proposal: three-state `G`.)
2. Side-by-side default width threshold (proposal: 120 columns).
3. Show staged and unstaged together by default, or unstaged first with `Tab` to switch? (Proposal: together
   in the Changes list, separate in the viewer.)
4. Syntax highlighting inside diffs: skip until asked (adds `syntect`, a few MB).
5. Stage/unstage/discard: keep out of v1 to stay read-only and private-by-default?
