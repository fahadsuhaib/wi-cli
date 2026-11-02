# wicli: live dashboard view (plan, from the reference screenshot)

> **PARKED (2026-11-03).** The user wants the screenshot only as a style reference for building TUIs,
> not monitoring. The file explorer in PLAN-EXPLORER.md uses its look. Kept for later.

Reference: a screenshot of a distributed-build monitor (title `mathlib-unlocking · result pending ·
lean-build/v6 / mathlib-oct31-unlocking`). Written 2026-11-03. This plan turns that look into a `wi`
feature. It extends PLAN-OVERVIEW.md items 1.1 (Overview tab), 1.2 (progress and timers) and 4.2
(resource meter). Nothing here is built yet.

## 1. What the screenshot contains

| # | Element | What it shows |
| --- | --- | --- |
| 1 | Title bar | Full-width inverted bar: job name, status (`result pending`), a long identifier |
| 2 | Key hints | One dim line: arrows move/fold, PgUp/PgDn switch workers, j/k scroll code, `a` follow, `q` back |
| 3 | Status line | A `LIVE` badge, then `6892/9031 · 76% · Modules finished` (green): overall progress |
| 4 | Left pane `BRANCHES` | A foldable tree (`▾ root waiting 0/4`, children `branch 0 worker1`...) with an `ELAPSED` column, selected row highlighted, footer `2/6 · wrap · fold` |
| 5 | Right pane header | The selected node (`Build modules`, `0/4`) and its source (`LeanBuild/Workflow.lean:28`) |
| 6 | Code excerpt | Three lines of source with line numbers and a `>` marker on the current line, highlighted |
| 7 | Workers grid | `Workers · 4 total · 1/1 · PgUp/PgDn`: cards side by side (3 per row, wraps), each with a header (`worker1 · branch 0:2/0:0`), its source location, a code excerpt and a status line (`Lake jobs 14 running/queued · slots 7`) |
| 8 | Per-worker metrics | `Node` hostname, `Host 8 logical CPUs · 30.7 GiB RAM`, then bars for CPU, Mem and Disk with the value after them, and sparklines with the current rate for Net RX/TX and I/O R/W |
| 9 | Colour meaning | Cyan headers, blue selection, green values, bars that go green, yellow, orange, red with load, dim labels |

Design ideas worth keeping: dense but aligned columns, one colour language (green is healthy, red is
hot), sparklines for trends, a tree on the left and detail on the right, and every key on screen.

## 2. What it would be in wi

A **Dashboard** tab (menu action, e.g. letter `j`; `Esc` or `Q` leaves it), with the same anatomy,
fed by data wi already has or can fetch on demand. Three sources, in the order I'd build them:

1. **Sessions** (agents and tabs). The tree is workspaces, then tabs, then the agent in each tab.
   Per row: state (`*` printing, `!` quiet, waiting for you, finished), elapsed time since it
   started or last printed, and the last line of output. The detail pane shows that tab's recent
   output (the "source/code" area of the screenshot) and, if it is an agent, its command and folder.
2. **Hosts** (this Mac or Linux box and each SSH box in the config). One card per host in the
   grid: node name, CPU count and RAM, bars for CPU, memory and disk, sparklines for network
   receive/send and disk read/write.
3. **Jobs** (later). Long-running commands recognised from output: progress such as `45%`,
   `Step 3/10`, `6892/9031`, cargo's `(n/m)`, with a progress line like the one in row 3.

## 3. Layout (80-160 columns)

```
 wi dashboard · 3 sessions · 2 hosts                                          <- title bar (inverted)
 up/down: select · left/right: fold · PgUp/PgDn: cards · Enter: open tab · q: back
 LIVE   build tab 1: 6892/9031 · 76% · compiling                                <- status line
 SESSIONS                  ELAPSED │ build (tab 2)
 v web-app                  1h02m │ cargo build --release   folder ~/work/web-app
   * build        running   14m55s │ > Compiling serde v1.0
   ! claude       quiet      3m10s │   Compiling tokio v1.54
   . tests        idle       2h01m │ HOSTS · 2 · PgUp/PgDn
 v api (box: dev-ab12)             │ mac-studio            dev-ab12
   * claude       running   42m12s │ CPU  [||||||||..]  71%   [|||.......]  28%
                                   │ Mem  [|||||.....]  8.2G  [||||||....] 11G
 3/7 · up/down wrap · left/right   │ Net  _.-^^-._  4.1M/s    __-_.  120K/s
```

Narrow terminals (under 100 columns): the tree and detail stack, the cards become one column.

## 4. How it works (all local, cheap)

- **No new daemons, no watchers.** The dashboard has a refresh tick only while it is the visible
  view (1 s for sessions, 2 s for hosts); leaving it stops every timer. Idle wi stays at zero wakeups.
- **Session data is free:** tab state, timers and last output already live in wi (`Tab`, the vt100 screen).
- **Host data is one small command per refresh**, over the existing SSH connection (no extra login,
  nothing left on the box): on Linux a single `sh` snippet reading `/proc/stat`, `/proc/meminfo`,
  `df`, `/proc/net/dev` and `/proc/diskstats` twice, one second apart, printing numbers only; on
  macOS locally `top -l 1`, `vm_stat`, `netstat -ib`, `iostat`. Rates are computed from deltas.
  Output is capped and parsed by pure functions (unit tested with captured samples).
- **Widgets** (new, reusable): `Bar` (value, max, thresholds), `Spark` (ring buffer of the last 60
  samples, 8-level blocks `_.,-~^`), `Tree` (fold, select, wrap), `Cards` (grid with paging),
  `ProgressLine`. They draw with ratatui and take every colour from the theme.
- **Theme roles to add** (dark and light, with contrast tests): bar fill good/warn/hot/critical,
  bar empty, spark line, selection, live badge, title bar. Green/yellow/orange/red mirror the screenshot.
- **Keys and mouse** (plain letters only): arrows select and fold, `Enter` jumps to that tab,
  PgUp/PgDn page the cards, `J`/`K` scroll the detail output, `A` follows the live tab, `Q` or `Esc`
  leaves. Click selects, wheel scrolls, double-click opens the tab.

## 5. Milestones

| Step | Scope | Done when |
| --- | --- | --- |
| D1 | Widgets (Bar, Spark, Tree, Cards, ProgressLine) and theme roles | Unit tests for geometry, thresholds, ring buffer, folding; both themes pass contrast tests |
| D2 | Dashboard tab with the Sessions source | Tree of tabs with live state and elapsed; detail pane shows recent output; keys and mouse; pty test with three tabs in different states |
| D3 | Hosts source, local first | CPU, memory, disk and network for this machine, sampled only while visible; parser tests on captured samples (macOS and Linux) |
| D4 | Hosts over SSH | One card per configured box through the existing connection; test against the Docker box |
| D5 | Jobs source | Progress patterns shown as a status line and a per-tab bar; patterns configurable |
| D6 | Polish | Narrow-width layout, light theme check, `LIVE` badge, README section |

Each step ends in a working, tested binary, as before. D1 to D3 is about two to three days of work
with agents running in parallel (widgets, hosts samplers and the tab separately).

## 6. Open questions

1. **What should it monitor?** My reading is your agent sessions and your boxes. If the goal is
   instead to reproduce that exact build monitor inside wi (fed by a build system you run), I need
   to know where its data comes from; the screenshot looks like a Lean/Mathlib cloud build tool.
2. **Which view first**: sessions (D2) or host metrics (D3/D4)?
3. **Where should it live**: its own tab (my proposal), a pinned side pane, or both?
4. **Refresh rates** (1 s and 2 s) and sparkline history (60 samples) are my defaults.
