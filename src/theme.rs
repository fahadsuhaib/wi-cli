//! Colour themes. Every colour wi draws itself comes from a semantic role here, so wi looks the
//! same in every terminal (the terminal's own 16-colour palette is never relied on for chrome).
//! Two built-in themes, `dark` and `light`; true colour is quantised to the xterm 256-colour cube
//! when the terminal does not advertise true colour.

use ratatui::style::Color;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dark,
    Light,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Dark => "dark",
            Kind::Light => "light",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Kind::Dark),
            "light" => Some(Kind::Light),
            _ => None,
        }
    }
    pub fn theme(self) -> Theme {
        match self {
            Kind::Dark => DARK,
            Kind::Light => LIGHT,
        }
    }
}

/// Semantic colour roles. Pairs named `x_bg` / `x_fg` are used together; single roles are
/// foreground colours drawn over `bg` (previews, dialogs) or over the bar they sit in.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub kind: Kind,
    /// Canvas behind previews.
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    // Header, tab strip and status bar.
    pub bar_bg: Color,
    pub bar_fg: Color,
    pub header: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub tab_active_bg: Color,
    pub tab_active_fg: Color,
    pub tab_inactive: Color,
    /// `*` printing.
    pub tab_streaming: Color,
    /// `!` went quiet.
    pub tab_quiet: Color,
    // Menu.
    pub menu_bg: Color,
    pub menu_fg: Color,
    pub menu_border: Color,
    pub menu_sel_bg: Color,
    pub menu_sel_fg: Color,
    pub menu_hotkey: Color,
    // Prompts and dialogs.
    pub dialog_bg: Color,
    pub dialog_fg: Color,
    pub dialog_border: Color,
    pub dialog_dim: Color,
    // Divider between shell and side preview.
    pub divider_focus: Color,
    pub divider_idle: Color,
    // Link labels, selection in previews, the line a `file:line` jump lands on.
    pub hint_bg: Color,
    pub hint_fg: Color,
    pub sel_bg: Color,
    pub sel_fg: Color,
    pub goto_bg: Color,
    // Previews.
    pub gutter: Color,
    pub banner: Color,
    pub md_h1: Color,
    pub md_h2: Color,
    pub md_h3: Color,
    pub md_code: Color,
    pub md_quote: Color,
    pub md_link: Color,
    pub md_bullet: Color,
    pub md_rule: Color,
    /// Background panel of code blocks and inline code.
    pub md_code_bg: Color,
    /// The bar at the start of every block-quote row.
    pub md_quote_bar: Color,
    pub md_task_done: Color,
    pub md_task_open: Color,
    /// Text of the YAML front-matter block.
    pub md_front: Color,
    pub json_key: Color,
    pub json_string: Color,
    pub json_number: Color,
    pub table_header: Color,
    pub table_sep: Color,
    pub diff_add: Color,
    pub diff_remove: Color,
    pub diff_hunk: Color,
    pub diff_file: Color,
    pub diff_untracked: Color,
    pub diff_branch: Color,
    pub error: Color,
    /// Reserved for warnings (no caller yet).
    #[allow(dead_code)]
    pub warning: Color,
    // Widget kit (`src/tui/`): title bar, hint line, badge, column header, tree rows, tones.
    /// Inverted full-width title bar.
    pub title_bg: Color,
    pub title_fg: Color,
    /// Label text of the one-line key hints (`hint_fg` is the link-label colour, a different thing).
    pub hintline_fg: Color,
    /// Coloured badge in the status line (`LOCAL`, a box name) and the header row of a panel.
    pub badge_bg: Color,
    pub badge_fg: Color,
    /// Header row of a column list (`FILES  SIZE  MODIFIED`).
    pub col_header_bg: Color,
    pub col_header_fg: Color,
    /// Fold glyphs and indentation guides of trees.
    #[allow(dead_code)] // used by the explorer view
    pub tree_guide: Color,
    /// Size and date columns.
    #[allow(dead_code)] // used by the explorer view
    pub size_fg: Color,
    #[allow(dead_code)] // used by the explorer view
    pub date_fg: Color,
    /// Selected row of a list (the whole row is painted).
    pub row_sel_bg: Color,
    pub row_sel_fg: Color,
    /// Semantic tones for status segments and cells.
    pub good: Color,
    pub warn: Color,
    pub hot: Color,
    /// The 16 named ANSI colours, used for shell output when `ansi_remap` is on.
    pub ansi: [Color; 16],
}

const fn c(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const DARK: Theme = Theme {
    kind: Kind::Dark,
    bg: c(0x1c1f26),
    fg: c(0xd7dae0),
    dim: c(0x8b93a1),
    bar_bg: c(0x2e3440),
    bar_fg: c(0xeceff4),
    header: c(0xffffff),
    status_bg: c(0x2e3440),
    status_fg: c(0xeceff4),
    tab_active_bg: c(0x7aa2f7),
    tab_active_fg: c(0x11141b),
    tab_inactive: c(0xc0c5d0),
    tab_streaming: c(0xf2cc60),
    tab_quiet: c(0x8fdc7f),
    menu_bg: c(0x262a33),
    menu_fg: c(0xd7dae0),
    menu_border: c(0x7aa2f7),
    menu_sel_bg: c(0x7aa2f7),
    menu_sel_fg: c(0x11141b),
    menu_hotkey: c(0xf2cc60),
    dialog_bg: c(0x262a33),
    dialog_fg: c(0xd7dae0),
    dialog_border: c(0x7aa2f7),
    dialog_dim: c(0x9aa3b2),
    divider_focus: c(0x7aa2f7),
    divider_idle: c(0x6b7484),
    hint_bg: c(0xf2cc60),
    hint_fg: c(0x11141b),
    sel_bg: c(0x3d59a1),
    sel_fg: c(0xffffff),
    goto_bg: c(0x3a4150),
    gutter: c(0x838b9d),
    banner: c(0xf2cc60),
    md_h1: c(0xd69cf5),
    md_h2: c(0x7dcfe0),
    md_h3: c(0x7aa2f7),
    md_code: c(0xa9b1c0),
    md_quote: c(0x838b9d),
    md_link: c(0x6cb6ff),
    md_bullet: c(0xf2cc60),
    md_rule: c(0x6b7484),
    md_code_bg: c(0x2b303b),
    md_quote_bar: c(0x6f8fd6),
    md_task_done: c(0x8fdc7f),
    md_task_open: c(0xc0c5d0),
    md_front: c(0x9aa3b2),
    json_key: c(0x7dcfe0),
    json_string: c(0x98d982),
    json_number: c(0xf2cc60),
    table_header: c(0xeceff4),
    table_sep: c(0x6b7484),
    diff_add: c(0x8fdc7f),
    diff_remove: c(0xf07178),
    diff_hunk: c(0x7dcfe0),
    diff_file: c(0xeceff4),
    diff_untracked: c(0xf2cc60),
    diff_branch: c(0xeceff4),
    error: c(0xf07178),
    warning: c(0xf2cc60),
    title_bg: c(0xd7dae0),
    title_fg: c(0x1c1f26),
    hintline_fg: c(0x8b93a1),
    badge_bg: c(0x7aa2f7),
    badge_fg: c(0x11141b),
    col_header_bg: c(0x2f3a52),
    col_header_fg: c(0xeceff4),
    tree_guide: c(0x6b7484),
    size_fg: c(0xa9b1c0),
    date_fg: c(0x8b93a1),
    row_sel_bg: c(0x34487a),
    row_sel_fg: c(0xffffff),
    good: c(0x8fdc7f),
    warn: c(0xf2cc60),
    hot: c(0xf07178),
    ansi: [
        c(0x3b4252), c(0xf07178), c(0x98d982), c(0xf2cc60), c(0x7aa2f7), c(0xd69cf5), c(0x7dcfe0), c(0xc8ccd4),
        c(0x6b7280), c(0xff8b92), c(0xb4f09a), c(0xffe08a), c(0x9dbcff), c(0xe5b8ff), c(0x9fe3f0), c(0xffffff),
    ],
};

pub const LIGHT: Theme = Theme {
    kind: Kind::Light,
    bg: c(0xfafafa),
    fg: c(0x24292f),
    dim: c(0x636c76),
    bar_bg: c(0xdde1e8),
    bar_fg: c(0x1f2328),
    header: c(0x000000),
    status_bg: c(0xdde1e8),
    status_fg: c(0x1f2328),
    tab_active_bg: c(0x0b57d0),
    tab_active_fg: c(0xffffff),
    tab_inactive: c(0x3b4452),
    tab_streaming: c(0x7a4e00),
    tab_quiet: c(0x1a6b2a),
    menu_bg: c(0xffffff),
    menu_fg: c(0x24292f),
    menu_border: c(0x0b57d0),
    menu_sel_bg: c(0x0b57d0),
    menu_sel_fg: c(0xffffff),
    menu_hotkey: c(0x9a3c00),
    dialog_bg: c(0xffffff),
    dialog_fg: c(0x24292f),
    dialog_border: c(0x0b57d0),
    dialog_dim: c(0x57606a),
    divider_focus: c(0x0b57d0),
    divider_idle: c(0x7d8694),
    hint_bg: c(0xf5c542),
    hint_fg: c(0x1f2328),
    sel_bg: c(0xb6d4fe),
    sel_fg: c(0x1f2328),
    goto_bg: c(0xd9e2f3),
    gutter: c(0x656e78),
    banner: c(0x7a4e00),
    md_h1: c(0x7a3fd0),
    md_h2: c(0x0a7087),
    md_h3: c(0x0b57d0),
    md_code: c(0x57606a),
    md_quote: c(0x656e78),
    md_link: c(0x0b57d0),
    md_bullet: c(0x7a4e00),
    md_rule: c(0x7d8694),
    md_code_bg: c(0xeceff3),
    md_quote_bar: c(0x4f6fc0),
    md_task_done: c(0x1a6b2a),
    md_task_open: c(0x57606a),
    md_front: c(0x57606a),
    json_key: c(0x0a7087),
    json_string: c(0x1a6b2a),
    json_number: c(0x7a4e00),
    table_header: c(0x1f2328),
    table_sep: c(0x7d8694),
    diff_add: c(0x1a7f37),
    diff_remove: c(0xcf222e),
    diff_hunk: c(0x0a7087),
    diff_file: c(0x1f2328),
    diff_untracked: c(0x7a4e00),
    diff_branch: c(0x1f2328),
    error: c(0xcf222e),
    warning: c(0x7a4e00),
    title_bg: c(0x24292f),
    title_fg: c(0xfafafa),
    hintline_fg: c(0x636c76),
    badge_bg: c(0x0b57d0),
    badge_fg: c(0xffffff),
    col_header_bg: c(0xcfd8ea),
    col_header_fg: c(0x1f2328),
    tree_guide: c(0x7d8694),
    size_fg: c(0x4a5361),
    date_fg: c(0x636c76),
    row_sel_bg: c(0xb6d4fe),
    row_sel_fg: c(0x1f2328),
    good: c(0x1a6b2a),
    warn: c(0x7a4e00),
    hot: c(0xcf222e),
    ansi: [
        c(0x24292f), c(0xcf222e), c(0x1a7f37), c(0x7a4e00), c(0x0b57d0), c(0x7a3fd0), c(0x0a7087), c(0x8c959f),
        c(0x6e7781), c(0xe5534b), c(0x2da44e), c(0x9a6700), c(0x218bff), c(0xa475f9), c(0x3192aa), c(0xffffff),
    ],
};

static CURRENT: RwLock<Theme> = RwLock::new(DARK);
static TRUECOLOR: AtomicBool = AtomicBool::new(true);
static ANSI_REMAP: AtomicBool = AtomicBool::new(true);

/// The active theme (a small `Copy` value).
pub fn theme() -> Theme {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

pub fn set_kind(k: Kind) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = k.theme();
}

pub fn kind() -> Kind {
    theme().kind
}

/// Startup configuration: theme, whether the terminal does true colour, and the ANSI remap switch.
pub fn init(k: Kind, truecolor: bool, ansi_remap: bool) {
    set_kind(k);
    TRUECOLOR.store(truecolor, Ordering::Relaxed);
    ANSI_REMAP.store(ansi_remap, Ordering::Relaxed);
}

pub fn truecolor() -> bool {
    TRUECOLOR.load(Ordering::Relaxed)
}

pub fn ansi_remap() -> bool {
    ANSI_REMAP.load(Ordering::Relaxed)
}

/// Does `COLORTERM` say the terminal handles 24-bit colour?
pub fn colorterm_is_truecolor(v: Option<&str>) -> bool {
    v.is_some_and(|v| {
        let v = v.to_ascii_lowercase();
        v.contains("truecolor") || v.contains("24bit")
    })
}

/// The theme colour for a named ANSI colour index (0..=15), when remapping is on.
pub fn remap_ansi(idx: u8) -> Option<Color> {
    (idx < 16 && ansi_remap()).then(|| theme().ansi[idx as usize])
}

// ---- 256-colour quantiser ----

const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2) as u32;
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// Nearest xterm-256 index for an RGB colour. Only the colour cube and grey ramp (16..=255) are
/// used: indexes 0..=15 are whatever the terminal's palette says.
pub fn quantize(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| LEVELS.iter().enumerate().min_by_key(|(_, l)| (**l as i32 - v as i32).abs()).map(|(i, _)| i).unwrap_or(0);
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_idx = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let gray = ((avg as i32 - 8 + 5) / 10).clamp(0, 23) as u8;
    let gv = 8 + 10 * gray;
    if dist((r, g, b), (gv, gv, gv)) < dist((r, g, b), cube) { 232 + gray } else { cube_idx }
}

/// Convert an RGB colour to a 256-colour index (other colours unchanged).
pub fn to_256(col: Color) -> Color {
    match col {
        Color::Rgb(r, g, b) => Color::Indexed(quantize(r, g, b)),
        other => other,
    }
}

// ---- Background detection ----

fn hex_channel(s: &str) -> Option<f64> {
    if s.is_empty() || s.len() > 4 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    let max = (1u32 << (4 * s.len())) - 1;
    Some(v as f64 / max as f64)
}

/// Parse a reply to `ESC ] 11 ; ? BEL`: `ESC ] 11 ; rgb:RRRR/GGGG/BBBB` ended by BEL or ESC \.
/// Each channel has 1-4 hex digits. Returns 0..=1 fractions.
pub fn parse_osc11(reply: &[u8]) -> Option<(f64, f64, f64)> {
    let text = String::from_utf8_lossy(reply);
    let at = text.find("rgb:")?;
    let rest = &text[at + 4..];
    let end = rest.find(['\x07', '\x1b']).unwrap_or(rest.len());
    let mut it = rest[..end].trim().split('/');
    let (r, g, b) = (hex_channel(it.next()?)?, hex_channel(it.next()?)?, hex_channel(it.next()?)?);
    it.next().is_none().then_some((r, g, b))
}

/// Light or dark from a background colour (perceived luminance).
pub fn kind_from_rgb((r, g, b): (f64, f64, f64)) -> Kind {
    if 0.299 * r + 0.587 * g + 0.114 * b > 0.5 { Kind::Light } else { Kind::Dark }
}

/// `COLORFGBG` is `fg;bg` (sometimes `fg;default;bg`) with ANSI indexes: 7 and 9..=15 are light.
pub fn kind_from_colorfgbg(v: &str) -> Option<Kind> {
    let bg: u8 = v.rsplit(';').next()?.trim().parse().ok()?;
    Some(if bg <= 6 || bg == 8 { Kind::Dark } else { Kind::Light })
}

/// Which theme to start with. `config` is the `theme` setting, `saved` the last live choice in the
/// state file, `ask_terminal` is called (at most once) only for `auto` without a saved choice.
/// An explicit config value always wins.
pub fn resolve(config: &str, saved: Option<&str>, ask_terminal: impl FnOnce() -> Option<Kind>, colorfgbg: Option<&str>) -> Kind {
    if let Some(k) = Kind::parse(config) {
        return k;
    }
    if let Some(k) = saved.and_then(Kind::parse) {
        return k;
    }
    auto_kind(ask_terminal(), colorfgbg)
}

/// What `auto` means: the terminal's answer, else `COLORFGBG`, else dark.
pub fn auto_kind(asked: Option<Kind>, colorfgbg: Option<&str>) -> Kind {
    asked.or_else(|| colorfgbg.and_then(kind_from_colorfgbg)).unwrap_or(Kind::Dark)
}

/// The theme setting as the menu cycles it: auto, dark, light, auto.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Auto,
    Dark,
    Light,
}

impl Mode {
    pub fn next(self) -> Mode {
        match self {
            Mode::Auto => Mode::Dark,
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Auto,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }
    /// The value kept in the state file: `auto` keeps nothing, so the next start detects again.
    pub fn saved(self) -> Option<String> {
        match self {
            Mode::Auto => None,
            m => Some(m.name().to_string()),
        }
    }
    /// The mode in force at start: an explicit config theme, else the saved choice, else auto.
    pub fn at_start(config: &str, saved: Option<&str>) -> Mode {
        match Kind::parse(config).or_else(|| saved.and_then(Kind::parse)) {
            Some(Kind::Dark) => Mode::Dark,
            Some(Kind::Light) => Mode::Light,
            None => Mode::Auto,
        }
    }
}

/// What the terminal said its background was at start (0 none, 1 dark, 2 light). Used when the
/// menu switches back to `auto` while wi runs: asking again would race wi's own reading of stdin.
static DETECTED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_detected(k: Option<Kind>) {
    let v = match k {
        None => 0,
        Some(Kind::Dark) => 1,
        Some(Kind::Light) => 2,
    };
    DETECTED.store(v, Ordering::Relaxed);
}

pub fn detected() -> Option<Kind> {
    match DETECTED.load(Ordering::Relaxed) {
        1 => Some(Kind::Dark),
        2 => Some(Kind::Light),
        _ => None,
    }
}

// ---- Contrast ----

#[cfg(test)]
pub fn rgb_of(col: Color) -> (u8, u8, u8) {
    match col {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

/// WCAG contrast ratio between two RGB colours.
#[cfg(test)]
pub fn contrast(a: Color, b: Color) -> f64 {
    let lum = |col: Color| {
        let (r, g, b) = rgb_of(col);
        let f = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
    };
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Kind; 2] = [Kind::Dark, Kind::Light];

    #[test]
    fn text_roles_have_readable_contrast() {
        for k in ALL {
            let t = k.theme();
            let pairs: Vec<(&str, Color, Color)> = vec![
                ("fg/bg", t.fg, t.bg),
                ("dim/bg", t.dim, t.bg),
                ("bar", t.bar_fg, t.bar_bg),
                ("header", t.header, t.bar_bg),
                ("status", t.status_fg, t.status_bg),
                ("tab active", t.tab_active_fg, t.tab_active_bg),
                ("tab inactive", t.tab_inactive, t.bar_bg),
                ("tab streaming", t.tab_streaming, t.bar_bg),
                ("tab quiet", t.tab_quiet, t.bar_bg),
                ("menu", t.menu_fg, t.menu_bg),
                ("menu selected", t.menu_sel_fg, t.menu_sel_bg),
                ("menu hotkey", t.menu_hotkey, t.menu_bg),
                ("dialog", t.dialog_fg, t.dialog_bg),
                ("dialog dim", t.dialog_dim, t.dialog_bg),
                ("hint", t.hint_fg, t.hint_bg),
                ("selection", t.sel_fg, t.sel_bg),
                ("text on goto mark", t.fg, t.goto_bg),
                ("gutter", t.gutter, t.bg),
                ("banner", t.banner, t.bg),
                ("h1", t.md_h1, t.bg),
                ("h2", t.md_h2, t.bg),
                ("h3", t.md_h3, t.bg),
                ("code", t.md_code, t.bg),
                ("quote", t.md_quote, t.bg),
                ("link", t.md_link, t.bg),
                ("bullet", t.md_bullet, t.bg),
                ("code on its panel", t.md_code, t.md_code_bg),
                ("text on code panel", t.fg, t.md_code_bg),
                ("task done", t.md_task_done, t.bg),
                ("task open", t.md_task_open, t.bg),
                ("front matter", t.md_front, t.bg),
                ("json key", t.json_key, t.bg),
                ("json string", t.json_string, t.bg),
                ("json number", t.json_number, t.bg),
                ("table header", t.table_header, t.bg),
                ("diff add", t.diff_add, t.bg),
                ("diff remove", t.diff_remove, t.bg),
                ("diff hunk", t.diff_hunk, t.bg),
                ("diff file", t.diff_file, t.bg),
                ("diff untracked", t.diff_untracked, t.bg),
                ("diff branch", t.diff_branch, t.bg),
                ("error", t.error, t.bg),
                ("warning", t.warning, t.bg),
                ("error on dialog", t.error, t.dialog_bg),
                ("title bar", t.title_fg, t.title_bg),
                ("hint line", t.hintline_fg, t.bg),
                ("hint line keys", t.fg, t.bg),
                ("badge", t.badge_fg, t.badge_bg),
                ("column header", t.col_header_fg, t.col_header_bg),
                ("size", t.size_fg, t.bg),
                ("date", t.date_fg, t.bg),
                ("selected row", t.row_sel_fg, t.row_sel_bg),
                ("good", t.good, t.bg),
                ("warn", t.warn, t.bg),
                ("hot", t.hot, t.bg),
            ];
            for (name, fg, bg) in pairs {
                let r = contrast(fg, bg);
                assert!(r >= 4.5, "{} {name}: contrast {r:.2} < 4.5", k.name());
            }
        }
    }

    #[test]
    fn accent_roles_are_visible() {
        for k in ALL {
            let t = k.theme();
            let pairs: Vec<(&str, Color, Color)> = vec![
                ("menu border", t.menu_border, t.bg),
                ("dialog border", t.dialog_border, t.bg),
                ("divider focus", t.divider_focus, t.bg),
                ("divider idle", t.divider_idle, t.bg),
                ("md rule", t.md_rule, t.bg),
                ("table sep", t.table_sep, t.bg),
                ("goto mark", t.goto_bg, t.bg),
                ("quote bar", t.md_quote_bar, t.bg),
                ("code panel", t.md_code_bg, t.bg),
                ("tree guide", t.tree_guide, t.bg),
                ("badge on canvas", t.badge_bg, t.bg),
                ("column header on canvas", t.col_header_bg, t.bg),
                ("selected row on canvas", t.row_sel_bg, t.bg),
                ("title bar on canvas", t.title_bg, t.bg),
            ];
            for (name, fg, bg) in pairs {
                let r = contrast(fg, bg);
                // The goto-line mark is a background tint: it only has to differ from the canvas.
                let need = if matches!(name, "goto mark" | "code panel" | "column header on canvas" | "selected row on canvas") { 1.1 } else { 3.0 };
                assert!(r >= need, "{} {name}: contrast {r:.2} < {need}", k.name());
            }
            for (i, col) in t.ansi.iter().enumerate() {
                if matches!(i, 0 | 7 | 15) {
                    continue;
                }
                let r = contrast(*col, t.bg);
                assert!(r >= 3.0, "{} ansi {i}: contrast {r:.2} < 3", k.name());
            }
        }
    }

    #[test]
    fn every_theme_defines_every_role_as_rgb_and_themes_differ() {
        for k in ALL {
            let t = k.theme();
            assert_eq!(t.kind, k);
            // The struct has no optional roles, so a missing role cannot compile; check none is a
            // placeholder (non-RGB) colour and that the palette has 16 distinct entries.
            let dbg = format!("{t:?}");
            assert!(!dbg.contains("Reset") && !dbg.contains("Indexed") && !dbg.contains("Black"), "{dbg}");
            let set: std::collections::HashSet<_> = t.ansi.iter().map(|c| rgb_of(*c)).collect();
            assert_eq!(set.len(), 16);
        }
        assert_ne!(DARK.bar_bg, LIGHT.bar_bg);
        assert_ne!(DARK.ansi[1], LIGHT.ansi[1]);
    }

    #[test]
    fn mode_cycles_auto_dark_light_and_saves_nothing_for_auto() {
        assert_eq!(Mode::Auto.next(), Mode::Dark);
        assert_eq!(Mode::Dark.next(), Mode::Light);
        assert_eq!(Mode::Light.next(), Mode::Auto);
        assert_eq!(Mode::Auto.saved(), None);
        assert_eq!(Mode::Dark.saved().as_deref(), Some("dark"));
        assert_eq!(Mode::Light.saved().as_deref(), Some("light"));
    }

    #[test]
    fn mode_at_start_follows_config_then_saved_then_auto() {
        assert_eq!(Mode::at_start("auto", None), Mode::Auto);
        assert_eq!(Mode::at_start("auto", Some("light")), Mode::Light);
        assert_eq!(Mode::at_start("auto", Some("auto")), Mode::Auto);
        assert_eq!(Mode::at_start("dark", Some("light")), Mode::Dark);
        assert_eq!(Mode::at_start("light", None), Mode::Light);
    }

    #[test]
    fn auto_kind_prefers_terminal_then_colorfgbg_then_dark() {
        assert_eq!(auto_kind(Some(Kind::Light), Some("15;0")), Kind::Light);
        assert_eq!(auto_kind(None, Some("0;15")), Kind::Light);
        assert_eq!(auto_kind(None, Some("junk")), Kind::Dark);
        assert_eq!(auto_kind(None, None), Kind::Dark);
        // A saved "auto" (or anything unknown) is not an override.
        assert_eq!(resolve("auto", Some("auto"), || Some(Kind::Light), None), Kind::Light);
    }

    #[test]
    fn detected_round_trips() {
        set_detected(Some(Kind::Light));
        assert_eq!(detected(), Some(Kind::Light));
        set_detected(None);
        assert_eq!(detected(), None);
    }

    #[test]
    fn quantizer_picks_nearest_xterm_index() {
        assert_eq!(quantize(0, 0, 0), 16);
        assert_eq!(quantize(255, 255, 255), 231);
        assert_eq!(quantize(255, 0, 0), 196);
        assert_eq!(quantize(0, 255, 0), 46);
        assert_eq!(quantize(0, 0, 255), 21);
        assert_eq!(quantize(128, 128, 128), 244);
        assert_eq!(quantize(95, 135, 175), 16 + 36 + 6 * 2 + 3);
        assert_eq!(quantize(8, 8, 8), 232);
        assert_eq!(quantize(238, 238, 238), 255);
        for v in [0u8, 17, 60, 100, 200, 250] {
            assert!(quantize(v, v / 2, 255 - v) >= 16);
        }
        assert_eq!(to_256(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        assert_eq!(to_256(Color::Indexed(1)), Color::Indexed(1));
        assert_eq!(to_256(Color::Reset), Color::Reset);
    }

    #[test]
    fn osc11_reply_parsing() {
        let p = |s: &str| parse_osc11(s.as_bytes());
        assert_eq!(p("\x1b]11;rgb:ffff/ffff/ffff\x07"), Some((1.0, 1.0, 1.0)));
        assert_eq!(p("\x1b]11;rgb:0000/0000/0000\x1b\\"), Some((0.0, 0.0, 0.0)));
        assert_eq!(p("\x1b]11;rgb:ff/00/ff\x07"), Some((1.0, 0.0, 1.0)));
        let (r, _, _) = p("\x1b]11;rgb:8/0/0\x07").unwrap();
        assert!((r - 8.0 / 15.0).abs() < 1e-9);
        let (r, _, _) = p("\x1b]11;rgb:1c1c/0/0\x07").unwrap();
        assert!((r - 0x1c1c as f64 / 65535.0).abs() < 1e-9);
        assert_eq!(p("\x1b]11;rgb:zz/00/00\x07"), None);
        assert_eq!(p("\x1b]11;rgb:12345/00/00\x07"), None);
        assert_eq!(p("\x1b]11;rgb:ff/00\x07"), None);
        assert_eq!(p("garbage"), None);
        assert_eq!(p(""), None);
    }

    #[test]
    fn luminance_decides_light_or_dark() {
        assert_eq!(kind_from_rgb((1.0, 1.0, 1.0)), Kind::Light);
        assert_eq!(kind_from_rgb((0.0, 0.0, 0.0)), Kind::Dark);
        assert_eq!(kind_from_rgb((0.98, 0.98, 0.98)), Kind::Light);
        assert_eq!(kind_from_rgb((0.11, 0.12, 0.15)), Kind::Dark);
        assert_eq!(kind_from_rgb((0.0, 0.0, 1.0)), Kind::Dark);
        assert_eq!(kind_from_rgb((1.0, 1.0, 0.0)), Kind::Light);
    }

    #[test]
    fn colorfgbg_parsing() {
        assert_eq!(kind_from_colorfgbg("15;0"), Some(Kind::Dark));
        assert_eq!(kind_from_colorfgbg("0;15"), Some(Kind::Light));
        assert_eq!(kind_from_colorfgbg("0;default;15"), Some(Kind::Light));
        assert_eq!(kind_from_colorfgbg("12;8"), Some(Kind::Dark));
        assert_eq!(kind_from_colorfgbg("0;7"), Some(Kind::Light));
        assert_eq!(kind_from_colorfgbg("default"), None);
        assert_eq!(kind_from_colorfgbg(""), None);
    }

    #[test]
    fn resolution_order() {
        let none = || None;
        assert_eq!(resolve("light", Some("dark"), || panic!("asked"), None), Kind::Light);
        assert_eq!(resolve("auto", Some("light"), || panic!("asked"), None), Kind::Light);
        assert_eq!(resolve("auto", None, || Some(Kind::Light), Some("15;0")), Kind::Light);
        assert_eq!(resolve("auto", None, none, Some("0;15")), Kind::Light);
        assert_eq!(resolve("auto", None, none, None), Kind::Dark);
        assert_eq!(resolve("bogus", None, none, None), Kind::Dark);
    }

    #[test]
    fn colorterm_detection() {
        assert!(colorterm_is_truecolor(Some("truecolor")));
        assert!(colorterm_is_truecolor(Some("24bit")));
        assert!(!colorterm_is_truecolor(Some("256color")));
        assert!(!colorterm_is_truecolor(None));
    }
}
