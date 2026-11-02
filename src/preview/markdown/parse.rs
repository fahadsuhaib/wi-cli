//! Markdown source to a small document model (blocks and inline runs), independent of the width.
//! Rendering to display lines for a given width lives in the parent module.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// Inline formatting of a run of text.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Fl {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: bool,
    pub dim: bool,
}

#[derive(Clone, Debug)]
pub enum Inl {
    Text(String, Fl),
    /// A hard line break.
    Break,
}

pub type Inlines = Vec<Inl>;

#[derive(Debug)]
pub struct Item {
    pub task: Option<bool>,
    pub blocks: Vec<Node>,
}

#[derive(Debug)]
pub enum Block {
    Heading(u8, Inlines),
    /// A paragraph written as one (loose lists have these inside items).
    Para(Inlines),
    /// Bare text of a tight list item.
    Text(Inlines),
    List { start: Option<u64>, items: Vec<Item>, loose: bool },
    Quote(Vec<Node>),
    Code { lang: String, text: String },
    Table { align: Vec<Alignment>, rows: Vec<Vec<Inlines>> },
    Rule,
    Html(String),
}

#[derive(Debug)]
pub struct Node {
    /// 0-based source line where the block starts.
    pub line: usize,
    pub block: Block,
}

#[derive(Debug, Default)]
pub struct Doc {
    /// Lines of the YAML front matter, when the file starts with one.
    pub front: Option<Vec<String>>,
    pub nodes: Vec<Node>,
}

/// `---` ... `---` at the very top. Returns the front-matter lines, the rest, and how many lines were consumed.
fn split_front(src: &str) -> (Option<Vec<String>>, &str, usize) {
    let mut it = src.split_inclusive('\n');
    if it.next().map(|l| l.trim_end()) != Some("---") {
        return (None, src, 0);
    }
    let mut front = Vec::new();
    let mut used = 1;
    let mut offset = src.find('\n').map(|i| i + 1).unwrap_or(src.len());
    for l in it {
        used += 1;
        offset += l.len();
        let t = l.trim_end();
        if t == "---" || t == "..." {
            return (Some(front), &src[offset..], used);
        }
        front.push(t.to_string());
    }
    (None, src, 0)
}

pub fn parse(src: &str) -> Doc {
    let (front, rest, base) = split_front(src);
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let ev: Vec<(Event, Range<usize>)> = Parser::new_ext(rest, opts).into_offset_iter().collect();
    let mut starts = vec![0usize];
    starts.extend(rest.match_indices('\n').map(|(i, _)| i + 1));
    let mut p = P { ev, i: 0, starts, base };
    let nodes = p.blocks();
    Doc { front, nodes }
}

struct P<'a> {
    ev: Vec<(Event<'a>, Range<usize>)>,
    i: usize,
    starts: Vec<usize>,
    base: usize,
}

fn is_br(html: &str) -> bool {
    let t = html.trim().to_ascii_lowercase();
    let t = t.trim_start_matches('<').trim_end_matches('>').trim_end_matches('/').trim();
    t == "br"
}

impl<'a> P<'a> {
    fn line_of(&self, off: usize) -> usize {
        self.base + self.starts.partition_point(|&s| s <= off).saturating_sub(1)
    }

    fn end(&mut self) {
        if matches!(self.ev.get(self.i), Some((Event::End(_), _))) {
            self.i += 1;
        }
    }

    /// Consume the inline events up to the next block-level event or closing tag.
    fn inline_run(&mut self) -> Inlines {
        let mut out: Inlines = Vec::new();
        let mut fl = Fl::default();
        let mut stack: Vec<Fl> = Vec::new();
        let mut links: Vec<(String, usize)> = Vec::new();
        let mut img: Option<String> = None;
        while let Some((ev, _)) = self.ev.get(self.i) {
            match ev {
                Event::Start(t) => match t {
                    Tag::Emphasis => {
                        stack.push(fl);
                        fl.italic = true;
                    }
                    Tag::Strong => {
                        stack.push(fl);
                        fl.bold = true;
                    }
                    Tag::Strikethrough => {
                        stack.push(fl);
                        fl.strike = true;
                    }
                    Tag::Link { dest_url, .. } => {
                        stack.push(fl);
                        fl.link = true;
                        links.push((dest_url.to_string(), out.len()));
                    }
                    Tag::Image { .. } => img = Some(String::new()),
                    _ => break,
                },
                Event::End(t) => match t {
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => fl = stack.pop().unwrap_or(fl),
                    TagEnd::Link => {
                        fl = stack.pop().unwrap_or(fl);
                        if let Some((url, at)) = links.pop() {
                            let shown: String = out[at..]
                                .iter()
                                .filter_map(|i| match i {
                                    Inl::Text(t, _) => Some(t.as_str()),
                                    Inl::Break => None,
                                })
                                .collect();
                            if !url.is_empty() && url != shown {
                                out.push(Inl::Text(format!(" ({url})"), Fl { dim: true, ..fl }));
                            }
                        }
                    }
                    TagEnd::Image => {
                        if let Some(alt) = img.take() {
                            out.push(Inl::Text(format!("[image: {alt}]"), Fl { dim: true, ..fl }));
                        }
                    }
                    _ => break,
                },
                Event::Text(t) => match img.as_mut() {
                    Some(a) => a.push_str(t),
                    None => out.push(Inl::Text(t.to_string(), fl)),
                },
                Event::Code(t) => match img.as_mut() {
                    Some(a) => a.push_str(t),
                    None => out.push(Inl::Text(t.to_string(), Fl { code: true, ..fl })),
                },
                Event::InlineHtml(h) => {
                    if img.is_none() && is_br(h) {
                        out.push(Inl::Break);
                    }
                }
                Event::SoftBreak => match img.as_mut() {
                    Some(a) => a.push(' '),
                    None => out.push(Inl::Text(" ".into(), fl)),
                },
                Event::HardBreak => {
                    if img.is_none() {
                        out.push(Inl::Break)
                    }
                }
                Event::FootnoteReference(n) => out.push(Inl::Text(format!("[{n}]"), fl)),
                Event::Rule | Event::TaskListMarker(_) => break,
                _ => {}
            }
            self.i += 1;
        }
        out
    }

    fn blocks(&mut self) -> Vec<Node> {
        let mut out = Vec::new();
        while let Some((ev, range)) = self.ev.get(self.i) {
            let line = self.line_of(range.start);
            let ev = ev.clone();
            let block = match ev {
                Event::End(_) => break,
                Event::Start(Tag::Paragraph) => {
                    self.i += 1;
                    let inl = self.inline_run();
                    self.end();
                    Some(Block::Para(inl))
                }
                Event::Start(Tag::Heading { level, .. }) => {
                    self.i += 1;
                    let inl = self.inline_run();
                    self.end();
                    Some(Block::Heading(level as u8, inl))
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    self.i += 1;
                    let inner = self.blocks();
                    self.end();
                    Some(Block::Quote(inner))
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    self.i += 1;
                    let lang = match kind {
                        CodeBlockKind::Fenced(l) => l.split_whitespace().next().unwrap_or("").to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    let mut text = String::new();
                    while let Some((e, _)) = self.ev.get(self.i) {
                        match e {
                            Event::Text(t) => text.push_str(t),
                            Event::End(_) => break,
                            _ => {}
                        }
                        self.i += 1;
                    }
                    self.end();
                    Some(Block::Code { lang, text })
                }
                Event::Start(Tag::List(start)) => {
                    self.i += 1;
                    let mut items = Vec::new();
                    while matches!(self.ev.get(self.i), Some((Event::Start(Tag::Item), _))) {
                        self.i += 1;
                        let mut task = None;
                        if let Some((Event::TaskListMarker(d), _)) = self.ev.get(self.i) {
                            task = Some(*d);
                            self.i += 1;
                        }
                        let blocks = self.blocks();
                        self.end();
                        items.push(Item { task, blocks });
                    }
                    self.end();
                    let loose = items.iter().any(|it| it.blocks.iter().any(|n| matches!(n.block, Block::Para(_))));
                    Some(Block::List { start, items, loose })
                }
                Event::Start(Tag::Table(align)) => {
                    self.i += 1;
                    let mut rows = Vec::new();
                    while let Some((e, _)) = self.ev.get(self.i) {
                        match e {
                            Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => {
                                self.i += 1;
                                let mut row = Vec::new();
                                while matches!(self.ev.get(self.i), Some((Event::Start(Tag::TableCell), _))) {
                                    self.i += 1;
                                    row.push(self.inline_run());
                                    self.end();
                                }
                                self.end();
                                rows.push(row);
                                continue;
                            }
                            Event::End(TagEnd::Table) => break,
                            _ => {}
                        }
                        self.i += 1;
                    }
                    self.end();
                    Some(Block::Table { align, rows })
                }
                Event::Start(Tag::HtmlBlock) => {
                    self.i += 1;
                    let mut s = String::new();
                    while let Some((e, _)) = self.ev.get(self.i) {
                        match e {
                            Event::Html(t) | Event::Text(t) => s.push_str(t),
                            Event::End(_) => break,
                            _ => {}
                        }
                        self.i += 1;
                    }
                    self.end();
                    Some(Block::Html(s))
                }
                Event::Rule => {
                    self.i += 1;
                    Some(Block::Rule)
                }
                Event::TaskListMarker(_) => {
                    self.i += 1;
                    None
                }
                Event::Start(Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. })
                | Event::Text(_)
                | Event::Code(_)
                | Event::InlineHtml(_)
                | Event::Html(_)
                | Event::SoftBreak
                | Event::HardBreak
                | Event::FootnoteReference(_) => {
                    let before = self.i;
                    let inl = self.inline_run();
                    if self.i == before {
                        self.i += 1;
                    }
                    (!inl.is_empty()).then_some(Block::Text(inl))
                }
                Event::Start(_) => {
                    // A container this renderer has no special form for: show its content.
                    self.i += 1;
                    let inner = self.blocks();
                    self.end();
                    out.extend(inner);
                    None
                }
                _ => {
                    self.i += 1;
                    None
                }
            };
            if let Some(block) = block {
                out.push(Node { line, block });
            }
        }
        out
    }
}
