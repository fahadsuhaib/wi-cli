//! Pure tree model, no UI. Node ids are '/'-joined paths (`src/ui/mod.rs`), so an id also says
//! where the node lives. Directories can be loaded lazily: `expand` reports when the caller must
//! feed children with `set_children`.

use std::cmp::Ordering;

pub struct Node<T> {
    pub id: String,
    pub label: String,
    pub is_dir: bool,
    pub expanded: bool,
    /// Children are present (always true for files and for dirs that were fed).
    pub loaded: bool,
    pub children: Vec<Node<T>>,
    pub payload: T,
}

impl<T> Node<T> {
    pub fn file(id: impl Into<String>, label: impl Into<String>, payload: T) -> Self {
        Node { id: id.into(), label: label.into(), is_dir: false, expanded: false, loaded: true, children: Vec::new(), payload }
    }
    /// A directory whose children are not loaded yet.
    pub fn dir(id: impl Into<String>, label: impl Into<String>, payload: T) -> Self {
        Node { id: id.into(), label: label.into(), is_dir: true, expanded: false, loaded: false, children: Vec::new(), payload }
    }
    pub fn with_children(mut self, children: Vec<Node<T>>) -> Self {
        self.children = children;
        self.loaded = true;
        self
    }
    pub fn expanded(mut self) -> Self {
        self.expanded = true;
        self
    }
}

/// Dirs first, then case-insensitive by label.
pub fn dirs_first<T>(a: &Node<T>, b: &Node<T>) -> Ordering {
    b.is_dir.cmp(&a.is_dir).then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase())).then_with(|| a.label.cmp(&b.label))
}

/// One visible row of the flattened tree.
pub struct FlatRow<'a, T> {
    pub node: &'a Node<T>,
    pub depth: usize,
    /// A fold glyph applies: a directory that has children or may still get some.
    pub has_children: bool,
    pub is_last: bool,
    /// For each ancestor (outermost first): was it the last child of its parent? Drives guides.
    pub ancestors_last: Vec<bool>,
}

/// Result of `expand_all_to`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reveal {
    Done,
    /// This directory (id) must be loaded first; call again after `set_children`.
    NeedsLoad(String),
    NotFound,
}

pub struct Tree<T> {
    pub roots: Vec<Node<T>>,
    sorter: fn(&Node<T>, &Node<T>) -> Ordering,
}

fn is_ancestor(anc: &str, p: &str) -> bool {
    p.len() > anc.len() && p.starts_with(anc) && (anc.ends_with('/') || p[anc.len()..].starts_with('/'))
}

fn find<'a, T>(nodes: &'a [Node<T>], path: &str) -> Option<&'a Node<T>> {
    for n in nodes {
        if n.id == path {
            return Some(n);
        }
        if is_ancestor(&n.id, path) {
            return find(&n.children, path);
        }
    }
    None
}

fn find_mut<'a, T>(nodes: &'a mut [Node<T>], path: &str) -> Option<&'a mut Node<T>> {
    for n in nodes {
        if n.id == path {
            return Some(n);
        }
        if is_ancestor(&n.id, path) {
            return find_mut(&mut n.children, path);
        }
    }
    None
}

fn sort_level<T>(nodes: &mut [Node<T>], cmp: fn(&Node<T>, &Node<T>) -> Ordering) {
    nodes.sort_by(cmp);
    for n in nodes {
        sort_level(&mut n.children, cmp);
    }
}

fn subtree_matches<T>(n: &Node<T>, m: &dyn Fn(&Node<T>) -> bool) -> bool {
    m(n) || n.children.iter().any(|c| subtree_matches(c, m))
}

impl<T> Tree<T> {
    pub fn new(mut roots: Vec<Node<T>>) -> Self {
        sort_level(&mut roots, dirs_first);
        Tree { roots, sorter: dirs_first }
    }

    /// Replace the comparator and re-sort every level.
    pub fn sort_with(&mut self, cmp: fn(&Node<T>, &Node<T>) -> Ordering) {
        self.sorter = cmp;
        sort_level(&mut self.roots, cmp);
    }

    pub fn find_by_path(&self, path: &str) -> Option<&Node<T>> {
        find(&self.roots, path)
    }
    pub fn find_mut(&mut self, path: &str) -> Option<&mut Node<T>> {
        find_mut(&mut self.roots, path)
    }

    /// Id of the parent of `id` (None for a root or an unknown id).
    pub fn parent_id(&self, id: &str) -> Option<String> {
        fn walk<'a, T>(nodes: &'a [Node<T>], id: &str, parent: Option<&'a Node<T>>) -> Option<Option<&'a str>> {
            for n in nodes {
                if n.id == id {
                    return Some(parent.map(|p| p.id.as_str()));
                }
                if is_ancestor(&n.id, id) {
                    return walk(&n.children, id, Some(n));
                }
            }
            None
        }
        walk(&self.roots, id, None).flatten().map(str::to_string)
    }

    /// Feed (lazily loaded) children of `id`; the directory becomes loaded. False if unknown.
    pub fn set_children(&mut self, id: &str, mut children: Vec<Node<T>>) -> bool {
        let sorter = self.sorter;
        let Some(n) = find_mut(&mut self.roots, id) else { return false };
        sort_level(&mut children, sorter);
        n.children = children;
        n.loaded = true;
        true
    }

    /// Expand a directory. True when its children are not loaded yet (caller must feed them).
    pub fn expand(&mut self, id: &str) -> bool {
        match find_mut(&mut self.roots, id) {
            Some(n) if n.is_dir => {
                n.expanded = true;
                !n.loaded
            }
            _ => false,
        }
    }
    pub fn collapse(&mut self, id: &str) {
        if let Some(n) = find_mut(&mut self.roots, id) {
            n.expanded = false;
        }
    }
    /// Toggle a directory; same return as `expand`.
    pub fn toggle(&mut self, id: &str) -> bool {
        match self.find_by_path(id) {
            Some(n) if n.is_dir && n.expanded => {
                self.collapse(id);
                false
            }
            Some(n) if n.is_dir => self.expand(id),
            _ => false,
        }
    }

    /// Expand every ancestor directory of `path` so the node is visible.
    pub fn expand_all_to(&mut self, path: &str) -> Reveal {
        fn go<T>(nodes: &mut [Node<T>], path: &str) -> Reveal {
            for n in nodes {
                if n.id == path {
                    return Reveal::Done;
                }
                if is_ancestor(&n.id, path) {
                    n.expanded = true;
                    if !n.loaded {
                        return Reveal::NeedsLoad(n.id.clone());
                    }
                    return go(&mut n.children, path);
                }
            }
            Reveal::NotFound
        }
        go(&mut self.roots, path)
    }

    /// Visible rows in order. With a filter: nodes that match and the ancestors of matches stay,
    /// folded directories on the way to a match show open, and nothing else is listed.
    pub fn flatten(&self) -> Vec<FlatRow<'_, T>> {
        let mut out = Vec::new();
        Self::walk(&self.roots, 0, &mut Vec::new(), None, &mut out);
        out
    }
    pub fn flatten_filtered(&self, matches: &dyn Fn(&Node<T>) -> bool) -> Vec<FlatRow<'_, T>> {
        let mut out = Vec::new();
        Self::walk(&self.roots, 0, &mut Vec::new(), Some(matches), &mut out);
        out
    }

    /// Visible rows when only some nodes may show (for example hiding dotfiles): a node that does
    /// not pass `keep` is dropped with its whole subtree, and folded directories stay folded
    /// (unlike `flatten_filtered`, which opens the way to matches).
    pub fn flatten_visible(&self, keep: &dyn Fn(&Node<T>) -> bool) -> Vec<FlatRow<'_, T>> {
        let mut out = Vec::new();
        Self::walk_visible(&self.roots, 0, &mut Vec::new(), keep, &mut out);
        out
    }

    fn walk_visible<'a>(nodes: &'a [Node<T>], depth: usize, anc: &mut Vec<bool>, keep: &dyn Fn(&Node<T>) -> bool, out: &mut Vec<FlatRow<'a, T>>) {
        let shown: Vec<&Node<T>> = nodes.iter().filter(|n| keep(n)).collect();
        let count = shown.len();
        for (i, n) in shown.into_iter().enumerate() {
            let is_last = i + 1 == count;
            out.push(FlatRow { node: n, depth, has_children: n.is_dir && (!n.children.is_empty() || !n.loaded), is_last, ancestors_last: anc.clone() });
            if n.expanded {
                anc.push(is_last);
                Self::walk_visible(&n.children, depth + 1, anc, keep, out);
                anc.pop();
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn walk<'a>(nodes: &'a [Node<T>], depth: usize, anc: &mut Vec<bool>, m: Option<&dyn Fn(&Node<T>) -> bool>, out: &mut Vec<FlatRow<'a, T>>) {
        let shown: Vec<&Node<T>> = nodes.iter().filter(|n| m.is_none_or(|m| subtree_matches(n, m))).collect();
        let count = shown.len();
        for (i, n) in shown.into_iter().enumerate() {
            let is_last = i + 1 == count;
            out.push(FlatRow { node: n, depth, has_children: n.is_dir && (!n.children.is_empty() || !n.loaded), is_last, ancestors_last: anc.clone() });
            if n.expanded || (m.is_some() && n.children.iter().any(|c| subtree_matches(c, m.unwrap()))) {
                anc.push(is_last);
                Self::walk(&n.children, depth + 1, anc, m, out);
                anc.pop();
            }
        }
    }

    /// Row to select after the rows changed (fold, filter, reload): the same node, else its
    /// nearest visible ancestor, else the old index clamped.
    pub fn reselect(&self, rows: &[FlatRow<'_, T>], prev_id: &str, prev_index: usize) -> usize {
        let mut id = prev_id.to_string();
        loop {
            if let Some(i) = rows.iter().position(|r| r.node.id == id) {
                return i;
            }
            match self.parent_id(&id) {
                Some(p) => id = p,
                None => return prev_index.min(rows.len().saturating_sub(1)),
            }
        }
    }

    /// Left arrow: the id of the parent row (None at a root).
    pub fn select_parent(&self, id: &str) -> Option<String> {
        self.parent_id(id)
    }
    /// Right arrow on an open directory: the id of its first child row.
    pub fn select_first_child(&self, id: &str) -> Option<String> {
        let n = self.find_by_path(id)?;
        if n.is_dir && n.expanded { n.children.first().map(|c| c.id.clone()) } else { None }
    }
}

/// Glyphs for fold marks and guides; `ascii()` is the fallback for terminals without them.
#[derive(Clone, Copy, Debug)]
pub struct Glyphs {
    pub open: &'static str,
    pub closed: &'static str,
    pub leaf: &'static str,
    pub guide: &'static str,
    pub tee: &'static str,
    pub last: &'static str,
    pub blank: &'static str,
    /// Draw indentation guides (`│ `) and `├ `/`└ ` on leaves, instead of plain indentation.
    pub guides: bool,
}

impl Glyphs {
    pub const fn unicode() -> Self {
        Glyphs { open: "▾ ", closed: "▸ ", leaf: "· ", guide: "│ ", tee: "├ ", last: "└ ", blank: "  ", guides: false }
    }
    pub const fn ascii() -> Self {
        Glyphs { open: "v ", closed: "> ", leaf: "- ", guide: "| ", tee: "+ ", last: "` ", blank: "  ", guides: false }
    }
    pub const fn with_guides(mut self) -> Self {
        self.guides = true;
        self
    }

    /// Indentation plus fold glyph for a row (the text to put before the label).
    pub fn prefix<T>(&self, r: &FlatRow<'_, T>) -> String {
        let mut s = String::new();
        for last in &r.ancestors_last {
            s.push_str(if self.guides && !last { self.guide } else { self.blank });
        }
        s.push_str(if r.node.is_dir && r.has_children {
            if r.node.expanded { self.open } else { self.closed }
        } else if self.guides && r.depth > 0 && !r.node.is_dir {
            if r.is_last { self.last } else { self.tee }
        } else {
            self.leaf
        });
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tree<u32> {
        Tree::new(vec![
            Node::dir("p", "p", 0).expanded().with_children(vec![
                Node::file("p/b.rs", "b.rs", 2),
                Node::file("p/A.md", "A.md", 1),
                Node::dir("p/src", "src", 0).expanded().with_children(vec![Node::file("p/src/main.rs", "main.rs", 3), Node::dir("p/src/deep", "deep", 0).with_children(vec![Node::file("p/src/deep/x.rs", "x.rs", 4)])]),
                Node::dir("p/lazy", "lazy", 0),
            ]),
        ])
    }
    fn ids(t: &Tree<u32>) -> Vec<String> {
        t.flatten().iter().map(|r| r.node.id.clone()).collect()
    }

    #[test]
    fn flatten_sorts_dirs_first_and_reports_shape() {
        let t = sample();
        assert_eq!(ids(&t), ["p", "p/lazy", "p/src", "p/src/deep", "p/src/main.rs", "p/A.md", "p/b.rs"]);
        let rows = t.flatten();
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[3].depth, 2);
        assert!(rows[1].has_children, "unloaded dir may have children");
        assert!(!rows[5].has_children);
        assert!(rows[6].is_last && !rows[5].is_last);
        assert_eq!(rows[4].ancestors_last, vec![true, false]);
    }

    #[test]
    fn flatten_visible_drops_nodes_but_keeps_folds() {
        let mut t = sample();
        t.collapse("p/src");
        let rows = t.flatten_visible(&|n| n.label != "A.md");
        let got: Vec<&str> = rows.iter().map(|r| r.node.id.as_str()).collect();
        assert_eq!(got, ["p", "p/lazy", "p/src", "p/b.rs"]);
    }

    #[test]
    fn fold_unfold_toggle() {
        let mut t = sample();
        t.collapse("p/src");
        assert_eq!(ids(&t), ["p", "p/lazy", "p/src", "p/A.md", "p/b.rs"]);
        assert!(!t.toggle("p/src"));
        assert_eq!(ids(&t).len(), 7);
        assert!(!t.toggle("p/b.rs"), "files do not toggle");
        t.expand("p/src/deep");
        assert!(ids(&t).contains(&"p/src/deep/x.rs".to_string()));
    }

    #[test]
    fn lazy_loading() {
        let mut t = sample();
        assert!(t.expand("p/lazy"), "needs a load");
        assert_eq!(ids(&t).iter().filter(|i| i.starts_with("p/lazy/")).count(), 0);
        assert!(t.set_children("p/lazy", vec![Node::file("p/lazy/z", "z", 9), Node::file("p/lazy/a", "a", 8)]));
        assert!(ids(&t).windows(3).any(|w| w == ["p/lazy", "p/lazy/a", "p/lazy/z"]));
        assert!(!t.expand("p/lazy"));
        assert!(!t.set_children("nope", vec![]));
        let mut t = sample();
        assert_eq!(t.expand_all_to("p/lazy/q/w"), Reveal::NeedsLoad("p/lazy".into()));
        t.set_children("p/lazy", vec![Node::dir("p/lazy/q", "q", 0)]);
        assert_eq!(t.expand_all_to("p/lazy/q/w"), Reveal::NeedsLoad("p/lazy/q".into()));
        assert_eq!(t.expand_all_to("p/zzz"), Reveal::NotFound);
    }

    #[test]
    fn expand_all_to_opens_ancestors() {
        let mut t = sample();
        t.collapse("p/src");
        t.collapse("p");
        assert_eq!(t.expand_all_to("p/src/deep/x.rs"), Reveal::Done);
        let v = ids(&t);
        assert!(v.contains(&"p/src/deep/x.rs".to_string()));
        assert_eq!(t.find_by_path("p/src/deep/x.rs").map(|n| n.payload), Some(4));
        assert!(t.find_by_path("p/src/dee").is_none());
    }

    #[test]
    fn filter_keeps_matches_and_ancestors() {
        let t = sample();
        let rows = t.flatten_filtered(&|n| n.label.contains("x.rs"));
        let v: Vec<_> = rows.iter().map(|r| r.node.id.as_str()).collect();
        // The folded `deep` dir shows open on the way to the match.
        assert_eq!(v, ["p", "p/src", "p/src/deep", "p/src/deep/x.rs"]);
        assert!(rows[3].is_last);
        let none = t.flatten_filtered(&|n| n.label == "nothing");
        assert!(none.is_empty());
        let md = t.flatten_filtered(&|n| n.label.ends_with(".md"));
        assert_eq!(md.len(), 2);
    }

    #[test]
    fn stable_selection_across_reflatten() {
        let mut t = sample();
        let rows = t.flatten();
        let i = t.reselect(&rows, "p/src/main.rs", 4);
        assert_eq!(i, 4);
        drop(rows);
        t.collapse("p/src");
        let rows = t.flatten();
        // The selected file vanished: its folder takes the selection.
        assert_eq!(rows[t.reselect(&rows, "p/src/main.rs", 4)].node.id, "p/src");
        t.collapse("p");
        let rows = t.flatten();
        assert_eq!(rows[t.reselect(&rows, "p/src/main.rs", 4)].node.id, "p");
        assert_eq!(t.reselect(&rows, "unknown", 9), 0);
        assert_eq!(t.reselect(&[], "unknown", 9), 0);
    }

    #[test]
    fn parent_and_child_jumps() {
        let t = sample();
        assert_eq!(t.select_parent("p/src/main.rs").as_deref(), Some("p/src"));
        assert_eq!(t.select_parent("p"), None);
        assert_eq!(t.select_first_child("p/src").as_deref(), Some("p/src/deep"));
        assert_eq!(t.select_first_child("p/src/deep"), None, "closed dir");
        assert_eq!(t.select_first_child("p/A.md"), None);
    }

    #[test]
    fn custom_sorting() {
        let mut t = sample();
        t.sort_with(|a, b| b.payload.cmp(&a.payload));
        assert_eq!(ids(&t)[..3], ["p", "p/b.rs", "p/A.md"].map(String::from));
        // Lazily fed children obey the same comparator.
        t.set_children("p/lazy", vec![Node::file("p/lazy/a", "a", 1), Node::file("p/lazy/b", "b", 5)]);
        t.expand("p/lazy");
        let v = ids(&t);
        let b = v.iter().position(|i| i == "p/lazy/b").unwrap();
        assert_eq!(v[b + 1], "p/lazy/a");
    }

    #[test]
    fn glyph_prefixes() {
        let t = sample();
        let rows = t.flatten();
        let g = Glyphs::unicode();
        assert_eq!(g.prefix(&rows[0]), "▾ ");
        assert_eq!(g.prefix(&rows[1]), "  ▸ ");
        assert_eq!(g.prefix(&rows[3]), "    ▸ ");
        assert_eq!(g.prefix(&rows[4]), "    · ");
        assert_eq!(Glyphs::ascii().prefix(&rows[2]), "  v ");
        let gg = Glyphs::unicode().with_guides();
        assert_eq!(gg.prefix(&rows[4]), "  │ └ ");
        assert_eq!(gg.prefix(&rows[5]), "  └ ".replace("└", "├"));
    }
}
