# wicli: file explorer view, built in the reference screenshot's style

Written 2026-11-03. The reference screenshot (a dense, aligned, two-pane monitor with a title bar,
key hints, a foldable tree with a right-hand column and a detail pane) is the model for how wi
screens should look and behave. This plan builds the **file explorer** that way, and the reusable
widgets that make the next screens look the same. No monitoring (see 03-dashboard.md, parked).

## 1. What we copy from the reference

| Reference element | In the explorer |
| --- | --- |
| Inverted title bar | `explorer · ~/work/web-app · main` (folder, git branch; box name when remote) |
| One dim line of key hints | Every key on screen, always: `up/down select · left/right fold · Enter open · ...` |
| Status line with `LIVE` badge | A badge (`LOCAL` or the box name), then `58 items · 3 changed · 12/58` |
| Left pane with column header (`BRANCHES  ELAPSED`) | `FILES  SIZE  MODIFIED`, right-aligned numeric columns, selected row highlighted |
| Foldable tree (`▾ root`, `·` leaves) | `▾ src`, `▸ docs`, `·` for files; indentation guides |
| Footer inside the pane (`2/6 · wrap · fold`) | `12/58 · up/down wraps · left/right folds` |
| Right pane header + `Source: file:line` + code excerpt with `>` marker | Selected item name, `Source: src/main.rs`, then its content with line numbers |
| Dense cards, one colour language | Git marks and sizes use the theme's green/yellow/red; labels dim |

## 2. Layout (mock, 120 columns)

```
 explorer · ~/work/web-app · main                                                      <- title bar
 up/down: select · left/right: fold · Enter: open · F: find · H: hidden · G: changed · T: tab here · q: back
 LOCAL   58 items · 3 changed · 2 hidden
 FILES                        SIZE  MODIFIED │ src/main.rs
 v src                              2h ago   │ Source: src/main.rs · 18 KB · modified 2h ago · M modified
   . app.rs                  116 KB  2h ago  │ 1   mod app;
 > . main.rs                  18 KB  2h ago  │ 2   mod config;
   . config.rs                 4 KB  3d ago  │ 3   mod links;
 v docs                                      │ ...
   . KEYBOARD.md               2 KB  1d ago  │
 > tests                                     │
   . Cargo.toml                1 KB  4h ago M│
 12/58 · up/down wrap · left/right fold      │ lines 1-30 of 412 · J/K scroll
```

Narrow terminals (under 100 columns): the detail pane hides and Enter opens the file in the normal
preview pane; the tree uses the full width.

## 3. Behaviour

- **Opens as its own full-screen view** (menu action, letter `x`; `Q` or Esc leaves). The tree is
  the active tab's folder, so each tab explores its own project.
- **Tree:** folders first, then files, case-insensitive; `left/right` fold and unfold (right on a
  folded folder opens it, left on a file jumps to its parent); `up/down` wrap around; Home/End/PgUp/PgDn.
  Folders are read lazily when first opened, capped at 5,000 entries with a visible notice.
- **Columns:** name, size (`18 KB`, folders blank), modified (`2h ago`, `3d ago`), and a git mark
  (`M` modified, `A` added, `?` untracked, `D` deleted) in the theme's diff colours. A folder shows
  a dot when something inside it changed.
- **Detail pane (right):** for a text-like file, the real preview (markdown rendered, JSON, CSV,
  code with line numbers) with `Source:` and metadata on top; for a folder, a summary (items, size
  of its direct children, changes inside) and its listing; for images, PDFs, archives and other
  binaries, an info card that says Enter asks before opening natively (the existing confirmation
  and safety rules apply unchanged).
- **Keys (plain letters only):**
  - `Enter` on a file: open it in the preview pane (or tab) next to the shell and leave the
    explorer; on a folder: fold/unfold. Complex files go to the confirmation dialog.
  - `J` / `K` scroll the detail pane, `PgUp`/`PgDn` page the tree.
  - `F` find in tree: type to narrow the visible rows with the fuzzy scorer from Find file.
  - `H` show or hide hidden files, `G` show only changed files, `R` refresh.
  - `T` open a new shell tab in the selected folder, `A` start an agent there (same confirmation
    flow), `C` copy the path (relative; `Shift` is not needed: pressing `C` twice copies absolute).
  - `S` cycles the sort (name, size, modified).
- **Mouse:** click selects, double-click opens, click the arrow to fold, wheel scrolls whichever
  pane is under the pointer, drag the divider to resize, Cmd/Ctrl+click a path in the detail pane
  works like everywhere else.
- **Local and remote identical:** everything goes through `Backend` (directory listings with size
  and mtime from `std::fs` locally and SFTP attributes remotely, git status through the existing
  exec helper in the tab's folder). Nothing resident, no watchers: `R` refreshes, and the tree
  refreshes a folder when it is opened.

## 4. The reusable TUI kit (the point of the reference)

New module `src/tui/` with small widgets that every wi screen can share, all themed:
- `TitleBar`, `HintBar`, `StatusLine` (badge + text), the three top lines.
- `Panel` (a titled pane in the reference's style: coloured header row, no box clutter).
- `ColumnList` (rows with left text and right-aligned columns, a header, selection, scroll,
  wrap-around selection, a footer line, hit-testing for the mouse).
- `Tree` (fold state, flatten to rows, expand/collapse, parent/child jumps, filter) as pure logic.
- `SplitView` (two panes with a draggable divider and a minimum width, reusing the pane geometry code).
- Theme roles added: title bar, hint dim, badge, column header, tree guide, size, date, selection.
Later screens (search results, Git changes list, the open-file prompt, agent prompt) can move onto
the kit so the whole app looks like the reference.

## 5. Milestones

| Step | Scope | Done when |
| --- | --- | --- |
| E1 | `src/tui/` kit: TitleBar, HintBar, StatusLine, Panel, ColumnList, Tree model, SplitView, theme roles | Unit tests for tree flatten/fold/filter/jumps, column alignment and truncation of wide characters, wrap selection, hit-testing; both themes pass contrast tests |
| E2 | Data layer: listing with size and mtime (local + SFTP), git status parser, relative-time and size formatters | Unit tests with fixtures; remote test against the Docker box |
| E3 | The explorer view: tab view, keys, detail pane using existing previews, folder summary, binary cards | pty test: tree, fold, select, detail text, hidden toggle, changed-only, sort |
| E4 | Actions and mouse: Enter/open, new tab here, agent here, copy path, find in tree, drag divider | pty tests for each action, local and remote |
| E5 | Polish: narrow layout, light theme, wide-character names, huge folder notice, README section | Screenshots-by-text check in both themes; full suite green |

E1 and E2 are independent (pure logic), so two agents build them in parallel; one agent then
integrates E3 to E5. About 1 to 1.5 days of agent work end to end.

## 6. Open questions (my defaults in brackets)

1. **Enter on a file**: open it in the preview pane and leave the explorer [yes], or keep the
   explorer open with the preview on the right only [no, the right pane already shows it].
2. **Edit**: add a key to open the file in `$EDITOR` in a new tab [yes, `E`... but `E` is "end
   sessions" in the menu; inside the explorer a plain `O` is free and I'd use it].
3. **Pinned sidebar** version (a narrow tree always visible beside the shell) [later, wide
   terminals only; the full-screen view first].
4. **Move the existing search results and finder onto the kit** [after E5, one at a time].
