//! Rendering: header, status bar, shell screen, menu overlay.

pub mod explorer;
pub mod search;
pub mod themepick;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use crate::theme::{self, theme};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

/// Everything the menu can do. Every user-visible action goes through this
/// enum so extensions can contribute entries later.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Preview,
    Git,
    Download,
    /// Label every file path and URL on screen so one keypress opens it.
    OpenLink,
    NewTab,
    /// Prompt for a folder and open a shell tab there.
    NewTabIn,
    /// Prompt for a folder and move this idle shell tab to it.
    ChangeDir,
    NextTab,
    PrevTab,
    Mouse,
    /// Open the theme picker (auto or a named theme, previewed live).
    Theme,
    /// Check for a newer wi release and install it (local binary only, after confirmations).
    Update,
    /// Name the active tab (an empty name goes back to the automatic title).
    RenameTab,
    /// Restart wi in place (the same tabs come back).
    Restart,
    /// Hands-free dictation on or off: each spoken phrase is typed into the focused pane.
    Dictation,
    Agent,
    CloseTab,
    Reconnect,
    Scroll,
    /// Move the keyboard between the shell and the side preview.
    SwitchPane,
    /// Close the side preview (or unpin the pinned tab) and give the shell the full width back.
    ClosePreview,
    /// Pin the active tab to a pane on the right (or left) beside the next tab.
    PinRight,
    PinLeft,
    /// Move the pinned pane to the other side.
    SwapSides,
    /// Back to one pane; a pinned tab stays a normal tab.
    Unsplit,
    /// Search text in the files under the active folder (shows the last results first).
    SearchText,
    /// Fuzzy find a file by name.
    FindFile,
    /// The full-screen file explorer for the active tab's folder.
    Explorer,
    /// Find text in the active shell tab's scrollback.
    FindOutput,
    Info,
    /// The files recently shown in a preview, to open one again.
    RecentFiles,
    Quit,
    EndAndQuit,
}

pub struct MenuItem {
    pub label: &'static str,
    pub key: char,
    pub action: Action,
    /// Only shown for remote workspaces.
    pub remote_only: bool,
    /// Header the entry is listed under (one of `SECTIONS`; empty means `OTHER`).
    pub section: &'static str,
    /// Extra words the filter matches besides the label.
    pub keywords: &'static [&'static str],
}

/// Menu entries that apply to this kind of workspace.
pub fn menu_items(remote: bool) -> Vec<&'static MenuItem> {
    MENU.iter().filter(|m| remote || !m.remote_only).collect()
}

/// On a remote box with tmux, "Quit" only detaches: the shells keep running there.
pub const DETACH_ITEM: MenuItem = MenuItem { label: "Detach (keep sessions)", key: 'q', action: Action::Quit, remote_only: true, section: "SESSION", keywords: &["quit", "exit", "disconnect", "leave"] };
pub const END_ITEM: MenuItem = MenuItem { label: "End sessions and quit", key: 'e', action: Action::EndAndQuit, remote_only: true, section: "SESSION", keywords: &["quit", "exit", "kill", "close"] };

/// Menu entries for the theme item: `auto (dark)`, `auto (light)`, then one per named theme. The
/// label shows what is set; the item opens the picker.
static THEME_ITEMS: std::sync::LazyLock<Vec<MenuItem>> = std::sync::LazyLock::new(|| {
    let mk = |label: String| MenuItem { label: Box::leak(label.into_boxed_str()), key: 'y', action: Action::Theme, remote_only: false, section: "VIEW", keywords: &["dark", "light", "auto", "colour", "color", "monokai", "solarized", "picker"] };
    let mut v = vec![mk("Theme… auto (dark)".into()), mk("Theme… auto (light)".into())];
    v.extend(theme::ALL.iter().map(|t| mk(format!("Theme… {}", t.label))));
    v
});

/// The theme entry showing the current mode (and, for auto, what it picked).
pub fn theme_item(mode: theme::Mode, kind: theme::Kind) -> &'static MenuItem {
    &THEME_ITEMS[match mode {
        theme::Mode::Auto => usize::from(kind == theme::Kind::Light),
        theme::Mode::Named(i) => 2 + i,
    }]
}

/// Swap the menu's theme entry for the one that shows `mode`.
pub fn set_theme_item(items: &mut [&'static MenuItem], mode: theme::Mode, kind: theme::Kind) {
    if let Some(slot) = items.iter_mut().find(|m| m.action == Action::Theme) {
        *slot = theme_item(mode, kind);
    }
}

static TMUX_MENU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tell the menu that shells live in tmux (set once after connecting).
pub fn set_tmux_menu(on: bool) {
    TMUX_MENU.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The theme setting the menu shows (0 auto, else 1 + index into `theme::ALL`).
static THEME_MODE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Tell the menu which theme mode is set (its entry shows it).
pub fn set_theme_mode(m: theme::Mode) {
    let v = match m { theme::Mode::Auto => 0, theme::Mode::Named(i) => 1 + i };
    THEME_MODE.store(v, std::sync::atomic::Ordering::Relaxed);
}

fn theme_mode() -> theme::Mode {
    match THEME_MODE.load(std::sync::atomic::Ordering::Relaxed) { 0 => theme::Mode::Auto, v => theme::Mode::Named((v - 1).min(theme::ALL.len() - 1)) }
}

/// Menu entries for a workspace, with the Quit entry replaced by Detach / End for tmux boxes.
pub fn menu_items_for(remote: bool, tmux: bool) -> Vec<&'static MenuItem> {
    let mut items = menu_items(remote);
    set_theme_item(&mut items, theme_mode(), theme::kind());
    if tmux && let Some(i) = items.iter().position(|m| m.action == Action::Quit) {
        items[i] = &DETACH_ITEM;
        items.insert(i + 1, &END_ITEM);
    }
    items
}

/// Hotkey of a menu entry that has none.
pub const NO_KEY: char = '\0';

const fn mi(label: &'static str, key: char, action: Action, remote_only: bool, section: &'static str, keywords: &'static [&'static str]) -> MenuItem {
    MenuItem { label, key, action, remote_only, section, keywords }
}

pub const MENU: &[MenuItem] = &[
    mi("Preview a file", 'p', Action::Preview, false, "FILES", &["open", "view", "show", "read", "markdown"]),
    mi("Recent files", NO_KEY, Action::RecentFiles, false, "FILES", &["history", "previous", "open", "again", "last", "path"]),
    mi("Find file", 'k', Action::FindFile, false, "FILES", &["open", "fuzzy", "name", "locate"]),
    mi("Explorer", 'x', Action::Explorer, false, "FILES", &["tree", "browse", "folders", "files"]),
    mi("Git changes", 'g', Action::Git, false, "FILES", &["diff", "status", "commit", "branch", "modified"]),
    mi("Open path on screen", 'o', Action::OpenLink, false, "FILES", &["link", "url", "click", "hint"]),
    mi("Search in files", 'l', Action::SearchText, false, "SEARCH", &["grep", "find", "text", "ripgrep"]),
    mi("Find in output", 'u', Action::FindOutput, false, "SEARCH", &["search", "scrollback", "text", "history"]),
    mi("New shell tab", 'n', Action::NewTab, false, "TABS", &["open", "terminal", "create"]),
    mi("New tab in folder", 'f', Action::NewTabIn, false, "TABS", &["cd", "directory", "project", "open"]),
    mi("Change folder of this tab", 'h', Action::ChangeDir, false, "TABS", &["cd", "directory", "project"]),
    mi("Next tab", 't', Action::NextTab, false, "TABS", &["switch", "right", "forward"]),
    mi("Previous tab", 'b', Action::PrevTab, false, "TABS", &["switch", "left", "back"]),
    mi("Close tab", 'c', Action::CloseTab, false, "TABS", &["kill", "end", "remove"]),
    mi("Rename tab", NO_KEY, Action::RenameTab, false, "TABS", &["title", "name", "label", "call"]),
    mi("Switch pane", 'w', Action::SwitchPane, false, "TABS", &["focus", "split", "side"]),
    mi("Close preview", 'v', Action::ClosePreview, false, "TABS", &["side", "pane", "hide"]),
    // No hotkey (NO_KEY): the letters are used up; these are found with Space and a few letters.
    mi("Pin tab right", NO_KEY, Action::PinRight, false, "TABS", &["pane", "split", "side", "beside", "pin"]),
    mi("Pin tab left", NO_KEY, Action::PinLeft, false, "TABS", &["pane", "split", "side", "beside", "pin"]),
    mi("Swap pane sides", NO_KEY, Action::SwapSides, false, "TABS", &["pane", "split", "flip", "left", "right", "move"]),
    mi("Unsplit (unpin)", NO_KEY, Action::Unsplit, false, "TABS", &["pane", "split", "single", "pin", "close"]),
    mi("Scroll", 's', Action::Scroll, false, "VIEW", &["history", "back", "up", "scrollback"]),
    mi("Theme…", 'y', Action::Theme, false, "VIEW", &["dark", "light", "auto", "colour", "color", "monokai", "solarized", "picker"]),
    mi("Mouse on/off", 'm', Action::Mouse, false, "VIEW", &["select", "copy", "click"]),
    mi("Start / change agent", 'a', Action::Agent, false, "AGENT", &["claude", "codex", "ai", "assistant"]),
    mi("Info", 'i', Action::Info, false, "SESSION", &["status", "folder", "box", "about"]),
    mi("Reconnect", 'r', Action::Reconnect, true, "SESSION", &["connection", "ssh", "retry"]),
    mi("Download", 'd', Action::Download, true, "SESSION", &["fetch", "copy", "save", "file"]),
    mi("Dictation (voice) on/off", NO_KEY, Action::Dictation, false, "AGENT", &["dictate", "voice", "speak", "talk", "microphone", "mic", "listen", "speech"]),
    mi("Update wi", NO_KEY, Action::Update, false, "SESSION", &["upgrade", "new", "version", "release", "download", "latest"]),
    mi("Restart wi", NO_KEY, Action::Restart, false, "SESSION", &["reload", "relaunch", "again", "new version"]),
    mi("Quit", 'q', Action::Quit, false, "SESSION", &["exit", "detach", "disconnect", "leave"]),
];

/// Section headers in display order; entries without a known section go under `OTHER`.
pub const SECTIONS: &[&str] = &["FILES", "SEARCH", "TABS", "VIEW", "AGENT", "SESSION"];
const OTHER: &str = "OTHER";

/// The menu's own state: selection (an index into the entries as listed), filter text and scroll.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct MenuState {
    pub sel: usize,
    pub filter: String,
    /// Typing edits the filter instead of running hotkeys.
    pub filtering: bool,
    /// First list row shown (headers included).
    pub top: usize,
}

/// One line of the menu list: a section header or an entry (index into the item list).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    Header(&'static str),
    Item(usize),
}

/// What to list: grouped under headers when the filter is empty, otherwise a flat list of the
/// entries that match, best match first.
pub fn menu_rows(items: &[&MenuItem], filter: &str) -> Vec<Row> {
    if filter.trim().is_empty() {
        let mut rows = Vec::new();
        for sec in SECTIONS.iter().copied().chain(std::iter::once(OTHER)) {
            let mine: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, m)| if sec == OTHER { !SECTIONS.contains(&m.section) } else { m.section == sec })
                .map(|(i, _)| i)
                .collect();
            if !mine.is_empty() {
                rows.push(Row::Header(sec));
                rows.extend(mine.into_iter().map(Row::Item));
            }
        }
        return rows;
    }
    let mut hits: Vec<(i32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let by_label = crate::search::fuzzy::score(filter, m.label).map(|(s, _)| s + 200);
            let by_words = crate::search::fuzzy::score(filter, &m.keywords.join(" ")).map(|(s, _)| s);
            by_label.into_iter().chain(by_words).max().map(|s| (s, i))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, i)| Row::Item(i)).collect()
}

/// The selectable entries (item indexes) of a row list, in order: headers are skipped.
pub fn menu_entries(rows: &[Row]) -> Vec<usize> {
    rows.iter().filter_map(|r| if let Row::Item(i) = r { Some(*i) } else { None }).collect()
}

/// First visible row so that rows `lo..=sel` are on screen in a window of `h` slots, where a slot
/// at either edge is spent on a "more" marker whenever rows are cut off there.
pub fn scroll_top(total: usize, h: usize, lo: usize, sel: usize, top: usize) -> usize {
    if total <= h || h == 0 {
        return 0;
    }
    let mut top = top.min(total - h);
    if h < 4 {
        if sel < top {
            top = sel;
        } else if sel >= top + h {
            top = sel + 1 - h;
        }
        return top.min(total - h);
    }
    if lo < top + usize::from(top > 0) {
        top = lo.saturating_sub(1);
    }
    if top + h < total && sel + 2 > top + h {
        top = sel + 2 - h;
    }
    top.min(total - h)
}

/// What one line inside the menu box shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LRow {
    Header(&'static str),
    Item(usize),
    MoreAbove,
    MoreBelow,
    NoMatch,
}

pub struct MenuLayout {
    pub rect: Rect,
    /// Screen row of the filter input.
    pub filter_y: u16,
    /// Lines of the list, each with its screen row.
    pub rows: Vec<(u16, LRow)>,
    /// Height of the list area.
    pub list_h: usize,
    /// The first list row shown (store it back so scrolling is remembered).
    pub top: usize,
    /// Selected position among the entries, clamped.
    pub sel: usize,
}

/// The one place that decides where every menu line goes: drawing and mouse hit-testing both use it.
pub fn menu_layout(main: Rect, items: &[&MenuItem], st: &MenuState) -> MenuLayout {
    let rows = menu_rows(items, &st.filter);
    let entries = menu_entries(&rows);
    let sel = st.sel.min(entries.len().saturating_sub(1));
    let longest = items.iter().map(|m| m.label.chars().count()).max().unwrap_or(0);
    let w = if main.width >= 100 { (longest + 6).max(30) } else { 30 };
    let w = (w as u16).min(main.width);
    let wanted = rows.len().max(1);
    let h = ((wanted + 3) as u16).min(main.height);
    let rect = Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h);
    let list_h = (h as usize).saturating_sub(3);
    let mut out = Vec::new();
    let mut top = 0;
    if entries.is_empty() {
        if list_h > 0 {
            out.push((rect.y + 2, LRow::NoMatch));
        }
    } else {
        let sel_row = rows.iter().position(|r| *r == Row::Item(entries[sel])).unwrap_or(0);
        let lo = if sel_row > 0 && matches!(rows[sel_row - 1], Row::Header(_)) { sel_row - 1 } else { sel_row };
        top = scroll_top(rows.len(), list_h, lo, sel_row, st.top);
        for s in 0..list_h.min(rows.len()) {
            let r = top + s;
            let y = rect.y + 2 + s as u16;
            let line = if s == 0 && top > 0 && list_h >= 4 {
                LRow::MoreAbove
            } else if s == list_h - 1 && top + list_h < rows.len() && list_h >= 4 {
                LRow::MoreBelow
            } else {
                match rows[r] {
                    Row::Header(h) => LRow::Header(h),
                    Row::Item(i) => LRow::Item(i),
                }
            };
            out.push((y, line));
        }
    }
    MenuLayout { rect, filter_y: rect.y + 1, rows: out, list_h, top, sel }
}

/// What a click at a screen position means for the menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuHit {
    Outside,
    /// On the box but not on anything clickable.
    Inert,
    Filter,
    /// An entry (index into the item list).
    Item(usize),
}

pub fn menu_hit(l: &MenuLayout, col: u16, row: u16) -> MenuHit {
    let r = l.rect;
    if col < r.x || col >= r.x + r.width || row < r.y || row >= r.y + r.height {
        return MenuHit::Outside;
    }
    if col == r.x || col + 1 == r.x + r.width {
        return MenuHit::Inert;
    }
    if row == l.filter_y {
        return MenuHit::Filter;
    }
    match l.rows.iter().find(|(y, _)| *y == row) {
        Some((_, LRow::Item(i))) => MenuHit::Item(*i),
        _ => MenuHit::Inert,
    }
}

pub use crate::ask::Ask;

/// What the prompt is asking for.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptKind {
    /// A file to preview.
    #[default]
    File,
    /// A folder for a new shell tab.
    NewFolder,
    /// A folder to move the active (idle) tab to.
    ChangeFolder,
}

impl PromptKind {
    pub fn is_folder(self) -> bool {
        self != PromptKind::File
    }
    pub fn label(self) -> &'static str {
        match self {
            PromptKind::File => " open file: ",
            PromptKind::NewFolder => " new tab in folder: ",
            PromptKind::ChangeFolder => " change folder to: ",
        }
    }
}

impl Prompt {
    /// The folder prompt: starts as `start` (the active tab's folder, with a trailing slash) and
    /// lists `recent` folders until the input is edited.
    pub fn folder(kind: PromptKind, start: &str, recent: Vec<String>) -> Prompt {
        let total = recent.len();
        let input = if start.ends_with('/') { start.to_string() } else { format!("{start}/") };
        let cur = input.chars().count();
        Prompt { input, cur, shown: recent, total, kind, recent: total > 0, ..Prompt::default() }
    }

    /// Replace the whole input (completion, a picked folder); the cursor goes to the end.
    pub fn set_input(&mut self, s: String) {
        self.cur = s.chars().count();
        self.input = s;
    }

    /// Insert typed or pasted text at the cursor (newlines and other control characters dropped).
    pub fn insert(&mut self, s: &str) {
        crate::lineedit::insert(&mut self.input, &mut self.cur, s);
    }
}

/// State of the "open a file" / folder prompt.
#[derive(Default, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    /// `shown` lists recent folders (the input has not been edited yet) rather than completions.
    pub recent: bool,
    /// Highlighted recent folder (arrow keys).
    pub sel: Option<usize>,
    pub input: String,
    /// Cursor position in `input`, in characters.
    pub cur: usize,
    /// Completion candidates to show (directories end in `/`).
    pub shown: Vec<String>,
    /// Total number of matches, which may exceed what is shown.
    pub total: usize,
    pub error: Option<String>,
}

/// Text of one tab in the strip, e.g. ` 2 build * `.
pub fn tab_label(i: usize, title: &str, state: crate::tab::TabState) -> String {
    use crate::tab::TabState;
    let mark = match state {
        TabState::Streaming => " *",
        TabState::Quiet => " !",
        TabState::Idle => "",
    };
    format!(" {} {}{} ", i + 1, title, mark)
}

/// Which tab is under column `x` of the strip.
pub fn tab_at(tabs: &[(String, bool, crate::tab::TabState)], x: u16) -> Option<usize> {
    let mut start = 0usize;
    for (i, (title, _, state)) in tabs.iter().enumerate() {
        let w = tab_label(i, title, *state).chars().count();
        if (x as usize) < start + w {
            return Some(i);
        }
        start += w;
    }
    None
}

/// Where the open-file prompt (candidates plus input line) is drawn.
pub fn prompt_rect(main: Rect, p: &Prompt) -> Rect {
    let more = p.total.saturating_sub(p.shown.len());
    let rows = p.shown.len() + usize::from(more > 0) + usize::from(p.error.is_some()) + 1;
    let h = (rows as u16).min(main.height);
    Rect::new(main.x, main.y + main.height - h, main.width, h)
}

/// A yes/no question shown before anything is opened or downloaded.
#[derive(Clone)]
pub struct Confirm {
    pub title: String,
    pub lines: Vec<String>,
    /// What "yes" does, e.g. "Open".
    pub yes: String,
}

/// A label drawn over a link on screen (position relative to the whole terminal screen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub label: String,
    pub row: u16,
    pub col: u16,
    pub width: u16,
}

/// Make text safe to show in a dialog: control characters and the bidirectional overrides that
/// can make a name read differently from what it is are replaced.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' => '·',
            c if c.is_control() => '·',
            c => c,
        })
        .collect()
}

/// Short labels for `n` links: single letters from the home row first, then pairs.
pub fn hint_labels(n: usize) -> Vec<String> {
    const KEYS: &str = "asdfghjklqwertyuiopzxcvbnm";
    let keys: Vec<char> = KEYS.chars().collect();
    if n <= keys.len() {
        return keys.iter().take(n).map(|c| c.to_string()).collect();
    }
    let mut out = Vec::with_capacity(n);
    'outer: for a in &keys {
        for b in &keys {
            out.push(format!("{a}{b}"));
            if out.len() == n {
                break 'outer;
            }
        }
    }
    out
}

/// What the pane beside the active tab shows.
#[derive(Clone, Copy)]
pub enum Partner<'a> {
    Screen(&'a vt100::Screen),
    Preview(&'a crate::preview::Preview),
}

/// A pinned tab is on screen beside the active one.
#[derive(Clone, Copy)]
pub struct PinView {
    /// The side the pinned tab is on.
    pub side: crate::tab::Side,
    /// The active tab is the pinned one (the pane beside it is the other tab).
    pub active_is_pinned: bool,
}

pub struct View<'a> {
    /// With `pin`: the tab in the other pane (the active tab is drawn as usual).
    pub partner: Option<Partner<'a>>,
    pub pin: Option<PinView>,
    pub confirm: Option<&'a Confirm>,
    pub hints: Option<(&'a [Hint], &'a str)>,
    /// Selected cells and the pane they are in, relative to that pane (or the preview body).
    pub selection: Option<(crate::tab::Pane, crate::select::Sel)>,
    /// The preview attached beside the shell, with the side/percentage setting that places it.
    pub side: Option<&'a crate::preview::Preview>,
    pub split: (crate::tab::Side, u16),
    pub focus_side: bool,
    pub ask: Option<&'a Ask>,
    pub pick: Option<&'a crate::pick::Pick>,
    pub rename: Option<&'a crate::rename::Rename>,
    /// The theme picker, when open.
    pub themes: Option<&'a themepick::ThemePick>,
    pub preview: Option<&'a crate::preview::Preview>,
    pub prompt: Option<&'a Prompt>,
    pub screen: &'a vt100::Screen,
    pub name: &'a str,
    pub detail: &'a str,
    pub remote: bool,
    pub menu_label: &'a str,
    /// (title, active, state) per tab; the strip shows only with 2+ tabs.
    pub tabs: &'a [(String, bool, crate::tab::TabState)],
    pub message: Option<&'a str>,
    /// Some(selected index) when the menu is open.
    pub menu: Option<MenuState>,
    /// Some((offset, total)) while scrolling back through history.
    pub scroll: Option<(usize, usize)>,
    /// A search screen (prompt, results, file finder, find prompt) covering the main area.
    pub search: Option<search::SearchView<'a>>,
    /// Matches to light up in the shell pane (row, start col, end col, is current) and the
    /// status text that goes with find-in-output.
    pub find_hl: &'a [(u16, u16, u16, bool)],
    /// Replaces the status bar text when set.
    pub status: Option<&'a str>,
    /// The file explorer, drawn over the whole screen (no header, tab strip or status bar).
    pub explorer: Option<&'a crate::explorer::ExplorerState>,
    /// The dictation indicator for the right end of the status bar, while the mode is on.
    pub mic: Option<&'a str>,
}

/// Where the dictation indicator sits on the status row: (first column, width), right-aligned.
pub fn mic_span(text: &str, width: u16) -> (u16, u16) {
    let w = (text.chars().count() as u16 + 2).min(width);
    (width - w, w)
}

/// A vt100 colour as a wi colour. `default` is what the terminal's default colour becomes: the
/// canvas colour when wi paints the window, else `Reset` (the terminal's own).
fn vt_color(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => theme::remap_ansi(i).unwrap_or(Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

struct ScreenWidget<'a>(&'a vt100::Screen, Option<crate::select::Sel>);

impl Widget for ScreenWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        render_screen_cells(self.0, self.1, area, buf, theme::canvas());
    }
}

/// Draw a terminal screen. With a canvas `(background, text)`, default-coloured cells (empty
/// ones to the end of each row included) get those colours.
fn render_screen_cells(screen: &vt100::Screen, sel: Option<crate::select::Sel>, area: Rect, buf: &mut Buffer, canvas: Option<(Color, Color)>) {
    let (def_bg, def_fg) = canvas.unwrap_or((Color::Reset, Color::Reset));
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = screen.cell(row, col) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut fg = vt_color(cell.fgcolor(), def_fg);
            let bg = vt_color(cell.bgcolor(), def_bg);
            let mut m = Modifier::empty();
            if cell.bold() { m |= Modifier::BOLD; }
            if cell.dim() {
                // Faint text is blended toward the cell background when painting: the terminal's
                // own dimming works against its background, which may be the opposite theme.
                if canvas.is_some() && !cell.inverse() && matches!(fg, Color::Rgb(..)) {
                    fg = theme::blend(fg, if matches!(bg, Color::Rgb(..)) { bg } else { def_bg });
                } else {
                    m |= Modifier::DIM;
                }
            }
            if cell.italic() { m |= Modifier::ITALIC; }
            if cell.underline() { m |= Modifier::UNDERLINED; }
            let selected = sel.is_some_and(|s| crate::select::contains(s, row, col));
            if cell.inverse() != selected { m |= Modifier::REVERSED; }
            let out = &mut buf[(area.x + col, area.y + row)];
            let text = cell.contents();
            out.set_symbol(if text.is_empty() { " " } else { text });
            out.set_style(Style::default().fg(fg).bg(bg).add_modifier(m));
        }
    }
}

/// Draw a frame. When the terminal has no true colour, every theme colour is then reduced to the
/// nearest 256-colour index in one pass over the buffer.
pub fn draw(f: &mut Frame, v: &View) {
    draw_frame(f, v);
    if let Some((bg, fg)) = theme::canvas() {
        // Whatever is still the terminal's default (gaps, cleared areas, widgets that set no
        // background) becomes the theme's canvas, so the whole window is one theme.
        for cell in f.buffer_mut().content.iter_mut() {
            if cell.bg == Color::Reset { cell.bg = bg; }
            if cell.fg == Color::Reset { cell.fg = fg; }
        }
    }
    if !theme::truecolor() {
        for cell in f.buffer_mut().content.iter_mut() {
            cell.fg = theme::to_256(cell.fg);
            cell.bg = theme::to_256(cell.bg);
        }
    }
}

fn draw_frame(f: &mut Frame, v: &View) {
    if let Some(ex) = v.explorer {
        explorer::draw(f, ex);
        return;
    }
    let t = theme();
    let strip_h = if v.tabs.len() > 1 { 1 } else { 0 };
    let [header, strip, main, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(strip_h),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let bar = Style::default().bg(t.bar_bg).fg(t.bar_fg);
    let status_bar = Style::default().bg(t.status_bg).fg(t.status_fg);

    let tail = if v.remote { "  connected" } else { "" };
    let fixed = format!(" wicli  {}  ", v.name).chars().count() + tail.chars().count();
    let room = (header.width as usize).saturating_sub(fixed);
    let detail = left_trim(v.detail, room);
    f.render_widget(
        Paragraph::new(format!(" wicli  {}  {detail}{tail}", v.name)).style(bar.fg(t.header).add_modifier(Modifier::BOLD)),
        header,
    );

    if strip_h == 1 {
        let mut spans = Vec::new();
        for (i, (title, active, state)) in v.tabs.iter().enumerate() {
            use crate::tab::TabState;
            let label = tab_label(i, title, *state);
            let style = if *active {
                Style::default().bg(t.tab_active_bg).fg(t.tab_active_fg).add_modifier(Modifier::BOLD)
            } else {
                match state {
                    TabState::Streaming => Style::default().fg(t.tab_streaming),
                    TabState::Quiet => Style::default().fg(t.tab_quiet).add_modifier(Modifier::BOLD),
                    TabState::Idle => Style::default().fg(t.tab_inactive),
                }
            };
            spans.push(Span::styled(label, style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(bar), strip);
    }

    let split = match (v.pin, v.partner) {
        (Some(pin), Some(_)) => crate::tab::split_pinned(main, pin.side, v.split.1, pin.active_is_pinned),
        _ => v.side.and_then(|_| crate::tab::split_main(main, v.split.0, v.split.1)),
    };
    let sel_in = |pane| v.selection.filter(|(p, _)| *p == pane).map(|(_, s)| s);
    let mut preview_rows = 0usize;
    let mut shell_area = main;
    let mut active_prev = v.preview;
    if let Some(sp) = &split {
        let other = match (v.pin, v.partner, v.side) {
            (Some(_), Some(p), _) => Some(p),
            (_, _, Some(side)) => Some(Partner::Preview(side)),
            _ => None,
        };
        let mut other_rows = 0;
        match other {
            Some(Partner::Preview(p)) => other_rows = draw_preview(f, sp.side, p, sel_in(crate::tab::Pane::Side)),
            Some(Partner::Screen(s)) => f.render_widget(ScreenWidget(s, None), sp.side),
            None => {}
        }
        shell_area = sp.shell;
        if v.pin.is_none() {
            preview_rows = other_rows;
            active_prev = v.focus_side.then_some(v.side).flatten();
            f.render_widget(ScreenWidget(v.screen, sel_in(crate::tab::Pane::Shell)), shell_area);
        } else if let Some(p) = v.preview {
            preview_rows = draw_preview(f, shell_area, p, sel_in(crate::tab::Pane::Shell));
        } else {
            f.render_widget(ScreenWidget(v.screen, sel_in(crate::tab::Pane::Shell)), shell_area);
        }
        let accent = Style::default().fg(t.divider_focus).add_modifier(Modifier::BOLD);
        let div = sp.divider;
        for y in 0..div.height {
            let c = &mut f.buffer_mut()[(div.x, div.y + y)];
            c.set_symbol("│");
            c.set_style(if v.focus_side { accent } else { Style::default().fg(t.divider_idle) });
        }
        // An arrow at the top of the divider points at the pane that has the keyboard.
        let side_is_right = sp.side.x > sp.shell.x;
        let c = &mut f.buffer_mut()[(div.x, div.y)];
        c.set_symbol(if v.focus_side == side_is_right { "▶" } else { "◀" });
        c.set_style(accent);
        // Buttons in the top corner of the preview or pinned pane: swap sides and close.
        let r = match v.pin {
            Some(pin) if pin.active_is_pinned => sp.shell,
            _ => sp.side,
        };
        if r.width >= (crate::tab::CLOSE_W + crate::tab::SWAP_W) * 2 {
            let btn = Style::default().fg(t.bar_fg).bg(t.bar_bg).add_modifier(Modifier::BOLD);
            for (i, sym) in [" ", "⇄", " ", " ", "✕", " "].into_iter().enumerate() {
                let c = &mut f.buffer_mut()[(r.x + r.width - crate::tab::CLOSE_W - crate::tab::SWAP_W + i as u16, r.y)];
                c.set_symbol(sym);
                c.set_style(btn);
            }
        }
    } else if let Some(p) = v.preview {
        preview_rows = draw_preview(f, main, p, sel_in(crate::tab::Pane::Shell));
    } else {
        f.render_widget(ScreenWidget(v.screen, sel_in(crate::tab::Pane::Shell)), main);
    }

    for &(row, s, e, cur) in v.find_hl {
        let buf = f.buffer_mut();
        for c in s..e {
            let (x, y) = (shell_area.x + c, shell_area.y + row);
            if x < shell_area.x + shell_area.width && y < shell_area.y + shell_area.height {
                let th = theme();
                let st = if cur { Style::default().fg(th.bg).bg(th.diff_add).add_modifier(Modifier::BOLD) } else { Style::default().fg(th.hint_fg).bg(th.hint_bg) };
                buf[(x, y)].set_style(st);
            }
        }
    }

    if let Some((hints, typed)) = v.hints {
        let buf = f.buffer_mut();
        let area = buf.area;
        for h in hints.iter().filter(|h| h.label.starts_with(typed)) {
            for i in 0..h.width {
                let (x, y) = (h.col + i, h.row);
                if x < area.width && y < area.height {
                    let cell = &mut buf[(x, y)];
                    let st = cell.style().add_modifier(Modifier::UNDERLINED);
                    cell.set_style(st);
                }
            }
            for (i, ch) in h.label.chars().enumerate() {
                let (x, y) = (h.col + i as u16, h.row);
                if x < area.width && y < area.height {
                    let cell = &mut buf[(x, y)];
                    cell.set_char(ch);
                    cell.set_style(Style::default().fg(t.hint_fg).bg(t.hint_bg).add_modifier(Modifier::BOLD));
                }
            }
        }
    }

    let scroll_hint = v.scroll.map(|(n, m)| format!(" scroll: arrows, PgUp/PgDn, Home/End   Esc exits   typing goes live   scrollback {n}/{m}"));
    let preview_hint = active_prev.map(|p| {
        let total = p.display_len();
        let last = (p.top + preview_rows).min(total);
        let diff = p.kind == crate::preview::Kind::Diff;
        let files = match (diff, p.find.is_some()) {
            (true, true) => "N/P file · Enter/B match · ",
            (true, false) => "N/P file · ",
            (false, true) => "N/B match · ",
            (false, false) => "",
        };
        let files = if p.kind == crate::preview::Kind::Diff { files.to_string() } else { format!("{files}H recent · ") };
        let back = if split.is_some() && v.pin.is_none() { "Esc to the shell · Q closes" } else { "Esc back · Q closes" };
        let md = p.md_hint().map(|h| format!("{h} · ")).unwrap_or_default();
        let wrap = if p.md_rendered() { "" } else { "W wrap · " };
        format!(" lines {}-{} of {}  ·  arrows scroll · {files}F find · {md}{wrap}R reload · {back}", (p.top + 1).min(total), last, total)
    });
    let status_text = v.status.map(|t| format!(" {t}"));
    let hint = match (v.menu.as_ref(), v.message) {
        _ if status_text.is_some() => status_text.unwrap_or_default(),
        _ if v.confirm.is_some_and(|c| c.yes.is_empty()) => " Esc cancels the download".to_string(),
        _ if v.confirm.is_some() => " Y or Enter to continue · N or Esc to cancel".to_string(),
        _ if v.hints.is_some() => " type a label to open that link · Esc cancels".to_string(),
        _ if v.themes.is_some() => " arrows preview a theme · Enter applies and saves · type to filter · Esc cancels".to_string(),
        _ if v.rename.is_some() => " Enter saves · Esc cancels · empty name goes back to the automatic title".to_string(),
        _ if v.pick.is_some() => " arrows choose · Enter opens it · Esc cancels".to_string(),
        _ if v.ask.is_some() => " Enter starts · E edits the command · N not now · X never here".to_string(),
        _ if v.prompt.is_some_and(|p| p.kind.is_folder()) => " Tab completes folders · arrows pick a recent one · Enter confirms · Esc cancels".to_string(),
        _ if v.prompt.is_some() => " Tab completes · Enter opens · Esc cancels".to_string(),
        _ if scroll_hint.is_some() => scroll_hint.unwrap_or_default(),
        (None, None) if preview_hint.is_some() => preview_hint.unwrap_or_default(),
        (Some(_), _) => " arrows + Enter, a letter, or a tab number · Space searches · Esc closes".to_string(),
        (None, Some(m)) if preview_hint.is_some() => format!(" {m} ·{}", preview_hint.unwrap_or_default()),
        (None, Some(m)) => format!(" {m}"),
        (None, None) if split.is_some() && v.pin.is_some() => format!(" {}  menu · click a pane or menu W to switch · ⇄ swaps sides · menu V unsplits", v.menu_label),
        (None, None) if split.is_some() => format!(" {}  menu · click a pane or menu W to switch · menu V closes the preview", v.menu_label),
        (None, None) => format!(" {}  menu", v.menu_label),
    };
    f.render_widget(Paragraph::new(hint).style(status_bar), status);
    if let Some(m) = v.mic {
        let (x, w) = mic_span(m, status.width);
        let r = Rect::new(status.x + x, status.y, w, 1);
        let hot = !m.starts_with("● loading");
        f.render_widget(Paragraph::new(format!(" {m} ")).style(Style::default().bg(t.status_bg).fg(if hot { Color::Red } else { t.status_fg }).add_modifier(Modifier::BOLD)), r);
    }

    if let Some(c) = v.confirm {
        draw_confirm(f, main, c);
        return;
    }
    if let Some(a) = v.ask {
        draw_ask(f, main, a);
        return;
    }
    if let Some(p) = v.pick {
        draw_pick(f, main, p);
        return;
    }
    if let Some(r) = v.rename {
        draw_rename(f, main, r);
        return;
    }
    if let Some(tp) = v.themes {
        themepick::draw(f, main, tp);
        return;
    }
    if let Some(sv) = &v.search {
        search::draw(f, main, sv);
        return;
    }
    if let Some(pr) = v.prompt {
        draw_prompt(f, main, pr);
        return;
    }
    match v.menu.as_ref() {
        Some(st) => draw_menu(f, main, st, v.remote),
        None if v.preview.is_some() || (split.is_some() && v.focus_side) => {}
        None if !v.screen.hide_cursor() && v.scroll.is_none() => {
            let (r, c) = v.screen.cursor_position();
            if r < shell_area.height && c < shell_area.width {
                f.set_cursor_position((shell_area.x + c, shell_area.y + r));
            }
        }
        None => {}
    }
}

/// A preview (banner on top, then the body) drawn into `area`. Returns the body height.
fn draw_preview(f: &mut Frame, area: Rect, p: &crate::preview::Preview, selection: Option<crate::select::Sel>) -> usize {
    let (banner_area, body) = if p.has_header() && area.height > 1 {
        let [b, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        (Some(b), body)
    } else {
        (None, area)
    };
    if let Some(b) = banner_area {
        f.render_widget(Paragraph::new(preview_header(p, b.width as usize)).style(Style::default().fg(theme().banner).bg(theme().bg)), b);
    }
    let mut vis = p.visible(body.height as usize, body.width as usize);
    if let Some(sel) = selection {
        for (r, line) in vis.iter_mut().enumerate() {
            *line = crate::preview::highlight_line(line, r as u16, sel);
        }
    }
    f.render_widget(Paragraph::new(vis).style(Style::default().fg(theme().fg).bg(theme().bg)), body);
    body.height as usize
}

/// The row above a preview: the full path of the file (cut in the middle when it does not fit,
/// the file name in bold), then the banner note when there is one.
fn preview_header(p: &crate::preview::Preview, width: usize) -> Line<'static> {
    let t = theme();
    let note_style = Style::default().fg(t.banner).bg(t.bg).add_modifier(Modifier::BOLD);
    if p.kind == crate::preview::Kind::Diff {
        return Line::from(Span::styled(format!(" {}", p.banner.as_deref().unwrap_or_default()), note_style));
    }
    let note = p.banner.as_deref().map(|n| format!("  ·  {n}")).unwrap_or_default();
    // Keep clear of the swap and close buttons a side pane draws over its top right corner.
    let buttons = if width >= 24 { (crate::tab::CLOSE_W + crate::tab::SWAP_W) as usize } else { 0 };
    let width = width - buttons;
    let room = width.saturating_sub(1);
    let note_w = note.chars().count();
    // The note yields to the path, but never takes more than half the row from it.
    let path_room = if note_w == 0 { room } else { room.saturating_sub(note_w).max(room / 2) };
    let shown = crate::history::shorten_middle(&p.path, path_room);
    let split = shown.rfind('/').map_or(0, |i| i + 1);
    let (dir, name) = shown.split_at(split);
    let mut spans = vec![
        Span::styled(format!(" {dir}"), Style::default().fg(t.dim).bg(t.bg)),
        Span::styled(name.to_string(), Style::default().fg(t.fg).bg(t.bg).add_modifier(Modifier::BOLD)),
    ];
    if !note.is_empty() {
        let used = 1 + shown.chars().count();
        let left = width.saturating_sub(used);
        spans.push(Span::styled(note.chars().take(left).collect::<String>(), note_style));
    }
    Line::from(spans)
}

/// A bordered box in the given theme colours.
fn bordered(title: &str, border: Color, bg: Color, fg: Color) -> Block<'_> {
    Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(border)).style(Style::default().bg(bg).fg(fg))
}

fn draw_menu(f: &mut Frame, main: Rect, st: &MenuState, remote: bool) {
    let items = menu_items_for(remote, TMUX_MENU.load(std::sync::atomic::Ordering::Relaxed));
    let lay = menu_layout(main, &items, st);
    let t = theme();
    let area = lay.rect;
    let inner_w = (area.width as usize).saturating_sub(2);
    let plain = Style::default().bg(t.menu_bg).fg(t.menu_fg);
    let dim = Style::default().bg(t.menu_bg).fg(t.dim);
    let pad = |used: usize| " ".repeat(inner_w.saturating_sub(used));
    let selected_item = menu_entries(&menu_rows(&items, &st.filter)).get(lay.sel).copied();
    let mut lines: Vec<Line> = Vec::new();
    // The filter row: the input while filtering, a hint otherwise.
    let prefix = " filter: ";
    if st.filtering || !st.filter.is_empty() {
        let room = inner_w.saturating_sub(prefix.chars().count() + 1);
        let shown = left_trim(&st.filter, room);
        let used = prefix.chars().count() + shown.chars().count();
        lines.push(Line::from(vec![Span::styled(prefix, dim), Span::styled(shown, plain.add_modifier(Modifier::BOLD)), Span::styled(pad(used), plain)]));
        if st.filtering && lay.filter_y < area.y + area.height {
            f.set_cursor_position((area.x + 1 + used as u16, lay.filter_y));
        }
    } else {
        let hint = " Space: search";
        lines.push(Line::from(vec![Span::styled(hint, dim), Span::styled(pad(hint.chars().count()), dim)]));
    }
    for (_, row) in &lay.rows {
        let line = match *row {
            LRow::Header(h) => {
                let text = format!(" {h}");
                Line::from(vec![Span::styled(text.clone(), dim.add_modifier(Modifier::BOLD)), Span::styled(pad(text.chars().count()), dim)])
            }
            LRow::MoreAbove | LRow::MoreBelow => {
                let text = if *row == LRow::MoreAbove { " ▲ more above" } else { " ▼ more below" };
                Line::from(vec![Span::styled(text, dim), Span::styled(pad(text.chars().count()), dim)])
            }
            LRow::NoMatch => Line::from(vec![Span::styled(" no match", dim), Span::styled(pad(9), dim)]),
            LRow::Item(i) => {
                let item = items[i];
                let sel = Some(i) == selected_item;
                let base = if sel { Style::default().bg(t.menu_sel_bg).fg(t.menu_sel_fg) } else { plain };
                let hot_style = if sel { base } else { base.fg(t.menu_hotkey) };
                // Underline the hotkey letter wherever it first appears in the label.
                let label = item.label;
                let pos = if item.key == NO_KEY { None } else { Some(label.to_lowercase().find(item.key).unwrap_or(0)) };
                let (a, rest) = label.split_at(pos.unwrap_or(0));
                let mut rest_chars = rest.chars();
                let hot = rest_chars.next().unwrap_or(' ');
                let tail = rest_chars.as_str();
                let used = 1 + label.chars().count();
                if pos.is_none() {
                    Line::from(Span::styled(format!(" {label}{}", pad(used)), base))
                } else {
                Line::from(vec![
                    Span::styled(format!(" {a}"), base),
                    Span::styled(hot.to_string(), hot_style.add_modifier(Modifier::UNDERLINED | Modifier::BOLD)),
                    Span::styled(format!("{tail}{}", pad(used)), base),
                ])
                }
            }
        };
        lines.push(line);
    }
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).style(plain).block(bordered(" menu ", t.menu_border, t.menu_bg, t.menu_fg)), area);
}

/// Keep the tail of a long path, marking the cut with an ellipsis.
fn left_trim(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max == 0 {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    format!("…{}", s.chars().skip(n - keep).collect::<String>())
}

fn draw_prompt(f: &mut Frame, main: Rect, p: &Prompt) {
    let more = p.total.saturating_sub(p.shown.len());
    let area = prompt_rect(main, p);
    let t = theme();
    let dim = Style::default().fg(t.dialog_dim);
    let mut lines: Vec<Line> = p
        .shown
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let st = if p.recent && p.sel == Some(i) { Style::default().bg(t.menu_sel_bg).fg(t.menu_sel_fg) } else { dim };
            Line::from(Span::styled(format!("   {s}"), st))
        })
        .collect();
    if more > 0 {
        lines.push(Line::from(Span::styled(format!("   … {more} more"), dim)));
    }
    if let Some(e) = &p.error {
        lines.push(Line::from(Span::styled(format!(" {e}"), Style::default().fg(t.error))));
    }
    let label = p.kind.label();
    lines.push(Line::from(vec![Span::styled(label, Style::default().add_modifier(Modifier::BOLD)), Span::raw(p.input.clone())]));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.dialog_bg).fg(t.dialog_fg)), area);
    let x = area.x + (label.chars().count() + crate::lineedit::cursor_col(&p.input, p.cur)) as u16;
    f.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y + area.height - 1));
}

/// The one-line "rename tab" prompt at the bottom of the main area, with the real cursor.
fn draw_rename(f: &mut Frame, main: Rect, r: &crate::rename::Rename) {
    let t = theme();
    let area = Rect::new(main.x, main.y + main.height.saturating_sub(1), main.width, 1.min(main.height));
    let label = " rename tab: ";
    let line = Line::from(vec![Span::styled(label, Style::default().add_modifier(Modifier::BOLD)), Span::raw(r.buf.clone())]);
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(line).style(Style::default().bg(t.dialog_bg).fg(t.dialog_fg)), area);
    let x = area.x + (label.chars().count() + r.cursor_col()) as u16;
    f.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y));
}

fn draw_ask(f: &mut Frame, main: Rect, a: &Ask) {
    let t = theme();
    let dim = Style::default().fg(t.dialog_dim);
    let editing = a.edit.is_some();
    let w = 76.min(main.width);
    let inner_w = w.saturating_sub(2) as usize;
    // border 2 + intro 1 + list + bottom (hint 2, or field 6)
    let bottom: u16 = if editing { 6 } else { 2 };
    let list_h = (a.rows() as u16).min(main.height.saturating_sub(3 + bottom).max(1)).min(10) as usize;
    let h = (3 + list_h as u16 + bottom).min(main.height);
    let area = Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h);
    // keep the selection in view
    let mut top = a.scroll.get().min(a.rows().saturating_sub(list_h));
    if a.sel < top { top = a.sel; }
    if a.sel >= top + list_h { top = a.sel + 1 - list_h; }
    a.scroll.set(top);

    let pad = |s: String| -> String {
        let n = unicode_width::UnicodeWidthStr::width(s.as_str());
        if n >= inner_w { s.chars().take(inner_w).collect() } else { format!("{s}{}", " ".repeat(inner_w - n)) }
    };
    let mut lines = vec![Line::from(Span::styled(" Pick how to start it. The choice is remembered for this folder.", dim))];
    let n_opt = a.options.len();
    for i in top..(top + list_h).min(a.rows()) {
        let sel = i == a.sel;
        let (text, tag, bold) = if i < n_opt {
            let o = &a.options[i];
            (format!("{:<7} {}", o.agent.short(), sanitize(&o.command)), if o.last_here { "last used here".to_string() } else { String::new() }, true)
        } else {
            match i - n_opt {
                0 => ("Other command…".to_string(), "O".to_string(), false),
                1 => ("Not now".to_string(), "N".to_string(), false),
                _ => ("Never here".to_string(), "X".to_string(), false),
            }
        };
        let left = format!(" {} {}", if sel { "›" } else { " " }, text);
        let tagw = unicode_width::UnicodeWidthStr::width(tag.as_str());
        let gap = inner_w.saturating_sub(unicode_width::UnicodeWidthStr::width(left.as_str()) + tagw + 1);
        let row = pad(format!("{left}{}{tag} ", " ".repeat(gap)));
        let mut st = if editing { dim } else { Style::default() };
        if sel && !editing { st = Style::default().bg(t.menu_sel_bg).fg(t.menu_sel_fg); }
        if bold && sel && !editing { st = st.add_modifier(Modifier::BOLD); }
        lines.push(Line::from(Span::styled(row, st)));
    }
    lines.push(Line::from(""));
    let mut cursor = None;
    if let Some(e) = &a.edit {
        let label = format!(" Command:  starts as {} (Tab switches)", e.agent.label());
        lines.push(Line::from(vec![Span::styled(" Command:", Style::default().add_modifier(Modifier::BOLD)), Span::styled(label[" Command:".len()..].to_string(), dim)]));
        let bw = inner_w.saturating_sub(2);
        let fw = bw.saturating_sub(2); // text cells inside the box
        let bs = Style::default().fg(t.dialog_border).add_modifier(Modifier::BOLD);
        lines.push(Line::from(Span::styled(format!(" ┌{}┐", "─".repeat(bw.saturating_sub(2))), bs)));
        // scroll the text horizontally so the cursor stays inside the box
        let chars: Vec<char> = e.buf.chars().collect();
        let mut start = 0;
        while start < e.cur && chars[start..e.cur].iter().map(|c| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0)).sum::<usize>() > fw.saturating_sub(1) { start += 1; }
        let mut shown = String::new();
        let mut used = 0;
        for c in &chars[start..] {
            let cw = unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
            if used + cw > fw { break; }
            shown.push(*c);
            used += cw;
        }
        let before: usize = chars[start..e.cur].iter().map(|c| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0)).sum();
        lines.push(Line::from(vec![
            Span::styled(" │", bs),
            Span::raw(format!("{shown}{}", " ".repeat(fw.saturating_sub(used)))),
            Span::styled("│", bs),
        ]));
        lines.push(Line::from(Span::styled(format!(" └{}┘", "─".repeat(bw.saturating_sub(2))), bs)));
        let (hint, st) = match &a.error {
            Some(e) => (format!(" {e}"), Style::default().fg(t.error)),
            None => (" Type the command · Enter start · Esc back to the list".to_string(), dim),
        };
        lines.push(Line::from(Span::styled(hint, st)));
        // row of the text line: top border row + intro + list + blank + label + box top
        let y = area.y + 1 + 1 + list_h as u16 + 1 + 1 + 1;
        cursor = Some((area.x + 1 + 2 + before as u16, y));
    } else {
        lines.push(Line::from(Span::styled(" ↑↓ choose · Enter start · E edit · Esc not now", dim)));
    }
    let title = format!(" Start an agent in {} ", sanitize(&a.folder.chars().take(30).collect::<String>()));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.dialog_bg).fg(t.dialog_fg)).block(bordered(&title, t.dialog_border, t.dialog_bg, t.dialog_fg)), area);
    if let Some(c) = cursor {
        f.set_cursor_position(c);
    }
}

fn draw_pick(f: &mut Frame, main: Rect, p: &crate::pick::Pick) {
    let t = theme();
    let dim = Style::default().fg(t.dialog_dim);
    let w = 90.min(main.width);
    let inner_w = w.saturating_sub(2) as usize;
    let list_h = (p.items.len() as u16).min(main.height.saturating_sub(5).max(1)).min(12) as usize;
    let h = (list_h as u16 + 4).min(main.height);
    let area = Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h);
    let mut top = p.scroll.get().min(p.items.len().saturating_sub(list_h));
    if p.sel < top { top = p.sel; }
    if p.sel >= top + list_h { top = p.sel + 1 - list_h; }
    p.scroll.set(top);
    let mut lines = vec![Line::from(Span::styled(" Several places match. Nothing is opened until you choose.", dim))];
    for i in top..(top + list_h).min(p.items.len()) {
        let text = sanitize(&p.items[i]);
        let room = inner_w.saturating_sub(4);
        let n = text.chars().count();
        let shown = if n > room { format!("…{}", text.chars().skip(n - room.saturating_sub(1)).collect::<String>()) } else { text };
        let row = format!(" {} {}", if i == p.sel { "›" } else { " " }, shown);
        let pad = inner_w.saturating_sub(unicode_width::UnicodeWidthStr::width(row.as_str()));
        let st = if i == p.sel { Style::default().bg(t.menu_sel_bg).fg(t.menu_sel_fg) } else { Style::default() };
        lines.push(Line::from(Span::styled(format!("{row}{}", " ".repeat(pad)), st)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" ↑↓ choose · Enter open · Esc cancel", dim)));
    let title = format!(" Which {}? ", sanitize(&p.path.chars().take(40).collect::<String>()));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.dialog_bg).fg(t.dialog_fg)).block(bordered(&title, t.dialog_border, t.dialog_bg, t.dialog_fg)), area);
}

fn draw_confirm(f: &mut Frame, main: Rect, c: &Confirm) {
    let w = 78.min(main.width);
    let h = (c.lines.len() as u16 + 5).min(main.height);
    let area = Rect::new(main.x + main.width.saturating_sub(w) / 2, main.y + main.height.saturating_sub(h) / 2, w, h);
    let t = theme();
    let dim = Style::default().fg(t.dialog_dim);
    let sel = Style::default().bg(t.menu_sel_bg).fg(t.menu_sel_fg);
    let mut lines: Vec<Line> = vec![Line::from("")];
    for l in &c.lines {
        lines.push(Line::from(format!(" {}", sanitize(l))));
    }
    lines.push(Line::from(""));
    if c.yes.is_empty() {
        lines.push(Line::from(vec![Span::styled(" Cancel ", sel), Span::styled("   Esc", dim)]));
    } else {
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", c.yes), sel.add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(" Cancel ", sel),
            Span::styled("   Y / Enter  ·  N / Esc", dim),
        ]));
    }
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.dialog_bg).fg(t.dialog_fg)).block(bordered(&format!(" {} ", sanitize(&c.title)), t.dialog_border, t.dialog_bg, t.dialog_fg)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_removes_control_and_bidi_tricks() {
        assert_eq!(sanitize("a\x1b[31mb\n"), "a·[31mb·");
        assert_eq!(sanitize("evil\u{202E}gpj.exe"), "evil·gpj.exe");
        assert_eq!(sanitize("日本語 ok"), "日本語 ok");
    }

    #[test]
    fn prompt_edits_at_the_cursor() {
        let mut p = Prompt::default();
        p.insert("/tmp/fooo.txt\n");
        assert_eq!((p.input.as_str(), p.cur), ("/tmp/fooo.txt", 13));
        p.cur = 8;
        p.insert("X");
        assert_eq!((p.input.as_str(), p.cur), ("/tmp/fooXo.txt", 9));
        p.insert("a\r\nb");
        assert_eq!(p.input, "/tmp/fooXabo.txt");
        p.set_input("/new/".into());
        assert_eq!(p.cur, 5);
        let f = Prompt::folder(PromptKind::NewFolder, "/p/web", vec![]);
        assert_eq!(f.cur, f.input.chars().count());
    }

    #[test]
    fn folder_prompt_starts_in_the_tab_folder_with_recents() {
        let p = Prompt::folder(PromptKind::NewFolder, "/p/web", vec!["/p/a".into(), "/p/b".into()]);
        assert_eq!(p.input, "/p/web/");
        assert!(p.recent && p.shown.len() == 2 && p.sel.is_none());
        let p = Prompt::folder(PromptKind::ChangeFolder, "/p/web/", vec![]);
        assert_eq!(p.input, "/p/web/");
        assert!(!p.recent);
        assert!(PromptKind::NewFolder.is_folder() && !PromptKind::File.is_folder());
    }

    fn first_label(items: &[&MenuItem], filter: &str) -> Option<&'static str> {
        menu_entries(&menu_rows(items, filter)).first().map(|&i| items[i].label)
    }

    #[test]
    fn menu_groups_under_headers() {
        let items = menu_items_for(false, false);
        let rows = menu_rows(&items, "");
        let heads: Vec<&str> = rows.iter().filter_map(|r| if let Row::Header(h) = r { Some(*h) } else { None }).collect();
        assert_eq!(heads, ["FILES", "SEARCH", "TABS", "VIEW", "AGENT", "SESSION"]);
        assert!(matches!(rows[0], Row::Header("FILES")));
        // Every entry is listed exactly once and headers are not entries.
        let mut e = menu_entries(&rows);
        assert_eq!(e.len(), items.len());
        e.sort();
        e.dedup();
        assert_eq!(e.len(), items.len());
        // An entry with no known section goes under OTHER.
        static ODD: MenuItem = MenuItem { label: "Odd", key: 'z', action: Action::Info, remote_only: false, section: "", keywords: &[] };
        let mut with = items.clone();
        with.push(&ODD);
        let rows = menu_rows(&with, "");
        assert_eq!(rows[rows.len() - 2], Row::Header("OTHER"));
        assert_eq!(rows[rows.len() - 1], Row::Item(with.len() - 1));
    }

    #[test]
    fn menu_filter_ranking() {
        let items = menu_items_for(false, false);
        assert_eq!(first_label(&items, "git"), Some("Git changes"));
        assert_eq!(first_label(&items, "grep"), Some("Search in files"));
        assert_eq!(first_label(&items, "srch"), Some("Search in files"));
        assert_eq!(first_label(&items, "sf"), Some("Search in files"));
        assert!(first_label(&items, "theme").is_some_and(|l| l.starts_with("Theme")));
        assert!(first_label(&items, "dark").is_some_and(|l| l.starts_with("Theme")));
        assert!(first_label(&items, "solarized").is_some_and(|l| l.starts_with("Theme")));
        assert_eq!(first_label(&items, "claude"), Some("Start / change agent"));
        assert!(menu_rows(&items, "zzzq").is_empty());
        // No headers while filtering.
        assert!(menu_rows(&items, "tab").iter().all(|r| matches!(r, Row::Item(_))));
    }

    #[test]
    fn update_is_findable_and_theme_label_shows_mode() {
        let items = menu_items_for(false, false);
        assert_eq!(first_label(&items, "update"), Some("Update wi"));
        assert_eq!(first_label(&items, "upgrade"), Some("Update wi"));
        let mut items = items;
        let named = |n: &str| theme::Mode::Named(theme::index_of(n).unwrap());
        set_theme_item(&mut items, theme::Mode::Auto, theme::Kind::Light);
        assert_eq!(first_label(&items, "theme"), Some("Theme… auto (light)"));
        set_theme_item(&mut items, theme::Mode::Auto, theme::Kind::Dark);
        assert_eq!(first_label(&items, "theme"), Some("Theme… auto (dark)"));
        set_theme_item(&mut items, named("dark"), theme::Kind::Dark);
        assert_eq!(first_label(&items, "theme"), Some("Theme… Dark"));
        set_theme_item(&mut items, named("light"), theme::Kind::Light);
        assert_eq!(first_label(&items, "theme"), Some("Theme… Light"));
        set_theme_item(&mut items, named("tomorrow-night-blue"), theme::Kind::Dark);
        assert_eq!(first_label(&items, "theme"), Some("Theme… Tomorrow Night Blue"));
        assert_eq!(items.iter().filter(|m| m.key == 'y').count(), 1);
    }

    #[test]
    fn menu_remote_and_tmux_variants() {
        let local = menu_items_for(false, false);
        assert!(!local.iter().any(|m| m.remote_only));
        let remote = menu_items_for(true, false);
        assert!(remote.iter().any(|m| m.action == Action::Download) && remote.iter().any(|m| m.action == Action::Reconnect));
        let tmux = menu_items_for(true, true);
        assert!(tmux.iter().any(|m| m.label.starts_with("Detach")) && tmux.iter().any(|m| m.action == Action::EndAndQuit));
        assert!(!tmux.iter().any(|m| m.label == "Quit"));
        assert_eq!(first_label(&tmux, "disconnect"), Some("Detach (keep sessions)"));
        assert_eq!(first_label(&tmux, "end"), Some("End sessions and quit"));
    }

    #[test]
    fn scroll_top_keeps_selection_visible() {
        // Everything fits: no scrolling.
        assert_eq!(scroll_top(5, 8, 3, 4, 2), 0);
        for total in [20usize, 30] {
            for h in [4usize, 7, 11] {
                for sel in 0..total {
                    for top in [0, 3, total] {
                        let t = scroll_top(total, h, sel, sel, top);
                        assert!(t + h <= total);
                        let first = t + usize::from(t > 0);
                        let last = t + h - 1 - usize::from(t + h < total);
                        assert!(sel >= first && sel <= last, "total {total} h {h} sel {sel} top {top} -> {t}");
                    }
                }
            }
        }
        // Tiny window: still visible, no markers.
        assert_eq!(scroll_top(10, 2, 9, 9, 0), 8);
    }

    #[test]
    fn menu_layout_scrolls_and_hit_tests() {
        let items = menu_items_for(false, false);
        let main = Rect::new(0, 1, 80, 12);
        let mut st = MenuState::default();
        let lay = menu_layout(main, &items, &st);
        assert_eq!(lay.rect.height, 12);
        assert_eq!(lay.list_h, 9);
        assert_eq!(lay.rows.len(), 9);
        assert_eq!(lay.rows[0].1, LRow::Header("FILES"));
        assert_eq!(lay.rows[8].1, LRow::MoreBelow);
        // Rows are contiguous below the filter row.
        for (n, (y, _)) in lay.rows.iter().enumerate() {
            assert_eq!(*y, lay.rect.y + 2 + n as u16);
        }
        let x = lay.rect.x + 3;
        assert_eq!(menu_hit(&lay, x, lay.filter_y), MenuHit::Filter);
        assert_eq!(menu_hit(&lay, x, lay.rows[0].0), MenuHit::Inert, "headers do nothing");
        let LRow::Item(first) = lay.rows[1].1 else { panic!() };
        assert_eq!(menu_hit(&lay, x, lay.rows[1].0), MenuHit::Item(first));
        assert_eq!(menu_hit(&lay, x, lay.rows[8].0), MenuHit::Inert, "marker row");
        assert_eq!(menu_hit(&lay, lay.rect.x, lay.rows[1].0), MenuHit::Inert, "border");
        assert_eq!(menu_hit(&lay, 0, 0), MenuHit::Outside);
        // Select the last entry: the window scrolls to the end and the marker moves up.
        st.sel = menu_entries(&menu_rows(&items, "")).len() - 1;
        let lay = menu_layout(main, &items, &st);
        assert_eq!(lay.rows[0].1, LRow::MoreAbove);
        let last = items.iter().position(|m| m.label == "Quit").unwrap();
        assert_eq!(lay.rows.last().unwrap().1, LRow::Item(last));
        let (y, _) = *lay.rows.last().unwrap();
        assert_eq!(menu_hit(&lay, x, y), MenuHit::Item(last));
        // No match.
        st.filter = "zzzq".into();
        let lay = menu_layout(main, &items, &st);
        assert_eq!(lay.rows, vec![(lay.rect.y + 2, LRow::NoMatch)]);
        assert_eq!(lay.rect.height, 4);
    }

    #[test]
    fn menu_box_widens_on_wide_terminals_only() {
        let items = menu_items_for(true, true);
        let narrow = menu_layout(Rect::new(0, 1, 80, 40), &items, &MenuState::default()).rect;
        assert_eq!(narrow.width, 30);
        let wide = menu_layout(Rect::new(0, 1, 120, 40), &items, &MenuState::default()).rect;
        let longest = items.iter().map(|m| m.label.chars().count()).max().unwrap();
        assert_eq!(wide.width as usize, longest + 6);
        assert_eq!(menu_layout(Rect::new(0, 1, 20, 40), &items, &MenuState::default()).rect.width, 20);
    }

    #[test]
    fn menu_keys_are_unique() {
        let mut keys: Vec<char> = MENU.iter().map(|m| m.key).filter(|&k| k != NO_KEY).collect();
        keys.push(END_ITEM.key);
        let set: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(set.len(), keys.len(), "{keys:?}");
    }

    #[test]
    fn hint_labels_are_unique_and_prefix_free() {
        for n in [1, 5, 26, 27, 100, 600] {
            let l = hint_labels(n);
            assert_eq!(l.len(), n);
            let set: std::collections::HashSet<_> = l.iter().collect();
            assert_eq!(set.len(), n);
            if n > 26 {
                assert!(l.iter().all(|x| x.chars().count() == 2), "pairs only, so no label is a prefix of another");
            }
        }
        assert_eq!(hint_labels(3), vec!["a", "s", "d"]);
    }

    fn render_screen(p: &vt100::Parser, w: u16, h: u16) -> Buffer {
        render_with(p, w, h, None)
    }

    fn render_with(p: &vt100::Parser, w: u16, h: u16, canvas: Option<(Color, Color)>) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        render_screen_cells(p.screen(), None, Rect::new(0, 0, w, h), &mut buf, canvas);
        buf
    }

    #[test]
    fn shell_area_is_painted_with_the_theme_canvas() {
        for t in theme::ALL {
            let mut p = parser(3, 20, 0);
            p.process(b"hi \x1b[31mred\x1b[0m \x1b[48;2;1;2;3mbg\x1b[0m");
            let buf = render_with(&p, 20, 3, Some((t.bg, t.fg)));
            for (x, y) in [(0, 0), (1, 0), (19, 0), (10, 2), (19, 2)] {
                assert_eq!((buf[(x, y)].bg, buf[(x, y)].fg), (t.bg, t.fg), "{} cell {x},{y}", t.name);
            }
            assert_eq!(buf[(3, 0)].bg, t.bg);
            assert_eq!(buf[(3, 0)].fg, theme::remap_ansi(1).unwrap_or(Color::Indexed(1)));
            assert_eq!(buf[(7, 0)].bg, Color::Rgb(1, 2, 3), "explicit backgrounds are untouched");
        }
    }

    #[test]
    fn no_paint_leaves_default_colours_to_the_terminal() {
        let mut p = parser(2, 10, 0);
        p.process(b"hi");
        let buf = render_with(&p, 10, 2, None);
        assert_eq!((buf[(0, 0)].bg, buf[(0, 0)].fg, buf[(9, 1)].bg), (Color::Reset, Color::Reset, Color::Reset));
    }

    #[test]
    fn painted_faint_text_is_blended_and_stays_legible() {
        for t in theme::ALL {
            let mut p = parser(1, 10, 0);
            p.process(b"\x1b[2;38;2;0;0;0mf\x1b[0m");
            let buf = render_with(&p, 10, 1, Some((t.bg, t.fg)));
            assert!(!buf[(0, 0)].modifier.contains(Modifier::DIM));
            if t.kind == theme::Kind::Light {
                assert!(theme::contrast(buf[(0, 0)].fg, t.bg) >= 3.0, "{}", t.name);
                assert_ne!(buf[(0, 0)].fg, Color::Rgb(0, 0, 0));
            }
        }
    }

    fn parser(rows: u16, cols: u16, sb: usize) -> vt100::Parser {
        vt100::Parser::new(crate::tab::nz(rows), crate::tab::nz(cols), sb)
    }

    #[test]
    fn dim_text_is_drawn_dim() {
        let mut p = parser(2, 20, 0);
        p.process(b"\x1b[2;38;2;200;100;50mfaint\x1b[0m plain");
        let buf = render_screen(&p, 20, 2);
        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM));
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(200, 100, 50));
        assert!(!buf[(8, 0)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn scrollback_rows_keep_their_background_to_the_row_end() {
        let mut p = parser(3, 30, 50);
        let mut s = String::new();
        for i in 0..8 { s += &format!("\x1b[48;2;0;40;0m+ {i} line\x1b[K\x1b[0m\r\n"); }
        p.process(s.as_bytes());
        p.screen_mut().set_scrollback(4);
        let buf = render_screen(&p, 30, 3);
        for y in 0..3 {
            assert_eq!(buf[(29, y)].bg, Color::Rgb(0, 40, 0), "row {y} fills to the last column (erase keeps the background)");
        }
    }

    #[test]
    fn scrollback_from_before_a_widening_keeps_its_old_width() {
        // vt100 does not reflow: rows scrolled off while the pane was narrower (a side preview
        // was open) stay that wide, so a background block ends at the old column. Claude Code
        // redraws at the new width, which shows the diff twice. This documents the behaviour.
        let mut p = parser(3, 30, 50);
        let mut s = String::new();
        for i in 0..8 { s += &format!("\x1b[48;2;0;40;0m+ {i}\x1b[K\x1b[0m\r\n"); }
        p.process(s.as_bytes());
        p.screen_mut().set_size(crate::tab::nz(3), crate::tab::nz(60));
        p.screen_mut().set_scrollback(4);
        let buf = render_screen(&p, 60, 3);
        assert_eq!(buf[(29, 0)].bg, Color::Rgb(0, 40, 0));
        assert_eq!(buf[(30, 0)].bg, Color::Reset);
    }
}
