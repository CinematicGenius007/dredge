//! Drawing: `draw(frame, state, theme)` turns a [`State`] into cells. No input, no queries.

use dredge_render::{bar_cells, Caps, Glyphs, Ink, Segment, Shade};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Widget, Wrap};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::state::{
    crumbs, fmt_count, legend_cols, progress_text, Cat, Input, Insight, Level, Overview, Scan,
    State, View, INSIGHT_LINES, MIN_HEIGHT, MIN_WIDTH,
};

/// How colors are emitted. The bar's look is the same as `dredge bar` on every tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    TrueColor,
    Ansi256,
    /// `NO_COLOR`: the plain glyph bar and no colors.
    Plain,
}

impl Theme {
    /// Same environment rules as `dredge bar --color auto` on a terminal.
    pub fn detect() -> Theme {
        let env = |k: &str| std::env::var(k).ok();
        Theme::from_env(
            std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()),
            env("COLORTERM").as_deref(),
        )
    }

    pub fn from_env(no_color: bool, colorterm: Option<&str>) -> Theme {
        if no_color {
            Theme::Plain
        } else if matches!(colorterm, Some("truecolor" | "24bit")) {
            Theme::TrueColor
        } else {
            Theme::Ansi256
        }
    }

    fn color(self, shade: Shade) -> Color {
        match self {
            Theme::TrueColor => {
                let (r, g, b) = shade.rgb();
                Color::Rgb(r, g, b)
            }
            Theme::Ansi256 => Color::Indexed(shade.xterm256()),
            Theme::Plain => Color::Reset,
        }
    }

    fn ink(self, ink: Ink) -> Color {
        self.color(Shade { ink, ground: false })
    }

    /// One accent per meaning, as in the CLI: orange marks a cleanup hint, amber a warning.
    fn accent(self, rgb: (u8, u8, u8), xterm: u8) -> Style {
        match self {
            Theme::TrueColor => Style::new().fg(Color::Rgb(rgb.0, rgb.1, rgb.2)),
            Theme::Ansi256 => Style::new().fg(Color::Indexed(xterm)),
            Theme::Plain => Style::new(),
        }
    }

    fn hint(self) -> Style {
        self.accent((234, 88, 12), 208)
    }

    fn warn(self) -> Style {
        self.accent((229, 165, 10), 214)
    }
}

const DIM: Style = Style::new().fg(Color::DarkGray);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);
const ERROR: Style = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);

pub fn draw(f: &mut Frame, s: &State, theme: Theme) {
    let area = f.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = Paragraph::new(format!(
            "Terminal too small (need {MIN_WIDTH}x{MIN_HEIGHT})"
        ))
        .wrap(Wrap { trim: true });
        f.render_widget(msg, area);
        return;
    }
    let w = area.width;
    let body = Rect::new(0, 1, w, area.height - 2);
    let edge = inset(Rect::new(0, 0, w, 1));
    let buf = f.buffer_mut();
    header(buf, s, edge, theme);
    if s.help {
        help(buf, body);
    } else {
        match s.view {
            View::Overview => overview(buf, s, body, theme),
            View::Paths => paths(buf, s, body),
            View::Browse => browse(buf, s, body, theme),
            View::Insights => insights(buf, s, body),
        }
    }
    status(
        buf,
        s,
        Rect::new(edge.x, area.height - 1, edge.width, 1),
        theme,
    );
    if theme == Theme::Plain {
        for cell in &mut buf.content {
            cell.set_fg(Color::Reset).set_bg(Color::Reset);
        }
    }
}

// ---------------------------------------------------------------------------------------------
pub const CONTROL_MARKER: &str = "(contains control characters)";

// Text helpers (widths count Unicode scalar values, like dredge-render)

fn fit(s: &str, width: usize) -> String {
    let owned = dredge_render::sanitize(s);
    let s = owned.as_str();
    if UnicodeWidthStr::width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let n = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + n > width - 1 {
            break;
        }
        out.push(c);
        used += n;
    }
    out.push('…');
    out
}

fn fit_tail(s: &str, width: usize) -> String {
    let owned = dredge_render::sanitize(s);
    let s = owned.as_str();
    if UnicodeWidthStr::width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut tail = Vec::new();
    let mut used = 0;
    for c in s.chars().rev() {
        let n = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + n > width - 1 {
            break;
        }
        tail.push(c);
        used += n;
    }
    format!("…{}", tail.into_iter().rev().collect::<String>())
}

fn pad(s: &str, w: usize) -> String {
    format!(
        "{s}{}",
        " ".repeat(w.saturating_sub(UnicodeWidthStr::width(s)))
    )
}

fn lpad(s: &str, w: usize) -> String {
    format!(
        "{}{s}",
        " ".repeat(w.saturating_sub(UnicodeWidthStr::width(s)))
    )
}

pub fn fmt_age(secs: u64) -> String {
    match secs {
        0..=59 => "<1m".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

/// Last line of defense: no control character from file-derived text reaches the terminal.
fn clean(mut line: Line<'_>) -> Line<'static> {
    for span in &mut line.spans {
        span.content = dredge_render::sanitize(&span.content).into();
    }
    Line {
        spans: line
            .spans
            .into_iter()
            .map(|sp| Span::styled(sp.content.into_owned(), sp.style))
            .collect(),
        style: line.style,
        alignment: line.alignment,
    }
}

fn put(buf: &mut Buffer, rect: Rect, row: usize, line: Line) {
    if row < usize::from(rect.height) {
        buf.set_line(rect.x, rect.y + row as u16, &clean(line), rect.width);
    }
}

// ---------------------------------------------------------------------------------------------
// Header and status

/// `~` for the home folder, as the CLI prints paths. Display only.
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(h) if h.len() > 1 && path.starts_with(&h) => {
            let rest = &path[h.len()..];
            if rest.is_empty() {
                "~".into()
            } else if rest.starts_with('/') {
                format!("~{rest}")
            } else {
                path.to_string()
            }
        }
        _ => path.to_string(),
    }
}

/// A path as two spans, faint parent and plain leaf, cut in the parent's middle to fit `max`.
/// `dir` adds the trailing `/` a folder gets everywhere else.
fn path_spans(p: &str, dir: bool, max: usize) -> (Vec<Span<'static>>, usize) {
    let home = std::env::var("HOME").ok();
    let (parent, leaf) = dredge_render::human::fit_path(p, home.as_deref(), dir, max);
    let w = UnicodeWidthStr::width(parent.as_str()) + UnicodeWidthStr::width(leaf.as_str());
    (vec![Span::styled(parent, DIM), Span::raw(leaf)], w)
}

/// Snapshots older than this are flagged in amber, the same week the CLI uses.
const STALE_SECS: u64 = dredge_render::human::STALE_SECS;

fn header(buf: &mut Buffer, s: &State, rect: Rect, theme: Theme) {
    let w = usize::from(rect.width);
    let (right, right_style) = match (&s.scan, &s.overview) {
        (Scan::Running(p), _) => (format!("scanning \u{b7} {}", progress_text(p)), DIM),
        (Scan::Idle, Some(o)) => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(l) = &o.label {
                parts.push(l.clone());
            }
            if let Some(f) = &o.fs {
                parts.push(f.clone());
            }
            let age = dredge_render::human::age(o.age_secs);
            if o.age_secs > STALE_SECS {
                parts.push(format!("! scanned {age} ago"));
                (parts.join(" \u{b7} "), theme.warn())
            } else {
                parts.push(format!("scanned {age} ago"));
                (parts.join(" \u{b7} "), DIM)
            }
        }
        (Scan::Idle, None) if s.loaded => ("no snapshot".into(), DIM),
        (Scan::Idle, None) => (String::new(), DIM),
    };
    let lead = "dredge \u{203a} ";
    let rw = UnicodeWidthStr::width(right.as_str());
    let path_w = w
        .saturating_sub(lead.len() + if rw > 0 { rw + 2 } else { 0 })
        .max(8);
    let title = fit_tail(&tilde(&s.root), path_w);
    let left = vec![
        Span::styled("dredge", BOLD),
        Span::styled(" \u{203a} ", DIM),
        Span::styled(title, BOLD),
    ];
    put(
        buf,
        rect,
        0,
        split_line(left, Span::styled(right, right_style), w),
    );
}

type Keys = &'static [(&'static str, &'static str)];

/// Key hints for the footer: the view's own keys on the left, help and quit on the right.
fn hints(s: &State) -> (Keys, Keys) {
    const END: Keys = &[("?", "help"), ("q", "quit")];
    if s.help {
        return (&[("?", "close help"), ("esc", "close")], &[("q", "quit")]);
    }
    let left: Keys = match s.view {
        View::Overview if s.scanning() && s.overview.is_some() => {
            &[("esc", "cancel scan"), ("b", "browse last snapshot")]
        }
        View::Overview if s.scanning() => &[("esc", "cancel scan")],
        View::Overview if s.overview.is_none() => &[("r", "scan")],
        View::Overview => &[
            ("\u{2190}\u{2192}", "category"),
            ("\u{21b5}", "open"),
            ("b", "browse"),
            ("i", "insights"),
            ("c", "copy hint"),
            ("r", "rescan"),
        ],
        View::Paths => &[
            ("\u{2191}\u{2193}", "select"),
            ("\u{21b5}", "browse"),
            ("y", "copy"),
            ("esc", "back"),
            ("i", "insights"),
        ],
        View::Browse
            if s.current()
                .and_then(Level::selected)
                .is_some_and(|e| !e.is_dir) =>
        {
            &[
                ("\u{2191}\u{2193}", "move"),
                ("\u{2190}", "back"),
                ("y", "copy path"),
                ("/", "filter"),
                ("g", "jump"),
                ("esc", "exit"),
            ]
        }
        View::Browse => &[
            ("\u{2191}\u{2193}", "move"),
            ("\u{2192}", "open"),
            ("\u{2190}", "back"),
            ("/", "filter"),
            ("g", "jump"),
            ("y", "copy"),
            ("esc", "exit"),
        ],
        View::Insights => &[
            ("\u{2191}\u{2193}", "select"),
            ("\u{21b5}", "show"),
            ("y", "copy command"),
            ("esc", "back"),
        ],
    };
    (left, END)
}

/// Bold keys and dim labels. Pairs that do not fit are dropped from the end, never cut.
fn key_spans(keys: Keys, w: usize) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut used = 0;
    for (k, label) in keys {
        let gap = if spans.is_empty() { 0 } else { 3 };
        let n = gap + UnicodeWidthStr::width(*k) + 1 + UnicodeWidthStr::width(*label);
        if used + n > w {
            break;
        }
        if gap > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(*k, BOLD));
        spans.push(Span::styled(format!(" {label}"), DIM));
        used += n;
    }
    (spans, used)
}

fn hint_line(s: &State, w: usize) -> Line<'static> {
    let (left, right) = hints(s);
    // Help and quit stay visible; the view's own keys give way first.
    let (r, rw) = key_spans(right, w);
    let room = if w >= rw + 4 + 12 { w - rw - 4 } else { w };
    let (mut spans, lw) = key_spans(left, room);
    if lw + 4 + rw <= w {
        spans.push(Span::raw(" ".repeat(w - lw - rw)));
        spans.extend(r);
    }
    Line::from(spans)
}

/// Left text and a right-aligned note on one line; the note is dropped when it does not fit.
fn split_line(left: Vec<Span<'static>>, right: Span<'static>, w: usize) -> Line<'static> {
    let lw: usize = left
        .iter()
        .map(|sp| UnicodeWidthStr::width(sp.content.as_ref()))
        .sum();
    let rw = UnicodeWidthStr::width(right.content.as_ref());
    let mut spans = left;
    if lw + 2 + rw <= w {
        spans.push(Span::raw(" ".repeat(w - lw - rw)));
        spans.push(right);
    }
    Line::from(spans)
}

/// The prompt line while `/` or `g` owns the keyboard.
fn prompt(s: &State, w: usize, theme: Theme) -> Option<Line<'static>> {
    let input = s.input.filter(|_| s.view == View::Browse && !s.help)?;
    let level = s.current()?;
    Some(match input {
        Input::Filter => {
            let shown = level.visible().len();
            let total = level.listing.entries.len();
            let mut accent = BOLD;
            if theme != Theme::Plain {
                accent = accent.fg(theme.ink(Ink::Caches));
            }
            split_line(
                vec![
                    Span::styled("/ ", accent),
                    Span::raw(fit(&level.filter, w.saturating_sub(4))),
                    Span::styled("\u{258f}", BOLD),
                ],
                Span::styled(format!("{shown} of {total}  Enter keep  Esc clear"), DIM),
                w,
            )
        }
        Input::Jump => {
            let n = crumbs(&s.root, &level.listing.path).len().min(9);
            split_line(
                vec![
                    Span::styled("g", BOLD),
                    Span::raw(" jump to level   "),
                    Span::styled(
                        if n > 1 {
                            format!("1\u{2013}{n}")
                        } else {
                            "1".into()
                        },
                        BOLD,
                    ),
                    Span::raw(" pick   "),
                    Span::styled("~", BOLD),
                    Span::raw(" root"),
                ],
                Span::styled("Esc cancel", DIM),
                w,
            )
        }
    })
}

fn status(buf: &mut Buffer, s: &State, rect: Rect, theme: Theme) {
    let w = usize::from(rect.width);
    let line = if let Some(p) = prompt(s, w, theme) {
        p
    } else {
        match &s.status {
            Some(st) => Line::styled(
                fit(&st.text, w),
                if st.error { ERROR } else { Style::new() },
            ),
            None => hint_line(s, w),
        }
    };
    put(buf, rect, 0, line);
}

// ---------------------------------------------------------------------------------------------
// Overview

fn segments(o: &Overview) -> Vec<Segment> {
    let mut v: Vec<Segment> = o
        .cats
        .iter()
        .map(|c| Segment::new(c.ink.label(), c.bytes, c.ink))
        .collect();
    if let Some(free) = o.free_bytes.filter(|f| *f > 0) {
        v.push(Segment::new("Free", free, Ink::Free));
    }
    v
}

fn overview(buf: &mut Buffer, s: &State, body: Rect, theme: Theme) {
    if let Scan::Running(p) = &s.scan {
        return scanning(buf, s, p, inset(body), theme);
    }
    let Some(o) = &s.overview else {
        let msg = if !s.loaded {
            "Loading\u{2026}".to_string()
        } else {
            format!(
                "No snapshot for {} yet.\nPress r to scan it.",
                tilde(&s.root)
            )
        };
        let area = inset(body);
        for (i, l) in msg.lines().enumerate() {
            put(buf, area, i + 1, Line::raw(fit(l, usize::from(area.width))));
        }
        return;
    };
    if o.cats.is_empty() {
        put(
            buf,
            inset(body),
            1,
            Line::raw("No space used under this path."),
        );
        return;
    }
    let area = inset(body);
    let w = usize::from(area.width);
    let h = usize::from(area.height);
    // Breathing room around the totals only when there is height to spare.
    let roomy = h >= 16;
    let mut row = usize::from(roomy);
    put(buf, area, row, totals(o, w));
    row += 1 + usize::from(roomy);

    let segs = segments(o);
    let focus = Some(s.focus);
    if theme == Theme::Plain {
        let line = &dredge_render::render_bar(&segs, w, focus, Caps::PLAIN)[0];
        put(buf, area, row, Line::raw(line.0.clone()));
        row += 1;
    } else {
        for cells in bar_cells(&segs, w, focus, Glyphs::HalfBlock) {
            let spans: Vec<Span> = cells
                .iter()
                .map(|c| {
                    Span::styled(
                        c.ch.to_string(),
                        Style::new().fg(theme.color(c.fg)).bg(theme.color(c.bg)),
                    )
                })
                .collect();
            put(buf, area, row, Line::from(spans));
            row += 1;
        }
    }
    if let Some(c) = s.focused() {
        let span = dredge_render::segment_spans(&segs, w)
            .get(s.focus)
            .copied()
            .flatten();
        put(buf, area, row, pointer(c, o, span, w));
    }
    row += 2;

    // The top cleanup hint sits on the last row; the legend and the focused category's
    // paths share what is left, the legend first.
    let hint_rows = if h >= 10 { 2 } else { 0 };
    let avail = h.saturating_sub(row + hint_rows);
    let cols = legend_cols(&o.cats, area.width);
    let entries = legend_entries(o, s.focus, theme, cols);
    let total_rows = entries.len().div_ceil(cols);
    let shown = total_rows.min(avail.saturating_sub(4).max(1)).min(avail);
    let first = if shown >= total_rows {
        0
    } else {
        (s.focus / cols + 1).saturating_sub(shown)
    };
    for (r, chunk) in entries.chunks(cols).skip(first).take(shown).enumerate() {
        let mut spans = Vec::new();
        for e in chunk {
            spans.extend(e.iter().cloned());
        }
        put(buf, area, row + r, Line::from(spans));
    }
    row += shown;
    let mut left = avail.saturating_sub(shown);
    if left >= 3 {
        if let Some(c) = s.focused() {
            // A blank, the title, then one row per path (at least one, for the empty note).
            let need = (2 + c.top.len().max(1)).min(left);
            inside(
                buf,
                s,
                c,
                Rect::new(
                    area.x,
                    area.y + row as u16 + 1,
                    area.width,
                    (need - 1) as u16,
                ),
                theme,
            );
            row += need;
            left -= need;
        }
    }
    if hint_rows == 0 {
        return;
    }
    // Spare rows hold a short list of cleanup hints; otherwise the top one sits on the last row.
    let spare = left + hint_rows;
    match s.insights.as_deref() {
        Some(list) if list.len() > 1 && spare >= 4 => {
            let k = list.len().min(spare - 2).min(HINTS_SHOWN);
            hint_list(
                buf,
                list,
                k,
                Rect::new(area.x, area.y + row as u16 + 1, area.width, (k + 1) as u16),
                theme,
            );
        }
        _ => put(buf, area, h - 1, top_hint(s, w, theme)),
    }
}

/// Most cleanup hints the overview lists; `i` shows them all.
const HINTS_SHOWN: usize = 5;

/// `Cleanup hints`: the largest few, path first so the leaf always shows, the message after it.
fn hint_list(buf: &mut Buffer, list: &[Insight], k: usize, area: Rect, theme: Theme) {
    let w = usize::from(area.width);
    let more = list.len().saturating_sub(k);
    let right = if more > 0 {
        format!("c copy the first   i all {}", list.len())
    } else {
        "c copy the first".into()
    };
    put(
        buf,
        area,
        0,
        split_line(
            vec![
                Span::styled("Cleanup hints", BOLD),
                Span::styled("  dredge never runs these", DIM),
            ],
            Span::styled(right, DIM),
            w,
        ),
    );
    let hw = list
        .iter()
        .take(k)
        .map(|i| i.human.len())
        .max()
        .unwrap_or(0);
    for (n, ins) in list.iter().take(k).enumerate() {
        let room = w.saturating_sub(2 + hw + 2);
        // The path gets up to three fifths; the message fills what is left.
        let (path, pw) = path_spans(&ins.path, true, (room * 3 / 5).max(room.min(16)));
        let rest = room.saturating_sub(pw + 2);
        let mut spans = vec![
            Span::styled("\u{25c6} ", theme.hint()),
            Span::styled(lpad(&ins.human, hw), BOLD),
            Span::raw("  "),
        ];
        spans.extend(path);
        if rest >= 8 {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(fit(&ins.message, rest), DIM));
        }
        put(buf, area, 1 + n, Line::from(spans));
    }
}

/// Two columns of margin on each side once the terminal is wide enough to spare them.
fn inset(body: Rect) -> Rect {
    let m = if body.width >= 40 { 2 } else { 0 };
    Rect::new(body.x + m, body.y, body.width - 2 * m, body.height)
}

/// `412.3 GiB used of 494.4 GiB · 82.1 GiB free`, or the folder's own size off a mount point.
fn totals(o: &Overview, w: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(o.used_human.clone(), BOLD)];
    match o.total_bytes.filter(|t| *t > 0) {
        Some(t) => {
            let pct = o.used_bytes as f64 / t as f64 * 100.0;
            spans.push(Span::styled(
                format!(" used of {} ({pct:.0}%)", dredge_core::util::human_bytes(t)),
                DIM,
            ));
        }
        None => spans.push(Span::styled(" in this folder", DIM)),
    }
    if let Some(f) = o.free_bytes {
        spans.push(Span::styled("   \u{b7}   ", DIM));
        spans.push(Span::styled(dredge_core::util::human_bytes(f), BOLD));
        spans.push(Span::styled(" free", DIM));
    }
    let used: usize = spans
        .iter()
        .map(|sp| UnicodeWidthStr::width(sp.content.as_ref()))
        .sum();
    if used > w {
        return Line::raw(fit(
            &spans
                .iter()
                .map(|sp| sp.content.as_ref())
                .collect::<String>(),
            w,
        ));
    }
    Line::from(spans)
}

/// The line under the bar: `└ Developer  300 MiB · 30% of disk · 12 files   ↵ open`, starting
/// under the focused segment, or ending at it (`┘`) when it does not fit to the right. Detail
/// is dropped from the end until one of the two fits.
fn pointer(c: &Cat, o: &Overview, span: Option<(usize, usize)>, w: usize) -> Line<'static> {
    let share = match o.total_bytes.filter(|t| *t > 0) {
        Some(t) => format!("{:.1}% of disk", c.bytes as f64 / t as f64 * 100.0),
        None => format!("{:.1}% of used", c.share),
    };
    let files = format!(
        "{} {}",
        fmt_count(c.files),
        if c.files == 1 { "file" } else { "files" }
    );
    let open = if c.top.is_empty() {
        ""
    } else {
        "   \u{21b5} open"
    };
    let label = c.ink.label();
    let variants = [
        (format!("  {} \u{b7} {share} \u{b7} {files}", c.human), open),
        (format!("  {} \u{b7} {share} \u{b7} {files}", c.human), ""),
        (format!("  {} \u{b7} {share}", c.human), ""),
        (format!("  {}", c.human), ""),
    ];
    let (at, n) = span.unwrap_or((0, 1));
    for (facts, open) in &variants {
        let text_w = 2
            + UnicodeWidthStr::width(label)
            + UnicodeWidthStr::width(facts.as_str())
            + UnicodeWidthStr::width(*open);
        let body = |lead: &'static str, tail: &'static str| {
            vec![
                Span::styled(lead, DIM),
                Span::styled(label.to_string(), BOLD),
                Span::raw(facts.clone()),
                Span::styled(open.to_string(), DIM),
                Span::styled(tail, DIM),
            ]
        };
        if at + text_w <= w {
            let mut v = vec![Span::raw(" ".repeat(at))];
            v.extend(body("\u{2514} ", ""));
            return Line::from(v);
        }
        let end = (at + n).min(w);
        if text_w <= end {
            let mut v = vec![Span::raw(" ".repeat(end - text_w))];
            v.extend(body("", " \u{2518}"));
            return Line::from(v);
        }
    }
    Line::raw(fit(&format!("{label}  {}", c.human), w))
}

/// A cleanup hint covers `path` when one is inside the other.
fn reclaimable(s: &State, path: &str) -> bool {
    let p = std::path::Path::new(path);
    s.insights.as_deref().is_some_and(|l| {
        l.iter().any(|i| {
            let q = std::path::Path::new(&i.path);
            p.starts_with(q) || q.starts_with(p)
        })
    })
}

/// "Inside Developer": the focused category's largest paths with a share bar each.
fn inside(buf: &mut Buffer, s: &State, c: &Cat, area: Rect, theme: Theme) {
    let w = usize::from(area.width);
    if c.top.is_empty() {
        put(
            buf,
            area,
            0,
            Line::from(vec![
                Span::styled("Inside ", DIM),
                Span::styled(c.ink.label(), BOLD),
                Span::styled("  no single large path; it is spread thin", DIM),
            ]),
        );
        return;
    }
    put(
        buf,
        area,
        0,
        Line::from(vec![
            Span::styled("Inside ", DIM),
            Span::styled(c.ink.label(), BOLD),
            Span::styled("  largest paths", DIM),
        ]),
    );
    let rows = usize::from(area.height).saturating_sub(1);
    let ink = if theme == Theme::Plain {
        Style::new()
    } else {
        Style::new().fg(theme.ink(c.ink))
    };
    let sizes: Vec<String> = (0..c.top.len())
        .map(|i| {
            c.top_bytes
                .get(i)
                .filter(|b| **b > 0)
                .map_or_else(String::new, |b| dredge_core::util::human_bytes(*b))
        })
        .collect();
    let sw = sizes.iter().map(|x| x.len()).max().unwrap_or(0);
    let bar_w = if w >= 60 { 10 } else { 0 };
    for (i, p) in c.top.iter().enumerate().take(rows) {
        let frac = match (c.top_bytes.get(i), c.bytes) {
            (Some(b), t) if t > 0 => (*b as f64 / t as f64).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let mut spans = vec![Span::styled(format!("{:>2}  ", i + 1), DIM)];
        if bar_w > 0 {
            let filled =
                ((frac * bar_w as f64).round() as usize).clamp(usize::from(frac > 0.0), bar_w);
            spans.push(Span::styled("\u{2501}".repeat(filled), ink));
            spans.push(Span::styled("\u{2500}".repeat(bar_w - filled), DIM));
            spans.push(Span::raw("  "));
        }
        spans.push(Span::raw(lpad(&sizes[i], sw)));
        spans.push(Span::raw("  "));
        let tag = if reclaimable(s, p) {
            "  \u{25c6} reclaimable"
        } else {
            ""
        };
        let used = 4 + if bar_w > 0 { bar_w + 2 } else { 0 } + sw + 2;
        let room = w.saturating_sub(used);
        let tag = if room >= 24 + tag.len() { tag } else { "" };
        let (path, _) = path_spans(p, true, room.saturating_sub(UnicodeWidthStr::width(tag)));
        spans.extend(path);
        if !tag.is_empty() {
            spans.push(Span::styled(tag, theme.hint()));
        }
        put(buf, area, 1 + i, Line::from(spans));
    }
}

/// The last body row: the largest cleanup hint, and how to act on it.
fn top_hint(s: &State, w: usize, theme: Theme) -> Line<'static> {
    let Some(list) = &s.insights else {
        return Line::styled("\u{25c6} looking for cleanup hints\u{2026}", DIM);
    };
    let Some(top) = list.first() else {
        return Line::styled("no cleanup hints for this path", DIM);
    };
    let more = list.len() - 1;
    let right = if more > 0 {
        format!("c copy   i {more} more")
    } else {
        "c copy".into()
    };
    let rw = UnicodeWidthStr::width(right.as_str());
    let lead = format!("{}  ", top.human);
    let room = w.saturating_sub(2 + lead.len() + rw + 3);
    // Path first, as in the list: its leaf always shows; the message fills what is left.
    let (path, pw) = path_spans(&top.path, true, (room * 3 / 5).max(room.min(16)));
    let mut left = vec![
        Span::styled("\u{25c6} ", theme.hint()),
        Span::styled(lead, BOLD),
    ];
    left.extend(path);
    let rest = room.saturating_sub(pw + 2);
    if rest >= 8 {
        left.push(Span::raw("  "));
        left.push(Span::styled(fit(&top.message, rest), DIM));
    }
    split_line(left, Span::styled(right, DIM), w)
}

/// The scanning screen: what has been read so far, and a percentage only when there is an
/// honest total to compare against.
fn scanning(buf: &mut Buffer, s: &State, p: &crate::state::Progress, area: Rect, theme: Theme) {
    let w = usize::from(area.width);
    let h = usize::from(area.height);
    let mut row = usize::from(h >= 12);
    put(
        buf,
        area,
        row,
        Line::from(vec![
            Span::styled("Scanning ", DIM),
            Span::styled(fit_tail(&tilde(&s.root), w.saturating_sub(9)), BOLD),
        ]),
    );
    row += 1 + usize::from(h >= 12);
    put(
        buf,
        area,
        row,
        Line::from(vec![
            Span::styled(dredge_core::util::human_bytes(p.bytes), BOLD),
            Span::styled("  read so far", DIM),
        ]),
    );
    row += 1;
    let secs = p.elapsed_ms as f64 / 1000.0;
    let mut facts = format!(
        "{} files \u{b7} {} folders \u{b7} {}",
        fmt_count(p.files),
        fmt_count(p.dirs),
        elapsed(p.elapsed_ms)
    );
    if secs >= 1.0 {
        facts.push_str(&format!(
            " \u{b7} {} files/s",
            fmt_count((p.files as f64 / secs) as u64)
        ));
    }
    put(buf, area, row, Line::styled(fit(&facts, w), DIM));
    row += 1;
    if p.errors > 0 {
        put(
            buf,
            area,
            row,
            Line::styled(
                format!(
                    "! {} {} could not be read so far",
                    fmt_count(p.errors),
                    if p.errors == 1 { "folder" } else { "folders" }
                ),
                theme.warn(),
            ),
        );
        row += 1;
    }
    row += 1;
    match p.expected_bytes.filter(|e| *e > 0) {
        Some(total) => {
            let frac = (p.bytes as f64 / total as f64).min(0.99);
            let bar_w = w.saturating_sub(24).clamp(0, 48);
            let filled = (frac * bar_w as f64).round() as usize;
            let accent = if theme == Theme::Plain {
                BOLD
            } else {
                Style::new().fg(theme.ink(Ink::Caches))
            };
            let mut spans = vec![
                Span::styled("\u{2501}".repeat(filled), accent),
                Span::styled("\u{2500}".repeat(bar_w - filled), DIM),
                Span::styled(format!("  {:.0}%", frac * 100.0), BOLD),
            ];
            // An estimate only once the rate has settled.
            if secs >= 2.0 && frac >= 0.02 {
                let left = (secs / frac - secs).max(1.0);
                spans.push(Span::styled(
                    format!("  about {} left", elapsed((left * 1000.0) as u64)),
                    DIM,
                ));
            }
            put(buf, area, row, Line::from(spans));
            row += 1;
            put(
                buf,
                area,
                row,
                Line::styled(fit("measured against the volume's used space", w), DIM),
            );
        }
        None => put(
            buf,
            area,
            row,
            Line::styled(
                fit(
                    "no percentage: a folder's size is only known once it has been read",
                    w,
                ),
                DIM,
            ),
        ),
    }
    row += 2;
    let threads = match p.threads {
        0 => String::new(),
        1 => "reading with 1 thread, low priority so the machine stays usable".into(),
        n => format!("reading with {n} threads, low priority so the machine stays usable"),
    };
    put(buf, area, row, Line::styled(fit(&threads, w), DIM));
    row += 1;
    let keep = match &s.overview {
        Some(o) => format!(
            "the snapshot from {} ago stays in use until this one finishes",
            dredge_render::human::age(o.age_secs)
        ),
        None => "results appear when the scan finishes".into(),
    };
    put(buf, area, row, Line::styled(fit(&keep, w), DIM));
}

fn elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..=9 => format!("{:.1}s", ms as f64 / 1000.0),
        10..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs / 60 % 60),
    }
}

/// One cell group per legend entry (focusable categories, then Free).
fn legend_entries(
    o: &Overview,
    focus: usize,
    theme: Theme,
    cols: usize,
) -> Vec<Vec<Span<'static>>> {
    let lw = o
        .cats
        .iter()
        .map(|c| c.ink.label().chars().count())
        .chain([4])
        .max()
        .unwrap_or(4);
    let hw = o
        .cats
        .iter()
        .map(|c| c.human.chars().count())
        .max()
        .unwrap_or(0);
    let colw = 3 + lw + 1 + hw + 3;
    let mut all: Vec<(Ink, String, String, bool)> = o
        .cats
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                c.ink,
                c.ink.label().to_string(),
                c.human.clone(),
                i == focus,
            )
        })
        .collect();
    if let Some(free) = o.free_bytes.filter(|f| *f > 0) {
        all.push((
            Ink::Free,
            "Free".into(),
            dredge_core::util::human_bytes(free),
            false,
        ));
    }
    let n = all.len();
    all.into_iter()
        .enumerate()
        .map(|(i, (ink, label, human, focused))| {
            let body = format!("{} {}", pad(&label, lw), lpad(&human, hw));
            let swatch = if theme == Theme::Plain {
                ink.plain_glyph().to_string()
            } else {
                "\u{25a0}".to_string()
            };
            let mut sw = Style::new();
            if theme != Theme::Plain {
                sw = sw.fg(theme.ink(ink));
            }
            let text_style = if focused { BOLD } else { Style::new() };
            let last_in_row = (i + 1) % cols == 0 || i + 1 == n;
            let gutter = if last_in_row {
                0
            } else {
                colw - (3 + body.chars().count())
            };
            vec![
                Span::styled(if focused { "\u{203a}" } else { " " }, BOLD),
                Span::styled(swatch, sw),
                Span::raw(" "),
                Span::styled(body, text_style),
                Span::raw(" ".repeat(gutter)),
            ]
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Lists

fn subtitle(buf: &mut Buffer, body: Rect, text: String) {
    put(
        buf,
        body,
        0,
        Line::styled(fit_tail(&text, usize::from(body.width)), DIM),
    );
}

fn list_area(body: Rect) -> Rect {
    Rect::new(
        body.x,
        body.y + 1,
        body.width,
        body.height.saturating_sub(1),
    )
}

fn paths(buf: &mut Buffer, s: &State, body: Rect) {
    let Some(c) = s.focused() else { return };
    subtitle(buf, body, format!("{}: largest paths", c.ink.label()));
    let area = list_area(body);
    let w = usize::from(area.width);
    let offset = s
        .paths_sel
        .saturating_add(1)
        .saturating_sub(usize::from(area.height));
    for (i, p) in c.top.iter().enumerate().skip(offset) {
        let style = if i == s.paths_sel {
            SELECTED
        } else {
            Style::new()
        };
        put(
            buf,
            area,
            i - offset,
            Line::styled(pad(&fit_tail(p, w), w), style),
        );
    }
}

/// The name the breadcrumb shows for the scan root: `~` for the home directory, else its
/// last component.
fn root_label(root: &str) -> String {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty());
    if home.is_some_and(|h| std::path::Path::new(&h) == std::path::Path::new(root)) {
        return "~".into();
    }
    std::path::Path::new(root)
        .file_name()
        .map_or_else(|| root.to_string(), |n| n.to_string_lossy().into_owned())
}

/// `root › a › b › current`, numbered while `g` is pending. Middle levels fold into `…` when
/// the line is too narrow, so the root and the current folder always show.
fn breadcrumb(s: &State, level: &Level, w: usize) -> Line<'static> {
    let paths = crumbs(&s.root, &level.listing.path);
    let numbered = s.input == Some(Input::Jump);
    let mut labels: Vec<(Option<usize>, String)> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let label = if i == 0 && paths.len() > 1 || p == &s.root {
                root_label(&s.root)
            } else {
                std::path::Path::new(p)
                    .file_name()
                    .map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned())
            };
            (Some(i + 1).filter(|n| numbered && *n <= 9), label)
        })
        .collect();

    let l = &level.listing;
    let mut right = if level.filter.is_empty() {
        format!("{}  \u{b7}  {} items", l.human, l.entries.len())
    } else {
        format!(
            "{} of {} match \u{201c}{}\u{201d}",
            level.visible().len(),
            l.entries.len(),
            level.filter
        )
    };
    if l.remaining > 0 {
        right.push_str(&format!("  (+{} more)", l.remaining));
    }

    let tag_w = |t: &Option<usize>| t.map_or(0, |n| n.to_string().len() + 3);
    let width = |ls: &[(Option<usize>, String)]| -> usize {
        ls.iter()
            .map(|(t, s)| tag_w(t) + UnicodeWidthStr::width(s.as_str()))
            .sum::<usize>()
            + 3 * ls.len().saturating_sub(1)
    };
    let room = w.saturating_sub(UnicodeWidthStr::width(right.as_str()) + 2);
    let room = if room < 12 { w } else { room };
    // Fold middle levels into one ellipsis, oldest first.
    while width(&labels) > room && labels.len() > 2 {
        if labels[1].1 == "\u{2026}" {
            if labels.len() == 3 {
                break;
            }
            labels.remove(2);
        } else {
            labels[1] = (None, "\u{2026}".into());
        }
    }
    let n = labels.len();
    let mut spans = Vec::new();
    for (i, (tag, label)) in labels.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" \u{203a} ", DIM));
        }
        if let Some(t) = tag {
            spans.push(Span::styled(
                format!(" {t} "),
                BOLD.add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::raw(" "));
        }
        let last = i + 1 == n;
        spans.push(Span::styled(label, if last { BOLD } else { Style::new() }));
    }
    let used: usize = spans
        .iter()
        .map(|sp| UnicodeWidthStr::width(sp.content.as_ref()))
        .sum();
    if used > w {
        return Line::styled(fit_tail(&level.listing.path, w), BOLD);
    }
    split_line(spans, Span::styled(right, DIM), w)
}

fn share_text(p: Option<f64>) -> String {
    match p {
        None => String::new(),
        Some(p) if p > 0.0 && p < 1.0 => "<1%".into(),
        Some(p) => format!("{p:.0}%"),
    }
}

fn browse(buf: &mut Buffer, s: &State, body: Rect, theme: Theme) {
    let Some(level) = s.current() else { return };
    let w = usize::from(body.width);
    put(buf, body, 0, breadcrumb(s, level, w));
    let l = &level.listing;
    let area = list_area(body);
    if l.entries.is_empty() {
        let mut row = 0;
        if level.up_row() {
            put(buf, area, 0, up_line(s, level, level.on_up(), w, theme));
            row = 1;
        }
        put(
            buf,
            area,
            row,
            Line::styled("  Empty folder. \u{2190} goes back.", DIM),
        );
        return;
    }
    let visible = level.visible();
    if visible.is_empty() {
        put(
            buf,
            area,
            0,
            Line::styled(
                fit(
                    &format!(
                        "  Nothing here matches \u{201c}{}\u{201d}. Esc clears the filter.",
                        level.filter
                    ),
                    w,
                ),
                DIM,
            ),
        );
        return;
    }

    // marker 2, name, [2 bar], 2 size, 2 share
    let hw = l
        .entries
        .iter()
        .map(|e| e.human.chars().count())
        .max()
        .unwrap_or(0)
        .max(4);
    let sw = 5;
    let bar_w = if w >= 60 { 12 } else { 0 };
    // The bar takes its own width plus a two-column gap on each side of its cells.
    let bar_total = if bar_w > 0 { bar_w + 2 } else { 0 };
    let name_w = w.saturating_sub(2 + bar_total + 2 + hw + 2 + sw).max(4);
    let gap = |n: usize| " ".repeat(n);

    let head = format!(
        "  {}{}{}{}{}{}",
        pad("NAME", name_w),
        gap(bar_total),
        gap(2),
        lpad("SIZE", hw),
        gap(2),
        lpad("SHARE", sw)
    );
    put(buf, area, 0, Line::styled(fit(&head, w), DIM));
    let rows = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );

    let accent = |ink: Ink| {
        if theme == Theme::Plain {
            Style::new()
        } else {
            Style::new().fg(theme.ink(ink))
        }
    };
    let up = usize::from(level.up_row());
    let end = (level.offset + usize::from(rows.height)).min(up + visible.len());
    for r in level.offset..end {
        let selected = r == level.sel;
        if r < up {
            put(
                buf,
                rows,
                r - level.offset,
                up_line(s, level, selected, w, theme),
            );
            continue;
        }
        let e = &l.entries[visible[r - up]];
        let name = if e.is_dir {
            format!("{}/", e.name)
        } else {
            e.name.clone()
        };
        let row_style = if selected {
            SELECTED.add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let mut spans = vec![Span::styled(
            if selected { "\u{258c} " } else { "  " },
            accent(Ink::Developer).add_modifier(Modifier::BOLD),
        )];
        spans.push(Span::styled(pad(&fit(&name, name_w), name_w), row_style));
        if bar_w > 0 {
            let cells = bar_w - 2;
            let filled = e.pct_of_parent.map_or(0, |p| {
                let n = (p / 100.0 * cells as f64).round() as usize;
                if p > 0.0 {
                    n.clamp(1, cells)
                } else {
                    0
                }
            });
            spans.push(Span::styled(gap(2), row_style));
            spans.push(Span::styled(
                "\u{2501}".repeat(filled),
                accent(if e.is_dir { Ink::Developer } else { Ink::Other }).patch(row_style),
            ));
            spans.push(Span::styled(
                "\u{2500}".repeat(cells - filled),
                DIM.patch(row_style),
            ));
            spans.push(Span::styled(gap(2), row_style));
        }
        spans.push(Span::styled(
            format!("{}{}", gap(2), lpad(&e.human, hw)),
            row_style,
        ));
        spans.push(Span::styled(
            format!("{}{}", gap(2), lpad(&share_text(e.pct_of_parent), sw)),
            if selected { row_style } else { DIM },
        ));
        put(buf, rows, r - level.offset, Line::from(spans));
    }
}

/// `../  parent`: the row that goes up a level, in the name lane, with no size.
fn up_line(s: &State, level: &Level, selected: bool, w: usize, theme: Theme) -> Line<'static> {
    let parent = std::path::Path::new(&level.listing.path)
        .parent()
        .map(|p| {
            if p == std::path::Path::new(&s.root) {
                root_label(&s.root)
            } else {
                p.file_name()
                    .map_or_else(|| p.to_string_lossy(), |n| n.to_string_lossy())
                    .into_owned()
            }
        })
        .unwrap_or_default();
    let style = if selected {
        SELECTED.add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let marker = if theme == Theme::Plain {
        Style::new()
    } else {
        Style::new().fg(theme.ink(Ink::Developer))
    };
    let tail = fit(&format!("  {parent}"), w.saturating_sub(5));
    let fill = " ".repeat(w.saturating_sub(5 + tail.width()));
    Line::from(vec![
        Span::styled(
            if selected { "\u{258c} " } else { "  " },
            marker.add_modifier(Modifier::BOLD),
        ),
        Span::styled("../", style),
        Span::styled(tail, DIM.patch(style)),
        Span::styled(fill, style),
    ])
}

/// The command as displayed: control characters made visible, with a marker when there were any.
fn command_shown(cmd: &str) -> String {
    if cmd.chars().any(dredge_render::is_unsafe_char) {
        format!("{}  {CONTROL_MARKER}", dredge_render::sanitize(cmd))
    } else {
        cmd.to_string()
    }
}

fn insight_lines(i: &Insight, selected: bool, w: usize) -> [Line<'static>; INSIGHT_LINES] {
    let mark = if selected { "\u{203a} " } else { "  " };
    let head = if selected { BOLD } else { Style::new() };
    [
        Line::from(vec![
            Span::styled(mark, BOLD),
            Span::styled(
                fit(&format!("{}  {}", i.human, i.message), w.saturating_sub(2)),
                head,
            ),
        ]),
        Line::styled(format!("  {}", fit_tail(&i.path, w.saturating_sub(2))), DIM),
        Line::from(vec![
            Span::raw("  $ "),
            Span::raw(fit(
                &command_shown(&i.suggested_command),
                w.saturating_sub(4),
            )),
        ]),
    ]
}

fn insights(buf: &mut Buffer, s: &State, body: Rect) {
    subtitle(
        buf,
        body,
        "Insights: y copies a command to the clipboard, dredge never runs it".into(),
    );
    let area = list_area(body);
    let w = usize::from(area.width);
    let Some(list) = &s.insights else {
        put(buf, area, 0, Line::styled("Loading\u{2026}", DIM));
        return;
    };
    if list.is_empty() {
        put(
            buf,
            area,
            0,
            Line::styled("No cleanup hints under this path.", DIM),
        );
        return;
    }
    let slots = usize::from(area.height) / INSIGHT_LINES;
    for (k, ins) in list
        .iter()
        .enumerate()
        .skip(s.insight_offset)
        .take(slots.max(1))
    {
        let base = (k - s.insight_offset) * INSIGHT_LINES;
        for (j, line) in insight_lines(ins, k == s.insight_sel, w)
            .into_iter()
            .enumerate()
        {
            put(buf, area, base + j, line);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Help

fn help(buf: &mut Buffer, body: Rect) {
    Clear.render(body, buf);
    let rows = [
        ("\u{2190} \u{2192} h l", "move category focus (wraps)"),
        (
            "\u{2191} \u{2193} k j",
            "move by legend row / list selection",
        ),
        (
            "Enter",
            "open: category \u{2192} largest paths \u{2192} folders",
        ),
        ("b", "browse the scan root as folders"),
        (
            "\u{2192} \u{2190}",
            "open a folder / up one level, past where you came in",
        ),
        ("/", "filter this folder; Enter keeps it, Esc clears"),
        ("g 1-9 ~", "jump to a breadcrumb level, or ~ for the root"),
        ("Esc", "leave browse (or clear the filter)"),
        ("i", "toggle cleanup insights"),
        (
            "c",
            "copy the top hint's command from the overview (never run)",
        ),
        (
            "y",
            "copy the selected path, or an insight's command (never run)",
        ),
        ("r", "rescan the root in the background"),
        ("Esc", "while scanning: cancel; the last snapshot stays"),
        ("?", "toggle this help"),
        ("q Ctrl-C", "quit"),
    ];
    put(buf, body, 0, Line::styled("Keys", BOLD));
    for (i, (k, d)) in rows.iter().enumerate() {
        put(
            buf,
            body,
            i + 1,
            Line::from(vec![
                Span::styled(pad(k, 15), BOLD),
                Span::raw(fit(d, usize::from(body.width).saturating_sub(15))),
            ]),
        );
    }
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::state::{
        update, Cat, Entry, Event, Insight, Key, Listing, Overview, Progress, State,
    };

    fn cat(ink: Ink, bytes: u64, top: &[&str]) -> Cat {
        Cat {
            ink,
            bytes,
            human: dredge_core::util::human_bytes(bytes),
            share: bytes as f64 / 6.0,
            files: 12,
            top: top.iter().map(|s| s.to_string()).collect(),
            top_bytes: top.iter().map(|_| bytes / 2).collect(),
        }
    }

    fn overview() -> Overview {
        Overview {
            path: "/data".into(),
            used_bytes: 600 << 20,
            used_human: "600.0 MiB".into(),
            total_bytes: Some(1000 << 20),
            free_bytes: Some(400 << 20),
            label: Some("Data".into()),
            fs: Some("apfs".into()),
            age_secs: 7300,
            cats: vec![
                cat(
                    Ink::Developer,
                    300 << 20,
                    &["/data/proj/node_modules", "/data/proj/target"],
                ),
                cat(Ink::Photos, 200 << 20, &["/data/pics"]),
                cat(Ink::Caches, 100 << 20, &[]),
            ],
        }
    }

    fn loaded(size: (u16, u16)) -> State {
        update(
            State::new("/data", size),
            Event::Overview(Ok(Some(overview()))),
        )
        .0
    }

    fn render(s: &State, theme: Theme) -> Buffer {
        let mut t = Terminal::new(TestBackend::new(s.size.0, s.size.1)).unwrap();
        t.draw(|f| draw(f, s, theme)).unwrap();
        t.backend().buffer().clone()
    }

    fn text(b: &Buffer) -> String {
        (0..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(s: State, k: Key) -> State {
        update(s, Event::Key(k)).0
    }

    #[test]
    fn overview_at_80x24_follows_the_board() {
        let s = loaded((80, 24));
        let b = render(&s, Theme::TrueColor);
        let t = text(&b);
        for want in [
            "dredge \u{203a} /data",
            "Data \u{b7} apfs \u{b7} scanned 2 h ago",
            "600.0 MiB used of 1000.0 MiB (60%)",
            "400.0 MiB free",
            "\u{2514} Developer  300.0 MiB \u{b7} 30.0% of disk \u{b7} 12 files   \u{21b5} open",
            "Photos",
            "Caches",
            "Free",
            "Inside Developer",
            "150.0 MiB  /data/proj/node_modules/",
            "looking for cleanup hints",
            "? help",
            "q quit",
        ] {
            assert!(t.contains(want), "missing {want:?} in:\n{t}");
        }
        // The dithered half-block bar, in the ink colors, inside a two-column margin.
        let bar = b[(2, 4)].clone();
        assert_eq!(bar.symbol(), "\u{2580}");
        assert!(matches!(bar.fg, Color::Rgb(..)) && matches!(bar.bg, Color::Rgb(..)));
        assert_eq!(b[(77, 5)].symbol(), "\u{2580}");
        assert_eq!(b[(78, 5)].symbol(), " ");
    }

    #[test]
    fn tui_bar_cells_match_what_dredge_bar_paints() {
        let s = loaded((80, 24));
        let b = render(&s, Theme::TrueColor);
        let o = overview();
        let segs = segments(&o);
        let rows = bar_cells(&segs, 76, Some(0), Glyphs::HalfBlock);
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.iter().enumerate() {
                let cell = &b[(2 + x as u16, 4 + y as u16)];
                assert_eq!(cell.symbol(), c.ch.to_string());
                assert_eq!(cell.fg, Theme::TrueColor.color(c.fg));
                assert_eq!(cell.bg, Theme::TrueColor.color(c.bg));
            }
        }
    }

    #[test]
    fn focus_moves_the_pointer_and_the_inside_list() {
        let s = press(loaded((80, 24)), Key::Right);
        let t = text(&render(&s, Theme::TrueColor));
        assert!(
            t.contains("/data/pics") && !t.contains("node_modules"),
            "{t}"
        );
        // The pointer starts under the Photos segment, which begins after Developer's cells.
        let segs = segments(&overview());
        let (at, _) = dredge_render::segment_spans(&segs, 76)[1].unwrap();
        let line = row_with(&render(&s, Theme::TrueColor), "\u{2514} Photos").unwrap();
        assert_eq!(line.find('\u{2514}'), Some(2 + at), "{line}");
        let s = press(s, Key::Right);
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("no single large path"), "{t}");
        let ptr = row_with(&render(&s, Theme::TrueColor), "Caches  ").unwrap();
        assert!(!ptr.contains("\u{21b5} open"), "nothing to open: {ptr}");
    }

    #[test]
    fn pointer_ends_under_a_segment_near_the_right_edge() {
        let mut s = loaded((80, 24));
        s.focus = 2;
        let segs = segments(s.overview.as_ref().unwrap());
        let (at, n) = dredge_render::segment_spans(&segs, 76)[2].unwrap();
        let line = row_with(&render(&s, Theme::TrueColor), "Caches  100").unwrap();
        // Text that fits to the right starts with └; otherwise it ends with ┘ under the segment.
        match (line.find('\u{2514}'), line.find('\u{2518}')) {
            (Some(x), None) => assert_eq!(x, 2 + at, "{line}"),
            (None, Some(_)) => {
                let end = line.trim_end().chars().count();
                assert_eq!(end, 2 + at + n, "{line}");
            }
            _ => panic!("no pointer glyph: {line}"),
        }
    }

    #[test]
    fn top_hint_and_reclaimable_paths() {
        let s = loaded((80, 24));
        let list = vec![
            Insight {
                human: "150.0 MiB".into(),
                message: "Node dependencies".into(),
                path: "/data/proj/node_modules".into(),
                suggested_command: "rm -rf /data/proj/node_modules".into(),
            },
            Insight {
                human: "1.0 MiB".into(),
                message: "other".into(),
                path: "/elsewhere".into(),
                suggested_command: "true".into(),
            },
        ];
        let s = update(s, Event::Insights(Ok(list))).0;
        let b = render(&s, Theme::TrueColor);
        // Room to spare: a short list, path first, message after.
        let t = text(&b);
        assert!(
            t.contains("Cleanup hints  dredge never runs these")
                && t.contains("c copy the first")
                && t.contains("\u{25c6} 150.0 MiB  /data/proj/node_modules/  Node dependencies")
                && t.contains("\u{25c6}   1.0 MiB  /elsewhere/  other"),
            "{t}"
        );
        // Short terminal: one line, the top hint only.
        let short = update(s.clone(), Event::Resize(80, 14)).0;
        let hint = row_with(&render(&short, Theme::TrueColor), "\u{25c6} 150.0 MiB").unwrap();
        assert!(
            hint.contains("Node dependencies")
                && hint.contains("node_modules")
                && hint.contains("c copy   i 1 more"),
            "{hint}"
        );
        let nm = row_with(&b, " 1  \u{2501}").unwrap();
        assert!(nm.contains("/data/proj/node_modules/"), "{nm}");
        assert!(nm.contains("\u{25c6} reclaimable"), "{nm}");
        let target = row_with(&b, "/data/proj/target").unwrap();
        assert!(!target.contains("reclaimable"), "{target}");
        let s = update(s, Event::Insights(Ok(vec![]))).0;
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("no cleanup hints for this path"), "{t}");
    }

    #[test]
    fn stale_snapshot_is_flagged_in_the_header() {
        let mut o = overview();
        o.age_secs = 9 * 86_400;
        let s = update(State::new("/data", (80, 24)), Event::Overview(Ok(Some(o)))).0;
        let b = render(&s, Theme::TrueColor);
        let head = text(&b).lines().next().unwrap().to_string();
        assert!(head.contains("! scanned 9 days ago"), "{head}");
    }

    #[test]
    fn scanning_screen_is_honest_about_progress() {
        let scan = |expected: Option<u64>| {
            let s = press(loaded((80, 24)), Key::Char('r'));
            let p = Progress {
                files: 120_000,
                dirs: 9_000,
                bytes: 300 << 20,
                errors: 2,
                elapsed_ms: 4_000,
                threads: 2,
                expected_bytes: expected,
            };
            text(&render(&update(s, Event::Progress(p)).0, Theme::TrueColor))
        };
        let t = scan(Some(600 << 20));
        for want in [
            "Scanning /data",
            "300.0 MiB  read so far",
            "120,000 files \u{b7} 9,000 folders \u{b7} 4.0s \u{b7} 30,000 files/s",
            "! 2 folders could not be read so far",
            "50%",
            "about 4.0s left",
            "reading with 2 threads",
            "the snapshot from 2 h ago stays in use",
            "esc cancel scan",
            "b browse last snapshot",
        ] {
            assert!(t.contains(want), "missing {want:?} in:\n{t}");
        }
        let t = scan(None);
        assert!(t.contains("no percentage") && !t.contains("50%"), "{t}");
        // Past the expected total the bar holds at 99% instead of claiming done.
        let t = scan(Some(100 << 20));
        assert!(t.contains("99%"), "{t}");
    }

    #[test]
    fn small_and_tiny_terminals_do_not_panic() {
        for size in [(40, 10), (20, 6), (24, 7), (30, 12)] {
            let t = text(&render(&loaded(size), Theme::Ansi256));
            assert!(t.contains("/data") || t.contains("data"), "{size:?}:\n{t}");
        }
        for size in [(10, 3), (19, 24), (80, 5), (1, 1), (0, 0)] {
            let s = loaded(size);
            let t = text(&render(&s, Theme::TrueColor));
            if size.0 > 0 && size.1 > 0 && size != (1, 1) {
                assert!(
                    t.contains("small") || t.contains("Terminal") || t.contains("too"),
                    "{size:?}:\n{t}"
                );
            }
        }
        let t = text(&render(&loaded((10, 3)), Theme::TrueColor));
        assert!(t.contains("too small"), "{t}");
    }

    #[test]
    fn insights_show_sanitized_command_with_marker() {
        let mut s = loaded((100, 24));
        s.view = crate::state::View::Insights;
        s.insights = Some(vec![Insight {
            human: "1 GiB".into(),
            message: "m\x1b[2Jx".into(),
            path: "/p\x07q".into(),
            suggested_command: "rm -rf '/p\x1b]52;c;AA\x07'".into(),
        }]);
        let b = render(&s, Theme::TrueColor);
        let t = text(&b);
        assert!(t.contains(CONTROL_MARKER), "{t}");
        assert!(t.contains("\\u{1b}"), "{t}");
        for y in 0..b.area.height {
            for x in 0..b.area.width {
                let sym = b.cell((x, y)).unwrap().symbol();
                assert!(sym.chars().all(|c| !c.is_control()), "{sym:?}");
            }
        }
    }

    #[test]
    fn every_view_survives_every_size() {
        let listing = Listing {
            path: "/data/proj".into(),
            human: "1.0 GiB".into(),
            entries: (0..40)
                .map(|i| Entry {
                    name: format!("entry-{i}"),
                    path: format!("/data/proj/entry-{i}"),
                    is_dir: i % 2 == 0,
                    bytes: 1 << 20,
                    human: "1.0 MiB".into(),
                    pct_of_parent: Some(2.5),
                })
                .collect(),
            remaining: 3,
        };
        let ins = vec![Insight {
            human: "4.2 GiB".into(),
            message: "node_modules is re-created by npm install".into(),
            path: "/data/proj/node_modules".into(),
            suggested_command: "rm -rf /data/proj/node_modules".into(),
        }];
        for w in [0u16, 1, 5, 20, 21, 33, 80, 200] {
            for h in [0u16, 1, 5, 6, 7, 9, 24, 60] {
                let base = loaded((w, h));
                let paths = press(base.clone(), Key::Enter);
                let browse = update(paths.clone(), Event::Browsed(Ok(listing.clone()))).0;
                let insights = update(
                    press(base.clone(), Key::Char('i')),
                    Event::Insights(Ok(ins.clone())),
                )
                .0;
                let help = press(base.clone(), Key::Char('?'));
                let scanning = update(
                    press(base.clone(), Key::Char('r')),
                    Event::Progress(Progress {
                        files: 9,
                        dirs: 2,
                        bytes: 5,
                        errors: 1,
                        elapsed_ms: 4000,
                        threads: 2,
                        expected_bytes: Some(20),
                    }),
                )
                .0;
                let mut nosnap = update(
                    State::new("/very/long/root/path/to/somewhere", (w, h)),
                    Event::Overview(Ok(None)),
                )
                .0;
                nosnap.status = None;
                for s in [base, paths, browse, insights, help, scanning, nosnap] {
                    for theme in [Theme::TrueColor, Theme::Plain] {
                        let _ = render(&s, theme);
                    }
                }
            }
        }
    }

    #[test]
    fn no_snapshot_offers_rescan() {
        let s = update(State::new("/data", (80, 24)), Event::Overview(Ok(None))).0;
        let t = text(&render(&s, Theme::TrueColor));
        assert!(
            t.contains("No snapshot for /data yet.") && t.contains("Press r to scan"),
            "{t}"
        );
        assert!(t.contains("r scan") && t.contains("no snapshot"), "{t}");
        let s = press(s, Key::Char('r'));
        let t = text(&render(&s, Theme::TrueColor));
        assert!(
            t.contains("Scanning /data") && t.contains("results appear when the scan finishes"),
            "{t}"
        );
        assert!(!t.contains("browse last snapshot"), "{t}");
    }

    #[test]
    fn zero_byte_categories_and_empty_overview() {
        let mut o = overview();
        o.cats.clear();
        o.used_bytes = 0;
        let s = update(State::new("/data", (80, 24)), Event::Overview(Ok(Some(o)))).0;
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("No space used"), "{t}");
        let mut o = overview();
        o.cats.truncate(1);
        o.cats[0].bytes = 0;
        o.cats[0].human = "0 B".into();
        o.cats[0].share = 0.0;
        o.used_bytes = 0;
        o.free_bytes = None;
        let s = update(State::new("/data", (80, 24)), Event::Overview(Ok(Some(o)))).0;
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("0.0% of disk"), "{t}");
    }

    #[test]
    fn plain_theme_uses_glyph_bar_without_colors() {
        let b = render(&loaded((40, 12)), Theme::Plain);
        // 10 body rows: no spare line, so the bar sits right under the totals.
        let row: String = (0..40).map(|x| b[(x, 2)].symbol().to_string()).collect();
        assert!(
            row.contains('=') && row.contains('+') && !row.contains('\u{2580}'),
            "{row}"
        );
        assert_eq!(b[(2, 2)].fg, Color::Reset);
    }

    #[test]
    fn error_status_shows_in_red() {
        let s = update(loaded((80, 24)), Event::Browsed(Err("boom: gone".into()))).0;
        let b = render(&s, Theme::TrueColor);
        let last: String = (0..80).map(|x| b[(x, 23)].symbol().to_string()).collect();
        assert!(last.contains("boom: gone"));
        assert_eq!(b[(2, 23)].fg, Color::Red);
    }

    fn big_listing(n: usize) -> Listing {
        Listing {
            path: "/data/big".into(),
            human: "9.0 GiB".into(),
            entries: (0..n)
                .map(|i| Entry {
                    name: format!("item{i:03}"),
                    path: format!("/data/big/item{i:03}"),
                    is_dir: false,
                    bytes: 1000,
                    human: "1000 B".into(),
                    pct_of_parent: None,
                })
                .collect(),
            remaining: 0,
        }
    }

    #[test]
    fn long_browse_list_scrolls_with_the_selection() {
        let s = press(loaded((80, 24)), Key::Enter);
        let mut s = update(s, Event::Browsed(Ok(big_listing(100)))).0;
        let t = text(&render(&s, Theme::TrueColor));
        // ../ takes the first row, so one entry fewer fits.
        assert!(
            t.contains("../  data") && t.contains("item018") && !t.contains("item019"),
            "{t}"
        );
        assert!(
            t.contains("NAME") && t.contains("SIZE") && t.contains("SHARE"),
            "{t}"
        );
        for _ in 0..50 {
            s = press(s, Key::Down);
        }
        let b = render(&s, Theme::TrueColor);
        let t = text(&b);
        assert!(t.contains("item050"), "{t}");
        assert!(!t.contains("item000") && !t.contains("item030"), "{t}");
        // The selected row is the last visible one, and it is highlighted.
        let sel_row = (0..24u16)
            .find(|&y| {
                (0..80)
                    .map(|x| b[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .contains("item050")
            })
            .unwrap();
        assert_eq!(b[(0, sel_row)].symbol(), "\u{258c}");
        assert!(b[(2, sel_row)].modifier.contains(Modifier::REVERSED));
        assert_eq!(sel_row, 22);
        let s = press(s, Key::End);
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("item099") && !t.contains("item050"), "{t}");
        let s = press(s, Key::Home);
        assert!(text(&render(&s, Theme::TrueColor)).contains("item000"));
    }

    fn tree(path: &str, names: &[(&str, bool, f64)]) -> Listing {
        Listing {
            path: path.into(),
            human: "84.2 GiB".into(),
            entries: names
                .iter()
                .map(|&(n, d, p)| Entry {
                    name: n.into(),
                    path: format!("{path}/{n}"),
                    is_dir: d,
                    bytes: 1,
                    human: "1.0 GiB".into(),
                    pct_of_parent: Some(p),
                })
                .collect(),
            remaining: 0,
        }
    }

    /// First body row containing `needle` (the header row is skipped).
    fn row_with(b: &Buffer, needle: &str) -> Option<String> {
        (1..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .find(|r| r.contains(needle))
    }

    #[test]
    fn browse_shows_breadcrumb_bars_and_share() {
        let s = press(loaded((80, 24)), Key::Char('b'));
        let s = update(s, Event::Browsed(Ok(tree("/data", &[("proj", true, 0.0)])))).0;
        let s = press(s, Key::Right);
        let s = update(
            s,
            Event::Browsed(Ok(tree(
                "/data/proj",
                &[("app", true, 51.0), ("notes.txt", false, 0.4)],
            ))),
        )
        .0;
        let b = render(&s, Theme::TrueColor);
        let crumb = row_with(&b, "\u{203a}").unwrap();
        assert!(crumb.starts_with("data \u{203a} proj"), "{crumb}");
        assert!(
            crumb.contains("84.2 GiB") && crumb.contains("2 items"),
            "{crumb}"
        );
        let app = row_with(&b, "app/").unwrap();
        assert!(
            app.contains("\u{2501}") && app.trim_end().ends_with("51%"),
            "{app}"
        );
        let file = row_with(&b, "notes.txt").unwrap();
        assert!(
            file.trim_end().ends_with("<1%") && !file.contains("notes.txt/"),
            "{file}"
        );
        // A file selected: the hints say y copies it and drop "open".
        let s = press(s, Key::Down);
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("y copy path"), "{t}");
    }

    #[test]
    fn breadcrumb_numbers_while_jumping_and_folds_when_narrow() {
        let mut s = press(loaded((80, 24)), Key::Char('b'));
        s = update(s, Event::Browsed(Ok(tree("/data", &[("a", true, 1.0)])))).0;
        for p in [
            "/data/a",
            "/data/a/bb",
            "/data/a/bb/ccc",
            "/data/a/bb/ccc/dddd",
        ] {
            s = press(s, Key::Right);
            let name = match p.rsplit('/').next().unwrap() {
                "a" => "bb",
                "bb" => "ccc",
                "ccc" => "dddd",
                _ => "e",
            };
            s = update(s, Event::Browsed(Ok(tree(p, &[(name, true, 1.0)])))).0;
        }
        let s = press(s, Key::Char('g'));
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains(" 1  data") && t.contains(" 5  dddd"), "{t}");
        assert!(
            t.contains("jump to level") && t.contains("1\u{2013}5"),
            "{t}"
        );
        // Narrow: the middle folds into an ellipsis but root and current stay.
        let s = update(press(s, Key::Esc), Event::Resize(34, 12)).0;
        let crumb = row_with(&render(&s, Theme::TrueColor), "\u{203a}").unwrap();
        assert!(
            crumb.contains("data") && crumb.contains("\u{2026}") && crumb.contains("dddd"),
            "{crumb}"
        );
    }

    #[test]
    fn filter_bar_and_empty_states() {
        let s = press(loaded((80, 24)), Key::Char('b'));
        let s = update(
            s,
            Event::Browsed(Ok(tree(
                "/data",
                &[
                    ("next", true, 9.0),
                    ("react", true, 5.0),
                    ("next-auth", true, 1.0),
                ],
            ))),
        )
        .0;
        let s = ["/", "n", "e", "x"]
            .iter()
            .fold(s, |s, c| press(s, Key::Char(c.chars().next().unwrap())));
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("/ nex") && t.contains("2 of 3"), "{t}");
        assert!(!t.contains("react"), "{t}");
        let s = press(press(s, Key::Char('z')), Key::Enter);
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("Nothing here matches"), "{t}");
        assert!(t.contains("0 of 3 match"), "{t}");
        let s = update(
            press(press(s, Key::Esc), Key::Right),
            Event::Browsed(Ok(tree("/data/next", &[]))),
        )
        .0;
        assert!(text(&render(&s, Theme::TrueColor)).contains("Empty folder"));
    }

    #[test]
    fn insights_list_shows_message_and_command_and_scrolls() {
        let many: Vec<Insight> = (0..30)
            .map(|i| Insight {
                human: format!("{i} GiB"),
                message: format!("message number {i}"),
                path: format!("/data/p{i}"),
                suggested_command: format!("rm -rf /data/p{i}"),
            })
            .collect();
        let s = press(loaded((80, 24)), Key::Char('i'));
        let mut s = update(s, Event::Insights(Ok(many))).0;
        let t = text(&render(&s, Theme::TrueColor));
        assert!(
            t.contains("message number 0") && t.contains("$ rm -rf /data/p0"),
            "{t}"
        );
        assert!(t.contains("dredge never runs it"), "{t}");
        for _ in 0..15 {
            s = press(s, Key::Down);
        }
        let t = text(&render(&s, Theme::TrueColor));
        assert!(
            t.contains("message number 15") && t.contains("$ rm -rf /data/p15"),
            "{t}"
        );
        assert!(
            !t.contains("message number 0\n") && !t.contains("number 0 "),
            "{t}"
        );
    }

    #[test]
    fn help_lists_the_keys() {
        let t = text(&render(
            &press(loaded((80, 24)), Key::Char('?')),
            Theme::TrueColor,
        ));
        for k in [
            "Enter",
            "Esc",
            "filter",
            "breadcrumb",
            "insights",
            "never run",
            "rescan",
            "quit",
        ] {
            assert!(t.contains(k), "{k}\n{t}");
        }
    }

    #[test]
    fn paths_scroll_to_selected_item() {
        let mut s = loaded((40, 10));
        s.overview.as_mut().unwrap().cats[0].top =
            (0..20).map(|i| format!("path-{i:02}")).collect();
        s = press(s, Key::Enter);
        s = press(s, Key::End);
        let t = text(&render(&s, Theme::Plain));
        assert!(t.contains("path-19"), "{t}");
    }

    #[test]
    fn unicode_fitting_uses_display_columns() {
        for width in 0..10 {
            for text in ["界界界abc", "e\u{301}界界xyz"] {
                assert!(UnicodeWidthStr::width(fit(text, width).as_str()) <= width);
                assert!(UnicodeWidthStr::width(fit_tail(text, width).as_str()) <= width);
            }
        }
    }

    #[test]
    fn plain_theme_clears_all_colors() {
        let mut s = loaded((80, 24));
        s.status = Some(crate::state::Status {
            text: "error".into(),
            error: true,
        });
        assert!(render(&s, Theme::Plain)
            .content
            .iter()
            .all(|c| c.fg == Color::Reset && c.bg == Color::Reset));
    }

    #[test]
    fn age_formatting() {
        assert_eq!(
            [0, 59, 60, 3599, 3600, 86_399, 86_400, 900_000].map(fmt_age),
            ["<1m", "<1m", "1m", "59m", "1h", "23h", "1d", "10d"]
        );
    }

    #[test]
    fn theme_from_env() {
        assert_eq!(Theme::from_env(true, Some("truecolor")), Theme::Plain);
        assert_eq!(Theme::from_env(false, Some("24bit")), Theme::TrueColor);
        assert_eq!(Theme::from_env(false, None), Theme::Ansi256);
    }
}
