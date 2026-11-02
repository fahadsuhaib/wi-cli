# TUI kit (`src/tui/`)

Reusable widgets for dense, aligned screens (first user: the file explorer). Everything draws into a
ratatui `Buffer` with colours from `Theme` and measures text with unicode-width (`tui::fit`, `tui::width`).

## Widgets
- `bars::TitleBar::new(left).right(text).render(area, buf, &t)`: inverted full-width bar.
- `bars::HintBar::new(&[("Enter","open"), ...])`: one dim line, ` · ` between hints, trailing hints dropped, never wraps.
- `bars::StatusLine::new("LOCAL").dim("12 files").seg("1 error", Tone::Hot)`: badge, then segments (Tone: Normal/Dim/Good/Warn/Hot).
- `panel::Panel::new(title).right(text).footer(text)`: `render` returns `PanelAreas { header, body, footer }`; `layout` gives the rectangles only. `panel::divider` draws one `│` column.
- `columns::ColumnList`: `Column::new("SIZE", Width::Min(5)).right()`; `Width` is `Fill`, `Fixed(n)` or `Min(n)`. `render(&rows, area, buf, &t)` with `Row::new(cells).with_prefix(glyphs, style)`; `Cell::styled(text, style)` for a colour. Navigation: `up/down(len)` (wrap on by default), `page_up/page_down`, `home`, `end`, `scroll(delta, len)`, `select`. `footer(len, "up/down wrap")` gives `12/58 · up/down wrap`. `hit_test(col,row)` and `hit_header(col,row)` use the last render. The selected row is painted whole in `row_sel_*`; cell colours do not apply on it.
- `tree::Tree<T>`: nodes with `id` (a `/`-joined path), `label`, `is_dir`, `expanded`, `loaded`, `children`, `payload`. `flatten()` / `flatten_filtered(&closure)` give `FlatRow`s; `toggle/expand/collapse` return true when children must be fed with `set_children`; `expand_all_to(path) -> Reveal`; `reselect(rows, prev_id, prev_index)` keeps the selection; `select_parent/select_first_child`; `sort_with(fn)` (dirs first by default). `Glyphs::unicode()` / `ascii()` / `.with_guides()` build the row prefix.
- `split::SplitView::new(pct, left_min, right_min).layout(area) -> SplitLayout` (collapses to `single` pane when narrow), `on_divider`, `pane_at`, `pct_for_col` for dragging.

## Example
```rust
let areas = Panel::new("FILES").footer(list.footer(rows.len(), "up/down wrap")).render(lay.left, buf, &t);
list.render(&rows, areas.body, buf, &t);
```

## Theme roles
`title_bg/fg`, `hintline_fg`, `badge_bg/fg`, `col_header_bg/fg`, `tree_guide`, `size_fg`, `date_fg`, `row_sel_bg/fg`, `good`, `warn`, `hot` (both themes, contrast-tested). `hint_fg` already means link-label text, hence `hintline_fg`.

## Rules of the look
Title bar on top (inverted), then one dim hint line, then a status line (badge + dim counts, tones only for good/warn/hot). Panes have a coloured header row, no boxes; a footer row inside the pane shows `pos/total · hints`. Numeric columns are right-aligned with a column header; the selection is a full-row highlight; Up/Down wrap around. Truncate with `…`, never split a wide character, never wrap.

## Used by the explorer (`src/explorer.rs`, `src/ui/explorer.rs`)
The file explorer (menu `x`) is the first screen built from the kit: `TitleBar`, `HintBar`, `StatusLine`, a `Panel` for the right-hand detail pane, a `ColumnList` (its header row doubles as the left pane's title; a header `hit_header` click sorts), `Tree<DirEntry>` with lazy `set_children` and `flatten_visible` (added for the explorer: drop nodes such as dotfiles without opening folders; `flatten_filtered` is used for find and changed-only), and `SplitView` (collapses under 100 columns; `pct_for_col` drives the divider drag). The tree keeps its own order (`sort_with` a no-op comparator) because the explorer sorts entries with `explorer_data::sort_entries` before they reach the tree.
