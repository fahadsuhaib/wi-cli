//! The file explorer: state, key and mouse handling, lazy loading. No drawing here (see
//! `ui/explorer.rs`) and no direct I/O: loads are described as `Req`, run by `start` (the main
//! loop polls the future) and handed back as `Done`, so everything below is testable without a
//! terminal or a box.

use crate::explorer_data::{DetailedListing, DirEntry, GitMark, GitMarks, SortKey, child_path, format_size, relative_time, sort_entries, summarize};
use crate::mouse::{Button, Kind, MouseEvent};
use crate::opener::{self, Handling, OpenCfg};
use crate::preview::{Opts, Preview};
use crate::theme::theme;
use crate::tui::columns::{Column, ColumnList, Width};
use crate::tui::split::{Pane, SplitLayout, SplitView};
use crate::tui::tree::{Glyphs, Node, Tree};
use crate::workspace::Backend;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

/// A folder read that takes longer than this ends in an error row.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(10);
/// The detail pane hides below this many columns (left 44 + divider + right 55).
pub const SPLIT_LEFT_MIN: u16 = 44;
pub const SPLIT_RIGHT_MIN: u16 = 55;
/// Rows used by the title bar, hint line and status line above the panes.
pub const TOP_ROWS: u16 = 3;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const DIR_LIST_CAP: usize = 5000;

// ---------------------------------------------------------------- keys

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ek {
    Up,
    Down,
    Left,
    Right,
    PgUp,
    PgDn,
    Home,
    End,
    Enter,
    Esc,
    Tab,
    Backspace,
    Clear,
    Char(char),
    /// The global menu key.
    Menu,
}

/// Decode raw input (the enhanced-key decoder has already turned reports into legacy bytes).
pub fn decode(buf: &[u8], menu_key: u8) -> Vec<Ek> {
    let mut keys = Vec::new();
    let mut text: Vec<u8> = Vec::new();
    let flush = |text: &mut Vec<u8>, keys: &mut Vec<Ek>| {
        if !text.is_empty() {
            keys.extend(String::from_utf8_lossy(text).chars().filter(|c| !c.is_control()).map(Ek::Char));
            text.clear();
        }
    };
    let mut i = 0;
    while i < buf.len() {
        let b = buf[i];
        if b >= 0x20 && b != 0x7f && b != menu_key {
            text.push(b);
            i += 1;
            continue;
        }
        flush(&mut text, &mut keys);
        match b {
            0x1b => {
                if matches!(buf.get(i + 1), Some(b'[') | Some(b'O')) {
                    let mut j = i + 2;
                    while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                        j += 1;
                    }
                    if j >= buf.len() {
                        break;
                    }
                    let params = &buf[i + 2..j];
                    let first = params.split(|&c| c == b';').next().unwrap_or(&[]);
                    keys.extend(match (buf[j], first) {
                        (b'A', _) => Some(Ek::Up),
                        (b'B', _) => Some(Ek::Down),
                        (b'C', _) => Some(Ek::Right),
                        (b'D', _) => Some(Ek::Left),
                        (b'H', _) | (b'~', b"1") | (b'~', b"7") => Some(Ek::Home),
                        (b'F', _) | (b'~', b"4") | (b'~', b"8") => Some(Ek::End),
                        (b'~', b"5") => Some(Ek::PgUp),
                        (b'~', b"6") => Some(Ek::PgDn),
                        _ => None,
                    });
                    i = j;
                } else {
                    keys.push(Ek::Esc);
                }
            }
            b'\r' | b'\n' => keys.push(Ek::Enter),
            b'\t' => keys.push(Ek::Tab),
            0x7f | 0x08 => keys.push(Ek::Backspace),
            0x15 => keys.push(Ek::Clear),
            x if x == menu_key => keys.push(Ek::Menu),
            _ => {}
        }
        i += 1;
    }
    flush(&mut text, &mut keys);
    keys
}

// ---------------------------------------------------------------- loading

/// One read the main loop must run. Directory ids are '/'-joined paths relative to the explorer's
/// root; the empty id is the root itself.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Req {
    Dir(String),
    Marks,
    Read(String),
}

pub enum Done {
    Dir { epoch: u64, id: String, res: Result<DetailedListing, String> },
    Marks { epoch: u64, res: Result<Option<GitMarks>, String> },
    Read { epoch: u64, id: String, res: Result<(Vec<u8>, u64), String> },
}

pub type ExJob<'a> = Pin<Box<dyn Future<Output = Done> + 'a>>;

/// Run one request against the backend, bounded by `LOAD_TIMEOUT`.
pub fn start(backend: &Backend, root: String, epoch: u64, req: Req, read_cap: usize) -> ExJob<'_> {
    Box::pin(async move {
        let abs = |id: &str| if id.is_empty() { root.clone() } else { child_path(&root, id) };
        let to = |e: tokio::time::error::Elapsed| format!("timed out after {} s ({e})", LOAD_TIMEOUT.as_secs());
        match req {
            Req::Dir(id) => {
                let r = tokio::time::timeout(LOAD_TIMEOUT, backend.list_dir_detailed(&abs(&id))).await;
                let res = match r {
                    Ok(r) => r.map_err(|e| e.to_string()),
                    Err(e) => Err(to(e)),
                };
                Done::Dir { epoch, id, res }
            }
            Req::Marks => {
                let r = tokio::time::timeout(LOAD_TIMEOUT, backend.git_marks(&root)).await;
                let res = match r {
                    Ok(r) => r.map_err(|e| e.to_string()),
                    Err(e) => Err(to(e)),
                };
                Done::Marks { epoch, res }
            }
            Req::Read(id) => {
                let r = tokio::time::timeout(LOAD_TIMEOUT, backend.read_prefix(&abs(&id), read_cap)).await;
                let res = match r {
                    Ok(r) => r.map_err(|e| e.to_string()),
                    Err(e) => Err(to(e)),
                };
                Done::Read { epoch, id, res }
            }
        }
    })
}

// ---------------------------------------------------------------- state

/// What a key asks the main loop to do (everything else is handled inside the state).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Act {
    Leave,
    /// Open this absolute path like a clicked path (preview, or a confirmation first).
    OpenFile(String),
    /// Open the file in `$EDITOR` in a new shell tab started in `dir`.
    Editor { file: String, dir: String },
    NewTab(String),
    Agent(String),
    /// Copy this text to the clipboard (`what` names it in the status message).
    Copy { text: String, what: &'static str },
    /// Type these root-relative paths as `@path ` mentions into an agent tab's input.
    Send { rels: Vec<String>, root: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExMouse {
    None,
    /// A double click: act as Enter on the selected row.
    Enter,
}

pub struct Filter {
    pub text: String,
    /// Typing goes into the filter (otherwise it only narrows the rows).
    pub editing: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RowKind {
    Node,
    /// A folder being read.
    Loading,
    Error(String),
    /// A folder with more entries than the cap: total seen.
    More(usize),
}

/// One visible row, with everything the screen needs.
#[derive(Clone, Debug)]
pub struct VRow {
    /// The node's id; for a notice row a unique id that starts with the folder's id.
    pub id: String,
    pub kind: RowKind,
    pub depth: usize,
    pub prefix: String,
    pub label: String,
    pub is_dir: bool,
    pub expanded: bool,
    pub has_children: bool,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub mark: Option<GitMark>,
    /// A folder with a changed file somewhere inside.
    pub dirty: bool,
    /// Folder the notice belongs to (empty for the root).
    pub parent: String,
    /// A file whose modified time changed (or that appeared) since the explorer was opened.
    pub new: bool,
    /// Marked with Space for sending to an agent.
    pub picked: bool,
}

pub enum DState {
    Empty,
    Loading,
    Lines(Vec<Line<'static>>),
    File(Box<Preview>),
    Error(String),
}

pub struct Settings {
    pub csv_rows: usize,
    pub read_cap: usize,
    pub open_cfg: OpenCfg,
    pub pct: u16,
}

pub struct ExplorerState {
    pub root: String,
    /// `LOCAL` or the box name.
    pub badge: String,
    pub remote: bool,
    pub tree: Tree<DirEntry>,
    root_loaded: bool,
    root_err: Option<String>,
    pub marks: Option<GitMarks>,
    mark_prefix: String,
    pub sort: SortKey,
    pub desc: bool,
    pub show_hidden: bool,
    pub changed_only: bool,
    pub filter: Option<Filter>,
    pub rows: Vec<VRow>,
    pub list: ColumnList,
    sel_id: String,
    pub split: SplitView,
    pub focus_detail: bool,
    pub detail: DState,
    pub detail_for: String,
    pub detail_top: usize,
    /// Rows available to the detail content (set by `prepare`).
    pub detail_h: usize,
    pub queue: Vec<Req>,
    loading: HashSet<String>,
    errors: HashMap<String, String>,
    truncs: HashMap<String, usize>,
    pub epoch: u64,
    /// Status-line message; true = an error.
    pub message: Option<(String, bool)>,
    pub now: i64,
    pub lay: SplitLayout,
    pub split_area: Rect,
    /// (items, changed, hidden) for the status line.
    pub counts: (usize, usize, usize),
    csv_rows: usize,
    pub read_cap: usize,
    open_cfg: OpenCfg,
    last_click: Option<(Instant, usize)>,
    dragging: bool,
    /// The file whose first Enter showed it in the detail pane; the next Enter opens it.
    entered: Option<String>,
    /// Files and folders marked with Space (kept across folds and refreshes).
    pub picked: BTreeSet<String>,
    /// First modified time seen for every file, and the folders already listed once.
    baseline: HashMap<String, Option<i64>>,
    listed: HashSet<String>,
    /// Files that changed or appeared after their folder was first listed.
    changed_ids: HashSet<String>,
}

fn keep_order<T>(_: &Node<T>, _: &Node<T>) -> std::cmp::Ordering {
    std::cmp::Ordering::Equal
}

fn hidden_path(id: &str) -> bool {
    id.split('/').any(|c| c.starts_with('.'))
}

fn columns(sort: SortKey, desc: bool) -> Vec<Column> {
    let arrow = if desc { " ▼" } else { " ▲" };
    // Newest first is the modified column, descending.
    let (sort, arrow) = if sort == SortKey::Newest { (SortKey::Modified, " ▼") } else { (sort, arrow) };
    let title = |name: &str, k: SortKey| if sort == k { format!("{name}{arrow}") } else { name.to_string() };
    vec![
        Column::new(title("FILES", SortKey::Name), Width::Fill),
        Column::new(title("SIZE", SortKey::Size), Width::Min(7)).right(),
        Column::new(title("MODIFIED", SortKey::Modified), Width::Min(10)).right(),
        Column::new(" ", Width::Fixed(1)),
        Column::new(" ", Width::Fixed(1)),
    ]
}

pub fn mark_word(m: GitMark) -> &'static str {
    match m {
        GitMark::Modified => "modified",
        GitMark::Added => "added",
        GitMark::Deleted => "deleted",
        GitMark::Renamed => "renamed",
        GitMark::Untracked => "untracked",
        GitMark::Conflict => "conflict",
        GitMark::Ignored => "ignored",
    }
}

fn parent_of(id: &str) -> &str {
    id.rfind('/').map_or("", |p| &id[..p])
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

impl ExplorerState {
    /// A fresh explorer rooted at `root` (absolute). The root listing and git marks are queued.
    pub fn new(root: String, remote: Option<String>, st: Settings) -> Self {
        let mut tree: Tree<DirEntry> = Tree::new(Vec::new());
        tree.sort_with(keep_order);
        let mut s = ExplorerState {
            root,
            badge: remote.clone().unwrap_or_else(|| "LOCAL".into()),
            remote: remote.is_some(),
            tree,
            root_loaded: false,
            root_err: None,
            marks: None,
            mark_prefix: String::new(),
            sort: SortKey::Name,
            desc: false,
            show_hidden: false,
            changed_only: false,
            filter: None,
            rows: Vec::new(),
            list: ColumnList::new(columns(SortKey::Name, false)),
            sel_id: String::new(),
            split: SplitView::new(st.pct, SPLIT_LEFT_MIN, SPLIT_RIGHT_MIN),
            focus_detail: false,
            detail: DState::Empty,
            detail_for: String::new(),
            detail_top: 0,
            detail_h: 10,
            queue: vec![Req::Dir(String::new()), Req::Marks],
            loading: HashSet::from([String::new()]),
            errors: HashMap::new(),
            truncs: HashMap::new(),
            epoch: 0,
            message: None,
            now: now_secs(),
            lay: SplitLayout { left: Rect::default(), divider: Rect::default(), right: Rect::default(), single: Some(Pane::Left) },
            split_area: Rect::default(),
            counts: (0, 0, 0),
            csv_rows: st.csv_rows,
            read_cap: st.read_cap,
            open_cfg: st.open_cfg,
            last_click: None,
            dragging: false,
            entered: None,
            picked: BTreeSet::new(),
            baseline: HashMap::new(),
            listed: HashSet::new(),
            changed_ids: HashSet::new(),
        };
        s.rebuild();
        s
    }

    pub fn pct(&self) -> u16 {
        self.split.pct
    }

    pub fn branch(&self) -> Option<&str> {
        self.marks.as_ref().and_then(|m| m.branch.as_deref())
    }

    // ---- requests

    /// The next read to run, with the generation it belongs to.
    pub fn next_req(&mut self) -> Option<(u64, Req)> {
        if self.queue.is_empty() { None } else { Some((self.epoch, self.queue.remove(0))) }
    }

    fn want_dir(&mut self, id: &str) {
        if self.loading.insert(id.to_string()) {
            self.queue.push(Req::Dir(id.to_string()));
        }
        self.errors.remove(id);
    }

    fn abs(&self, id: &str) -> String {
        if id.is_empty() { self.root.clone() } else { child_path(&self.root, id) }
    }

    /// Feed a finished read back in.
    pub fn apply(&mut self, d: Done) {
        match d {
            Done::Dir { epoch, id, res } if epoch == self.epoch => {
                self.loading.remove(&id);
                match res {
                    Ok(l) => self.set_listing(&id, l),
                    Err(e) => {
                        if id.is_empty() {
                            self.root_err = Some(e);
                        } else {
                            self.errors.insert(id, e);
                        }
                    }
                }
                self.rebuild();
            }
            Done::Marks { epoch, res } if epoch == self.epoch => {
                let marks = res.ok().flatten();
                self.set_marks(marks);
                self.rebuild();
            }
            Done::Read { epoch, id, res } if epoch == self.epoch && id == self.detail_for => {
                self.detail_top = 0;
                self.detail = match res {
                    Err(e) => DState::Error(e),
                    Ok((bytes, total)) => self.build_file_detail(&id, &bytes, total),
                };
            }
            _ => {}
        }
    }

    fn set_marks(&mut self, marks: Option<GitMarks>) {
        self.mark_prefix = String::new();
        self.marks = None;
        let Some(m) = marks else { return };
        let mut root = self.root.trim_end_matches('/').to_string();
        if !root.starts_with(m.repo_root.trim_end_matches('/')) && !self.remote {
            // The repo root is the resolved path; the folder may have been reached through a link.
            if let Ok(c) = std::fs::canonicalize(&root) {
                root = c.to_string_lossy().into_owned();
            }
        }
        let rr = m.repo_root.trim_end_matches('/');
        match root.strip_prefix(rr) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                self.mark_prefix = rest.trim_start_matches('/').to_string();
                self.marks = Some(m);
            }
            _ => {}
        }
        if self.marks.is_none() {
            self.changed_only = false;
        }
        self.rebuild_dir_detail();
    }

    fn set_listing(&mut self, id: &str, l: DetailedListing) {
        let mut entries = l.entries;
        sort_entries(&mut entries, self.sort, self.desc);
        let was_listed = !self.listed.insert(id.to_string());
        for e in entries.iter().filter(|e| !e.is_dir) {
            let nid = if id.is_empty() { e.name.clone() } else { format!("{id}/{}", e.name) };
            match self.baseline.get(&nid) {
                Some(m) if *m == e.mtime => {}
                Some(_) => {
                    self.changed_ids.insert(nid.clone());
                    self.baseline.insert(nid, e.mtime);
                }
                None => {
                    if was_listed {
                        self.changed_ids.insert(nid.clone());
                    }
                    self.baseline.insert(nid, e.mtime);
                }
            }
        }
        let mut old: Vec<Node<DirEntry>> = if id.is_empty() {
            std::mem::take(&mut self.tree.roots)
        } else {
            self.tree.find_mut(id).map(|n| std::mem::take(&mut n.children)).unwrap_or_default()
        };
        let nodes: Vec<Node<DirEntry>> = entries
            .into_iter()
            .map(|e| {
                let nid = if id.is_empty() { e.name.clone() } else { format!("{id}/{}", e.name) };
                let mut n = if e.is_dir { Node::dir(nid, e.name.clone(), e.clone()) } else { Node::file(nid, e.name.clone(), e.clone()) };
                if let Some(p) = old.iter().position(|o| o.id == n.id && o.is_dir == n.is_dir) {
                    let o = old.swap_remove(p);
                    n.expanded = o.expanded;
                    n.loaded = o.loaded;
                    n.children = o.children;
                }
                n
            })
            .collect();
        if l.truncated {
            self.truncs.insert(id.to_string(), l.total_seen);
        } else {
            self.truncs.remove(id);
        }
        self.errors.remove(id);
        if id.is_empty() {
            self.tree.roots = nodes;
            self.root_loaded = true;
            self.root_err = None;
        } else {
            self.tree.set_children(id, nodes);
        }
        if self.detail_for == id {
            self.rebuild_dir_detail();
        }
    }

    // ---- rows

    fn mark_key(&self, id: &str) -> String {
        if self.mark_prefix.is_empty() {
            id.to_string()
        } else if id.is_empty() {
            self.mark_prefix.clone()
        } else {
            format!("{}/{id}", self.mark_prefix)
        }
    }

    fn mark_of(&self, id: &str) -> Option<GitMark> {
        self.marks.as_ref()?.marks.get(&self.mark_key(id)).copied().filter(|m| *m != GitMark::Ignored)
    }

    fn dirty_dir(&self, id: &str) -> bool {
        self.marks.as_ref().is_some_and(|m| m.dirty_dirs.contains(&self.mark_key(id)))
    }

    /// Whether a node may show under the hidden / changed / find settings.
    fn visible(&self, n: &Node<DirEntry>) -> bool {
        if !self.show_hidden && hidden_path(&n.id) {
            return false;
        }
        if self.changed_only && !(self.mark_of(&n.id).is_some() || (n.is_dir && self.dirty_dir(&n.id))) {
            return false;
        }
        if let Some(f) = self.filter.as_ref().filter(|f| !f.text.is_empty()) {
            return crate::search::fuzzy::score(&f.text, &n.id).is_some();
        }
        true
    }

    fn filtering(&self) -> bool {
        self.changed_only || self.filter.as_ref().is_some_and(|f| !f.text.is_empty())
    }

    /// Recompute the visible rows, keeping the selection on the same item when it is still there.
    pub fn rebuild(&mut self) {
        let prev = (self.sel_id.clone(), self.list.selected);
        let mut rows: Vec<VRow> = Vec::new();
        {
            let g = Glyphs::unicode().with_guides();
            let keep = |n: &Node<DirEntry>| self.visible(n);
            let flat = if self.filtering() {
                self.tree.flatten_filtered(&keep)
            } else if !self.show_hidden {
                self.tree.flatten_visible(&keep)
            } else {
                self.tree.flatten()
            };
            let notices = !self.filtering();
            if !self.root_loaded && notices {
                rows.push(self.notice(String::new(), 0, self.root_err.clone().map_or(RowKind::Loading, RowKind::Error)));
            } else if self.root_loaded && notices && let Some(&t) = self.truncs.get("") {
                rows.push(self.notice(String::new(), 0, RowKind::More(t)));
            }
            for r in &flat {
                let n = r.node;
                rows.push(VRow {
                    id: n.id.clone(),
                    kind: RowKind::Node,
                    depth: r.depth,
                    prefix: g.prefix(r),
                    label: n.label.clone(),
                    is_dir: n.is_dir,
                    expanded: n.expanded,
                    has_children: r.has_children,
                    size: n.payload.size,
                    mtime: n.payload.mtime,
                    mark: self.mark_of(&n.id),
                    dirty: n.is_dir && self.dirty_dir(&n.id),
                    parent: parent_of(&n.id).to_string(),
                    new: !n.is_dir && self.changed_ids.contains(&n.id),
                    picked: self.picked.contains(&n.id),
                });
                if n.is_dir && n.expanded && notices {
                    let kind = if let Some(e) = self.errors.get(&n.id) {
                        Some(RowKind::Error(e.clone()))
                    } else if !n.loaded {
                        Some(RowKind::Loading)
                    } else {
                        self.truncs.get(&n.id).map(|t| RowKind::More(*t))
                    };
                    if let Some(k) = kind {
                        rows.push(self.notice(n.id.clone(), r.depth + 1, k));
                    }
                }
            }
        }
        if self.filtering() {
            // Folders opened only to show a match read as open.
            for i in 0..rows.len().saturating_sub(1) {
                if rows[i].kind == RowKind::Node && rows[i].is_dir && !rows[i].expanded && rows[i + 1].depth > rows[i].depth && rows[i + 1].kind == RowKind::Node {
                    rows[i].expanded = true;
                    if let Some(p) = rows[i].prefix.strip_suffix("▸ ") {
                        rows[i].prefix = format!("{p}▾ ");
                    }
                }
            }
        }
        self.rows = rows;
        self.counts = self.count();
        self.restore_sel(&prev.0, prev.1);
    }

    fn notice(&self, parent: String, depth: usize, kind: RowKind) -> VRow {
        let tag = match &kind {
            RowKind::Loading => "loading",
            RowKind::Error(_) => "error",
            _ => "more",
        };
        let text = match &kind {
            RowKind::Loading => "loading...".to_string(),
            RowKind::Error(e) => format!("could not read: {e}"),
            RowKind::More(t) => format!("showing first {} of {}", fmt_thousands(DIR_LIST_CAP), fmt_thousands(*t)),
            RowKind::Node => String::new(),
        };
        VRow {
            id: format!("{parent}\0{tag}"),
            kind,
            depth,
            prefix: "  ".repeat(depth + 1),
            label: text,
            is_dir: false,
            expanded: false,
            has_children: false,
            size: None,
            mtime: None,
            mark: None,
            dirty: false,
            parent,
            new: false,
            picked: false,
        }
    }

    fn count(&self) -> (usize, usize, usize) {
        fn walk(nodes: &[Node<DirEntry>], show_hidden: bool, items: &mut usize, hidden: &mut usize) {
            for n in nodes {
                if !show_hidden && n.label.starts_with('.') {
                    *hidden += 1;
                    continue;
                }
                *items += 1;
                walk(&n.children, show_hidden, items, hidden);
            }
        }
        let (mut items, mut hidden) = (0, 0);
        walk(&self.tree.roots, self.show_hidden, &mut items, &mut hidden);
        let changed = self.marks.as_ref().map_or(0, |m| {
            let pre = if self.mark_prefix.is_empty() { String::new() } else { format!("{}/", self.mark_prefix) };
            m.marks.iter().filter(|(k, v)| **v != GitMark::Ignored && k.starts_with(&pre)).count()
        });
        (items, changed, hidden)
    }

    fn restore_sel(&mut self, prev_id: &str, prev_idx: usize) {
        let mut id = prev_id.split('\0').next().unwrap_or("").to_string();
        let found = loop {
            if !id.is_empty() && let Some(i) = self.rows.iter().position(|r| r.id == id) {
                break Some(i);
            }
            if id.is_empty() {
                break None;
            }
            id = parent_of(&id).to_string();
        };
        let i = found.unwrap_or(prev_idx);
        self.list.select(i, self.rows.len());
        self.sync_sel();
    }

    fn sync_sel(&mut self) {
        self.list.select(self.list.selected, self.rows.len());
        let id = self.rows.get(self.list.selected).map(|r| r.id.clone()).unwrap_or_default();
        if id != self.sel_id {
            self.entered = None;
        }
        self.sel_id = id;
        self.update_detail();
    }

    pub fn selected(&self) -> Option<&VRow> {
        self.rows.get(self.list.selected)
    }

    /// Id of the item the selected row stands for (a notice row stands for its folder; None for
    /// the root's own notices).
    pub fn cur_id(&self) -> Option<String> {
        let r = self.selected()?;
        match r.kind {
            RowKind::Node => Some(r.id.clone()),
            _ if r.parent.is_empty() => None,
            _ => Some(r.parent.clone()),
        }
    }

    fn entry(&self, id: &str) -> Option<&Node<DirEntry>> {
        self.tree.find_by_path(id)
    }

    // ---- detail pane

    fn update_detail(&mut self) {
        let Some(id) = self.cur_id() else {
            self.detail_for = String::new();
            self.detail = DState::Empty;
            return;
        };
        if id == self.detail_for && !matches!(self.detail, DState::Empty) {
            return;
        }
        self.detail_for = id.clone();
        self.detail_top = 0;
        self.detail = DState::Loading;
        let Some(n) = self.entry(&id) else { return };
        if n.is_dir {
            if n.loaded {
                self.detail = DState::Lines(self.dir_lines(&id));
            } else {
                self.want_dir(&id);
            }
        } else {
            self.queue.retain(|r| !matches!(r, Req::Read(_)));
            self.queue.push(Req::Read(id));
        }
    }

    fn rebuild_dir_detail(&mut self) {
        let id = self.detail_for.clone();
        if let Some(n) = self.entry(&id)
            && n.is_dir
            && n.loaded
        {
            self.detail = DState::Lines(self.dir_lines(&id));
        }
    }

    /// Summary and listing of a loaded folder.
    fn dir_lines(&self, id: &str) -> Vec<Line<'static>> {
        let t = theme();
        let dim = Style::default().fg(t.dim);
        let Some(n) = self.entry(id) else { return Vec::new() };
        let ents: Vec<DirEntry> = n.children.iter().map(|c| c.payload.clone()).collect();
        let s = summarize(&ents);
        let mut out = vec![Line::from(vec![
            Span::styled(format!("{} items", s.items), Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(format!(" · {} folders · {} files · {} in files", s.dirs, s.files, format_size(s.bytes)), dim),
        ])];
        let mut extra = Vec::new();
        if s.hidden > 0 {
            extra.push(format!("{} hidden", s.hidden));
        }
        if let Some(m) = &self.marks {
            let pre = format!("{}/", self.mark_key(id));
            let k = m.marks.iter().filter(|(k, v)| **v != GitMark::Ignored && (k.starts_with(&pre) || self.mark_key(id).is_empty())).count();
            if k > 0 {
                extra.push(format!("{k} changed inside"));
            }
        }
        if let Some(t) = self.truncs.get(id) {
            extra.push(format!("showing first {} of {}", fmt_thousands(DIR_LIST_CAP), fmt_thousands(*t)));
        }
        if !extra.is_empty() {
            out.push(Line::from(Span::styled(extra.join(" · "), dim)));
        }
        out.push(Line::from(""));
        for c in n.children.iter().filter(|c| self.show_hidden || !c.payload.hidden) {
            let e = &c.payload;
            if e.is_dir {
                out.push(Line::from(Span::styled(format!("▸ {}/", e.name), Style::default().add_modifier(Modifier::BOLD))));
            } else {
                let size = e.size.map(format_size).unwrap_or_default();
                out.push(Line::from(vec![Span::raw(format!("· {}", e.name)), Span::styled(format!("  {size}"), dim)]));
            }
        }
        if n.children.is_empty() {
            out.push(Line::from(Span::styled("(empty folder)", dim)));
        }
        out
    }

    fn build_file_detail(&self, id: &str, bytes: &[u8], total: u64) -> DState {
        let abs = self.abs(id);
        match opener::classify(&abs, bytes, total, &self.open_cfg) {
            Handling::Native => DState::Lines(self.card_lines(id, total)),
            Handling::Preview => DState::File(Box::new(Preview::build(&abs, bytes, total, &Opts { csv_rows: self.csv_rows }))),
        }
    }

    /// Info card for a file that is not shown here (images, archives, binaries).
    fn card_lines(&self, id: &str, size: u64) -> Vec<Line<'static>> {
        let t = theme();
        let dim = Style::default().fg(t.dim);
        let name = id.rsplit('/').next().unwrap_or(id).to_string();
        let kind = std::path::Path::new(&name).extension().map(|e| format!("{} file", e.to_string_lossy().to_lowercase())).unwrap_or_else(|| "binary data".into());
        let mut v = vec![
            Line::from(Span::styled(name, Style::default().add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(vec![Span::styled("size  ", dim), Span::raw(format_size(size))]),
            Line::from(vec![Span::styled("type  ", dim), Span::raw(kind)]),
        ];
        if let Some(e) = self.entry(id).and_then(|n| n.payload.mtime) {
            v.push(Line::from(vec![Span::styled("when  ", dim), Span::raw(relative_time(self.now, e))]));
        }
        v.push(Line::from(""));
        v.push(Line::from(Span::styled("Not shown here. Enter asks before opening it natively.", dim)));
        v
    }

    /// The `Source:` line above the detail content.
    pub fn source_line(&self) -> String {
        let Some(id) = self.cur_id() else { return String::new() };
        let Some(n) = self.entry(&id) else { return String::new() };
        let mut parts = vec![if n.is_dir { format!("{id}/") } else { id.clone() }];
        if !n.is_dir && let Some(s) = n.payload.size {
            parts.push(format_size(s));
        }
        if let Some(m) = n.payload.mtime {
            parts.push(format!("modified {}", relative_time(self.now, m)));
        }
        match (self.mark_of(&id), n.is_dir && self.dirty_dir(&id)) {
            (Some(m), _) => parts.push(mark_word(m).to_string()),
            (None, true) => parts.push("changes inside".to_string()),
            _ => {}
        }
        format!("Source: {}", parts.join(" · "))
    }

    /// Lay the screen out for a terminal of `cols` x `rows` and size the detail content.
    pub fn prepare(&mut self, cols: u16, rows: u16) {
        self.split_area = Rect::new(0, TOP_ROWS.min(rows), cols, rows.saturating_sub(TOP_ROWS));
        self.lay = self.split.layout(self.split_area);
        let r = self.lay.right;
        let h = (r.height as usize).saturating_sub(3);
        self.detail_h = h;
        let w = r.width.saturating_sub(1);
        let lines_h = match &mut self.detail {
            DState::File(p) => {
                let body = h.saturating_sub(usize::from(p.banner.is_some()));
                p.layout(w, body);
                body
            }
            _ => h,
        };
        if let DState::Lines(l) = &self.detail {
            self.detail_top = self.detail_top.min(l.len().saturating_sub(lines_h.max(1)));
        }
    }

    // ---- scrolling the detail

    fn detail_len(&self) -> usize {
        match &self.detail {
            DState::File(p) => p.display_len(),
            DState::Lines(l) => l.len(),
            _ => 0,
        }
    }

    fn detail_body_h(&self) -> usize {
        match &self.detail {
            DState::File(p) => self.detail_h.saturating_sub(usize::from(p.banner.is_some())),
            _ => self.detail_h,
        }
        .max(1)
    }

    pub fn scroll_detail(&mut self, delta: isize) {
        let max = self.detail_len().saturating_sub(self.detail_body_h()) as isize;
        match &mut self.detail {
            DState::File(p) => p.top = (p.top as isize + delta).clamp(0, max) as usize,
            DState::Lines(_) => self.detail_top = (self.detail_top as isize + delta).clamp(0, max) as usize,
            _ => {}
        }
    }

    /// Top row of the detail content now.
    pub fn detail_pos(&self) -> usize {
        match &self.detail {
            DState::File(p) => p.top,
            _ => self.detail_top,
        }
    }

    pub fn detail_total(&self) -> usize {
        self.detail_len()
    }

    // ---- keys

    fn page(&self) -> isize {
        self.detail_body_h().saturating_sub(1).max(1) as isize
    }

    /// Folder an action applies to: the selected folder, or the folder of the selected file.
    fn target_dir(&self) -> String {
        match self.cur_id().and_then(|id| self.entry(&id).map(|n| (id.clone(), n.is_dir))) {
            Some((id, true)) => self.abs(&id),
            Some((id, false)) => self.abs(parent_of(&id)),
            None => self.root.clone(),
        }
    }

    fn toggle_dir(&mut self, id: &str, open: Option<bool>) {
        let is_open = self.entry(id).is_some_and(|n| n.expanded);
        match (open, is_open) {
            (Some(true), true) | (Some(false), false) => return,
            _ => {}
        }
        if is_open {
            self.tree.collapse(id);
        } else if self.tree.expand(id) {
            self.want_dir(id);
        }
        self.rebuild();
    }

    pub fn key(&mut self, k: Ek) -> Option<Act> {
        self.message = None;
        // Typing a find filter takes every key.
        if let Some(f) = self.filter.as_mut().filter(|f| f.editing) {
            match k {
                Ek::Char(c) => f.text.push(c),
                Ek::Backspace => {
                    f.text.pop();
                }
                Ek::Clear => f.text.clear(),
                Ek::Enter => {
                    f.editing = false;
                    return None;
                }
                Ek::Esc => self.filter = None,
                Ek::Menu => return Some(Act::Leave),
                Ek::Up | Ek::Down | Ek::PgUp | Ek::PgDn | Ek::Home | Ek::End => return self.nav(k),
                _ => return None,
            }
            self.list.home();
            self.rebuild();
            // Select the first row that matches itself, not just an ancestor of a match.
            let text = self.filter.as_ref().map(|f| f.text.clone()).unwrap_or_default();
            if let Some(i) = self.rows.iter().position(|r| r.kind == RowKind::Node && crate::search::fuzzy::score(&text, &r.id).is_some()) {
                self.list.select(i, self.rows.len());
                self.sync_sel();
            }
            return None;
        }
        let len = self.rows.len();
        match k {
            Ek::Menu => return Some(Act::Leave),
            Ek::Esc => {
                if self.filter.is_some() {
                    self.filter = None;
                    // The match stays selected: open the folders on the way to it.
                    if let Some(id) = self.cur_id() {
                        self.tree.expand_all_to(&id);
                    }
                    self.rebuild();
                } else if self.focus_detail {
                    self.focus_detail = false;
                    self.entered = None;
                } else {
                    return Some(Act::Leave);
                }
            }
            Ek::Up | Ek::Down | Ek::PgUp | Ek::PgDn | Ek::Home | Ek::End => return self.nav(k),
            Ek::Tab => {
                self.focus_detail = !self.focus_detail && self.lay.single.is_none();
                self.entered = None;
            }
            Ek::Left => {
                if let Some(id) = self.cur_id().filter(|_| self.selected().is_some_and(|r| r.kind == RowKind::Node)) {
                    if self.entry(&id).is_some_and(|n| n.is_dir && n.expanded) {
                        self.toggle_dir(&id, Some(false));
                    } else if let Some(p) = self.tree.select_parent(&id) && let Some(i) = self.rows.iter().position(|r| r.id == p) {
                        self.list.select(i, len);
                        self.sync_sel();
                    }
                }
            }
            Ek::Right => {
                if let Some(r) = self.selected().filter(|r| r.kind == RowKind::Node && r.is_dir).cloned() {
                    if !r.expanded {
                        self.toggle_dir(&r.id, Some(true));
                    } else if let Some(c) = self.tree.select_first_child(&r.id) && let Some(i) = self.rows.iter().position(|x| x.id == c) {
                        self.list.select(i, len);
                        self.sync_sel();
                    }
                }
            }
            Ek::Enter => {
                let r = self.selected()?.clone();
                if r.kind != RowKind::Node {
                    return None;
                }
                if r.is_dir {
                    self.toggle_dir(&r.id, None);
                } else if self.lay.single.is_some() || self.entered.as_deref() == Some(r.id.as_str()) {
                    // No detail pane (narrow), or the second Enter: open it next to the shell.
                    self.entered = None;
                    self.focus_detail = false;
                    return Some(Act::OpenFile(self.abs(&r.id)));
                } else {
                    // The first Enter stays here: the detail pane takes the keys (scroll with the arrows).
                    self.entered = Some(r.id.clone());
                    self.focus_detail = true;
                    self.message = Some(("Enter again or O opens it in the preview pane · Esc back to the tree".into(), false));
                }
            }
            Ek::Char(' ') => {
                if let Some(r) = self.selected().filter(|r| r.kind == RowKind::Node).cloned() {
                    if !self.picked.remove(&r.id) {
                        self.picked.insert(r.id.clone());
                    }
                    self.focus_detail = false;
                    self.rebuild();
                    self.list.down(len);
                    self.sync_sel();
                }
            }
            Ek::Char(c) => return self.letter(c),
            Ek::Backspace | Ek::Clear => {}
        }
        None
    }

    fn nav(&mut self, k: Ek) -> Option<Act> {
        let len = self.rows.len();
        if self.focus_detail && !self.filter.as_ref().is_some_and(|f| f.editing) {
            match k {
                Ek::Up => self.scroll_detail(-1),
                Ek::Down => self.scroll_detail(1),
                Ek::PgUp => self.scroll_detail(-self.page()),
                Ek::PgDn => self.scroll_detail(self.page()),
                Ek::Home => self.scroll_detail(isize::MIN / 2),
                _ => self.scroll_detail(isize::MAX / 2),
            }
            return None;
        }
        match k {
            Ek::Up => self.list.up(len),
            Ek::Down => self.list.down(len),
            Ek::PgUp => self.list.page_up(len),
            Ek::PgDn => self.list.page_down(len),
            Ek::Home => self.list.home(),
            _ => self.list.end(len),
        }
        self.sync_sel();
        None
    }

    /// Plain-letter commands. The command letters are `Q J K F H G S R C P T A O E M`; any other
    /// letter, digit, `.`, `_` or `-` starts the find filter with that character already typed.
    fn letter(&mut self, typed: char) -> Option<Act> {
        let c = typed.to_ascii_lowercase();
        match c {
            'q' => return Some(Act::Leave),
            'j' => self.scroll_detail(1),
            'k' => self.scroll_detail(-1),
            'f' => {
                self.filter = Some(Filter { text: String::new(), editing: true });
            }
            'h' => {
                self.show_hidden = !self.show_hidden;
                self.message = Some((if self.show_hidden { "hidden files shown" } else { "hidden files hidden" }.into(), false));
                self.rebuild();
            }
            'g' => {
                if self.marks.is_none() {
                    self.message = Some(("not a git repository: no changes to show".into(), true));
                } else {
                    self.changed_only = !self.changed_only;
                    self.list.home();
                    self.rebuild();
                }
            }
            's' => self.set_sort(match self.sort {
                SortKey::Name => SortKey::Size,
                SortKey::Size => SortKey::Modified,
                SortKey::Modified => SortKey::Newest,
                SortKey::Newest => SortKey::Name,
            }, false),
            'r' => self.refresh(),
            'c' | 'p' => {
                let id = self.cur_id()?;
                return Some(if c == 'c' { Act::Copy { text: id, what: "relative path" } } else { Act::Copy { text: self.abs(&id), what: "full path" } });
            }
            't' => return Some(Act::NewTab(self.target_dir())),
            'a' => return Some(Act::Agent(self.target_dir())),
            'o' => {
                let id = self.cur_id()?;
                if self.entry(&id).is_some_and(|n| n.is_dir) {
                    self.message = Some(("O opens a file in the preview pane; T opens a shell in this folder".into(), true));
                } else {
                    self.entered = None;
                    self.focus_detail = false;
                    return Some(Act::OpenFile(self.abs(&id)));
                }
            }
            'e' => {
                let id = self.cur_id()?;
                if self.entry(&id).is_some_and(|n| n.is_dir) {
                    self.message = Some(("E opens a file in your editor; T opens a shell in this folder".into(), true));
                } else {
                    return Some(Act::Editor { file: self.abs(&id), dir: self.abs(parent_of(&id)) });
                }
            }
            'm' => {
                let rels: Vec<String> = if self.picked.is_empty() {
                    self.cur_id().filter(|_| self.selected().is_some_and(|r| r.kind == RowKind::Node)).into_iter().collect()
                } else {
                    self.picked.iter().cloned().collect()
                };
                if rels.is_empty() {
                    self.message = Some(("nothing selected to send".into(), true));
                } else {
                    return Some(Act::Send { rels, root: self.root.clone() });
                }
            }
            _ if typed.is_alphanumeric() || matches!(typed, '.' | '_' | '-') => {
                // Not a command: start the find filter with this character.
                self.filter = Some(Filter { text: String::new(), editing: true });
                return self.key(Ek::Char(typed));
            }
            _ => {}
        }
        None
    }

    /// The marks were sent: forget them.
    pub fn sent(&mut self) {
        self.picked.clear();
        self.rebuild();
    }

    /// Sort by `key`; `toggle` flips the direction when it is already the sort key.
    pub fn set_sort(&mut self, key: SortKey, toggle: bool) {
        self.desc = if key == self.sort { toggle && !self.desc } else { false };
        self.sort = key;
        self.list.columns = columns(self.sort, self.desc);
        fn resort(nodes: &mut Vec<Node<DirEntry>>, key: SortKey, desc: bool) {
            let mut es: Vec<DirEntry> = nodes.iter().map(|n| n.payload.clone()).collect();
            sort_entries(&mut es, key, desc);
            let mut by: HashMap<String, Node<DirEntry>> = nodes.drain(..).map(|n| (n.label.clone(), n)).collect();
            for e in es {
                if let Some(mut n) = by.remove(&e.name) {
                    resort(&mut n.children, key, desc);
                    nodes.push(n);
                }
            }
        }
        resort(&mut self.tree.roots, self.sort, self.desc);
        self.message = Some((format!("sorted by {}{}", sort_name(self.sort), if self.desc { " (descending)" } else { "" }), false));
        self.rebuild();
    }

    /// Reload every open folder and the git marks.
    pub fn refresh(&mut self) {
        self.epoch += 1;
        self.queue.clear();
        self.loading.clear();
        self.errors.clear();
        fn walk(nodes: &mut [Node<DirEntry>], open: &mut Vec<String>) {
            for n in nodes.iter_mut().filter(|n| n.is_dir) {
                if n.expanded {
                    open.push(n.id.clone());
                    walk(&mut n.children, open);
                } else {
                    n.loaded = false;
                    n.children.clear();
                }
            }
        }
        let mut open = Vec::new();
        walk(&mut self.tree.roots, &mut open);
        self.want_dir("");
        for id in open {
            self.want_dir(&id);
        }
        self.queue.push(Req::Marks);
        self.detail_for = String::new();
        self.message = Some(("refreshing".into(), false));
        self.rebuild();
    }

    // ---- mouse

    pub fn mouse(&mut self, m: &MouseEvent) -> ExMouse {
        let len = self.rows.len();
        match m.kind {
            Kind::Down(Button::Left) => {
                self.message = None;
                if self.lay.on_divider(m.col, m.row) {
                    self.dragging = true;
                } else if let Some(c) = self.list.hit_header(m.col, m.row) {
                    match c {
                        0 => self.set_sort(SortKey::Name, true),
                        1 => self.set_sort(SortKey::Size, true),
                        2 => self.set_sort(SortKey::Modified, true),
                        _ => {}
                    }
                } else if let Some(i) = self.list.hit_test(m.col, m.row) {
                    self.focus_detail = false;
                    self.list.select(i, len);
                    self.sync_sel();
                    let r = self.rows[i].clone();
                    let arrow_x = self.lay.left.x + 2 * (r.depth as u16);
                    if r.kind == RowKind::Node && r.is_dir && r.has_children && m.col >= arrow_x && m.col < arrow_x + 2 {
                        self.last_click = None;
                        self.toggle_dir(&r.id, None);
                        return ExMouse::None;
                    }
                    let now = Instant::now();
                    let double = self.last_click.is_some_and(|(t, j)| j == i && now.duration_since(t) < DOUBLE_CLICK);
                    self.last_click = if double { None } else { Some((now, i)) };
                    if double {
                        if !r.is_dir {
                            self.entered = Some(r.id.clone());
                        }
                        return ExMouse::Enter;
                    }
                } else if self.lay.pane_at(m.col, m.row) == Some(Pane::Right) {
                    self.focus_detail = true;
                }
            }
            Kind::Drag(Button::Left) if self.dragging => {
                self.split.pct = self.split.pct_for_col(self.split_area, m.col);
                self.lay = self.split.layout(self.split_area);
            }
            Kind::Up(Button::Left) => self.dragging = false,
            Kind::WheelUp | Kind::WheelDown => {
                let d = if m.kind == Kind::WheelUp { -3 } else { 3 };
                match self.lay.pane_at(m.col, m.row) {
                    Some(Pane::Right) => self.scroll_detail(d),
                    Some(Pane::Left) => {
                        self.list.scroll(d, len);
                        self.sync_sel();
                    }
                    None => {}
                }
            }
            _ => {}
        }
        ExMouse::None
    }
}

pub fn sort_name(k: SortKey) -> &'static str {
    match k {
        SortKey::Name => "name",
        SortKey::Size => "size",
        SortKey::Modified => "modified",
        SortKey::Newest => "newest first",
    }
}

fn fmt_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A synthetic link for an absolute path, so a file chosen in the explorer is resolved exactly
/// like a clicked path (preview, confirmation, refusals).
pub fn path_link(abs: &str) -> crate::links::Link {
    crate::links::Link { kind: crate::links::LinkKind::Path { path: abs.to_string(), line: None, col: None }, row: 0, start_col: 0, end_col: 0, wrap_rows: 0 }
}

/// What is typed into the new tab's shell to edit `file` (also works through a remote shell).
pub fn editor_command(file: &str) -> String {
    format!("${{EDITOR:-vi}} -- {}\n", crate::exec::sq(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(name: &str, dir: bool, size: u64, mtime: i64) -> DirEntry {
        DirEntry { name: name.into(), is_dir: dir, is_symlink: false, size: if dir { None } else { Some(size) }, mtime: Some(mtime), hidden: name.starts_with('.') }
    }

    fn listing(v: Vec<DirEntry>) -> DetailedListing {
        let n = v.len();
        DetailedListing { entries: v, truncated: false, total_seen: n }
    }

    fn st() -> ExplorerState {
        let mut s = ExplorerState::new("/p".into(), None, Settings { csv_rows: 100, read_cap: 1 << 20, open_cfg: OpenCfg::default(), pct: 40 });
        s.now = 1_000_000;
        s.prepare(120, 36);
        s
    }

    fn loaded() -> ExplorerState {
        let mut s = st();
        s.apply(Done::Dir {
            epoch: 0,
            id: String::new(),
            res: Ok(listing(vec![
                ent("zeta.txt", false, 10, 900_000),
                ent("src", true, 0, 800_000),
                ent(".env", false, 5, 700_000),
                ent("Docs", true, 0, 950_000),
                ent("alpha.md", false, 2000, 990_000),
                ent(".git", true, 0, 1),
            ])),
        });
        s
    }

    fn ids(s: &ExplorerState) -> Vec<String> {
        s.rows.iter().map(|r| r.id.clone()).collect()
    }

    fn marks(pairs: &[(&str, GitMark)]) -> GitMarks {
        let mut m = GitMarks { repo_root: "/p".into(), branch: Some("main".into()), ..GitMarks::default() };
        for (k, v) in pairs {
            m.marks.insert(k.to_string(), *v);
            let mut e = k.len();
            while let Some(i) = k[..e].rfind('/') {
                m.dirty_dirs.insert(k[..i].to_string());
                e = i;
            }
        }
        m
    }

    #[test]
    fn root_loads_folders_first_case_insensitive_hidden_off() {
        let mut s = st();
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.rows[0].kind, RowKind::Loading);
        assert_eq!(s.next_req(), Some((0, Req::Dir(String::new()))));
        assert_eq!(s.next_req(), Some((0, Req::Marks)));
        s = loaded();
        assert_eq!(ids(&s), ["Docs", "src", "alpha.md", "zeta.txt"]);
        assert_eq!(s.counts, (4, 0, 2));
        s.key(Ek::Char('h'));
        assert_eq!(ids(&s), [".git", "Docs", "src", ".env", "alpha.md", "zeta.txt"]);
        assert_eq!(s.counts.2, 0);
        s.key(Ek::Char('H'));
        assert_eq!(s.rows.len(), 4);
    }

    #[test]
    fn lazy_load_expand_merge_and_collapse() {
        let mut s = loaded();
        s.next_req();
        s.next_req();
        // select src (row 1) and expand with Right
        s.key(Ek::Down);
        assert_eq!(s.cur_id().as_deref(), Some("src"));
        s.key(Ek::Right);
        assert!(s.rows.iter().any(|r| r.kind == RowKind::Loading && r.parent == "src"));
        // the read queued when the folder was selected is reused, not repeated
        let n = |s: &ExplorerState| s.queue.iter().filter(|r| **r == Req::Dir("src".into())).count();
        assert_eq!(n(&s), 1);
        s.key(Ek::Right);
        assert_eq!(n(&s), 1);
        s.apply(Done::Dir { epoch: 0, id: "src".into(), res: Ok(listing(vec![ent("main.rs", false, 100, 1), ent("lib", true, 0, 2)])) });
        assert_eq!(ids(&s), ["Docs", "src", "src/lib", "src/main.rs", "alpha.md", "zeta.txt"]);
        // Right on an open folder moves to its first child
        s.key(Ek::Right);
        assert_eq!(s.cur_id().as_deref(), Some("src/lib"));
        // Left on a file/closed folder goes to the parent
        s.key(Ek::Left);
        assert_eq!(s.cur_id().as_deref(), Some("src"));
        // Left on an open folder collapses it
        s.key(Ek::Left);
        assert!(!s.rows[1].expanded);
        assert_eq!(ids(&s), ["Docs", "src", "alpha.md", "zeta.txt"]);
        // Re-open: already loaded, no new request
        s.queue.clear();
        s.key(Ek::Enter);
        assert!(s.queue.is_empty());
        assert_eq!(s.rows.len(), 6);
        // Enter toggles
        s.key(Ek::Enter);
        assert_eq!(s.rows.len(), 4);
    }

    #[test]
    fn load_error_and_truncation_rows() {
        let mut s = loaded();
        s.key(Ek::Down);
        s.key(Ek::Enter);
        s.apply(Done::Dir { epoch: 0, id: "src".into(), res: Err("timed out after 10 s".into()) });
        let r = s.rows.iter().find(|r| matches!(r.kind, RowKind::Error(_))).unwrap();
        assert!(r.label.contains("timed out"));
        // collapse and expand again retries
        s.key(Ek::Enter);
        s.queue.clear();
        s.key(Ek::Enter);
        assert_eq!(s.queue, vec![Req::Dir("src".into())]);
        s.apply(Done::Dir { epoch: 0, id: "src".into(), res: Ok(DetailedListing { entries: vec![ent("a", false, 1, 1)], truncated: true, total_seen: 12_345 }) });
        let r = s.rows.iter().find(|r| matches!(r.kind, RowKind::More(_))).unwrap();
        assert_eq!(r.label, "showing first 5,000 of 12,345");
    }

    #[test]
    fn up_down_wrap_and_home_end() {
        let mut s = loaded();
        s.key(Ek::Up);
        assert_eq!(s.cur_id().as_deref(), Some("zeta.txt"));
        s.key(Ek::Down);
        assert_eq!(s.cur_id().as_deref(), Some("Docs"));
        s.key(Ek::End);
        assert_eq!(s.list.selected, 3);
        s.key(Ek::Home);
        assert_eq!(s.list.selected, 0);
    }

    #[test]
    fn sort_cycle_is_stable_and_keeps_selection() {
        let mut s = loaded();
        s.key(Ek::End); // zeta.txt
        s.key(Ek::Char('s'));
        assert_eq!(s.sort, SortKey::Size);
        // folders by name first, then files by size ascending: zeta(10), alpha(2000)
        assert_eq!(ids(&s), ["Docs", "src", "zeta.txt", "alpha.md"]);
        assert_eq!(s.cur_id().as_deref(), Some("zeta.txt"));
        s.key(Ek::Char('s'));
        assert_eq!(s.sort, SortKey::Modified);
        assert_eq!(ids(&s), ["src", "Docs", "zeta.txt", "alpha.md"]);
        s.key(Ek::Char('s'));
        assert_eq!(s.sort, SortKey::Newest);
        assert_eq!(ids(&s), ["Docs", "src", "alpha.md", "zeta.txt"]);
        s.key(Ek::Char('s'));
        assert_eq!(s.sort, SortKey::Name);
        assert_eq!(ids(&s), ["Docs", "src", "alpha.md", "zeta.txt"]);
        // header click on the same column flips the direction
        s.set_sort(SortKey::Size, true);
        s.set_sort(SortKey::Size, true);
        assert!(s.desc);
        assert_eq!(ids(&s), ["src", "Docs", "alpha.md", "zeta.txt"]);
        assert!(s.list.columns[1].title.contains('▼'));
    }

    #[test]
    fn filter_keeps_ancestors_and_esc_clears_then_leaves() {
        let mut s = loaded();
        s.key(Ek::Down);
        s.key(Ek::Enter);
        s.apply(Done::Dir { epoch: 0, id: "src".into(), res: Ok(listing(vec![ent("main.rs", false, 1, 1), ent("util.rs", false, 1, 1)])) });
        s.key(Ek::Char('f'));
        for c in "main".chars() {
            assert_eq!(s.key(Ek::Char(c)), None);
        }
        assert_eq!(ids(&s), ["src", "src/main.rs"]);
        assert_eq!(s.cur_id().as_deref(), Some("src/main.rs"), "the first real match is selected, not its ancestor");
        // keys are text while typing: 'q' does not leave
        assert_eq!(s.key(Ek::Char('q')), None);
        assert!(s.rows.is_empty());
        s.key(Ek::Backspace);
        assert_eq!(s.cur_id().as_deref(), Some("src/main.rs"));
        s.key(Ek::Enter); // accept: selection stays, filter still narrows
        assert!(!s.filter.as_ref().unwrap().editing);
        assert_eq!(ids(&s), ["src", "src/main.rs"]);
        assert_eq!(s.key(Ek::Esc), None);
        assert!(s.filter.is_none());
        assert_eq!(s.rows.len(), 6);
        assert_eq!(s.key(Ek::Esc), Some(Act::Leave));
    }

    #[test]
    fn changed_only_uses_git_marks_and_selection_survives() {
        let mut s = loaded();
        s.key(Ek::Char('g'));
        assert!(s.message.as_ref().unwrap().1, "no repo, no toggle");
        s.apply(Done::Marks { epoch: 0, res: Ok(Some(marks(&[("src/main.rs", GitMark::Modified), ("alpha.md", GitMark::Untracked)]))) });
        assert_eq!(s.counts.1, 2);
        assert_eq!(s.branch(), Some("main"));
        let r = s.rows.iter().find(|r| r.id == "alpha.md").unwrap();
        assert_eq!(r.mark, Some(GitMark::Untracked));
        let src = s.rows.iter().find(|r| r.id == "src").unwrap();
        assert!(src.dirty);
        s.key(Ek::End);
        s.key(Ek::Char('g'));
        assert!(s.changed_only);
        assert_eq!(ids(&s), ["src", "alpha.md"], "src is dirty even when unloaded; zeta/Docs are clean");
        s.key(Ek::Char('g'));
        assert_eq!(s.rows.len(), 4);
    }

    #[test]
    fn marks_for_a_subfolder_root_use_the_prefix() {
        let mut s = ExplorerState::new("/repo/app".into(), None, Settings { csv_rows: 10, read_cap: 1024, open_cfg: OpenCfg::default(), pct: 40 });
        s.apply(Done::Dir { epoch: 0, id: String::new(), res: Ok(listing(vec![ent("a.rs", false, 1, 1), ent("b.rs", false, 1, 1)])) });
        let mut m = marks(&[("app/a.rs", GitMark::Added), ("other/c.rs", GitMark::Modified)]);
        m.repo_root = "/repo".into();
        s.apply(Done::Marks { epoch: 0, res: Ok(Some(m)) });
        assert_eq!(s.rows.iter().find(|r| r.id == "a.rs").unwrap().mark, Some(GitMark::Added));
        assert_eq!(s.rows.iter().find(|r| r.id == "b.rs").unwrap().mark, None);
        assert_eq!(s.counts.1, 1, "only changes under this folder count");
    }

    #[test]
    fn refresh_keeps_selection_and_open_folders() {
        let mut s = loaded();
        s.key(Ek::Down);
        s.key(Ek::Enter);
        s.apply(Done::Dir { epoch: 0, id: "src".into(), res: Ok(listing(vec![ent("main.rs", false, 1, 1), ent("old.rs", false, 1, 1)])) });
        s.key(Ek::Down);
        s.key(Ek::Down);
        assert_eq!(s.cur_id().as_deref(), Some("src/old.rs"));
        s.key(Ek::Char('r'));
        assert_eq!(s.epoch, 1);
        assert_eq!(s.queue, vec![Req::Dir(String::new()), Req::Dir("src".into()), Req::Marks, Req::Read("src/old.rs".into())]);
        // stale answers are ignored
        s.apply(Done::Dir { epoch: 0, id: String::new(), res: Ok(listing(vec![])) });
        assert_eq!(s.rows.len(), 6);
        s.apply(Done::Dir { epoch: 1, id: "src".into(), res: Ok(listing(vec![ent("main.rs", false, 1, 1)])) });
        // old.rs vanished: the selection falls back to its parent folder
        assert_eq!(s.cur_id().as_deref(), Some("src"));
        assert!(s.rows.iter().any(|r| r.id == "src/main.rs"));
    }

    #[test]
    fn key_table_actions() {
        let mut s = loaded();
        assert_eq!(s.key(Ek::Char('c')), Some(Act::Copy { text: "Docs".into(), what: "relative path" }));
        assert_eq!(s.key(Ek::Char('P')), Some(Act::Copy { text: "/p/Docs".into(), what: "full path" }));
        assert_eq!(s.key(Ek::Char('t')), Some(Act::NewTab("/p/Docs".into())));
        assert_eq!(s.key(Ek::Char('a')), Some(Act::Agent("/p/Docs".into())));
        assert_eq!(s.key(Ek::Char('o')), None);
        assert!(s.message.is_some());
        s.key(Ek::End);
        assert_eq!(s.key(Ek::Char('E')), Some(Act::Editor { file: "/p/zeta.txt".into(), dir: "/p".into() }));
        assert_eq!(s.key(Ek::Char('O')), Some(Act::OpenFile("/p/zeta.txt".into())));
        // the first Enter stays in the explorer, the second opens
        assert_eq!(s.key(Ek::Enter), None);
        assert!(s.focus_detail);
        assert_eq!(s.key(Ek::Enter), Some(Act::OpenFile("/p/zeta.txt".into())));
        assert_eq!(s.key(Ek::Char('t')), Some(Act::NewTab("/p".into())));
        assert_eq!(s.key(Ek::Char('q')), Some(Act::Leave));
        assert_eq!(s.key(Ek::Menu), Some(Act::Leave));
        assert_eq!(s.key(Ek::Esc), Some(Act::Leave));
    }

    #[test]
    fn enter_previews_first_and_moving_forgets_it() {
        let mut s = loaded();
        s.key(Ek::End);
        assert_eq!(s.key(Ek::Enter), None);
        assert_eq!(s.key(Ek::Esc), None, "Esc returns to the tree, not out of the explorer");
        assert!(!s.focus_detail);
        assert_eq!(s.key(Ek::Enter), None, "a fresh first Enter after Esc");
        s.key(Ek::Esc);
        s.key(Ek::Up);
        s.key(Ek::Down);
        assert_eq!(s.key(Ek::Enter), None, "moving away and back starts over");
        assert_eq!(s.key(Ek::Enter), Some(Act::OpenFile("/p/zeta.txt".into())));
        // narrow terminal: no detail pane, so Enter opens at once
        s.prepare(90, 36);
        assert_eq!(s.key(Ek::Enter), Some(Act::OpenFile("/p/zeta.txt".into())));
    }

    #[test]
    fn plain_letters_start_the_filter_but_commands_stay_commands() {
        let mut s = loaded();
        assert!(s.filter.is_none());
        s.key(Ek::Char('z'));
        let f = s.filter.as_ref().unwrap();
        assert!(f.editing);
        assert_eq!(f.text, "z");
        assert_eq!(s.cur_id().as_deref(), Some("zeta.txt"));
        s.key(Ek::Char('e'));
        assert_eq!(s.filter.as_ref().unwrap().text, "ze", "while typing, command letters are text");
        s.key(Ek::Esc);
        assert!(s.filter.is_none());
        // a command letter does not filter
        s.key(Ek::Char('h'));
        assert!(s.filter.is_none() && s.show_hidden);
        // digits and uppercase start it too, keeping their case
        s.key(Ek::Char('A'));
        assert!(s.filter.is_none(), "A is the agent command");
        s.key(Ek::Char('1'));
        assert_eq!(s.filter.as_ref().unwrap().text, "1");
        s.key(Ek::Esc);
        s.key(Ek::Char('D'));
        assert_eq!(s.filter.as_ref().unwrap().text, "D");
        // F still opens an empty prompt
        s.key(Ek::Esc);
        s.key(Ek::Char('f'));
        assert_eq!(s.filter.as_ref().unwrap().text, "");
    }

    #[test]
    fn newest_first_sort_and_cycle() {
        let mut s = loaded();
        assert_eq!(ids(&s), ["Docs", "src", "alpha.md", "zeta.txt"]);
        s.key(Ek::Char('s'));
        s.key(Ek::Char('s'));
        assert_eq!(s.sort, SortKey::Modified);
        s.key(Ek::Char('s'));
        assert_eq!(s.sort, SortKey::Newest);
        // folders first, then the newest file first
        assert_eq!(ids(&s), ["Docs", "src", "alpha.md", "zeta.txt"]);
        assert_eq!(s.list.columns[2].title, "MODIFIED ▼");
        s.key(Ek::Char('s'));
        assert_eq!(s.sort, SortKey::Name);
        assert_eq!(sort_name(SortKey::Newest), "newest first");
    }

    #[test]
    fn changed_since_open_marks_new_and_edited_files() {
        let mut s = loaded();
        assert!(s.rows.iter().all(|r| !r.new), "nothing is new right after opening");
        s.key(Ek::Char('r'));
        let e = s.epoch;
        s.apply(Done::Dir {
            epoch: e,
            id: String::new(),
            res: Ok(listing(vec![
                ent("zeta.txt", false, 10, 900_000),
                ent("src", true, 0, 800_000),
                ent("Docs", true, 0, 950_000),
                ent("alpha.md", false, 2000, 995_000),
                ent("fresh.txt", false, 1, 996_000),
            ])),
        });
        let new = |s: &ExplorerState, id: &str| s.rows.iter().find(|r| r.id == id).unwrap().new;
        assert!(new(&s, "alpha.md"), "mtime changed");
        assert!(new(&s, "fresh.txt"), "appeared after the first listing");
        assert!(!new(&s, "zeta.txt"));
        assert!(!new(&s, "src"), "folders are never marked");
    }

    #[test]
    fn marks_persist_and_send_as_mentions() {
        let mut s = loaded();
        // nothing marked: M sends the selected item
        assert_eq!(s.key(Ek::Char('m')), Some(Act::Send { rels: vec!["Docs".into()], root: "/p".into() }));
        s.key(Ek::Char(' '));
        assert_eq!(s.cur_id().as_deref(), Some("src"), "Space marks and moves down");
        s.key(Ek::Right);
        s.apply(Done::Dir { epoch: 0, id: "src".into(), res: Ok(listing(vec![ent("main.rs", false, 100, 1)])) });
        s.key(Ek::Right);
        s.key(Ek::Char(' '));
        assert!(s.rows.iter().find(|r| r.id == "Docs").unwrap().picked);
        assert!(s.rows.iter().find(|r| r.id == "src/main.rs").unwrap().picked);
        // folding keeps the marks
        s.key(Ek::Left);
        s.key(Ek::Left);
        assert_eq!(s.picked.len(), 2);
        assert_eq!(s.key(Ek::Char('M')), Some(Act::Send { rels: vec!["Docs".into(), "src/main.rs".into()], root: "/p".into() }));
        s.sent();
        assert!(s.picked.is_empty());
        // Space toggles off
        s.key(Ek::Home);
        s.key(Ek::Char(' '));
        s.key(Ek::Home);
        s.key(Ek::Char(' '));
        assert!(s.picked.is_empty());
    }

    #[test]
    fn nested_file_actions_use_its_folder() {
        let mut s = loaded();
        s.key(Ek::Down);
        s.key(Ek::Enter);
        s.apply(Done::Dir { epoch: 0, id: "src".into(), res: Ok(listing(vec![ent("it's.rs", false, 1, 1)])) });
        s.key(Ek::Down);
        assert_eq!(s.key(Ek::Char('t')), Some(Act::NewTab("/p/src".into())));
        assert_eq!(s.key(Ek::Char('o')), Some(Act::OpenFile("/p/src/it's.rs".into())));
        assert_eq!(s.key(Ek::Char('e')), Some(Act::Editor { file: "/p/src/it's.rs".into(), dir: "/p/src".into() }));
        assert_eq!(editor_command("/p/src/it's.rs"), "${EDITOR:-vi} -- '/p/src/it'\\''s.rs'\n");
    }

    #[test]
    fn detail_follows_selection_and_scrolls() {
        let mut s = loaded();
        // Docs is a folder, not loaded: a silent load is queued
        assert!(s.queue.contains(&Req::Dir("Docs".into())));
        s.apply(Done::Dir { epoch: 0, id: "Docs".into(), res: Ok(listing((0..40).map(|i| ent(&format!("f{i:02}.txt"), false, 10, 1)).collect())) });
        assert!(!s.rows.iter().any(|r| r.id == "Docs/f00.txt"), "loading for the detail does not expand");
        match &s.detail {
            DState::Lines(l) => assert!(l.len() > 40),
            _ => panic!("folder summary expected"),
        }
        s.prepare(120, 20);
        s.key(Ek::Char('j'));
        s.key(Ek::Char('j'));
        assert_eq!(s.detail_pos(), 2);
        s.key(Ek::Char('k'));
        assert_eq!(s.detail_pos(), 1);
        // a file asks for a read and shows a preview once it arrives
        s.key(Ek::End);
        assert!(s.queue.contains(&Req::Read("zeta.txt".into())));
        s.apply(Done::Read { epoch: 0, id: "zeta.txt".into(), res: Ok((b"hello\nworld\n".to_vec(), 12)) });
        assert!(matches!(s.detail, DState::File(_)));
        assert!(s.source_line().starts_with("Source: zeta.txt · 10 B · modified "), "{}", s.source_line());
        // a stale answer for another file is dropped
        s.apply(Done::Read { epoch: 0, id: "other".into(), res: Err("x".into()) });
        assert!(matches!(s.detail, DState::File(_)));
    }

    #[test]
    fn native_files_get_an_info_card() {
        let mut s = loaded();
        s.key(Ek::Down);
        s.key(Ek::Down);
        let id = s.cur_id().unwrap();
        assert_eq!(id, "alpha.md");
        s.apply(Done::Read { epoch: 0, id, res: Ok((vec![0x89, b'P', 0, 0], 4)) });
        match &s.detail {
            DState::Lines(l) => assert!(l.iter().any(|x| x.spans.iter().any(|sp| sp.content.contains("Enter asks before opening it natively")))),
            _ => panic!("card expected"),
        }
    }

    #[test]
    fn keys_decode_arrows_and_text() {
        assert_eq!(decode(b"\x1b[C\x1b[D\x1b[1;2A", 0), vec![Ek::Right, Ek::Left, Ek::Up]);
        assert_eq!(decode(b"\x1b[5~\x1b[6~\x1bOH\x1b[F", 0), vec![Ek::PgUp, Ek::PgDn, Ek::Home, Ek::End]);
        assert_eq!(decode(b"o\x1b\r\0", 0), vec![Ek::Char('o'), Ek::Esc, Ek::Enter, Ek::Menu]);
        assert_eq!(decode("é日".as_bytes(), 0), vec![Ek::Char('é'), Ek::Char('日')]);
    }

    #[test]
    fn synthetic_link_is_a_plain_path() {
        let l = path_link("/p/a b.rs");
        assert_eq!(l.kind, crate::links::LinkKind::Path { path: "/p/a b.rs".into(), line: None, col: None });
    }

    #[test]
    fn narrow_terminal_collapses_the_detail_and_tab_does_not_focus_it() {
        let mut s = loaded();
        s.prepare(90, 30);
        assert_eq!(s.lay.single, Some(Pane::Left));
        s.key(Ek::Tab);
        assert!(!s.focus_detail);
        s.prepare(120, 30);
        s.key(Ek::Tab);
        assert!(s.focus_detail);
        s.key(Ek::Esc);
        assert!(!s.focus_detail);
    }
}
