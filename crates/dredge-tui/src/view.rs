//! Drawing: `draw(frame, state, theme)` turns a [`State`] into cells. No input, no queries.

use dredge_render::{bar_cells, Caps, Glyphs, Ink, Segment, Shade};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Widget, Wrap};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::state::{
    crumbs, legend_cols, progress_text, Cat, Input, Insight, Level, Overview, Scan, State, View,
    INSIGHT_LINES, MIN_HEIGHT, MIN_WIDTH,
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
    let buf = f.buffer_mut();
    header(buf, s, w);
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
    status(buf, s, Rect::new(0, area.height - 1, w, 1), theme);
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

fn header(buf: &mut Buffer, s: &State, w: u16) {
    let w = usize::from(w);
    let mut stats = String::new();
    if let Some(o) = &s.overview {
        stats.push_str(&o.used_human);
        if let Some(t) = o.total_bytes.filter(|t| *t > 0) {
            let pct = o.used_bytes as f64 / t as f64 * 100.0;
            stats.push_str(&format!(
                " of {} ({pct:.1}%)",
                dredge_core::util::human_bytes(t)
            ));
        }
        stats.push_str(&format!("  snapshot {} old", fmt_age(o.age_secs)));
    } else if s.loaded {
        stats.push_str("no snapshot");
    }
    let scan = match &s.scan {
        Scan::Running(p) => format!("  scanning: {}", progress_text(p)),
        Scan::Idle => String::new(),
    };
    let right = format!("{stats}{scan}");
    let path_w = w.saturating_sub(right.chars().count() + 2).max(8);
    let title = fit_tail(&s.root, path_w);
    let used = UnicodeWidthStr::width(title.as_str());
    let right_w = w.saturating_sub(used + 2);
    let mut spans = vec![Span::styled(title, BOLD)];
    if right_w > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            fit(&right, right_w),
            if scan.is_empty() { Style::new() } else { DIM },
        ));
    }
    buf.set_line(0, 0, &clean(Line::from(spans)), w as u16);
}

fn hints(s: &State) -> &'static str {
    if s.help {
        return "? or Esc close help  q quit";
    }
    match s.view {
        View::Overview if s.overview.is_none() => "r scan  ? help  q quit",
        View::Overview => "\u{2190}\u{2191}\u{2193}\u{2192} move  Enter open  b browse  i insights  r rescan  ? help  q quit",
        View::Paths => "\u{2191}\u{2193} select  Enter browse  y copy  Esc back  i insights  ? help  q quit",
        View::Browse if s.current().and_then(Level::selected).is_some_and(|e| !e.is_dir) => {
            "\u{2191}\u{2193} move  \u{2190} back  y copy path  / filter  g jump  Esc exit  ? help"
        }
        View::Browse => "\u{2191}\u{2193} move  \u{2192} open  \u{2190} back  / filter  g jump  y copy  Esc exit  ? help",
        View::Insights => "\u{2191}\u{2193} select  Enter show  y copy command  i or Esc back  q quit",
    }
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
            None => Line::styled(fit(hints(s), w), DIM),
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
    let Some(o) = &s.overview else {
        let msg = if !s.loaded {
            "Loading\u{2026}".to_string()
        } else if s.scanning() {
            "Scanning\u{2026} the first snapshot appears when it finishes.".into()
        } else {
            format!("No snapshot for {} yet.\nPress r to scan it.", s.root)
        };
        for (i, l) in msg.lines().enumerate() {
            put(buf, body, i + 1, Line::raw(fit(l, usize::from(body.width))));
        }
        return;
    };
    if o.cats.is_empty() {
        put(buf, body, 1, Line::raw("No space used under this path."));
        return;
    }
    let width = usize::from(body.width);
    let segs = segments(o);
    let focus = Some(s.focus);
    let mut row = 0;
    if theme == Theme::Plain {
        let line = &dredge_render::render_bar(&segs, width, focus, Caps::PLAIN)[0];
        put(buf, body, row, Line::raw(line.0.clone()));
        row += 1;
    } else {
        for cells in bar_cells(&segs, width, focus, Glyphs::HalfBlock) {
            let spans: Vec<Span> = cells
                .iter()
                .map(|c| {
                    Span::styled(
                        c.ch.to_string(),
                        Style::new().fg(theme.color(c.fg)).bg(theme.color(c.bg)),
                    )
                })
                .collect();
            put(buf, body, row, Line::from(spans));
            row += 1;
        }
    }
    row += 1;

    // Legend, scrolled so the focused row stays visible, leaving room for the detail pane.
    let cols = legend_cols(&o.cats, body.width);
    let entries = legend_entries(o, s.focus, theme, cols);
    let total_rows = entries.len().div_ceil(cols);
    let avail = usize::from(body.height).saturating_sub(row);
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
        put(buf, body, row + r, Line::from(spans));
    }
    row += shown;
    if avail > shown + 1 {
        row += 1;
        let pane = Rect::new(
            body.x,
            body.y + row as u16,
            body.width,
            body.height - row as u16,
        );
        if let Some(c) = s.focused() {
            detail(buf, c, o, pane);
        }
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

fn detail(buf: &mut Buffer, c: &Cat, o: &Overview, pane: Rect) {
    let title = format!(" {} ", c.ink.label());
    let inner = if pane.height >= 4 && pane.width >= 8 {
        let block = Block::bordered().title(title);
        let inner = block.inner(pane);
        block.render(pane, buf);
        inner
    } else {
        pane
    };
    let w = usize::from(inner.width);
    let share = if o.used_bytes > 0 {
        format!("{:.1}% of used", c.share)
    } else {
        "0% of used".into()
    };
    let facts = format!("{}  \u{b7}  {share}  \u{b7}  {} files", c.human, c.files);
    put(buf, inner, 0, Line::styled(fit(&facts, w), BOLD));
    if c.top.is_empty() {
        put(buf, inner, 1, Line::styled("No single large path.", DIM));
        return;
    }
    put(
        buf,
        inner,
        1,
        Line::styled("Largest paths (Enter to open):", DIM),
    );
    for (i, p) in c.top.iter().enumerate() {
        put(buf, inner, 2 + i, Line::raw(fit_tail(p, w)));
    }
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
            "y",
            "copy the selected path, or an insight's command (never run)",
        ),
        ("r", "rescan the root in the background"),
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
        }
    }

    fn overview() -> Overview {
        Overview {
            path: "/data".into(),
            used_bytes: 600 << 20,
            used_human: "600.0 MiB".into(),
            total_bytes: Some(1000 << 20),
            free_bytes: Some(400 << 20),
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
    fn overview_at_80x24_has_header_bar_legend_and_detail() {
        let s = loaded((80, 24));
        let b = render(&s, Theme::TrueColor);
        let t = text(&b);
        for want in [
            "/data",
            "600.0 MiB of 1000.0 MiB (60.0%)",
            "snapshot 2h old",
            "Developer",
            "Photos",
            "Caches",
            "Free",
            "Largest paths",
            "/data/proj/node_modules",
            "q quit",
        ] {
            assert!(t.contains(want), "missing {want:?} in:\n{t}");
        }
        // The bar is the dithered half-block bar in the ink colors, on rows 1 and 2.
        let bar = b[(0, 1)].clone();
        assert_eq!(bar.symbol(), "\u{2580}");
        assert!(matches!(bar.fg, Color::Rgb(..)) && matches!(bar.bg, Color::Rgb(..)));
        assert_eq!(b[(79, 2)].symbol(), "\u{2580}");
    }

    #[test]
    fn tui_bar_cells_match_what_dredge_bar_paints() {
        let s = loaded((80, 24));
        let b = render(&s, Theme::TrueColor);
        let o = overview();
        let segs = segments(&o);
        let rows = bar_cells(&segs, 80, Some(0), Glyphs::HalfBlock);
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.iter().enumerate() {
                let cell = &b[(x as u16, 1 + y as u16)];
                assert_eq!(cell.symbol(), c.ch.to_string());
                assert_eq!(cell.fg, Theme::TrueColor.color(c.fg));
                assert_eq!(cell.bg, Theme::TrueColor.color(c.bg));
            }
        }
    }

    #[test]
    fn focus_changes_the_detail_pane() {
        let s = press(loaded((80, 24)), Key::Right);
        let t = text(&render(&s, Theme::TrueColor));
        assert!(
            t.contains("/data/pics") && !t.contains("node_modules"),
            "{t}"
        );
        let s = press(s, Key::Right);
        assert!(text(&render(&s, Theme::TrueColor)).contains("No single large path"));
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
                let mut scanning = base.clone();
                scanning = update(
                    scanning,
                    Event::Progress(Progress {
                        files: 9,
                        dirs: 2,
                        bytes: 5,
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
        assert!(t.contains("r scan"), "{t}");
        let s = press(s, Key::Char('r'));
        let t = text(&render(&s, Theme::TrueColor));
        assert!(t.contains("scanning: 0 files"), "{t}");
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
        assert!(t.contains("0% of used"), "{t}");
    }

    #[test]
    fn plain_theme_uses_glyph_bar_without_colors() {
        let b = render(&loaded((40, 12)), Theme::Plain);
        let row1: String = (0..40).map(|x| b[(x, 1)].symbol().to_string()).collect();
        assert!(
            row1.contains('=') && row1.contains('+') && !row1.contains('\u{2580}'),
            "{row1}"
        );
        assert_eq!(b[(0, 1)].fg, Color::Reset);
    }

    #[test]
    fn error_status_shows_in_red() {
        let s = update(loaded((80, 24)), Event::Browsed(Err("boom: gone".into()))).0;
        let b = render(&s, Theme::TrueColor);
        let last: String = (0..80).map(|x| b[(x, 23)].symbol().to_string()).collect();
        assert!(last.contains("boom: gone"));
        assert_eq!(b[(0, 23)].fg, Color::Red);
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

    fn row_with(b: &Buffer, needle: &str) -> Option<String> {
        (0..b.area.height)
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
