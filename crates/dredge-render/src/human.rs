//! The human view of every command's output (RFC 0003, docs/UX.md).
//!
//! Each document becomes lines that follow one skeleton: a lead line with the answer, a table
//! with sizes on the left, a remainder line, at most one hint, and a footer with the snapshot
//! age and the next command. Pure like the rest of this crate: the caller says what the
//! terminal supports ([`Style`]) and prints the lines.
//!
//! This view is for people at a terminal. Scripts and agents get the JSON document, which is
//! the contract; nothing here may be parsed.

use dredge_core::output::{
    BrowseOut, CategoriesOut, DetailOut, DoctorOut, EntryRecord, ErrorBody, ForgetOut, InsightsOut,
    LargestOut, Output, ScanOut, SnapshotRef, SnapshotsOut, TypesOut, VolumeRecord, VolumesOut,
};
use dredge_core::util::human_bytes;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{color, params, render_bar, sanitize, Caps, ColorTier, Glyphs, Ink, Line, Segment};

/// What the terminal supports and how wide it is.
#[derive(Clone, Debug)]
pub struct Style {
    pub caps: Caps,
    /// Columns available. Below [`NARROW`] the bar column is dropped.
    pub width: usize,
    /// The user's home folder, shown as `~`.
    pub home: Option<String>,
}

/// Narrower than this, tables drop the bar column (rule 8).
pub const NARROW: usize = 60;
/// Older than this, the footer turns amber and names the scan that refreshes it (rule 5).
pub const STALE_SECS: u64 = 7 * 86_400;
const BAR: usize = 10;
const INDENT: &str = "  ";

// ---------------------------------------------------------------------------------------------
// Tones and spans

/// What a piece of text means. Each tone has one meaning (rule 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    /// The terminal's own foreground.
    Plain,
    /// The answer: the one bold number in a lead line, the command in a Next line.
    Bold,
    /// Context around the answer.
    Muted,
    /// Headers, parent folders, the snapshot age.
    Faint,
    /// ✓ fine.
    Green,
    /// ! needs a look.
    Amber,
    /// ✗ failed.
    Red,
    /// ◆ a hint.
    Hint,
    /// A category, only for categories.
    Ink(Ink),
}

fn sgr(tier: ColorTier, tone: Tone) -> Option<String> {
    let rgb = |r: u8, g: u8, b: u8, ansi: &str| match tier {
        ColorTier::TrueColor => format!("38;2;{r};{g};{b}"),
        ColorTier::Ansi256 => format!(
            "38;5;{}",
            16 + 36 * crate::cube(r) + 6 * crate::cube(g) + crate::cube(b)
        ),
        _ => ansi.to_string(),
    };
    if tier == ColorTier::None {
        return None;
    }
    Some(match tone {
        Tone::Plain => return None,
        Tone::Bold => "1".into(),
        Tone::Muted => rgb(128, 134, 145, "90"),
        Tone::Faint => format!("2;{}", rgb(128, 134, 145, "90")),
        Tone::Green => rgb(31, 160, 90, "32"),
        Tone::Amber => rgb(229, 165, 10, "33"),
        Tone::Red => rgb(229, 72, 77, "31"),
        Tone::Hint => rgb(234, 88, 12, "91"),
        Tone::Ink(ink) => params(color(tier, ink, false), false),
    })
}

/// A line being built: painted text plus its width in terminal cells.
#[derive(Default)]
struct Buf {
    s: String,
    w: usize,
}

impl Buf {
    fn put(&mut self, tier: ColorTier, tone: Tone, text: &str) -> &mut Self {
        let text = sanitize(text);
        if text.is_empty() {
            return self;
        }
        self.w += width(&text);
        match sgr(tier, tone) {
            Some(code) => {
                self.s.push_str(&format!("\x1b[{code}m{text}\x1b[0m"));
            }
            None => self.s.push_str(&text),
        }
        self
    }

    fn line(self) -> Line {
        Line(self.s.trim_end().to_string())
    }
}

fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn lpad(s: &str, w: usize) -> String {
    format!("{}{s}", " ".repeat(w.saturating_sub(width(s))))
}

fn rpad(s: &str, w: usize) -> String {
    format!("{s}{}", " ".repeat(w.saturating_sub(width(s))))
}

/// Cuts the end off `s` so it fits `max` cells, marking the cut with `…`.
fn cut_end(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// Cuts the middle out of `s` so it fits `max` cells.
fn cut_middle(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max < 3 {
        return cut_end(s, max);
    }
    let keep = max - 1;
    let head_w = keep / 2;
    let tail_w = keep - head_w;
    let mut head = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > head_w {
            break;
        }
        head.push(c);
        w += cw;
    }
    let mut tail: Vec<char> = Vec::new();
    let mut w = 0;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if w + cw > tail_w {
            break;
        }
        tail.push(c);
        w += cw;
    }
    tail.reverse();
    format!("{head}…{}", tail.into_iter().collect::<String>())
}

fn count(n: u64) -> String {
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

/// `3 h`, `12 days`: how old a snapshot is.
pub fn age(secs: u64) -> String {
    match secs {
        0..=59 => "under a minute".into(),
        60..=3599 => format!("{} min", secs / 60),
        3600..=86_399 => format!("{} h", secs / 3600),
        86_400..=172_799 => "1 day".into(),
        _ => format!("{} days", secs / 86_400),
    }
}

fn duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

fn pct(v: f64) -> String {
    format!("{v:.1}%")
}

fn flag_tag(flag: &str) -> (Tone, &'static str) {
    match flag {
        "cloud_placeholder" => (Tone::Faint, "cloud"),
        "hardlink_alias" => (Tone::Faint, "hard link"),
        "denied" => (Tone::Amber, "unreadable"),
        "estimated" => (Tone::Faint, "estimated"),
        "name_lossy" => (Tone::Faint, "name not UTF-8"),
        "mount_point" => (Tone::Faint, "mount point"),
        _ => (Tone::Faint, "flagged"),
    }
}

// ---------------------------------------------------------------------------------------------
// Paths

/// Parent and leaf of `p`, fitted to `max` cells the way every command prints a path (rule 3):
/// `~` for `home`, the parent cut in its middle, the leaf kept whole when it fits. Folders end
/// in `/`. For views drawn outside this module, such as the TUI.
pub fn fit_path(p: &str, home: Option<&str>, dir: bool, max: usize) -> (String, String) {
    Style {
        caps: Caps::PLAIN,
        width: max,
        home: home.map(str::to_string),
    }
    .fit(p, dir, max)
}

impl Style {
    fn tier(&self) -> ColorTier {
        self.caps.color
    }

    /// `~` for the home folder (rule 3).
    fn tilde(&self, p: &str) -> String {
        if let Some(home) = self.home.as_deref().filter(|h| h.len() > 1) {
            let home = home.trim_end_matches('/');
            if p == home {
                return "~".into();
            }
            if let Some(rest) = p.strip_prefix(home) {
                if rest.starts_with('/') {
                    return format!("~{rest}");
                }
            }
        }
        p.to_string()
    }

    /// Parent and leaf of a path for display. Folders end in `/`.
    fn split(&self, p: &str, dir: bool) -> (String, String) {
        let t = sanitize(&self.tilde(p));
        if t == "/" || t == "~" {
            return (String::new(), if t == "~" { "~/".into() } else { t });
        }
        let t = t.trim_end_matches('/').to_string();
        let (parent, leaf) = match t.rfind('/') {
            Some(i) => (t[..=i].to_string(), t[i + 1..].to_string()),
            None => (String::new(), t),
        };
        let leaf = if dir { format!("{leaf}/") } else { leaf };
        (parent, leaf)
    }

    /// Parent and leaf, fitted to `max` cells by cutting the parent's middle, never the leaf
    /// unless the leaf alone is too wide.
    fn fit(&self, p: &str, dir: bool, max: usize) -> (String, String) {
        let (parent, leaf) = self.split(p, dir);
        if width(&parent) + width(&leaf) <= max {
            return (parent, leaf);
        }
        if width(&leaf) + 2 > max {
            return (String::new(), cut_middle(&leaf, max));
        }
        let budget = max - width(&leaf);
        let segs: Vec<&str> = parent.trim_end_matches('/').split('/').collect();
        let head = format!("{}/", segs[0]);
        let mut tail = String::new();
        for seg in segs[1..].iter().rev() {
            let next = format!("{seg}/{tail}");
            if width(&head) + 2 + width(&next) > budget {
                break;
            }
            tail = next;
        }
        let short = if width(&head) + 2 + width(&tail) <= budget {
            format!("{head}…/{tail}")
        } else if budget >= 2 {
            "…/".into()
        } else {
            String::new()
        };
        (short, leaf)
    }

    /// One string for a path in running text.
    fn short(&self, p: &str, dir: bool, max: usize) -> String {
        let (a, b) = self.fit(p, dir, max);
        format!("{a}{b}")
    }

    /// A path as a shell argument in a Next line: `~/…` when that is safe to paste, else
    /// the full path in single quotes.
    fn arg(&self, p: &str) -> String {
        let safe = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "/._-~+@%:,=".contains(c))
        };
        let t = self.tilde(p);
        if safe(&t) {
            t
        } else if safe(p) {
            p.to_string()
        } else {
            format!("'{}'", p.replace('\'', r"'\''"))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The skeleton

enum Name {
    /// A path: faint parent, plain leaf.
    Path { path: String, dir: bool },
    /// A label in one tone.
    Text { text: String, tone: Tone },
}

struct Row {
    size: String,
    share: Option<f64>,
    /// Bar fill, 0..=1.
    frac: f64,
    bar_tone: Tone,
    /// A status glyph before the name (forget).
    mark: Option<(Tone, &'static str)>,
    name: Name,
    tags: Vec<(Tone, String)>,
    /// A faint path after the tags, fitted into what is left (rule 3: never cut the leaf).
    path_tag: Option<String>,
}

impl Row {
    fn new(size: u64, share: Option<f64>, name: Name) -> Row {
        Row {
            size: human_bytes(size),
            share,
            frac: share.unwrap_or(0.0) / 100.0,
            bar_tone: Tone::Plain,
            mark: None,
            name,
            tags: Vec::new(),
            path_tag: None,
        }
    }
}

/// Lines of one answer, in skeleton order.
struct Page<'a> {
    st: &'a Style,
    lines: Vec<Line>,
}

impl<'a> Page<'a> {
    fn new(st: &'a Style) -> Page<'a> {
        Page {
            st,
            lines: Vec::new(),
        }
    }

    fn buf(&self) -> Buf {
        Buf::default()
    }

    fn push(&mut self, b: Buf) {
        self.lines.push(b.line());
    }

    fn blank(&mut self) {
        if self.lines.last().is_some_and(|l| !l.0.is_empty()) {
            self.lines.push(Line(String::new()));
        }
    }

    /// Lead: the bold number, then muted context ending in a path that is fitted to the width.
    fn lead(
        &mut self,
        mark: Option<(Tone, &str)>,
        number: &str,
        before: &str,
        path: Option<(&str, bool)>,
        after: &str,
    ) {
        let t = self.st.tier();
        let mut b = self.buf();
        if let Some((tone, glyph)) = mark {
            b.put(t, tone, glyph).put(t, Tone::Plain, " ");
        }
        b.put(t, Tone::Bold, number);
        let before = if before.is_empty() || before.starts_with(' ') {
            before.to_string()
        } else {
            format!(" {before}")
        };
        let before = before.as_str();
        let w = self.st.width;
        let room = w.saturating_sub(b.w + width(before) + width(after));
        let Some((path, dir)) = path else {
            b.put(
                t,
                Tone::Muted,
                &cut_end(&format!("{before}{after}"), w.saturating_sub(b.w)),
            );
            self.push(b);
            return;
        };
        if room >= 12 {
            let p = self.st.short(path, dir, room);
            b.put(t, Tone::Muted, &format!("{before}{p}{after}"));
            self.push(b);
            return;
        }
        // Too narrow for one line: the sentence, then the path indented, then the facts on the
        // path's line when they fit or on their own.
        b.put(
            t,
            Tone::Muted,
            &cut_end(before.trim_end(), w.saturating_sub(b.w)),
        );
        self.push(b);
        let inner = w.saturating_sub(2);
        let rest = after.trim_start_matches([' ', '\u{b7}']);
        let with_after = inner.saturating_sub(width(after));
        let mut b = self.buf();
        if with_after >= 12 {
            let p = self.st.short(path, dir, with_after);
            b.put(t, Tone::Muted, &format!("  {p}{after}"));
            self.push(b);
        } else {
            let p = self.st.short(path, dir, inner);
            b.put(t, Tone::Muted, &format!("  {p}"));
            self.push(b);
            if !rest.is_empty() {
                let mut b = self.buf();
                b.put(t, Tone::Muted, &format!("  {}", cut_end(rest, inner)));
                self.push(b);
            }
        }
    }

    /// A plain sentence in one tone, cut to the width.
    fn say(&mut self, tone: Tone, text: &str) {
        let t = self.st.tier();
        let mut b = self.buf();
        b.put(t, tone, &cut_end(text, self.st.width));
        self.push(b);
    }

    fn table(&mut self, size_header: &str, header: &str, rows: &[Row]) {
        if rows.is_empty() {
            return;
        }
        let t = self.st.tier();
        let st = self.st;
        let sw = rows
            .iter()
            .map(|r| width(&r.size))
            .max()
            .unwrap_or(0)
            .max(width(size_header));
        let with_share = rows.iter().any(|r| r.share.is_some());
        let with_bar = with_share && st.width >= NARROW;
        let with_mark = rows.iter().any(|r| r.mark.is_some());
        let ascii = st.caps.glyphs == Glyphs::Ascii;
        let (fill, track) = if ascii { ('=', '-') } else { ('━', '─') };

        let mut prefix = INDENT.len() + sw;
        if with_share {
            prefix += 2 + 6;
        }
        if with_bar {
            prefix += 2 + BAR;
        }
        prefix += 2;
        if with_mark {
            prefix += 2;
        }

        // Header (faint caps).
        let mut h = format!("{INDENT}{}", lpad(size_header, sw));
        if with_share {
            h.push_str(&format!("  {}", lpad("SHARE", 6)));
        }
        if with_bar {
            h.push_str(&format!("  {}", " ".repeat(BAR)));
        }
        h.push_str("  ");
        if with_mark {
            h.push_str("  ");
        }
        h.push_str(header);
        let mut b = self.buf();
        b.put(t, Tone::Faint, &h);
        self.push(b);

        for r in rows {
            let mut b = self.buf();
            b.put(t, Tone::Plain, &format!("{INDENT}{}", lpad(&r.size, sw)));
            if with_share {
                let s = r.share.map(pct).unwrap_or_default();
                b.put(t, Tone::Muted, &format!("  {}", lpad(&s, 6)));
            }
            if with_bar {
                let n = ((r.frac.clamp(0.0, 1.0) * BAR as f64).round() as usize).min(BAR);
                b.put(t, Tone::Plain, "  ");
                b.put(t, r.bar_tone, &fill.to_string().repeat(n));
                b.put(t, Tone::Faint, &track.to_string().repeat(BAR - n));
            }
            b.put(t, Tone::Plain, "  ");
            if with_mark {
                match r.mark {
                    Some((tone, g)) => b.put(t, tone, g).put(t, Tone::Plain, " "),
                    None => b.put(t, Tone::Plain, "  "),
                };
            }
            let room = st.width.saturating_sub(prefix).max(12);
            let tags: Vec<(Tone, String)> = r
                .tags
                .iter()
                .map(|(tone, s)| (*tone, sanitize(s)))
                .collect();
            let tags_w: usize = tags.iter().map(|(_, s)| width(s) + 2).sum();
            // The name gets what the tags leave, but never less than half the room; tags that
            // still do not fit are cut at the end, then dropped.
            let used = match &r.name {
                Name::Path { path, dir } => {
                    let (pw, lw) = {
                        let (p, l) = st.split(path, *dir);
                        (width(&p), width(&l))
                    };
                    let max = (pw + lw).min(room.saturating_sub(tags_w).max(room / 2));
                    let (p, l) = st.fit(path, *dir, max);
                    b.put(t, Tone::Faint, &p).put(t, Tone::Plain, &l);
                    width(&p) + width(&l)
                }
                Name::Text { text, tone } => {
                    let text = sanitize(text);
                    let max = width(&text).min(room.saturating_sub(tags_w).max(room / 2));
                    let text = cut_end(&text, max);
                    b.put(t, *tone, &text);
                    width(&text)
                }
            };
            let left = room.saturating_sub(used);
            let mut taken = 0;
            for (tone, s) in &tags {
                if taken + width(s) + 2 > left {
                    let rest = left.saturating_sub(taken + 2);
                    if rest >= 8 {
                        b.put(t, Tone::Plain, "  ").put(t, *tone, &cut_end(s, rest));
                    }
                    break;
                }
                taken += width(s) + 2;
                b.put(t, Tone::Plain, "  ").put(t, *tone, s);
            }
            if let Some(path) = &r.path_tag {
                let rest = left.saturating_sub(taken + 2);
                if rest >= 12 {
                    let (par, leaf) = st.fit(path, false, rest);
                    b.put(t, Tone::Plain, "  ")
                        .put(t, Tone::Faint, &format!("{par}{leaf}"));
                }
            }
            self.push(b);
        }
    }

    /// `+ N more · size`: what `--limit` left out (rule: the rows plus this add up).
    fn remainder(&mut self, n: u64, what: &str, bytes: Option<u64>) {
        if n == 0 {
            return;
        }
        let mut s = format!("{INDENT}+ {} more", count(n));
        if !what.is_empty() {
            s.push_str(&format!(" {what}"));
        }
        if let Some(b) = bytes.filter(|b| *b > 0) {
            s.push_str(&format!(" · {}", human_bytes(b)));
        }
        self.say(Tone::Muted, &s);
    }

    /// At most one ◆ line.
    fn hint(&mut self, text: &str, tail: &str) {
        let t = self.st.tier();
        self.blank();
        let mut b = self.buf();
        b.put(t, Tone::Hint, "◆ ");
        let room = self.st.width.saturating_sub(2);
        let text = cut_end(text, room);
        let left = room.saturating_sub(width(&text));
        b.put(t, Tone::Plain, &text);
        b.put(t, Tone::Muted, &cut_end(tail, left));
        self.push(b);
    }

    /// Snapshot age (rule 5) and the Next commands (rule 6).
    fn footer(&mut self, snap: Option<&SnapshotRef>, note: Option<&str>, next: &[(String, &str)]) {
        let t = self.st.tier();
        self.blank();
        if let Some(s) = snap {
            let mut b = self.buf();
            let when = format!("snapshot {} old", age(s.age_secs));
            if s.age_secs > STALE_SECS {
                b.put(t, Tone::Amber, &format!("! {when}"));
                b.put(
                    t,
                    Tone::Muted,
                    &format!(" · dredge scan {} refreshes it", self.st.arg(&s.root)),
                );
            } else {
                b.put(t, Tone::Faint, &when);
            }
            self.push(b);
            if let Some(e) = &s.scan_errors {
                let n = e.denied + e.locked + e.other;
                if n > 0 {
                    self.say(
                        Tone::Amber,
                        &format!(
                            "! {} could not be read in that scan; sizes may be low",
                            plural(n, "item", "items")
                        ),
                    );
                }
            }
        }
        if let Some(n) = note {
            for line in wrap(n, self.st.width) {
                self.say(Tone::Faint, &line);
            }
        }
        for (i, (cmd, what)) in next.iter().enumerate() {
            let mut b = self.buf();
            b.put(t, Tone::Faint, if i == 0 { "Next  " } else { "      " });
            b.put(t, Tone::Bold, cmd);
            let left = self.st.width.saturating_sub(6 + width(cmd) + 2);
            if left >= 8 {
                b.put(t, Tone::Muted, &format!("  {}", cut_end(what, left)));
            }
            self.push(b);
        }
    }

    fn done(mut self) -> Vec<Line> {
        while self.lines.last().is_some_and(|l| l.0.is_empty()) {
            self.lines.pop();
        }
        self.lines
    }
}

// ---------------------------------------------------------------------------------------------
// Commands

/// The human view of `out`. Errors render with [`render_error`] instead (they go to stderr).
pub fn render_human(out: &Output, st: &Style) -> Vec<Line> {
    match out {
        Output::Volumes(e) => volumes(e, st),
        Output::Scan(e) => scan(e, st),
        Output::Snapshots(e) => snapshots(e, st),
        Output::Browse(e) => browse(e, st),
        Output::Largest(e) => largest(e, st),
        Output::Types(e) => types(e, st),
        Output::Categories(e) => categories(e, st),
        Output::Insights(e) => insights(e, st),
        Output::Detail(e) => detail(e, st),
        Output::Doctor(e) => doctor(e, st),
        Output::Forget(e) => forget(e, st),
        Output::Error(e) => render_error(&e.error, st),
        // Only the MCP server produces jobs; it never asks for the human view.
        Output::Job(_) => vec![Line(out.render(false))],
    }
}

/// `✗ message` and `fix hint`, for stderr.
pub fn render_error(e: &ErrorBody, st: &Style) -> Vec<Line> {
    let t = st.tier();
    let mut p = Page::new(st);
    let mut b = p.buf();
    b.put(t, Tone::Red, "✗ ").put(t, Tone::Plain, &e.message);
    p.push(b);
    if let Some(h) = &e.hint {
        let mut b = p.buf();
        b.put(t, Tone::Faint, "  fix ").put(t, Tone::Plain, h);
        p.push(b);
    }
    p.done()
}

fn volumes(e: &VolumesOut, st: &Style) -> Vec<Line> {
    let t = st.tier();
    let mut p = Page::new(st);
    let full = e
        .records
        .iter()
        .fold(None, |best: Option<&VolumeRecord>, v| match best {
            Some(b) if b.used_pct >= v.used_pct => Some(b),
            _ => Some(v),
        });
    let Some(full) = full else {
        p.say(Tone::Muted, "No volumes found.");
        p.footer(
            None,
            None,
            &[("dredge doctor".into(), "check what dredge can see")],
        );
        return p.done();
    };
    let name = full.label.clone().unwrap_or_else(|| full.mount.clone());
    p.lead(
        None,
        &human_bytes(full.free_bytes),
        &format!(
            "free on {} · {:.0}% used · ",
            sanitize(&name),
            full.used_pct
        ),
        None,
        &plural(e.records.len() as u64, "volume", "volumes"),
    );
    p.blank();
    let rows: Vec<Row> = e
        .records
        .iter()
        .map(|v| {
            let mut r = Row::new(
                v.used_bytes,
                Some(v.used_pct),
                Name::Text {
                    text: v.mount.clone(),
                    tone: Tone::Plain,
                },
            );
            if v.used_pct >= 90.0 {
                r.bar_tone = Tone::Amber;
            }
            let mut info = format!("of {} · {}", v.total_human, v.fs);
            if let Some(l) = &v.label {
                info.push_str(&format!(" · {l}"));
            }
            r.tags.push((Tone::Faint, info));
            r.tags.push(match v.snapshot_age_secs {
                Some(a) if a > STALE_SECS => (Tone::Amber, format!("scanned {} ago", age(a))),
                Some(a) => (Tone::Faint, format!("scanned {} ago", age(a))),
                None => (Tone::Faint, "not scanned".into()),
            });
            r
        })
        .collect();
    p.table("USED", "VOLUME", &rows);
    if !e.summary.focus.is_empty() {
        p.blank();
        let mut b = p.buf();
        b.put(t, Tone::Faint, "  FOCUS ROOTS");
        p.push(b);
        for f in &e.summary.focus {
            let mut b = p.buf();
            b.put(
                t,
                Tone::Plain,
                &format!("  {}", st.short(&f.path, true, st.width.saturating_sub(30))),
            );
            let (tone, s) = match (f.snapshot_age_secs, &f.covered_by) {
                (None, _) => (Tone::Amber, "not scanned".to_string()),
                (Some(a), Some(by)) => (
                    Tone::Faint,
                    format!("in the scan of {}, {} ago", st.tilde(by), age(a)),
                ),
                (Some(a), None) if a > STALE_SECS => {
                    (Tone::Amber, format!("scanned {} ago", age(a)))
                }
                (Some(a), None) => (Tone::Faint, format!("scanned {} ago", age(a))),
            };
            b.put(t, Tone::Plain, "  ").put(t, tone, &s);
            p.push(b);
        }
    }
    let first = match full.snapshot_age_secs {
        None => (
            format!("dredge scan {}", st.arg(&full.mount)),
            "see what fills it",
        ),
        Some(_) => (
            format!("dredge categories {}", st.arg(&full.mount)),
            "what kind of space it is",
        ),
    };
    p.footer(None, None, &[first, ("dredge tui".into(), "explore it")]);
    p.done()
}

fn scan(e: &ScanOut, st: &Style) -> Vec<Line> {
    let s = &e.summary;
    let mut p = Page::new(st);
    p.lead(
        Some((Tone::Green, "✓")),
        &s.human,
        "in ",
        Some((&s.root, true)),
        &format!(
            " · {} · {} · {}",
            plural(s.files, "file", "files"),
            plural(s.dirs, "folder", "folders"),
            duration(s.elapsed_ms)
        ),
    );
    p.say(
        Tone::Faint,
        &format!(
            "  {} files/s · snapshot {}",
            count(s.files_per_sec),
            human_bytes(s.snapshot_bytes)
        ),
    );
    if s.cloud_placeholders > 0 {
        p.say(
            Tone::Muted,
            &format!(
                "  {} not downloaded; counted at their size on this disk",
                plural(s.cloud_placeholders, "cloud file", "cloud files")
            ),
        );
    }
    if let Some(u) = s.unaccounted_bytes.filter(|u| *u > 0) {
        p.say(
            Tone::Muted,
            &format!(
                "  {} of the volume's used space is outside the scan (system metadata, unreadable folders)",
                human_bytes(u)
            ),
        );
    }
    let errs = s.errors.denied + s.errors.locked + s.errors.other;
    if errs > 0 {
        p.say(
            Tone::Amber,
            &format!(
                "! {} could not be read; sizes may be low",
                plural(errs, "item", "items")
            ),
        );
        if let Some(first) = s.errors.samples.first() {
            p.say(
                Tone::Faint,
                &format!(
                    "  for example {}",
                    st.short(first, false, st.width.saturating_sub(16))
                ),
            );
        }
    }
    if !s.evicted.is_empty() {
        let roots: Vec<String> = s.evicted.iter().map(|r| st.tilde(r)).collect();
        p.say(
            Tone::Muted,
            &format!(
                "  DREDGE_MAX_DATA removed older snapshots: {}",
                roots.join(", ")
            ),
        );
    }
    let root = st.arg(&s.root);
    p.footer(
        None,
        None,
        &[
            (
                format!("dredge categories {root}"),
                "what kind of space it is",
            ),
            (format!("dredge tui {root}"), "explore it"),
        ],
    );
    p.done()
}

fn entry_row(r: &EntryRecord, share: Option<f64>, full_path: bool) -> Row {
    let dir = r.kind == "dir";
    let name = if full_path {
        Name::Path {
            path: r.path.clone(),
            dir,
        }
    } else {
        Name::Text {
            text: if dir {
                format!("{}/", r.name)
            } else {
                r.name.clone()
            },
            tone: Tone::Plain,
        }
    };
    let mut row = Row::new(r.allocated_bytes, share, name);
    if matches!(r.kind, "symlink" | "junction" | "reparse") {
        row.tags.push((Tone::Faint, "link".into()));
    }
    for f in &r.flags {
        let (tone, s) = flag_tag(f);
        row.tags.push((tone, s.into()));
    }
    row
}

fn parent_of(p: &str) -> Option<String> {
    let t = p.trim_end_matches('/');
    let i = t.rfind('/')?;
    Some(if i == 0 {
        "/".into()
    } else {
        t[..i].to_string()
    })
}

fn browse(e: &BrowseOut, st: &Style) -> Vec<Line> {
    let s = &e.summary;
    let mut p = Page::new(st);
    if e.records.is_empty() {
        p.lead(None, &s.human, "in ", Some((&s.path, true)), " · empty");
        let next: Vec<(String, &str)> = parent_of(&s.path)
            .map(|up| (format!("dredge browse {}", st.arg(&up)), "go up a folder"))
            .into_iter()
            .collect();
        p.footer(e.snapshot.as_ref(), None, &next);
        return p.done();
    }
    p.lead(
        None,
        &s.human,
        "in ",
        Some((&s.path, true)),
        &format!(
            " · {}, largest first",
            plural(s.entries as u64, "item", "items")
        ),
    );
    p.blank();
    let rows: Vec<Row> = e
        .records
        .iter()
        .map(|r| entry_row(r, r.pct_of_parent, false))
        .collect();
    p.table("SIZE", "NAME", &rows);
    p.remainder(s.remaining_entries as u64, "", Some(s.remaining_bytes));
    let mut next = Vec::new();
    if let Some(d) = e.records.iter().find(|r| r.kind == "dir") {
        next.push((
            format!("dredge browse {}", st.arg(&d.path)),
            "go into the largest folder",
        ));
    }
    next.push((
        format!("dredge largest {}", st.arg(&s.path)),
        "the biggest files below",
    ));
    p.footer(e.snapshot.as_ref(), None, &next);
    p.done()
}

fn largest(e: &LargestOut, st: &Style) -> Vec<Line> {
    let s = &e.summary;
    let mut p = Page::new(st);
    let mut filters = String::new();
    if let Some(x) = &s.filters.ext {
        filters.push_str(&format!(" · only .{x}"));
    }
    if let Some(m) = s.filters.min_bytes {
        filters.push_str(&format!(" · at least {}", human_bytes(m)));
    }
    if e.records.is_empty() {
        p.lead(
            None,
            "No files",
            " match under ",
            Some((&s.path, true)),
            &filters,
        );
        let next = if s.filters.is_empty() {
            (
                format!("dredge browse {}", st.arg(&s.path)),
                "what is in it",
            )
        } else {
            (
                format!("dredge largest {}", st.arg(&s.path)),
                "without the filters",
            )
        };
        p.footer(e.snapshot.as_ref(), None, &[next]);
        return p.done();
    }
    p.lead(
        None,
        &human_bytes(s.matched_bytes),
        &format!("in {} under ", plural(s.matched_files, "file", "files")),
        Some((&s.path, true)),
        &filters,
    );
    p.blank();
    let rows: Vec<Row> = e
        .records
        .iter()
        .map(|r| {
            let mut row = entry_row(r, r.pct_of_scope, true);
            // The lead names the folder; rows show the path below it.
            let base = format!("{}/", s.path.trim_end_matches('/'));
            if let Some(rel) = r.path.strip_prefix(&base) {
                row.name = Name::Path {
                    path: rel.to_string(),
                    dir: false,
                };
            }
            row
        })
        .collect();
    p.table("SIZE", "FILE", &rows);
    if s.truncated {
        let shown_bytes: u64 = e.records.iter().map(|r| r.allocated_bytes).sum();
        p.remainder(
            s.matched_files.saturating_sub(e.records.len() as u64),
            "files",
            Some(s.matched_bytes.saturating_sub(shown_bytes)),
        );
    }
    p.footer(
        e.snapshot.as_ref(),
        None,
        &[(
            format!("dredge detail {}", st.arg(&e.records[0].path)),
            "everything about the biggest one",
        )],
    );
    p.done()
}

fn ext_label(ext: &str) -> String {
    if ext == "(none)" {
        "(no extension)".into()
    } else {
        format!(".{ext}")
    }
}

fn types(e: &TypesOut, st: &Style) -> Vec<Line> {
    let s = &e.summary;
    let mut p = Page::new(st);
    let Some(top) = e.records.first() else {
        p.lead(None, "No files", " under ", Some((&s.path, true)), "");
        p.footer(
            e.snapshot.as_ref(),
            None,
            &[(
                format!("dredge browse {}", st.arg(&s.path)),
                "what is in it",
            )],
        );
        return p.done();
    };
    let what = if top.ext == "(none)" {
        "files with no extension".to_string()
    } else {
        format!(".{} files", top.ext)
    };
    p.lead(
        None,
        &top.human,
        &format!("in {what}, {} of ", pct(top.pct_of_scope)),
        Some((&s.path, true)),
        &format!(" · {}", plural(s.types as u64, "type", "types")),
    );
    p.blank();
    let rows: Vec<Row> = e
        .records
        .iter()
        .map(|r| {
            let mut row = Row::new(
                r.allocated_bytes,
                Some(r.pct_of_scope),
                Name::Text {
                    text: ext_label(&r.ext),
                    tone: Tone::Plain,
                },
            );
            row.tags
                .push((Tone::Faint, plural(r.files, "file", "files")));
            row
        })
        .collect();
    p.table("SIZE", "TYPE", &rows);
    p.remainder(
        (s.types.saturating_sub(s.shown)) as u64,
        "types",
        Some(s.remaining_bytes),
    );
    let next = if top.ext == "(none)" {
        (
            format!("dredge largest {}", st.arg(&s.path)),
            "the biggest files",
        )
    } else {
        (
            format!(
                "dredge largest {} --ext {}",
                st.arg(&s.path),
                st.arg(&top.ext)
            ),
            "the biggest of them",
        )
    };
    p.footer(e.snapshot.as_ref(), None, &[next]);
    p.done()
}

fn categories(e: &CategoriesOut, st: &Style) -> Vec<Line> {
    let s = &e.summary;
    let mut p = Page::new(st);
    if e.records.iter().all(|r| r.allocated_bytes == 0) {
        p.lead(
            None,
            &s.human,
            "in ",
            Some((&s.path, true)),
            " · nothing to sort",
        );
        p.footer(
            e.snapshot.as_ref(),
            None,
            &[(
                format!("dredge browse {}", st.arg(&s.path)),
                "what is in it",
            )],
        );
        return p.done();
    }
    p.lead(
        None,
        &s.human,
        "in ",
        Some((&s.path, true)),
        &format!(
            " · {}",
            plural(s.categories as u64, "category", "categories")
        ),
    );
    p.blank();
    let segments: Vec<Segment> = e
        .records
        .iter()
        .filter(|r| r.allocated_bytes > 0)
        .map(Segment::from_record)
        .collect();
    let bar_w = st.width.saturating_sub(2 * INDENT.len()).clamp(10, 100);
    for l in render_bar(&segments, bar_w, None, st.caps) {
        p.lines.push(Line(format!("{INDENT}{}", l.0)));
    }
    p.blank();
    let rows: Vec<Row> = e
        .records
        .iter()
        .filter(|r| r.allocated_bytes > 0)
        .map(|r| {
            let ink = Ink::from_category(r.category);
            let mut row = Row::new(
                r.allocated_bytes,
                Some(r.share),
                Name::Text {
                    text: ink.label().into(),
                    tone: Tone::Ink(ink),
                },
            );
            row.bar_tone = Tone::Ink(ink);
            if let Some(top) = r.top.first() {
                row.path_tag = Some(top.clone());
            }
            row
        })
        .collect();
    p.table("SIZE", "CATEGORY", &rows);
    if s.truncated {
        p.remainder(
            (s.categories.saturating_sub(s.shown)) as u64,
            "categories",
            None,
        );
    }
    let path = st.arg(&s.path);
    p.footer(
        e.snapshot.as_ref(),
        None,
        &[
            (format!("dredge insights {path}"), "what could be cleared"),
            (format!("dredge browse {path}"), "folders, largest first"),
        ],
    );
    p.done()
}

fn insights(e: &InsightsOut, st: &Style) -> Vec<Line> {
    let t = st.tier();
    let s = &e.summary;
    let mut p = Page::new(st);
    if e.records.is_empty() {
        p.lead(
            None,
            "Nothing",
            " to suggest under ",
            Some((&s.path, true)),
            "",
        );
        p.footer(
            e.snapshot.as_ref(),
            None,
            &[(
                format!("dredge categories {}", st.arg(&s.path)),
                "what kind of space it is",
            )],
        );
        return p.done();
    }
    let total: u64 = e.records.iter().map(|r| r.allocated_bytes).sum();
    // The total covers the rows shown; say so when `--limit` left some out.
    let scope = if s.truncated {
        format!(
            "in the {} largest of {} hints under ",
            count(s.shown as u64),
            count(s.insights as u64)
        )
    } else {
        format!("in {} under ", plural(s.insights as u64, "hint", "hints"))
    };
    p.lead(
        None,
        &human_bytes(total),
        &format!("could be rebuilt or cleared, {scope}"),
        Some((&s.path, true)),
        "",
    );
    p.blank();
    let sw = e
        .records
        .iter()
        .map(|r| width(&r.human))
        .max()
        .unwrap_or(4)
        .max(4);
    let pad = INDENT.len() + sw + 2 + 2;
    let room = st.width.saturating_sub(pad).max(12);
    for r in &e.records {
        let mut b = p.buf();
        b.put(t, Tone::Plain, &format!("{INDENT}{}  ", lpad(&r.human, sw)));
        b.put(t, Tone::Hint, "◆ ");
        let (par, leaf) = st.fit(&r.path, true, room);
        b.put(t, Tone::Faint, &par).put(t, Tone::Plain, &leaf);
        p.push(b);
        let lines = wrap(&r.message, room);
        for (i, line) in lines.iter().take(2).enumerate() {
            let line = if i == 1 && lines.len() > 2 {
                cut_end(&format!("{line} …"), room)
            } else {
                line.clone()
            };
            let mut b = p.buf();
            b.put(t, Tone::Muted, &format!("{}{line}", " ".repeat(pad)));
            p.push(b);
        }
        let mut b = p.buf();
        // The command is never cut: it is there to be copied.
        b.put(t, Tone::Faint, &format!("{}$ ", " ".repeat(pad)));
        b.put(t, Tone::Plain, &r.suggested_command);
        p.push(b);
    }
    if s.truncated {
        p.remainder((s.insights.saturating_sub(s.shown)) as u64, "hints", None);
    }
    p.footer(
        e.snapshot.as_ref(),
        Some("dredge never runs these. Read each one before you run it."),
        &[(
            format!("dredge detail {}", st.arg(&e.records[0].path)),
            "check the first one before removing anything",
        )],
    );
    p.done()
}

fn detail(e: &DetailOut, st: &Style) -> Vec<Line> {
    let t = st.tier();
    let mut p = Page::new(st);
    let Some(r) = e.records.first() else {
        p.say(Tone::Muted, "Nothing known about this path.");
        return p.done();
    };
    let dir = r.kind == "dir";
    let kind = match r.kind {
        "dir" => "folder",
        "file" => "file",
        "symlink" => "symbolic link",
        k => k,
    };
    p.lead(None, &r.human, "", None, &format!(" · {kind}"));
    let mut b = p.buf();
    let (par, leaf) = st.fit(&r.path, dir, st.width.saturating_sub(2));
    b.put(t, Tone::Faint, &format!("{INDENT}{par}"))
        .put(t, Tone::Plain, &leaf);
    p.push(b);
    p.blank();
    let mut kv: Vec<(&str, String, Tone)> = Vec::new();
    if let Some(c) = r.category {
        let ink = Ink::from_category(c);
        kv.push(("category", ink.label().into(), Tone::Ink(ink)));
    }
    if let Some(l) = r.logical_bytes {
        kv.push((
            "logical",
            format!("{} before compression and sparse ranges", human_bytes(l)),
            Tone::Plain,
        ));
    }
    if let Some(f) = r.files {
        kv.push(("files", count(f), Tone::Plain));
    }
    if let Some(d) = r.dirs {
        kv.push(("folders", count(d), Tone::Plain));
    }
    if let Some(s) = r.pct_of_parent {
        kv.push((
            "share",
            format!("{} of the folder it is in", pct(s)),
            Tone::Plain,
        ));
    }
    if let Some(m) = &r.modified {
        kv.push(("modified", m.clone(), Tone::Plain));
    }
    if let Some(h) = r.hard_links.filter(|h| *h > 1) {
        kv.push((
            "links",
            format!("{h} hard links to the same data, counted once"),
            Tone::Plain,
        ));
    }
    if !r.flags.is_empty() {
        let f: Vec<&str> = r.flags.iter().map(|f| flag_tag(f).1).collect();
        kv.push(("flags", f.join(" · "), Tone::Plain));
    }
    let kw = kv.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);
    for (k, v, tone) in &kv {
        let mut b = p.buf();
        b.put(t, Tone::Faint, &format!("{INDENT}{}  ", rpad(k, kw)));
        b.put(t, *tone, &cut_end(v, st.width.saturating_sub(kw + 4)));
        p.push(b);
    }
    let next = if dir {
        Some((
            format!("dredge browse {}", st.arg(&r.path)),
            "what is inside",
        ))
    } else {
        e.summary.parent.as_ref().map(|par| {
            (
                format!("dredge browse {}", st.arg(par)),
                "the folder it is in",
            )
        })
    };
    p.footer(
        e.snapshot.as_ref(),
        None,
        &next.into_iter().collect::<Vec<_>>(),
    );
    p.done()
}

fn snapshots(e: &SnapshotsOut, st: &Style) -> Vec<Line> {
    let s = &e.summary;
    let mut p = Page::new(st);
    if e.records.is_empty() && s.temp_files == 0 {
        p.lead(
            None,
            "No snapshots",
            " yet in ",
            Some((&s.data_dir, true)),
            "",
        );
        p.footer(
            None,
            None,
            &[("dredge scan ~".into(), "scan your home folder")],
        );
        return p.done();
    }
    p.lead(
        None,
        &s.human,
        &format!(
            "in {} · ",
            plural(s.snapshots as u64, "snapshot", "snapshots")
        ),
        Some((&s.data_dir, true)),
        "",
    );
    p.blank();
    let total = s.file_bytes.max(1) as f64;
    let mut recs: Vec<_> = e.records.iter().collect();
    recs.sort_by(|a, b| {
        b.file_bytes
            .cmp(&a.file_bytes)
            .then_with(|| a.root.cmp(&b.root))
    });
    let rows: Vec<Row> = recs
        .into_iter()
        .map(|r| {
            let mut row = Row::new(
                r.file_bytes,
                Some(r.file_bytes as f64 / total * 100.0),
                Name::Path {
                    path: r.root.clone(),
                    dir: true,
                },
            );
            if r.shadowed_by.is_some() {
                row.tags.push((Tone::Amber, "! unused".into()));
            } else if !r.root_exists {
                row.tags.push((Tone::Amber, "! folder is gone".into()));
            }
            row.tags
                .push((Tone::Faint, format!("{} old", age(r.age_secs))));
            row
        })
        .collect();
    p.table("SIZE", "ROOT", &rows);
    if s.temp_files > 0 {
        p.say(
            Tone::Muted,
            &format!(
                "{INDENT}+ {} left by unfinished writes · {}",
                plural(s.temp_files as u64, "temp file", "temp files"),
                human_bytes(s.temp_bytes)
            ),
        );
    }
    if s.prunable > 0 {
        p.hint(
            &format!(
                "{} ({}) {} never read again",
                plural(s.prunable as u64, "file", "files"),
                human_bytes(s.prunable_bytes),
                if s.prunable == 1 { "is" } else { "are" }
            ),
            "; dredge forget --prune removes them",
        );
        p.footer(
            None,
            None,
            &[(
                "dredge forget --prune --dry-run".into(),
                "see what would be removed",
            )],
        );
    } else {
        p.footer(
            None,
            None,
            &[("dredge volumes".into(), "free space on each volume")],
        );
    }
    p.done()
}

fn forget_reason(reason: &str) -> &'static str {
    match reason {
        "shadowed" => "unused",
        "root_missing" => "folder is gone",
        "unsupported_format" => "old format",
        "corrupt" => "unreadable",
        "temp_file" => "unfinished write",
        "max_data" => "over DREDGE_MAX_DATA",
        "all" => "all",
        _ => "asked for",
    }
}

fn forget(e: &ForgetOut, st: &Style) -> Vec<Line> {
    let s = &e.summary;
    let mut p = Page::new(st);
    if e.records.is_empty() {
        p.lead(None, "Nothing", " to remove", None, "");
        p.footer(
            None,
            None,
            &[("dredge snapshots".into(), "what dredge keeps")],
        );
        return p.done();
    }
    let n = plural(s.removed as u64, "file", "files");
    if s.dry_run {
        p.lead(
            None,
            &s.human,
            &format!("would be freed · {n} · dry run, nothing removed"),
            None,
            "",
        );
    } else {
        p.lead(
            Some((Tone::Green, "✓")),
            &s.human,
            &format!("freed · {n} removed"),
            None,
            "",
        );
    }
    p.blank();
    let rows: Vec<Row> = e
        .records
        .iter()
        .map(|r| {
            let name = match &r.root {
                Some(root) => Name::Path {
                    path: root.clone(),
                    dir: true,
                },
                None => Name::Path {
                    path: r.file.clone(),
                    dir: false,
                },
            };
            let mut row = Row::new(r.file_bytes, None, name);
            row.mark = Some(match (&r.error, r.removed) {
                (Some(_), _) => (Tone::Red, "✗"),
                (None, true) => (Tone::Green, "✓"),
                (None, false) => (Tone::Faint, "·"),
            });
            let mut why = forget_reason(r.reason).to_string();
            if let Some(by) = &r.shadowed_by {
                why = format!("unused, covered by {}", st.tilde(by));
            }
            row.tags.push((Tone::Faint, why));
            if let Some(err) = &r.error {
                row.tags.push((Tone::Red, err.clone()));
            }
            row
        })
        .collect();
    p.table("SIZE", "SNAPSHOT", &rows);
    let left = format!(
        "{} left in {}",
        plural(s.remaining as u64, "snapshot", "snapshots"),
        st.tilde(&s.data_dir)
    );
    if s.dry_run {
        p.footer(
            None,
            Some(&format!(
                "{left}. Run the same command without --dry-run to remove them."
            )),
            &[("dredge snapshots".into(), "what dredge keeps")],
        );
    } else {
        p.footer(
            None,
            Some(&left),
            &[("dredge snapshots".into(), "what dredge keeps")],
        );
    }
    p.done()
}

/// Splits `text` into lines of at most `max` cells at spaces.
fn wrap(text: &str, max: usize) -> Vec<String> {
    let max = max.max(12);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let need = if cur.is_empty() {
            width(word)
        } else {
            width(&cur) + 1 + width(word)
        };
        if need > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&cut_end(word, max));
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The dredge command that fixes a doctor check, when there is one.
fn doctor_next(check: &str) -> Option<(String, &'static str)> {
    let id = check.split(':').next().unwrap_or(check);
    match id {
        "prunable" | "temp_files" | "snapshot_storage" => Some((
            "dredge forget --prune --dry-run".into(),
            "see what would be removed",
        )),
        "snapshot_header" | "snapshot_format" | "snapshot_count" => {
            Some(("dredge snapshots".into(), "what dredge keeps"))
        }
        "focus_scan" => Some((
            format!(
                "dredge scan {}",
                check.split_once(':').map_or("", |(_, p)| p)
            ),
            "scan it",
        )),
        _ => None,
    }
}

fn doctor(e: &DoctorOut, st: &Style) -> Vec<Line> {
    let t = st.tier();
    let mut p = Page::new(st);
    let ok = e.records.iter().filter(|r| r.status == "ok").count() as u64;
    let other = e.records.len() as u64 - ok;
    let tail = if other == 0 {
        " · all checks pass".to_string()
    } else {
        format!(" · {} to look at", count(other))
    };
    p.lead(None, &format!("{} ok", count(ok)), "", None, &tail);
    p.blank();

    // Passing checks that repeat per item (`snapshot_header:<file>`) fold into one row.
    let mut rows: Vec<(&str, String, String, Option<&String>)> = Vec::new();
    for r in &e.records {
        let (id, item) = r.check.split_once(':').unwrap_or((&r.check, ""));
        if r.status == "ok" && !item.is_empty() {
            if let Some(row) = rows.iter_mut().find(|(s, c, _, _)| *s == "ok" && c == id) {
                let n = row
                    .2
                    .split(' ')
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(1)
                    + 1;
                row.2 = format!("{n} pass");
                continue;
            }
            rows.push((r.status, id.to_string(), "1 pass".into(), None));
            continue;
        }
        let msg = if item.is_empty() {
            r.message.clone()
        } else {
            format!("{}: {}", st.tilde(item), r.message)
        };
        rows.push((r.status, id.to_string(), msg, r.hint.as_ref()));
    }
    for row in rows.iter_mut() {
        if row.0 == "ok" && row.2 == "1 pass" {
            // A single passing per-item check keeps its own message.
            if let Some(r) = e
                .records
                .iter()
                .find(|r| r.check.starts_with(&format!("{}:", row.1)))
            {
                row.2 = r.message.clone();
            }
        }
    }
    let cw = rows.iter().map(|r| width(&r.1)).max().unwrap_or(0).min(18);
    let pad = INDENT.len() + 2 + cw + 2;
    let room = st.width.saturating_sub(pad);
    for (status, id, msg, hint) in &rows {
        let (tone, g) = match *status {
            "ok" => (Tone::Green, "✓"),
            "warn" => (Tone::Amber, "!"),
            _ => (Tone::Red, "✗"),
        };
        let mut b = p.buf();
        b.put(t, Tone::Plain, INDENT)
            .put(t, tone, g)
            .put(t, Tone::Plain, " ");
        b.put(t, Tone::Plain, &format!("{}  ", rpad(&cut_end(id, cw), cw)));
        let msg_tone = if *status == "ok" {
            Tone::Muted
        } else {
            Tone::Plain
        };
        let lines = if *status == "ok" {
            vec![cut_end(msg, room)]
        } else {
            wrap(msg, room)
        };
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                p.push(std::mem::take(&mut b));
                b.put(t, Tone::Plain, &" ".repeat(pad));
            }
            b.put(t, msg_tone, line);
        }
        p.push(b);
        if let Some(h) = hint.filter(|_| *status != "ok") {
            for (i, line) in wrap(h, room.saturating_sub(4)).into_iter().enumerate() {
                let mut b = p.buf();
                let lead = if i == 0 { "fix " } else { "    " };
                b.put(t, Tone::Faint, &format!("{}{lead}", " ".repeat(pad)));
                b.put(t, Tone::Muted, &line);
                p.push(b);
            }
        }
    }
    let next = e
        .records
        .iter()
        .filter(|r| r.status != "ok")
        .find_map(|r| doctor_next(&r.check))
        .unwrap_or_else(|| ("dredge volumes".into(), "free space on each volume"));
    let note = format!("data folder {}", st.tilde(&e.summary.data_dir));
    p.footer(None, Some(&note), &[next]);
    p.done()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(width: usize) -> Style {
        Style {
            caps: Caps::PLAIN,
            width,
            home: Some("/Users/me".into()),
        }
    }

    #[test]
    fn paths_keep_the_leaf_and_cut_the_middle() {
        let s = st(80);
        assert_eq!(
            s.split("/Users/me/code/app", true),
            ("~/code/".into(), "app/".into())
        );
        assert_eq!(s.split("/Users/me", true), (String::new(), "~/".into()));
        assert_eq!(s.split("/", true), (String::new(), "/".into()));
        let (p, l) = s.fit("/Users/me/a/very/long/chain/of/folders/leaf.bin", false, 24);
        assert_eq!(l, "leaf.bin");
        assert!(p.starts_with("~/…/"), "{p}");
        assert!(width(&p) + width(&l) <= 24);
        let (p, l) = s.fit(
            "/x/an-extremely-long-file-name-that-does-not-fit.txt",
            false,
            20,
        );
        assert!(p.is_empty());
        assert_eq!(width(&l), 20);
    }

    #[test]
    fn shell_args_are_safe_to_paste() {
        let s = st(80);
        assert_eq!(s.arg("/Users/me/code"), "~/code");
        assert_eq!(s.arg("/Users/me/My Files"), "'/Users/me/My Files'");
        assert_eq!(s.arg("/tmp/it's"), r"'/tmp/it'\''s'");
    }

    #[test]
    fn helpers() {
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(count(12), "12");
        assert_eq!(age(30), "under a minute");
        assert_eq!(age(7200), "2 h");
        assert_eq!(age(3 * 86_400), "3 days");
        assert_eq!(cut_middle("abcdefghij", 5), "ab…ij");
        assert_eq!(cut_end("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn no_escapes_without_color() {
        let mut b = Buf::default();
        b.put(ColorTier::None, Tone::Bold, "x\u{1b}[31m");
        assert!(!b.s.contains('\x1b'));
        let mut b = Buf::default();
        b.put(ColorTier::TrueColor, Tone::Bold, "x");
        assert_eq!(b.s, "\x1b[1mx\x1b[0m");
        assert_eq!(b.w, 1);
    }
}
