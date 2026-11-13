//! Pure terminal rendering for `dredge bar`: no I/O, no terminal queries, no async.
//!
//! Every function maps its arguments to strings, so the same input gives the same bytes. The
//! caller decides what the terminal supports ([`Caps`]) and where the text goes.
//!
//! The look: each segment is a pale tinted block filled with a regular dot grid in the
//! category's saturated ink. A 4x4 Bayer matrix picks which sub-cells carry a dot; the dot
//! density depends on whether the segment is focused.

#![forbid(unsafe_code)]

use std::fmt;

use dredge_core::categories::Category;
use dredge_core::output::CategoryRecord;
use dredge_core::util::human_bytes;

/// Color capability of the output terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorTier {
    TrueColor,
    Ansi256,
    Ansi16,
    /// Plain text: ASCII fill, no escape codes. Overrides [`Glyphs`].
    None,
}

/// Cell shape used to draw the dots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyphs {
    /// `▀` with foreground and background: two pixel rows per cell row.
    HalfBlock,
    /// U+2800 patterns: a 2x4 dot grid per cell.
    Braille,
    /// `#` and `.` characters.
    Ascii,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub color: ColorTier,
    pub glyphs: Glyphs,
}

impl Caps {
    /// Plain ASCII, no escapes.
    pub const PLAIN: Caps = Caps {
        color: ColorTier::None,
        glyphs: Glyphs::Ascii,
    };
}

/// The fixed ink of a segment. One per category plus neutral free space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    Apps,
    Developer,
    Photos,
    Movies,
    Documents,
    Music,
    System,
    Caches,
    Trash,
    Other,
    Free,
}

impl Ink {
    /// Palette order: the order categories keep everywhere.
    pub const ALL: [Ink; 11] = [
        Ink::Apps,
        Ink::Developer,
        Ink::Photos,
        Ink::Movies,
        Ink::Documents,
        Ink::Music,
        Ink::System,
        Ink::Caches,
        Ink::Trash,
        Ink::Other,
        Ink::Free,
    ];

    pub fn from_category(c: Category) -> Ink {
        match c {
            Category::Apps => Ink::Apps,
            Category::Developer => Ink::Developer,
            Category::Photos => Ink::Photos,
            Category::Movies => Ink::Movies,
            Category::Music => Ink::Music,
            Category::Documents => Ink::Documents,
            Category::Caches => Ink::Caches,
            Category::Trash => Ink::Trash,
            Category::System => Ink::System,
            Category::Other => Ink::Other,
        }
    }

    /// Lowercase name as used in JSON and on the command line, e.g. `developer`.
    pub fn name(self) -> &'static str {
        match self {
            Ink::Apps => "apps",
            Ink::Developer => "developer",
            Ink::Photos => "photos",
            Ink::Movies => "movies",
            Ink::Documents => "documents",
            Ink::Music => "music",
            Ink::System => "system",
            Ink::Caches => "caches",
            Ink::Trash => "trash",
            Ink::Other => "other",
            Ink::Free => "free",
        }
    }

    pub fn from_name(s: &str) -> Option<Ink> {
        let s = s.trim().to_ascii_lowercase();
        Ink::ALL.into_iter().find(|i| i.name() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Ink::Apps => "Apps",
            Ink::Developer => "Developer",
            Ink::Photos => "Photos",
            Ink::Movies => "Movies",
            Ink::Documents => "Documents",
            Ink::Music => "Music",
            Ink::System => "System",
            Ink::Caches => "Caches",
            Ink::Trash => "Trash",
            Ink::Other => "Other",
            Ink::Free => "Free",
        }
    }

    /// The character that stands for this ink in plain (colorless) output.
    pub fn plain_glyph(self) -> char {
        match self {
            Ink::Apps => '#',
            Ink::Developer => '=',
            Ink::Photos => '+',
            Ink::Movies => '%',
            Ink::Documents => '*',
            Ink::Music => '@',
            Ink::System => '&',
            Ink::Caches => ':',
            Ink::Trash => ';',
            Ink::Other => '!',
            Ink::Free => '.',
        }
    }

    /// Saturated ink color (the dots).
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Ink::Apps => (37, 99, 235),       // blue
            Ink::Developer => (22, 163, 74),  // green
            Ink::Photos => (219, 39, 119),    // pink
            Ink::Movies => (147, 51, 234),    // purple
            Ink::Documents => (13, 148, 136), // teal
            Ink::Music => (217, 119, 6),      // amber
            Ink::System => (107, 114, 128),   // grey
            Ink::Caches => (234, 88, 12),     // orange
            Ink::Trash => (220, 38, 38),      // red
            Ink::Other => (71, 85, 105),      // slate
            Ink::Free => (156, 163, 175),     // neutral grey
        }
    }

    /// Pale ground behind the dots: the ink mixed 20/80 with near-white.
    fn tint(self) -> (u8, u8, u8) {
        let (r, g, b) = self.rgb();
        let mix = |c: u8| ((u16::from(c) * 20 + 250 * 80) / 100) as u8;
        (mix(r), mix(g), mix(b))
    }

    /// Foreground SGR code for the 16-color tier.
    fn sgr16(self) -> u8 {
        match self {
            Ink::Apps => 34,
            Ink::Developer => 32,
            Ink::Photos => 95,
            Ink::Movies => 35,
            Ink::Documents => 36,
            Ink::Music => 33,
            Ink::System => 90,
            Ink::Caches => 91,
            Ink::Trash => 31,
            Ink::Other => 37,
            Ink::Free => 90,
        }
    }
}

/// One slice of the bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub label: String,
    pub bytes: u64,
    pub ink: Ink,
}

impl Segment {
    pub fn new(label: impl Into<String>, bytes: u64, ink: Ink) -> Segment {
        Segment {
            label: sanitize(&label.into()),
            bytes,
            ink,
        }
    }

    pub fn from_record(r: &CategoryRecord) -> Segment {
        let ink = Ink::from_category(r.category);
        Segment::new(ink.label(), r.allocated_bytes, ink)
    }
}

/// One output line: pre-rendered text, with escape codes only when the tier allows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line(pub String);

impl Line {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Everything `render_report` composes.
#[derive(Clone, Debug)]
pub struct Report {
    pub title: String,
    pub used_bytes: u64,
    /// Capacity to show "used of total"; `None` prints used only.
    pub total_bytes: Option<u64>,
    pub segments: Vec<Segment>,
    /// Text of the top cleanup hint, shown as the last line.
    pub insight: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Dithering

/// 4x4 Bayer matrix: a pixel carries a dot when its entry is below the segment's dot count.
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Dots per 16 pixels. Focused is nearly solid, the rest recede, free space is a faint grain.
const DOTS_FOCUSED: u8 = 15;
const DOTS_UNFOCUSED: u8 = 3;
const DOTS_NO_FOCUS: u8 = 6;
const DOTS_FREE: u8 = 1;

fn dots(ink: Ink, idx: usize, focus: Option<usize>) -> u8 {
    match (ink, focus) {
        (Ink::Free, _) => DOTS_FREE,
        (_, None) => DOTS_NO_FOCUS,
        (_, Some(f)) if f == idx => DOTS_FOCUSED,
        _ => DOTS_UNFOCUSED,
    }
}

fn on(x: usize, y: usize, k: u8) -> bool {
    BAYER[y % 4][x % 4] < k
}

// ---------------------------------------------------------------------------------------------
// Widths

/// Cell widths proportional to bytes, summing to exactly `width` (largest remainder). A
/// non-empty segment gets at least one cell when there is room. All-zero input becomes one
/// zero-width list, and the caller fills the bar with free-space ground.
fn widths(segments: &[Segment], width: usize) -> Vec<usize> {
    let total: u128 = segments.iter().map(|s| u128::from(s.bytes)).sum();
    if total == 0 || width == 0 {
        return vec![0; segments.len()];
    }
    let w = width as u128;
    let mut out: Vec<usize> = segments
        .iter()
        .map(|s| (u128::from(s.bytes) * w / total) as usize)
        .collect();
    let mut rem: Vec<(u128, usize)> = segments
        .iter()
        .enumerate()
        .map(|(i, s)| (u128::from(s.bytes) * w % total, i))
        .collect();
    // Largest remainder first; ties go to the earlier segment.
    rem.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let left = width - out.iter().sum::<usize>();
    for &(_, i) in rem.iter().take(left) {
        out[i] += 1;
    }
    // Give every non-empty segment a cell, taking from the widest one.
    for i in 0..out.len() {
        if segments[i].bytes > 0 && out[i] == 0 {
            let donor = (0..out.len())
                .filter(|&j| out[j] > 1)
                .max_by(|&a, &b| out[a].cmp(&out[b]).then(b.cmp(&a)));
            match donor {
                Some(j) => {
                    out[j] -= 1;
                    out[i] = 1;
                }
                None => break,
            }
        }
    }
    out
}

/// Segment index of each column. All-zero input yields `None` columns (free-space ground).
fn columns(segments: &[Segment], width: usize) -> Vec<Option<usize>> {
    let mut cols = Vec::with_capacity(width);
    for (i, w) in widths(segments, width).into_iter().enumerate() {
        cols.extend(std::iter::repeat_n(Some(i), w));
    }
    cols.resize(width, None);
    cols
}

// ---------------------------------------------------------------------------------------------
// Colors and escapes

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Rgb(u8, u8, u8),
    Idx(u8),
    /// A 16-color foreground code; the background code is this plus 10.
    Sgr(u8),
}

fn cube(v: u8) -> u8 {
    // xterm cube levels are 0, 95, 135, 175, 215, 255.
    [0_u8, 95, 135, 175, 215, 255]
        .iter()
        .enumerate()
        .min_by_key(|(_, level)| v.abs_diff(**level))
        .map_or(0, |(i, _)| i as u8)
}

fn color(tier: ColorTier, ink: Ink, ground: bool) -> Col {
    let (r, g, b) = if ground { ink.tint() } else { ink.rgb() };
    match tier {
        ColorTier::TrueColor | ColorTier::None => Col::Rgb(r, g, b),
        ColorTier::Ansi256 => Col::Idx(16 + 36 * cube(r) + 6 * cube(g) + cube(b)),
        // No pale tints in 16 colors: dots on black.
        ColorTier::Ansi16 if ground => Col::Sgr(30),
        ColorTier::Ansi16 => Col::Sgr(ink.sgr16()),
    }
}

fn params(c: Col, bg: bool) -> String {
    match c {
        Col::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", if bg { 48 } else { 38 }),
        Col::Idx(n) => format!("{};5;{n}", if bg { 48 } else { 38 }),
        Col::Sgr(n) => (if bg { n + 10 } else { n }).to_string(),
    }
}

/// Appends characters, writing an escape only when the style changes.
struct Painter {
    out: String,
    cur: Option<(Col, Col)>,
}

impl Painter {
    fn new() -> Painter {
        Painter {
            out: String::new(),
            cur: None,
        }
    }

    fn cell(&mut self, fg: Col, bg: Col, ch: char) {
        if self.cur != Some((fg, bg)) {
            self.out
                .push_str(&format!("\x1b[{};{}m", params(fg, false), params(bg, true)));
            self.cur = Some((fg, bg));
        }
        self.out.push(ch);
    }

    fn finish(mut self) -> Line {
        if self.cur.is_some() {
            self.out.push_str("\x1b[0m");
        }
        Line(self.out)
    }
}

// ---------------------------------------------------------------------------------------------
// Bar

/// Pixel rows of text per bar for the colored tiers.
fn rows(glyphs: Glyphs) -> usize {
    match glyphs {
        Glyphs::Braille => 1,
        _ => 2,
    }
}

/// Braille dot bit for the dot at (`dx`, `dy`) of a 2x4 cell.
fn braille_bit(dx: usize, dy: usize) -> u32 {
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (0, _) => 0x40,
        _ => 0x80,
    }
}

/// One of a segment's two colors: its saturated ink (the dots) or its pale ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shade {
    pub ink: Ink,
    pub ground: bool,
}

impl Shade {
    /// True-color value.
    pub fn rgb(self) -> (u8, u8, u8) {
        if self.ground {
            self.ink.tint()
        } else {
            self.ink.rgb()
        }
    }

    /// Nearest xterm 256-color palette index.
    pub fn xterm256(self) -> u8 {
        let (r, g, b) = self.rgb();
        16 + 36 * cube(r) + 6 * cube(g) + cube(b)
    }
}

/// One terminal cell of the bar: a glyph with foreground and background shades. This is the
/// structured form of what [`render_bar`] prints, for front ends that draw cells themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Shade,
    pub bg: Shade,
}

/// The dithered bar as cells: `rows` lines (1 for braille, else 2), each `width` cells wide.
/// `focus` is an index into `segments`. [`render_bar`] paints exactly these cells.
pub fn bar_cells(
    segments: &[Segment],
    width: usize,
    focus: Option<usize>,
    glyphs: Glyphs,
) -> Vec<Vec<Cell>> {
    let cols = columns(segments, width);
    let seg_of = |c: Option<usize>| -> (Ink, u8) {
        match c {
            Some(i) => (segments[i].ink, dots(segments[i].ink, i, focus)),
            None => (Ink::Free, DOTS_FREE),
        }
    };
    (0..rows(glyphs))
        .map(|r| {
            cols.iter()
                .enumerate()
                .map(|(x, &c)| {
                    let (ink, k) = seg_of(c);
                    let dot = Shade { ink, ground: false };
                    let ground = Shade { ink, ground: true };
                    match glyphs {
                        Glyphs::HalfBlock => {
                            let pick = |y: usize| if on(x, y, k) { dot } else { ground };
                            Cell {
                                ch: '\u{2580}',
                                fg: pick(2 * r),
                                bg: pick(2 * r + 1),
                            }
                        }
                        Glyphs::Braille => {
                            let mut bits = 0;
                            for dy in 0..4 {
                                for dx in 0..2 {
                                    if on(2 * x + dx, dy, k) {
                                        bits |= braille_bit(dx, dy);
                                    }
                                }
                            }
                            Cell {
                                ch: char::from_u32(0x2800 + bits).unwrap_or(' '),
                                fg: dot,
                                bg: ground,
                            }
                        }
                        Glyphs::Ascii => Cell {
                            ch: if on(x, 2 * r, k) { '#' } else { '.' },
                            fg: dot,
                            bg: ground,
                        },
                    }
                })
                .collect()
        })
        .collect()
}

/// Draws the bar: 1 or 2 lines, each exactly `width` cells wide. `focus` is an index into
/// `segments`.
pub fn render_bar(
    segments: &[Segment],
    width: usize,
    focus: Option<usize>,
    caps: Caps,
) -> Vec<Line> {
    if caps.color == ColorTier::None {
        let line: String = columns(segments, width)
            .iter()
            .map(|c| c.map_or(Ink::Free, |i| segments[i].ink).plain_glyph())
            .collect();
        return vec![Line(line)];
    }

    let tier = caps.color;
    bar_cells(segments, width, focus, caps.glyphs)
        .into_iter()
        .map(|row| {
            let mut p = Painter::new();
            for c in row {
                p.cell(
                    color(tier, c.fg.ink, c.fg.ground),
                    color(tier, c.bg.ink, c.bg.ground),
                    c.ch,
                );
            }
            p.finish()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Sanitizing

/// True for characters that must never reach a terminal as-is: Unicode `Cc` (C0, DEL, C1),
/// line/paragraph separators and bidi override/isolate controls.
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Replaces every unsafe character with a visible `\u{..}` escape (lowercase hex). All
/// caller-supplied text (paths, labels, commands) goes through this before it is placed in a
/// [`Line`], so the only escape bytes in the output are the renderer's own SGR sequences.
pub fn sanitize(s: &str) -> String {
    if !s.chars().any(is_unsafe_char) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if is_unsafe_char(c) {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Legend

const MINI_BAR: usize = 8;

fn pad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    format!("{s}{}", " ".repeat(w.saturating_sub(n)))
}

fn lpad(s: &str, w: usize) -> String {
    let n = s.chars().count();
    format!("{}{s}", " ".repeat(w.saturating_sub(n)))
}

/// Legend grid: swatch in the segment's ink, then name and size (bold when focused), row-major, up to three columns.
/// Plain tier: `Apps 52.6 GiB  ####....` with the mini bar showing the share of the total.
pub fn render_legend(
    segments: &[Segment],
    width: usize,
    focus: Option<usize>,
    caps: Caps,
) -> Vec<Line> {
    let shown: Vec<(usize, &Segment)> = segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.bytes > 0)
        .collect();
    if shown.is_empty() {
        return Vec::new();
    }
    let total: u128 = segments.iter().map(|s| u128::from(s.bytes)).sum();
    let labels: Vec<String> = shown.iter().map(|(_, s)| sanitize(&s.label)).collect();
    let lw = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let humans: Vec<String> = shown.iter().map(|(_, s)| human_bytes(s.bytes)).collect();
    let hw = humans.iter().map(|h| h.chars().count()).max().unwrap_or(0);
    let plain = caps.color == ColorTier::None;

    // (visible text, painted text)
    let entries: Vec<(String, String)> = shown
        .iter()
        .zip(humans.iter().zip(&labels))
        .map(|((i, s), (h, label))| {
            let body = format!("{} {}", pad(label, lw), lpad(h, hw));
            if plain {
                let filled = ((u128::from(s.bytes) * MINI_BAR as u128 + total / 2) / total)
                    .clamp(1, MINI_BAR as u128) as usize;
                let fill = s.ink.plain_glyph();
                let mini = format!(
                    "{}{}",
                    fill.to_string().repeat(filled),
                    ".".repeat(MINI_BAR - filled)
                );
                let text = format!("{} {body}  {mini}", s.ink.plain_glyph());
                (text.clone(), text)
            } else {
                let swatch = if caps.glyphs == Glyphs::Ascii {
                    '#'
                } else {
                    '\u{25a0}'
                };
                let fg = params(color(caps.color, s.ink, false), false);
                let mut painted = format!("\x1b[{fg}m{swatch}\x1b[0m");
                painted.push(' ');
                if focus == Some(*i) {
                    painted.push_str(&format!("\x1b[1m{body}\x1b[22m"));
                } else {
                    painted.push_str(&body);
                }
                (format!("{swatch} {body}"), painted)
            }
        })
        .collect();

    let colw = entries
        .iter()
        .map(|(v, _)| v.chars().count())
        .max()
        .unwrap_or(0)
        + 3;
    let ncols = (width / colw.max(1)).clamp(1, 3).min(entries.len());
    entries
        .chunks(ncols)
        .map(|chunk| {
            let mut line = String::new();
            for (k, (vis, painted)) in chunk.iter().enumerate() {
                line.push_str(painted);
                if k + 1 < chunk.len() {
                    line.push_str(&" ".repeat(colw - vis.chars().count()));
                }
            }
            Line(line.trim_end().to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Report

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut t: String = s.chars().take(width - 1).collect();
    t.push('\u{2026}');
    t
}

/// Header (used of total), bar, legend grid and the top insight line.
pub fn render_report(report: &Report, width: usize, focus: Option<usize>, caps: Caps) -> Vec<Line> {
    let title = sanitize(&report.title);
    let mut stats = human_bytes(report.used_bytes);
    if let Some(total) = report.total_bytes.filter(|t| *t > 0) {
        let pct = report.used_bytes as f64 / total as f64 * 100.0;
        stats.push_str(&format!(" of {} ({pct:.1}%)", human_bytes(total)));
    }
    // Widths count Unicode scalar values, not terminal display cells; wide and combining
    // characters can therefore occupy a different number of cells on screen.
    let suffix = format!("  {stats}");
    let title_width = width.saturating_sub(suffix.chars().count());
    let header = if title.chars().count() + suffix.chars().count() <= width {
        format!("{title}{suffix}")
    } else if title_width >= 4 {
        let tail: String = title
            .chars()
            .rev()
            .take(title_width - 1)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("…{tail}{suffix}")
    } else {
        let tail: String = title
            .chars()
            .rev()
            .take(width.saturating_sub(1))
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if width == 0 {
            String::new()
        } else {
            format!("…{tail}")
        }
    };
    let mut out = vec![Line(header)];
    out.extend(render_bar(&report.segments, width, focus, caps));
    let legend = render_legend(&report.segments, width, focus, caps);
    if !legend.is_empty() {
        out.push(Line(String::new()));
        out.extend(legend);
    }
    if let Some(tip) = &report.insight {
        out.push(Line(String::new()));
        out.push(Line(truncate(&format!("Tip: {}", sanitize(tip)), width)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs() -> Vec<Segment> {
        vec![
            Segment::new("Apps", 52_600_000_000, Ink::Apps),
            Segment::new("Developer", 31_000_000_000, Ink::Developer),
            Segment::new("Photos", 18_000_000_000, Ink::Photos),
            Segment::new("Trash", 1_000, Ink::Trash),
            Segment::new("Free", 120_000_000_000, Ink::Free),
        ]
    }

    #[test]
    fn widths_sum_exactly() {
        for w in [0, 1, 2, 5, 13, 40, 100, 257] {
            let c = columns(&segs(), w);
            assert_eq!(c.len(), w);
            assert_eq!(
                widths(&segs(), w).iter().sum::<usize>(),
                if w == 0 { 0 } else { w }
            );
        }
    }

    #[test]
    fn tiny_segment_gets_a_cell() {
        let w = widths(&segs(), 40);
        assert!(w[3] >= 1, "{w:?}");
    }

    #[test]
    fn empty_and_zero_do_not_panic() {
        for caps in [
            Caps::PLAIN,
            Caps {
                color: ColorTier::TrueColor,
                glyphs: Glyphs::HalfBlock,
            },
        ] {
            assert_eq!(
                render_bar(&[], 10, None, caps)[0].0.matches('\x1b').count() > 0,
                caps.color != ColorTier::None
            );
            let z = [Segment::new("Apps", 0, Ink::Apps)];
            let _ = render_bar(&z, 10, Some(0), caps);
            let _ = render_bar(&z, 0, Some(3), caps);
            assert!(render_legend(&z, 40, None, caps).is_empty());
        }
    }

    #[test]
    fn single_segment_fills_bar() {
        let s = [Segment::new("Apps", 5, Ink::Apps)];
        assert_eq!(widths(&s, 37), vec![37]);
        assert_eq!(render_bar(&s, 37, None, Caps::PLAIN)[0].0, "#".repeat(37));
    }

    #[test]
    fn plain_has_no_escapes_and_matches_example() {
        let rep = Report {
            title: "/data".into(),
            used_bytes: 100,
            total_bytes: None,
            segments: segs(),
            insight: None,
        };
        let text: String = render_report(&rep, 60, None, Caps::PLAIN)
            .iter()
            .map(|l| l.0.clone() + "\n")
            .collect();
        assert!(!text.contains('\x1b'));
        assert!(text.contains("Apps"));
    }

    #[test]
    fn deterministic() {
        let caps = Caps {
            color: ColorTier::Ansi256,
            glyphs: Glyphs::Braille,
        };
        assert_eq!(
            render_bar(&segs(), 50, Some(1), caps),
            render_bar(&segs(), 50, Some(1), caps)
        );
    }

    #[test]
    fn focused_is_denser_than_unfocused() {
        assert!((0..16).filter(|&i| on(i % 4, i / 4, DOTS_FOCUSED)).count() == 15);
        assert!(
            (0..16)
                .filter(|&i| on(i % 4, i / 4, DOTS_UNFOCUSED))
                .count()
                == 3
        );
        assert!((0..16).filter(|&i| on(i % 4, i / 4, DOTS_FREE)).count() == 1);
    }

    #[test]
    fn plain_glyphs_identify_segments_and_legend() {
        let segments: Vec<_> = Ink::ALL
            .iter()
            .map(|&ink| Segment::new(ink.label(), 1, ink))
            .collect();
        let bar = &render_bar(&segments, 22, None, Caps::PLAIN)[0].0;
        assert_eq!(bar, "##==++%%**@@&&::;;!!..");
        for (segment, line) in segments
            .iter()
            .zip(render_legend(&segments, 1, None, Caps::PLAIN))
        {
            assert!(line.0.starts_with(segment.ink.plain_glyph()));
        }
    }

    #[test]
    fn headers_keep_path_tail_at_small_widths() {
        let report = Report {
            title: "/a/long/école/名前".into(),
            used_bytes: 1,
            total_bytes: None,
            segments: vec![],
            insight: None,
        };
        for width in 0..30 {
            let header = &render_report(&report, width, None, Caps::PLAIN)[0].0;
            assert!(header.chars().count() <= width, "{width}: {header}");
            if (4..15).contains(&width) {
                assert!(header.contains("名前"), "{width}: {header}");
            }
        }
    }

    #[test]
    fn large_values_and_remainder_ties_are_safe() {
        let s = vec![
            Segment::new("a", u64::MAX, Ink::Apps),
            Segment::new("b", u64::MAX, Ink::Developer),
            Segment::new("c", u64::MAX, Ink::Photos),
        ];
        assert_eq!(widths(&s, 5), vec![2, 2, 1]);
        for width in 0..=2 {
            assert_eq!(
                render_bar(&s, width, None, Caps::PLAIN)[0]
                    .0
                    .chars()
                    .count(),
                width
            );
            let _ = render_legend(&s, width, None, Caps::PLAIN);
        }
    }

    #[test]
    fn color_cube_uses_xterm_levels() {
        assert_eq!(
            [0, 47, 48, 94, 95, 114, 115, 134, 135, 255].map(cube),
            [0, 0, 1, 1, 1, 1, 1, 2, 2, 5]
        );
        assert_eq!(params(Col::Sgr(95), true), "105");
        assert_eq!(params(Col::Idx(196), false), "38;5;196");
    }

    #[test]
    fn braille_and_bayer_layouts_are_correct() {
        assert_eq!(
            (0..4).map(|y| braille_bit(0, y)).collect::<Vec<_>>(),
            [1, 2, 4, 64]
        );
        assert_eq!(
            (0..4).map(|y| braille_bit(1, y)).collect::<Vec<_>>(),
            [8, 16, 32, 128]
        );
        assert_eq!(
            BAYER,
            [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]
        );
    }

    #[test]
    fn sanitize_escapes_controls_and_bidi() {
        assert_eq!(sanitize("a\x1bb"), "a\\u{1b}b");
        assert_eq!(sanitize("\u{202e}x\u{85}"), "\\u{202e}x\\u{85}");
        assert_eq!(sanitize("café 名前"), "café 名前");
    }

    const NASTY: &str = "a\x1b[31m\x07b\rc\0d\x7fe\tf\ng\u{202e}h\u{2028}i";

    fn assert_clean(lines: &[Line], sgr_ok: bool) {
        for l in lines {
            let mut rest = l.0.as_str();
            let mut cleaned = String::new();
            while let Some(i) = rest.find("\x1b[") {
                cleaned.push_str(&rest[..i]);
                assert!(sgr_ok, "escape in plain output: {:?}", l.0);
                let after = &rest[i + 2..];
                let end = after.find('m').expect("unterminated SGR");
                assert!(
                    after[..end].chars().all(|c| c.is_ascii_digit() || c == ';'),
                    "non-SGR escape: {:?}",
                    l.0
                );
                rest = &after[end + 1..];
            }
            cleaned.push_str(rest);
            assert!(
                cleaned.chars().all(|c| !is_unsafe_char(c)),
                "unsafe char in {:?}",
                l.0
            );
            assert!(!l.0.contains('\n'));
        }
    }

    #[test]
    fn caller_text_is_sanitized_in_every_tier() {
        for color in [
            ColorTier::None,
            ColorTier::Ansi16,
            ColorTier::Ansi256,
            ColorTier::TrueColor,
        ] {
            let caps = Caps {
                color,
                glyphs: Glyphs::HalfBlock,
            };
            let mut segments = segs();
            segments[0].label = NASTY.into(); // pub field set directly
            let rep = Report {
                title: format!("/p/{NASTY}"),
                used_bytes: 100,
                total_bytes: Some(1000),
                segments,
                insight: Some(format!("rm -rf '{NASTY}'")),
            };
            for width in [20, 60, 200] {
                let lines = render_report(&rep, width, Some(0), caps);
                assert_clean(&lines, color != ColorTier::None);
                for l in &lines {
                    if color == ColorTier::None {
                        assert!(l.0.bytes().all(|b| b >= 0x20 && b != 0x7f), "{:?}", l.0);
                    }
                }
            }
        }
    }

    #[test]
    fn widths_use_sanitized_text() {
        let rep = Report {
            title: "\x1b\x07".into(),
            used_bytes: 1,
            total_bytes: None,
            segments: vec![],
            insight: Some("\x1b".repeat(50)),
        };
        for width in 0..40 {
            for l in render_report(&rep, width, None, Caps::PLAIN) {
                assert!(l.0.chars().count() <= width, "{width}: {:?}", l.0);
            }
        }
    }

    #[test]
    fn ink_names_round_trip() {
        for i in Ink::ALL {
            assert_eq!(Ink::from_name(i.name()), Some(i));
        }
        assert_eq!(Ink::from_name("nope"), None);
    }
}
