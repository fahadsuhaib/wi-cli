# wicli: mermaid diagrams in markdown previews

Written 2026-11-09. Goal: a ```` ```mermaid ```` block in a markdown file shows as a diagram in the preview
instead of source text, so docs and agent-written plans can be read without leaving wi. Same workstream and
branch as the git diff work (`unit/git-diff`); built after the diff viewer lands, as its own commits.
Read-only, pure Rust, no Node, no browser, no network.

## 1. Where it hooks in

`src/preview/markdown/parse.rs` already turns fenced code into `Block::Code { lang, text }`. Rendering
happens in `src/preview/markdown/mod.rs`. The change: when `lang == "mermaid"`, render the diagram to lines of
text (box-drawing characters) and draw those instead of the highlighted code. Nothing else in the preview,
scroll, find, wrap or auto-refresh code changes, because the output is ordinary styled lines.

## 2. Output form: text diagram, not an image

Text works in every terminal, locally and over SSH, inside a pinned pane, with the theme colours, and needs no
new terminal protocol. Real images (kitty/iTerm graphics plus an SVG rasterizer) are **out of scope**; they
fail in Terminal.app and GNOME Terminal and add a few MB. Revisit only if asked.

## 3. Renderer: spike first, then choose

Candidates (pure Rust, from a quick search on 2026-11-06; **verify each**): `merman` (`ascii` feature),
`figurehead` (flowchart, sequence, class, git graph), `mermaid-text`. SVG-only crates (`mermaid-rs-renderer`,
`ariel-rs`) do not fit.

**Spike (half a day, throwaway, in the scratchpad, not in the repo):** run 12 real diagrams through each
candidate: flowchart LR/TD with subgraphs and long labels, sequence, class, state, ER, gantt, pie, git graph,
a wide 30-node flowchart, one with unicode/emoji labels, one with syntax errors. Score on: diagram types
covered, readability at 80 and 120 columns, wrap/overflow behaviour, panics on bad input, compile time and
binary size added (release build, check against the current ~MB size), maintenance (last release, issues,
licence compatible with MIT), dependency count. Pick one crate; write the result into section 10.
If none is good enough, fallback: write a small flowchart-only renderer ourselves (nodes, edges, labels, TD/LR
layers), and show the other types as styled source with a note.

## 4. Behaviour

- Rendered inside the markdown flow as a bordered block titled `mermaid · flowchart` (type detected from the
  first line), dim border, theme text colours, node/edge text from the theme palette.
- Wider than the pane: do not wrap the diagram; allow horizontal scroll inside the block with the existing
  markdown scroll keys when the diagram has focus, and show `→ wider than pane` in the title. Alternative
  considered: re-render at the pane width (LR becomes TD); try this if the chosen crate supports a max width.
- Keys (markdown preview, existing letter rules; check for conflicts in `src/preview/`): `D` toggles
  diagram/source for **all** mermaid blocks in the file (state kept while the preview is open). Source view
  is today's code block. Check `D` is free in the markdown preview; otherwise use the next free letter.
- Failure or unsupported type: show the source block unchanged plus one dim line `mermaid: not rendered
  (<short reason>)`. Never panic, never block the UI: the renderer runs under `catch_unwind`, with a size cap
  (reject blocks over 20 KB or 400 nodes) and a time budget, off the UI loop if it can exceed ~20 ms.
- Cache the rendered lines per (block text hash, width) so scrolling and auto-refresh do not re-render.
- Applies to local and remote files alike (the file text is already fetched with bounded reads). Nothing
  leaves the machine.
- `.mmd` / `.mermaid` files: open in the same renderer as a whole-file preview (cheap once the block renderer
  exists; phase 2).
- Also in the diff viewer? No.

## 5. Edge cases

Empty block; unknown first line; init directives (`%%{init: ...}%%`) ignored safely; comments (`%%`);
very long labels (truncate with an ellipsis at a node-width cap, full text in source view); `<br/>` and
markdown labels; unicode width (CJK, emoji) using `unicode-width`; tabs; CRLF; several blocks in one file;
a mermaid block inside a list or quote; nested code fences; theme changes (re-render on theme switch, cache
keyed by theme); light themes; narrow panes under 40 columns (show source).

## 6. Phases and tests

| Phase | Work | Tests |
| --- | --- | --- |
| 0 | Spike and crate choice (section 3), result written to section 10 | spike notes only |
| 1 | Hook `lang == "mermaid"`, render flowcharts and the types the chosen crate handles, fallback note, `D` toggle, cache, caps, `catch_unwind` | unit: type detection, caps, fallback on error/panic/oversize, cache key; golden-text tests for 6 diagrams at 80 and 120 columns; pty: `dev/pty_mermaid.py` opens a markdown file with a flowchart and a broken block, checks box characters, the note, `D` toggle, scroll |
| 2 | `.mmd` files, horizontal scroll or width-aware re-layout, remaining diagram types, theme colouring per node kind | pty: `.mmd` open, wide diagram scroll, theme switch re-render |
| later, only if asked | image rendering on kitty/iTerm/Ghostty with text fallback | separate plan |

Run only affected tests while developing (markdown preview pty scripts, new mermaid tests, clippy, unit
tests); full `dev/run_tests.sh` before release.

## 7. Dependencies and size

One new crate (chosen in phase 0), added with default features off. Record release binary size before and
after in the commit message; budget: under +1.5 MB, and no C build or system library, so the macOS and Linux
release matrix is unchanged. If the crate is heavier, prefer the own flowchart renderer.

## 8. Docs to update when built

`CLAUDE.md` Features paragraph (previews: mermaid blocks), `docs/KEYBOARD.md` (markdown preview keys),
`README.md` feature list and "All features", in-app key hints for the markdown preview.

## 9. Open questions

1. Horizontal scroll inside the block, or re-layout to the pane width? (Proposal: re-layout if the crate allows
   it, else scroll.)
2. Which diagram types are required for phase 1? (Proposal: flowchart and sequence; the rest show source.)
3. Is a spike result of "own renderer needed" acceptable (more code to maintain)?
4. Colour per node kind or monochrome? (Proposal: monochrome in phase 1.)

## 10. Spike result

Run 2026-11-09 (throwaway project in the scratchpad, 12 cases from section 3 plus 10 edge cases, release build).

| Candidate | Version, licence | Result |
| --- | --- | --- |
| `mermaid-text` | 0.58.0 (2026-11-05), MIT, rust 1.92, one dependency (`ascii-dag`, MIT or Apache-2.0) | **Chosen.** Flowchart/graph (all shapes, subgraphs, edge labels), sequence, class, state, ER, pie, gantt, git graph, journey, timeline, mindmap, quadrant, requirement, sankey, xychart, block, packet, architecture. Pure text; `render_with_width` compacts gaps toward a column budget. No panic on the 22 cases. |
| `merman` | 0.9.3 stable (0.10 is alpha), MIT or Apache-2.0, rust 1.95 | Rejected: spike binary 12.5 MB, 34 s compile, ELK carries EPL-2.0 notices, and with a width budget a wide flowchart came back as a semantic summary instead of a drawing. |
| `figurehead` | 0.4.3 (2025-12-30), MIT | Rejected: no subgraphs (parse error on case 1), panics on a sequence diagram (slice index), only flowchart/class/sequence. |

Chosen: `mermaid-text = "0.58.0"`. Release binary of wi: 11,301,808 bytes before, 12,766,144 after, **+1,464,336 bytes (+1.4 MiB)**, inside the 1.5 MB budget. No C code. Render time 0 to 16 ms for normal diagrams, 163 ms for a 500-node chain (the cap is 400 nodes).

Decisions taken in phase 1:
- Types: everything the crate draws (wider than the proposal in question 2); types it rejects show source with the reason.
- Width (question 1): `render_with_width` only compacts gaps. A wide `graph`/`flowchart LR|RL` is retried top-down (a 30-node chain goes from 372 to 66 columns) and the title says "laid out top-down". Still wider than the pane: source plus `mermaid: not rendered (<n> columns, pane has <m>)`. Horizontal scroll is phase 2.
- Colour (question 4): monochrome, theme text colour, dim border; `style`/`classDef` colours are not used.
- Key `D`: free in the markdown preview (it exists only in the explorer, where it toggles diff/content). It does nothing in the raw source view (`M`).

Known gaps of the crate:
- `%%` lines (comments, `%%{init}%%`) are drawn as a node, so wi strips them first; YAML front matter is stripped too.
- `pie` with a `title` line inside the block is rejected; the source is shown.
- Markdown labels (`**bold**`) show literally; `<br/>` works.
- Edge routing can be ugly with back edges, sequence `loop` frames are rough, `A ---->|No| E` is mis-parsed, and some broken input parses "successfully" into garbage boxes (case 12); wi cannot tell.
- A stack overflow would not be caught by `catch_unwind`; the 20 KB and 400 node caps bound the depth in practice.
- Rendering runs on the UI thread (cached by text hash, width and theme); a block near the caps may stall one redraw by about 0.1 s.
