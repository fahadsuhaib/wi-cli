# 05 Pinned panes (roadmap A8)

Any tab can be pinned to a left or right pane that stays on screen beside the other tabs.

## Model
- `App.pinned` (tab id), `pin_side` (Left/Right, default Right), `pct` (pinned pane width) and `last_main`.
- Two tabs are on screen: the pinned tab on `pin_side`, and the active tab (or, while the pinned tab is active, the last other tab you used).
- The active tab is the one with the keyboard. Clicking or wheeling over the other pane makes that tab active, so keys, paste, scrollback, find, selection and links all keep working unchanged on the active tab. Menu W does the same.
- The tab beside the active one is drawn read-only from its own vt100 screen or preview; its output keeps arriving as usual (no activity marker while visible).
- Side previews (`preview_side`) are unchanged while nothing is pinned. While a tab is pinned, links and files open as ordinary tabs.

## Layout
- `tab::split_pinned` wraps `split_main`: 60-column minimum for the main pane, 40 for the pinned pane, 1-column divider; below 101 columns there is one pane (the pinned tab is then reached from the tab strip).
- `resize_all` sizes each visible tab's pty and vt100 to its pane; it is a no-op when sizes are unchanged and runs before each draw while a tab is pinned.
- The pinned pane's top-right corner carries two buttons: ⇄ swaps sides, ✕ unsplits (the tab stays a tab).

## Actions (menu, searchable; no hotkeys because the letters are used up)
Pin tab right, Pin tab left (pins the active tab; with a single tab a new shell opens beside it), Swap pane sides, Unsplit. `V` (close preview) unsplits while pinned; `W` moves the keyboard to the other pane.

## Remembered per workspace (`state.toml`, `[panes."<workspace key>"]`)
`side`, `pct`, and what was pinned: `shell` (folder), `agent` (name; re-pins the running agent tab) or `preview` (file path). Restored at start; a shell is started fresh (remote shells reattach via tmux only for the normal tab restore, a pinned shell starts new).

## Not done
The explorer as a pinnable tab (it stays full screen), git view pinned across restarts, pinned agents not auto-started when the agent prompt is shown.
