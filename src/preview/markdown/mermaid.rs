//! Mermaid blocks in markdown: detect the diagram type, guard the renderer (size caps, panics, width) and
//! cache the result. The drawing itself is the `mermaid-text` crate (Unicode box characters, pure Rust).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use unicode_width::UnicodeWidthStr;

/// Largest block (bytes of source) that is drawn.
pub const MAX_BYTES: usize = 20 * 1024;
/// Most nodes that are drawn.
pub const MAX_NODES: usize = 400;
/// Panes narrower than this show the source.
pub const MIN_WIDTH: usize = 40;
/// Cached diagrams kept (cleared when full).
const CACHE_MAX: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagram {
    /// Shown in the border title after `mermaid ·`.
    pub title: String,
    pub lines: Vec<String>,
}

pub type Drawn = Result<Diagram, String>;

pub fn is_mermaid(lang: &str) -> bool {
    lang.eq_ignore_ascii_case("mermaid")
}

/// Source without CRLF, tabs, YAML front matter and `%%` comment lines (this includes `%%{init: ...}%%`).
pub fn prepare(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.first().is_some_and(|l| l.trim() == "---")
        && let Some(end) = lines.iter().skip(1).position(|l| l.trim() == "---")
    {
        lines.drain(..=end + 1);
    }
    lines.retain(|l| !l.trim_start().starts_with("%%"));
    lines.join("\n")
}

/// Diagram type from the first meaningful line of already prepared source.
pub fn detect_type(prepared: &str) -> Option<&'static str> {
    let first = prepared.lines().map(str::trim).find(|l| !l.is_empty())?;
    let word = first.split(|c: char| c.is_whitespace() || c == ';').next()?.to_ascii_lowercase();
    Some(match word.as_str() {
        "graph" | "flowchart" | "flowchart-elk" => "flowchart",
        "sequencediagram" => "sequence",
        "classdiagram" | "classdiagram-v2" => "class",
        "statediagram" | "statediagram-v2" => "state",
        "erdiagram" => "er",
        "gantt" => "gantt",
        "pie" => "pie",
        "gitgraph" => "git graph",
        "journey" => "journey",
        "timeline" => "timeline",
        "mindmap" => "mindmap",
        "quadrantchart" => "quadrant",
        "requirementdiagram" => "requirement",
        "sankey-beta" | "sankey" => "sankey",
        "xychart-beta" | "xychart" => "xy chart",
        "block-beta" | "block" => "block",
        "packet-beta" | "packet" => "packet",
        "architecture-beta" | "architecture" => "architecture",
        _ => return None,
    })
}

fn keep(word: &mut String, seen: &mut std::collections::HashSet<String>, skip: &[&str]) {
    let w = std::mem::take(word);
    if !w.is_empty() && !skip.contains(&w.to_ascii_lowercase().as_str()) {
        seen.insert(w);
    }
}

/// Rough node count: distinct identifiers outside labels for graph-like types, lines for the others.
pub fn count_nodes(prepared: &str, kind: &str) -> usize {
    if !matches!(kind, "flowchart" | "state" | "class" | "er" | "block" | "architecture") {
        return prepared.lines().filter(|l| !l.trim().is_empty()).count();
    }
    const WORDS: &[&str] = &[
        "graph", "flowchart", "subgraph", "end", "direction", "style", "classdef", "class", "linkstyle", "click", "td", "tb", "bt", "lr", "rl",
        "statediagram", "statediagram-v2", "classdiagram", "erdiagram", "state", "as", "note", "of", "over",
    ];
    let mut seen = std::collections::HashSet::new();
    let mut depth = 0usize;
    let (mut quoted, mut pipe) = (false, false);
    let mut word = String::new();
    for c in prepared.chars() {
        if quoted {
            quoted = c != '"';
            continue;
        }
        match c {
            '"' => {
                keep(&mut word, &mut seen, WORDS);
                quoted = true;
            }
            '[' | '(' | '{' => {
                keep(&mut word, &mut seen, WORDS);
                depth += 1;
            }
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            '|' if pipe => {
                pipe = false;
                depth = depth.saturating_sub(1);
            }
            '|' if depth == 0 => {
                keep(&mut word, &mut seen, WORDS);
                pipe = true;
                depth = 1;
            }
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c == '_' => word.push(c),
            _ => keep(&mut word, &mut seen, WORDS),
        }
    }
    keep(&mut word, &mut seen, WORDS);
    seen.len()
}

thread_local! {
    static GUARDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// True while [`guarded`] runs on this thread; the panic hook stays quiet then (it would otherwise
/// restore the terminal and exit).
pub fn in_guard() -> bool {
    GUARDED.with(|g| g.get())
}

/// Run `f`, turning a panic into an error.
pub fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    GUARDED.with(|g| g.set(true));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    GUARDED.with(|g| g.set(false));
    r.map_err(|_| "renderer failed".to_string())
}

fn text_width(s: &str) -> usize {
    s.lines().map(|l| l.width()).max().unwrap_or(0)
}

/// Same source with the flow direction of a `graph`/`flowchart` header changed to top-down.
fn top_down(prepared: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut done = false;
    for l in prepared.lines() {
        if !done && !l.trim().is_empty() {
            done = true;
            let mut parts: Vec<&str> = l.split_whitespace().collect();
            if parts.len() >= 2 && matches!(parts[1].trim_end_matches(';'), "LR" | "RL") {
                parts[1] = "TD";
                out.push(parts.join(" "));
                continue;
            }
            return None;
        }
        out.push(l.to_string());
    }
    Some(out.join("\n"))
}

fn short(msg: &str) -> String {
    let one = msg.lines().next().unwrap_or("").trim();
    let one = one.strip_prefix("parse error: ").unwrap_or(one);
    let mut s: String = one.chars().take(60).collect();
    if one.chars().count() > 60 {
        s.push('…');
    }
    s
}

/// Draw `text` for a block `pane` columns wide (the room the markdown renderer gives the block).
fn draw(text: &str, pane: usize, f: &dyn Fn(&str, usize) -> Result<String, String>) -> Drawn {
    if text.len() > MAX_BYTES {
        return Err(format!("block over {} KB", MAX_BYTES / 1024));
    }
    let prepared = prepare(text);
    if prepared.trim().is_empty() {
        return Err("empty diagram".into());
    }
    let Some(kind) = detect_type(&prepared) else {
        let first = prepared.split_whitespace().next().unwrap_or("");
        return Err(format!("unknown diagram type '{}'", first.chars().take(24).collect::<String>()));
    };
    if pane < MIN_WIDTH {
        return Err(format!("pane under {MIN_WIDTH} columns"));
    }
    let n = count_nodes(&prepared, kind);
    if n > MAX_NODES {
        return Err(format!("over {MAX_NODES} nodes"));
    }
    let inner = pane - 4;
    let run = |src: &str| guarded(|| f(src, inner)).and_then(|r| r.map_err(|e| short(&e)));
    let mut title = kind.to_string();
    let mut out = run(&prepared)?;
    if text_width(&out) > inner
        && kind == "flowchart"
        && let Some(td) = top_down(&prepared)
        && let Ok(alt) = run(&td)
        && text_width(&alt) < text_width(&out)
    {
        out = alt;
        title.push_str(" · laid out top-down");
    }
    let wide = text_width(&out);
    if wide > inner {
        return Err(format!("{wide} columns, pane has {inner}"));
    }
    let mut lines: Vec<String> = out.lines().map(|l| l.trim_end().to_string()).collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    if lines.is_empty() {
        return Err("empty diagram".into());
    }
    Ok(Diagram { title, lines })
}

fn real(src: &str, width: usize) -> Result<String, String> {
    mermaid_text::render_with_width(src, Some(width)).map_err(|e| e.to_string())
}

pub type Key = (u64, usize, &'static str);

/// Cache key: hash of the block text, pane width, theme name.
pub fn cache_key(text: &str, width: usize, theme: &'static str) -> Key {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    (h.finish(), width, theme)
}

static CACHE: Mutex<Option<HashMap<Key, Drawn>>> = Mutex::new(None);

/// The diagram for `text` at `pane` columns, from the cache when this block was drawn before.
pub fn render(text: &str, pane: usize) -> Drawn {
    let key = cache_key(text, pane, crate::theme::theme().name);
    if let Ok(g) = CACHE.lock()
        && let Some(hit) = g.as_ref().and_then(|m| m.get(&key))
    {
        return hit.clone();
    }
    let out = draw(text, pane, &real);
    if let Ok(mut g) = CACHE.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() >= CACHE_MAX {
            m.clear();
        }
        m.insert(key, out.clone());
    }
    out
}

#[cfg(test)]
pub fn cached(text: &str, pane: usize) -> bool {
    let key = cache_key(text, pane, crate::theme::theme().name);
    CACHE.lock().ok().is_some_and(|g| g.as_ref().is_some_and(|m| m.contains_key(&key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOW: &str = "graph LR\n  A[Build] --> B[Test] --> C[Deploy]\n";

    #[test]
    fn detects_types() {
        let t = |s: &str| detect_type(&prepare(s));
        assert_eq!(t(FLOW), Some("flowchart"));
        assert_eq!(t("flowchart TD\nA-->B"), Some("flowchart"));
        assert_eq!(t("sequenceDiagram\nA->>B: hi"), Some("sequence"));
        assert_eq!(t("stateDiagram-v2\n[*] --> A"), Some("state"));
        assert_eq!(t("gitGraph\ncommit"), Some("git graph"));
        assert_eq!(t("%%{init: {\"theme\":\"dark\"}}%%\n%% c\n\ngraph TD\nA-->B"), Some("flowchart"));
        assert_eq!(t("---\ntitle: x\n---\nerDiagram\nA ||--o{ B : has"), Some("er"));
        assert_eq!(t("graph;A-->B"), Some("flowchart"));
        assert_eq!(t("hello world"), None);
        assert_eq!(t(""), None);
    }

    #[test]
    fn prepare_strips_comments_crlf_and_tabs() {
        assert_eq!(prepare("%%{init: x}%%\r\ngraph LR\r\n\tA-->B\r\n  %% note\r\n"), "graph LR\n    A-->B");
    }

    #[test]
    fn counts_nodes_outside_labels() {
        let n = count_nodes(&prepare("flowchart LR\n  A[One two three] --> B{x y}\n  B -->|yes there| C((z))\n  subgraph S\n  D\n  end"), "flowchart");
        assert_eq!(n, 5, "A B C D S");
        assert_eq!(count_nodes("pie\n\"a\":1\n\"b\":2\n", "pie"), 3);
    }

    #[test]
    fn caps_size_nodes_and_width() {
        let big = format!("graph TD\n{}", "A-->B\n".repeat(MAX_BYTES / 6));
        assert!(draw(&big, 80, &real).unwrap_err().contains("KB"));
        let many: String = std::iter::once("graph TD\n".to_string()).chain((0..=MAX_NODES).map(|i| format!("N{i} --> N{}\n", i + 1))).collect();
        assert!(many.len() < MAX_BYTES);
        assert!(draw(&many, 80, &real).unwrap_err().contains("nodes"));
        assert!(draw(FLOW, 39, &real).unwrap_err().contains("under 40"));
        assert!(draw(FLOW, 40, &real).is_ok() || draw(FLOW, 40, &real).unwrap_err().contains("columns"));
        assert_eq!(draw("  \n", 80, &real).unwrap_err(), "empty diagram");
        assert!(draw("what\nis this", 80, &real).unwrap_err().contains("unknown diagram type 'what'"));
    }

    #[test]
    fn renderer_errors_and_panics_become_reasons() {
        let bad = |_: &str, _: usize| -> Result<String, String> { Err("parse error: nope\nsecond line".into()) };
        assert_eq!(draw(FLOW, 80, &bad).unwrap_err(), "nope");
        let boom = |_: &str, _: usize| -> Result<String, String> { panic!("boom") };
        assert_eq!(draw(FLOW, 80, &boom).unwrap_err(), "renderer failed");
        assert!(!in_guard(), "guard flag cleared after a panic");
    }

    #[test]
    fn wide_flowchart_is_laid_out_top_down_or_rejected() {
        let wide: String = std::iter::once("flowchart LR\n".to_string()).chain((0..12).map(|i| format!("  N{i}[Node number {i}] --> N{}[Node number {}]\n", i + 1, i + 1))).collect();
        let d = draw(&wide, 80, &real).expect("fits top-down");
        assert!(d.title.contains("top-down"), "{}", d.title);
        assert!(d.lines.iter().all(|l| l.width() <= 76));
        let seen = std::cell::RefCell::new(Vec::new());
        let always_wide = |s: &str, _: usize| -> Result<String, String> {
            seen.borrow_mut().push(s.lines().next().unwrap_or("").to_string());
            Ok("x".repeat(200))
        };
        assert!(draw(&wide, 80, &always_wide).unwrap_err().contains("200 columns"));
        assert_eq!(*seen.borrow(), vec!["flowchart LR".to_string(), "flowchart TD".to_string()]);
    }

    #[test]
    fn cache_key_tracks_text_width_and_theme() {
        let a = cache_key("graph LR\nA-->B", 80, "dark");
        assert_eq!(a, cache_key("graph LR\nA-->B", 80, "dark"));
        assert_ne!(a, cache_key("graph LR\nA-->C", 80, "dark"));
        assert_ne!(a, cache_key("graph LR\nA-->B", 81, "dark"));
        assert_ne!(a, cache_key("graph LR\nA-->B", 80, "light"));
        let text = "graph LR\n  Cached1 --> Cached2\n";
        assert!(!cached(text, 77));
        let first = render(text, 77);
        assert!(cached(text, 77));
        assert_eq!(first, render(text, 77));
    }
}
