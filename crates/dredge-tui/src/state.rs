//! The pure state machine: `update(state, event) -> (state, effects)`. No terminal, no I/O, no
//! engine. Every number shown comes from data the runner fetched from `Engine` and delivered as
//! an [`Event`]; effects are requests the runner carries out.

use std::path::{Path, PathBuf};

use dredge_render::Ink;

/// A key, already decoded from the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Backspace,
    Esc,
    CtrlC,
    Char(char),
}

/// One category row, mapped from `Engine::categories`.
#[derive(Clone, Debug, PartialEq)]
pub struct Cat {
    pub ink: Ink,
    pub bytes: u64,
    pub human: String,
    pub share: f64,
    pub files: u64,
    pub top: Vec<String>,
}

/// Everything the category view shows, from `Engine::categories` and `Engine::run(Volumes)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Overview {
    pub path: String,
    pub used_bytes: u64,
    pub used_human: String,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub age_secs: u64,
    /// Non-empty categories only, largest first.
    pub cats: Vec<Cat>,
}

/// One child row, mapped from `Engine` browse output.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub bytes: u64,
    pub human: String,
    pub pct_of_parent: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub path: String,
    pub human: String,
    pub entries: Vec<Entry>,
    /// Entries not shown because of the row limit.
    pub remaining: usize,
}

/// One cleanup hint, mapped from `Engine::insights`.
#[derive(Clone, Debug, PartialEq)]
pub struct Insight {
    pub human: String,
    pub message: String,
    pub path: String,
    pub suggested_command: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scan {
    Idle,
    Running(Progress),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Overview,
    /// The focused category's top paths.
    Paths,
    Browse,
    Insights,
}

/// Something that happened: input, a terminal change, or a result the runner fetched.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Key(Key),
    Resize(u16, u16),
    /// `Ok(None)`: there is no snapshot for the root yet.
    Overview(Result<Option<Overview>, String>),
    Browsed(Result<Listing, String>),
    Insights(Result<Vec<Insight>, String>),
    Progress(Progress),
    ScanDone(Result<(), String>),
}

/// A request for the runner. The state machine never performs I/O itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Browse(String),
    Insights,
    Scan,
    /// Text for the clipboard (OSC 52). It is only ever copied, never run.
    CopyText(String),
    Quit,
}

/// One level of the browse stack. `sel` and `offset` index the rows on screen: `../` (when
/// shown) and then the visible (filtered) entries.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub listing: Listing,
    pub sel: usize,
    pub offset: usize,
    /// Case-insensitive substring filter on entry names; empty shows everything.
    pub filter: String,
    /// The folder has a parent Browse can go to, shown as a `../` row above the entries.
    pub up: bool,
}

impl Level {
    fn new(listing: Listing, up: bool) -> Level {
        let mut l = Level {
            listing,
            sel: 0,
            offset: 0,
            filter: String::new(),
            up,
        };
        l.sel = l.first_entry();
        l
    }

    /// The `../` row is showing: the folder has a parent and no filter is narrowing it.
    pub fn up_row(&self) -> bool {
        self.up && self.filter.is_empty()
    }

    /// Rows the cursor moves over: `../` (when shown) and then the visible entries.
    pub fn row_count(&self) -> usize {
        usize::from(self.up_row()) + self.visible().len()
    }

    /// The cursor is on `../`.
    pub fn on_up(&self) -> bool {
        self.up_row() && self.sel == 0
    }

    /// Row of the first entry, or `../` in an empty folder.
    fn first_entry(&self) -> usize {
        usize::from(self.up_row() && !self.visible().is_empty())
    }

    /// Indices into `listing.entries` of the rows the filter lets through, in order.
    pub fn visible(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.listing.entries.len()).collect();
        }
        let needle = self.filter.to_lowercase();
        self.listing
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }

    /// The entry under the cursor; `None` on `../`.
    pub fn selected(&self) -> Option<&Entry> {
        let i = self.sel.checked_sub(usize::from(self.up_row()))?;
        self.visible()
            .get(i)
            .and_then(|&i| self.listing.entries.get(i))
    }
}

/// What the next browse result is for. The runner only loads paths; the state machine
/// remembers why it asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    /// Opened a child: push it.
    Push,
    /// Went above the bottom of the stack (or jumped to an unloaded ancestor): the result
    /// becomes the whole stack, with the row on the way to `select` selected.
    Replace { select: Option<String> },
}

/// A one-line prompt that takes over the keyboard in Browse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// `/`: typing edits the current level's filter.
    Filter,
    /// `g`: the next key picks a breadcrumb level.
    Jump,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub root: String,
    pub size: (u16, u16),
    /// `None` until loaded, or when there is no snapshot.
    pub overview: Option<Overview>,
    pub loaded: bool,
    pub focus: usize,
    pub view: View,
    pub paths_sel: usize,
    pub stack: Vec<Level>,
    /// View that leaving Browse returns to.
    pub browse_from: View,
    pub pending: Pending,
    pub input: Option<Input>,
    pub insights: Option<Vec<Insight>>,
    pub insight_sel: usize,
    pub insight_offset: usize,
    /// View to return to when insights are toggled off.
    pub before_insights: View,
    pub scan: Scan,
    pub status: Option<Status>,
    pub help: bool,
    pub quit: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub error: bool,
}

pub const MIN_WIDTH: u16 = 20;
pub const MIN_HEIGHT: u16 = 6;
/// Lines each insight takes in the list: message, path, command.
pub const INSIGHT_LINES: usize = 3;
/// Browse row limit passed to the engine (its maximum).
pub const BROWSE_LIMIT: usize = 1000;

impl State {
    pub fn new(root: impl Into<String>, size: (u16, u16)) -> State {
        State {
            root: root.into(),
            size,
            overview: None,
            loaded: false,
            focus: 0,
            view: View::Overview,
            paths_sel: 0,
            stack: Vec::new(),
            browse_from: View::Overview,
            pending: Pending::Push,
            input: None,
            insights: None,
            insight_sel: 0,
            insight_offset: 0,
            before_insights: View::Overview,
            scan: Scan::Idle,
            status: None,
            help: false,
            quit: false,
        }
    }

    pub fn too_small(&self) -> bool {
        self.size.0 < MIN_WIDTH || self.size.1 < MIN_HEIGHT
    }

    pub fn cats(&self) -> &[Cat] {
        self.overview.as_ref().map_or(&[], |o| &o.cats)
    }

    pub fn focused(&self) -> Option<&Cat> {
        self.cats().get(self.focus)
    }

    pub fn scanning(&self) -> bool {
        matches!(self.scan, Scan::Running(_))
    }

    /// Rows available to a plain list (header, sub-title and status line are fixed).
    pub fn list_rows(&self) -> usize {
        usize::from(self.size.1).saturating_sub(3).max(1)
    }

    /// Rows for browse entries: a plain list minus the column header.
    pub fn browse_rows(&self) -> usize {
        self.list_rows().saturating_sub(1).max(1)
    }

    /// The folder Browse is showing.
    pub fn current(&self) -> Option<&Level> {
        self.stack.last()
    }

    fn insight_slots(&self) -> usize {
        (self.list_rows() / INSIGHT_LINES).max(1)
    }

    fn err(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
        });
    }

    fn note(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
        });
    }
}

/// How many legend columns fit in `width` for these categories (1 to 3). The view and the
/// state machine share this so Up and Down move by a whole legend row.
pub fn legend_cols(cats: &[Cat], width: u16) -> usize {
    let lw = cats
        .iter()
        .map(|c| c.ink.label().chars().count())
        .max()
        .unwrap_or(0);
    let hw = cats
        .iter()
        .map(|c| c.human.chars().count())
        .max()
        .unwrap_or(0);
    // marker, swatch, space, label, space, size, then three spaces of gutter
    let colw = 3 + lw + 1 + hw + 3;
    (usize::from(width) / colw.max(1))
        .clamp(1, 3)
        .min(cats.len().max(1))
}

/// Keep `sel` inside the window of `slots` items starting at `offset`.
pub fn adjust_offset(sel: usize, offset: usize, slots: usize) -> usize {
    let slots = slots.max(1);
    if sel < offset {
        sel
    } else if sel >= offset + slots {
        sel + 1 - slots
    } else {
        offset
    }
}

fn step(sel: usize, len: usize, key: Key, page: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let last = len - 1;
    match key {
        Key::Up | Key::Char('k') => sel.saturating_sub(1),
        Key::Down | Key::Char('j') => (sel + 1).min(last),
        Key::PageUp => sel.saturating_sub(page),
        Key::PageDown => (sel + page).min(last),
        Key::Home => 0,
        Key::End => last,
        _ => sel,
    }
}

fn is_nav(key: Key) -> bool {
    matches!(
        key,
        Key::Up
            | Key::Down
            | Key::PageUp
            | Key::PageDown
            | Key::Home
            | Key::End
            | Key::Char('j')
            | Key::Char('k')
    )
}

fn fmt_count(n: u64) -> String {
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

/// Human text for a running scan, used by the header.
pub fn progress_text(p: &Progress) -> String {
    format!(
        "{} files, {} dirs, {}",
        fmt_count(p.files),
        fmt_count(p.dirs),
        dredge_core::util::human_bytes(p.bytes)
    )
}

pub fn update(mut s: State, event: Event) -> (State, Vec<Effect>) {
    let mut fx = Vec::new();
    match event {
        Event::Resize(w, h) => {
            s.size = (w, h);
            readjust(&mut s);
        }
        Event::Overview(Ok(o)) => {
            s.loaded = true;
            s.overview = o;
            s.focus = s.focus.min(s.cats().len().saturating_sub(1));
            if s.overview.is_some() && s.status.as_ref().is_some_and(|st| st.error) {
                s.status = None;
            }
        }
        Event::Overview(Err(e)) => {
            s.loaded = true;
            s.err(e);
        }
        Event::Browsed(Ok(listing)) => {
            let under_root = {
                let p = Path::new(&listing.path);
                p != Path::new(&s.root) && p.starts_with(&s.root)
            };
            match std::mem::replace(&mut s.pending, Pending::Push) {
                Pending::Push => {
                    let up = under_root || !s.stack.is_empty();
                    s.stack.push(Level::new(listing, up));
                }
                Pending::Replace { select } => {
                    let mut l = Level::new(listing, under_root);
                    if let Some(target) = select {
                        // The row we came from, or the one on the way to it.
                        if let Some(i) = l
                            .listing
                            .entries
                            .iter()
                            .position(|e| Path::new(&target).starts_with(&e.path))
                        {
                            l.sel = i + usize::from(l.up_row());
                        }
                    }
                    l.offset = adjust_offset(l.sel, 0, s.browse_rows());
                    s.stack = vec![l];
                }
            }
            s.view = View::Browse;
            s.input = None;
            s.status = None;
        }
        Event::Browsed(Err(e)) => {
            s.pending = Pending::Push;
            s.err(e);
        }
        Event::Insights(Ok(list)) => {
            s.insights = Some(list);
            s.insight_sel = 0;
            s.insight_offset = 0;
        }
        Event::Insights(Err(e)) => {
            s.insights = Some(Vec::new());
            s.err(e);
        }
        Event::Progress(p) => {
            if s.scan != Scan::Running(p) {
                s.scan = Scan::Running(p);
            }
        }
        Event::ScanDone(r) => {
            s.scan = Scan::Idle;
            match r {
                Ok(()) => {
                    // Everything derived from the old snapshot is stale.
                    s.stack.clear();
                    s.insights = None;
                    s.view = View::Overview;
                    s.before_insights = View::Overview;
                    s.note("scan complete");
                }
                Err(e) => s.err(format!("scan failed: {e}")),
            }
        }
        Event::Key(k) => key(&mut s, k, &mut fx),
    }
    (s, fx)
}

fn readjust(s: &mut State) {
    let rows = s.browse_rows();
    let slots = s.insight_slots();
    for l in &mut s.stack {
        l.offset = adjust_offset(l.sel, l.offset, rows);
    }
    s.insight_offset = adjust_offset(s.insight_sel, s.insight_offset, slots);
}

fn key(s: &mut State, k: Key, fx: &mut Vec<Effect>) {
    if k == Key::CtrlC {
        s.quit = true;
        fx.push(Effect::Quit);
        return;
    }
    // A prompt owns every other key, so `q` or `?` can be typed into a filter.
    if let (View::Browse, Some(input)) = (s.view, s.input) {
        if !s.help {
            input_key(s, input, k, fx);
            return;
        }
    }
    if k == Key::Char('q') {
        s.quit = true;
        fx.push(Effect::Quit);
        return;
    }
    if s.help {
        if matches!(k, Key::Char('?') | Key::Esc | Key::Enter | Key::Backspace) {
            s.help = false;
        }
        return;
    }
    match k {
        Key::Char('?') => {
            s.help = true;
            return;
        }
        Key::Char('r') => {
            if s.scanning() {
                s.note("a scan is already running");
            } else {
                s.scan = Scan::Running(Progress::default());
                s.status = None;
                fx.push(Effect::Scan);
            }
            return;
        }
        Key::Char('i') => {
            toggle_insights(s, fx);
            return;
        }
        Key::Char('y') => {
            copy(s, fx);
            return;
        }
        _ => {}
    }
    match s.view {
        View::Overview => overview_key(s, k, fx),
        View::Paths => paths_key(s, k, fx),
        View::Browse => browse_key(s, k, fx),
        View::Insights => insights_key(s, k, fx),
    }
}

// ---------------------------------------------------------------------------------------------
// Paths and the browse stack

/// The breadcrumb for `path`: the scan root, then one entry per folder below it. A path
/// outside the root is a single crumb.
pub fn crumbs(root: &str, path: &str) -> Vec<String> {
    match Path::new(path).strip_prefix(root) {
        Ok(rest) => {
            let mut out = vec![root.to_string()];
            let mut cur = PathBuf::from(root);
            for c in rest.components() {
                cur.push(c);
                out.push(cur.to_string_lossy().into_owned());
            }
            out
        }
        Err(_) => vec![path.to_string()],
    }
}

fn open(s: &mut State, path: String, pending: Pending, fx: &mut Vec<Effect>) {
    s.pending = pending;
    s.input = None;
    fx.push(Effect::Browse(path));
}

/// Leave Browse for the view it was entered from.
fn leave(s: &mut State) {
    s.stack.clear();
    s.input = None;
    s.view = s.browse_from;
    if s.view == View::Insights && s.before_insights == View::Browse {
        s.before_insights = View::Overview;
    }
}

/// One level up. Above the folder Browse was entered at, load its parent and select the
/// folder we left, until the scan root; above the root, leave Browse.
fn go_up(s: &mut State, fx: &mut Vec<Effect>) {
    s.status = None;
    if s.stack.len() > 1 {
        s.stack.pop();
        return;
    }
    let Some(cur) = s.current().map(|l| l.listing.path.clone()) else {
        leave(s);
        return;
    };
    let p = Path::new(&cur);
    if p != Path::new(&s.root) && p.starts_with(&s.root) {
        if let Some(parent) = p.parent() {
            let parent = parent.to_string_lossy().into_owned();
            open(s, parent, Pending::Replace { select: Some(cur) }, fx);
            return;
        }
    }
    leave(s);
}

/// Go to breadcrumb level `idx` (0 is the scan root). Levels still on the stack are
/// restored as they were left; levels above the stack are loaded.
fn jump(s: &mut State, idx: usize, fx: &mut Vec<Effect>) {
    let Some(cur) = s.current().map(|l| l.listing.path.clone()) else {
        return;
    };
    let cs = crumbs(&s.root, &cur);
    let Some(target) = cs.get(idx).cloned() else {
        s.note(format!(
            "no level {}: the breadcrumb has {}",
            idx + 1,
            cs.len()
        ));
        return;
    };
    if Path::new(&target) == Path::new(&cur) {
        return;
    }
    s.status = None;
    if let Some(pos) = s
        .stack
        .iter()
        .position(|l| Path::new(&l.listing.path) == Path::new(&target))
    {
        s.stack.truncate(pos + 1);
    } else {
        open(s, target, Pending::Replace { select: Some(cur) }, fx);
    }
}

fn reset_cursor(s: &mut State) {
    if let Some(l) = s.stack.last_mut() {
        l.sel = l.first_entry();
        l.offset = 0;
    }
}

fn move_cursor(s: &mut State, k: Key) {
    let rows = s.browse_rows();
    if let Some(l) = s.stack.last_mut() {
        l.sel = step(l.sel, l.row_count(), k, rows);
        l.offset = adjust_offset(l.sel, l.offset, rows);
    }
}

fn input_key(s: &mut State, input: Input, k: Key, fx: &mut Vec<Effect>) {
    match input {
        Input::Filter => match k {
            Key::Enter => s.input = None,
            Key::Esc => {
                if let Some(l) = s.stack.last_mut() {
                    l.filter.clear();
                }
                reset_cursor(s);
                s.input = None;
            }
            Key::Backspace => {
                if let Some(l) = s.stack.last_mut() {
                    l.filter.pop();
                }
                reset_cursor(s);
            }
            Key::Char(c) if !c.is_control() => {
                if let Some(l) = s.stack.last_mut() {
                    l.filter.push(c);
                }
                reset_cursor(s);
            }
            Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Home | Key::End => {
                move_cursor(s, k);
            }
            _ => {}
        },
        Input::Jump => {
            s.input = None;
            match k {
                Key::Char(d @ '1'..='9') => jump(s, d as usize - '1' as usize, fx),
                Key::Char('~') => jump(s, 0, fx),
                _ => {}
            }
        }
    }
}

fn toggle_insights(s: &mut State, fx: &mut Vec<Effect>) {
    if s.view == View::Insights {
        s.view = s.before_insights;
        return;
    }
    if s.overview.is_none() {
        s.note("no snapshot yet: press r to scan");
        return;
    }
    s.before_insights = s.view;
    s.view = View::Insights;
    if s.insights.is_none() {
        fx.push(Effect::Insights);
    }
}

/// `y`: the selected path in Paths and Browse, the selected command in Insights.
fn copy(s: &mut State, fx: &mut Vec<Effect>) {
    let path = match s.view {
        View::Insights => return copy_command(s, fx),
        View::Overview => {
            s.note("y copies a path in a list, or a command in insights (i)");
            return;
        }
        View::Paths => s.focused().and_then(|c| c.top.get(s.paths_sel)).cloned(),
        View::Browse => s
            .current()
            .and_then(Level::selected)
            .map(|e| e.path.clone()),
    };
    match path {
        Some(p) if p.chars().any(dredge_render::is_unsafe_char) => s.note(format!(
            "not copied: path contains control characters: {}",
            dredge_render::sanitize(&p)
        )),
        Some(p) => {
            s.note(format!("copied path: {p}"));
            fx.push(Effect::CopyText(p));
        }
        None => s.note("nothing selected"),
    }
}

fn copy_command(s: &mut State, fx: &mut Vec<Effect>) {
    match s.insights.as_ref().and_then(|l| l.get(s.insight_sel)) {
        Some(i) => {
            let cmd = i.suggested_command.clone();
            if cmd.chars().any(dredge_render::is_unsafe_char) {
                s.note(format!(
                    "not copied: command contains control characters: {}",
                    dredge_render::sanitize(&cmd)
                ));
                return;
            }
            s.note(format!("copied (review before running): {cmd}"));
            fx.push(Effect::CopyText(cmd));
        }
        None => s.note("no insight selected"),
    }
}

fn overview_key(s: &mut State, k: Key, fx: &mut Vec<Effect>) {
    if k == Key::Char('b') {
        if s.overview.is_none() {
            s.note("no snapshot yet: press r to scan");
            return;
        }
        s.browse_from = View::Overview;
        s.stack.clear();
        let root = s.root.clone();
        open(s, root, Pending::Push, fx);
        return;
    }
    let n = s.cats().len();
    if n == 0 {
        return;
    }
    let cols = legend_cols(s.cats(), s.size.0);
    match k {
        Key::Left | Key::Char('h') => s.focus = (s.focus + n - 1) % n,
        Key::Right | Key::Char('l') => s.focus = (s.focus + 1) % n,
        Key::Up | Key::Char('k') => s.focus = s.focus.saturating_sub(cols),
        Key::Down | Key::Char('j') => s.focus = (s.focus + cols).min(n - 1),
        Key::Home => s.focus = 0,
        Key::End => s.focus = n - 1,
        Key::Enter => {
            if s.focused().is_some_and(|c| !c.top.is_empty()) {
                s.paths_sel = 0;
                s.view = View::Paths;
            } else {
                s.note("nothing to drill into for this category");
            }
        }
        _ => {}
    }
}

fn paths_key(s: &mut State, k: Key, fx: &mut Vec<Effect>) {
    let len = s.focused().map_or(0, |c| c.top.len());
    match k {
        Key::Enter | Key::Right | Key::Char('l') => {
            if let Some(p) = s.focused().and_then(|c| c.top.get(s.paths_sel)).cloned() {
                s.browse_from = View::Paths;
                s.stack.clear();
                open(s, p, Pending::Push, fx);
            }
        }
        Key::Backspace | Key::Esc | Key::Left | Key::Char('h') => s.view = View::Overview,
        k if is_nav(k) => s.paths_sel = step(s.paths_sel, len, k, 1),
        _ => {}
    }
}

fn browse_key(s: &mut State, k: Key, fx: &mut Vec<Effect>) {
    match k {
        Key::Enter | Key::Right | Key::Char('l') => {
            if s.current().is_some_and(Level::on_up) {
                return go_up(s, fx);
            }
            let Some(e) = s.current().and_then(Level::selected).cloned() else {
                return;
            };
            if e.is_dir {
                open(s, e.path, Pending::Push, fx);
            } else {
                s.note("a file, so there is nothing to open: y copies its path");
            }
        }
        Key::Backspace | Key::Left | Key::Char('h') => go_up(s, fx),
        Key::Esc => {
            if s.current().is_some_and(|l| !l.filter.is_empty()) {
                if let Some(l) = s.stack.last_mut() {
                    l.filter.clear();
                }
                reset_cursor(s);
            } else {
                leave(s);
            }
        }
        Key::Char('/') => {
            s.input = Some(Input::Filter);
            s.status = None;
        }
        Key::Char('g') => {
            s.input = Some(Input::Jump);
            s.status = None;
        }
        Key::Char('~') => jump(s, 0, fx),
        k if is_nav(k) => move_cursor(s, k),
        _ => {}
    }
}

fn insights_key(s: &mut State, k: Key, fx: &mut Vec<Effect>) {
    let len = s.insights.as_ref().map_or(0, Vec::len);
    let slots = s.insight_slots();
    match k {
        // Show the insight's path inside its folder, so ← keeps going up from there.
        Key::Enter | Key::Right | Key::Char('l') => {
            let Some(path) = s
                .insights
                .as_ref()
                .and_then(|l| l.get(s.insight_sel))
                .map(|i| i.path.clone())
            else {
                return;
            };
            s.browse_from = View::Insights;
            s.stack.clear();
            let parent = Path::new(&path)
                .parent()
                .filter(|p| p.starts_with(&s.root))
                .map(|p| p.to_string_lossy().into_owned());
            match parent {
                Some(parent) => open(s, parent, Pending::Replace { select: Some(path) }, fx),
                None => open(s, path, Pending::Push, fx),
            }
        }
        Key::Backspace | Key::Esc | Key::Left | Key::Char('h') => s.view = s.before_insights,
        k if is_nav(k) => {
            s.insight_sel = step(s.insight_sel, len, k, slots);
            s.insight_offset = adjust_offset(s.insight_sel, s.insight_offset, slots);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(ink: Ink, bytes: u64, top: &[&str]) -> Cat {
        Cat {
            ink,
            bytes,
            human: format!("{bytes} B"),
            share: 10.0,
            files: 3,
            top: top.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub(crate) fn overview() -> Overview {
        Overview {
            path: "/data".into(),
            used_bytes: 600,
            used_human: "600 B".into(),
            total_bytes: Some(1000),
            free_bytes: Some(400),
            age_secs: 90,
            cats: vec![
                cat(
                    Ink::Developer,
                    300,
                    &["/data/proj/node_modules", "/data/proj/target"],
                ),
                cat(Ink::Photos, 200, &["/data/pics"]),
                cat(Ink::Caches, 100, &[]),
            ],
        }
    }

    fn entry(name: &str, dir: bool) -> Entry {
        Entry {
            name: name.into(),
            path: format!("/data/proj/node_modules/{name}"),
            is_dir: dir,
            bytes: 10,
            human: "10 B".into(),
            pct_of_parent: Some(5.0),
        }
    }

    fn listing(path: &str, n: usize) -> Listing {
        Listing {
            path: path.into(),
            human: "1 KiB".into(),
            entries: (0..n)
                .map(|i| entry(&format!("e{i}"), i % 2 == 0))
                .collect(),
            remaining: 0,
        }
    }

    fn insights() -> Vec<Insight> {
        (0..3)
            .map(|i| Insight {
                human: "1 GiB".into(),
                message: format!("m{i}"),
                path: format!("/p{i}"),
                suggested_command: format!("rm -rf /p{i}"),
            })
            .collect()
    }

    fn loaded() -> State {
        let (s, _) = update(
            State::new("/data", (80, 24)),
            Event::Overview(Ok(Some(overview()))),
        );
        s
    }

    fn press(s: State, k: Key) -> (State, Vec<Effect>) {
        update(s, Event::Key(k))
    }

    fn keys(mut s: State, ks: &[Key]) -> State {
        for &k in ks {
            s = press(s, k).0;
        }
        s
    }

    #[test]
    fn overview_focus_wraps_left_right_and_clamps_up_down() {
        let s = loaded();
        assert_eq!(s.focus, 0);
        let s = press(s, Key::Left).0;
        assert_eq!(s.focus, 2, "left wraps to the last category");
        let s = press(s, Key::Right).0;
        assert_eq!(s.focus, 0, "right wraps to the first");
        let s = press(s, Key::Char('l')).0;
        assert_eq!(s.focus, 1);
        let s = press(s, Key::Char('h')).0;
        assert_eq!(s.focus, 0);
        // 80 columns fit 3 legend columns for three short labels: down clamps at the end.
        let s = press(s, Key::Down).0;
        assert_eq!(s.focus, 2);
        let s = press(s, Key::Char('j')).0;
        assert_eq!(s.focus, 2);
        let s = press(s, Key::Up).0;
        assert_eq!(s.focus, 0);
        let s = press(s, Key::Char('k')).0;
        assert_eq!(s.focus, 0);
        assert_eq!(press(s, Key::End).0.focus, 2);
    }

    #[test]
    fn narrow_legend_moves_one_row_per_down() {
        let (s, _) = update(loaded(), Event::Resize(24, 10));
        assert_eq!(legend_cols(s.cats(), s.size.0), 1);
        assert_eq!(press(s, Key::Down).0.focus, 1);
    }

    #[test]
    fn quit_keys() {
        for k in [Key::Char('q'), Key::CtrlC] {
            let (s, fx) = press(loaded(), k);
            assert!(s.quit);
            assert_eq!(fx, vec![Effect::Quit]);
        }
        // Quit works in every view and under help.
        for pre in [vec![Key::Char('i')], vec![Key::Enter], vec![Key::Char('?')]] {
            let (_, fx) = press(keys(loaded(), &pre), Key::Char('q'));
            assert_eq!(fx, vec![Effect::Quit]);
        }
    }

    #[test]
    fn help_toggles_and_swallows_other_keys() {
        let s = press(loaded(), Key::Char('?')).0;
        assert!(s.help);
        let (s, fx) = press(s, Key::Right);
        assert!(s.help && s.focus == 0 && fx.is_empty());
        let (s, fx) = press(s, Key::Char('r'));
        assert!(s.help && fx.is_empty() && !s.scanning());
        assert!(!press(s.clone(), Key::Char('?')).0.help);
        assert!(!press(s, Key::Esc).0.help);
    }

    #[test]
    fn enter_opens_paths_then_browse_and_backs_out_level_by_level() {
        let s = loaded();
        let (s, fx) = press(s, Key::Enter);
        assert_eq!(s.view, View::Paths);
        assert!(fx.is_empty());
        let (s, fx) = press(s, Key::Down);
        assert_eq!(s.paths_sel, 1);
        assert!(fx.is_empty());
        let (s, fx) = press(s, Key::Enter);
        assert_eq!(fx, vec![Effect::Browse("/data/proj/target".into())]);
        let (s, _) = update(s, Event::Browsed(Ok(listing("/data/proj/target", 4))));
        assert_eq!((s.view, s.stack.len()), (View::Browse, 1));
        // entries alternate dir, file, dir, file: Enter on a dir drills
        let (s, fx) = press(s, Key::Enter);
        assert_eq!(
            fx,
            vec![Effect::Browse("/data/proj/node_modules/e0".into())]
        );
        let (s, _) = update(
            s,
            Event::Browsed(Ok(listing("/data/proj/node_modules/e0", 2))),
        );
        assert_eq!(s.stack.len(), 2);
        // a file does not drill
        let s = press(s, Key::Down).0;
        let (s, fx) = press(s, Key::Enter);
        assert!(fx.is_empty());
        assert!(s.status.as_ref().unwrap().text.contains("a file"));
        // back out: one browse level, then Esc leaves Browse for Paths, then Overview
        let s = press(s, Key::Backspace).0;
        assert_eq!((s.view, s.stack.len()), (View::Browse, 1));
        let s = press(s, Key::Esc).0;
        assert_eq!((s.view, s.stack.len()), (View::Paths, 0));
        let s = press(s, Key::Backspace).0;
        assert_eq!(s.view, View::Overview);
        // Esc on the overview does nothing
        let s = press(s, Key::Esc).0;
        assert_eq!(s.view, View::Overview);
    }

    #[test]
    fn enter_on_category_without_paths_stays_put() {
        let s = keys(loaded(), &[Key::End]);
        let (s, fx) = press(s, Key::Enter);
        assert_eq!(s.view, View::Overview);
        assert!(fx.is_empty());
        assert!(s.status.is_some());
    }

    #[test]
    fn browse_list_selection_clamps_and_scrolls() {
        let (s, _) = update(loaded(), Event::Browsed(Ok(listing("/x", 100))));
        let rows = s.browse_rows();
        assert_eq!(rows, 20);
        let s = press(s, Key::Up).0;
        assert_eq!(s.stack[0].sel, 0);
        let s = keys(s, &[Key::Down; 30]);
        let l = &s.stack[0];
        assert_eq!(l.sel, 30);
        assert!(l.offset <= l.sel && l.sel < l.offset + rows);
        assert_eq!(l.offset, 30 + 1 - rows);
        let s = press(s, Key::End).0;
        assert_eq!(s.stack[0].sel, 99);
        assert_eq!(s.stack[0].offset, 99 + 1 - rows);
        let s = keys(s, &[Key::Down, Key::Char('j')]);
        assert_eq!(s.stack[0].sel, 99, "clamped at the end");
        let s = press(s, Key::PageUp).0;
        assert_eq!(s.stack[0].sel, 99 - rows);
        let s = press(s, Key::Home).0;
        assert_eq!((s.stack[0].sel, s.stack[0].offset), (0, 0));
    }

    #[test]
    fn resize_keeps_selection_visible() {
        let (s, _) = update(loaded(), Event::Browsed(Ok(listing("/x", 100))));
        let s = press(s, Key::End).0;
        let (s, _) = update(s, Event::Resize(80, 8));
        let l = &s.stack[0];
        assert!(l.sel >= l.offset && l.sel < l.offset + s.browse_rows());
    }

    #[test]
    fn insights_toggle_loads_once_and_returns() {
        let s = loaded();
        let (s, fx) = press(s, Key::Char('i'));
        assert_eq!((s.view, fx), (View::Insights, vec![Effect::Insights]));
        let (s, _) = update(s, Event::Insights(Ok(insights())));
        let (s, fx) = press(s, Key::Char('i'));
        assert_eq!((s.view, fx), (View::Overview, vec![]));
        let (s, fx) = press(s, Key::Char('i'));
        assert_eq!(
            (s.view, fx),
            (View::Insights, vec![]),
            "cached: no second fetch"
        );
        let s = press(s, Key::Esc).0;
        assert_eq!(s.view, View::Overview);
    }

    #[test]
    fn insights_return_to_the_view_they_came_from() {
        let s = keys(loaded(), &[Key::Enter]);
        assert_eq!(s.view, View::Paths);
        let s = keys(s, &[Key::Char('i'), Key::Char('i')]);
        assert_eq!(s.view, View::Paths);
    }

    #[test]
    fn y_refuses_commands_with_control_characters() {
        let mut list = insights();
        list[0].suggested_command = "rm -rf '/p\x1b]52;c;AAAA\x07'".into();
        let (s, _) = press(loaded(), Key::Char('i'));
        let (s, _) = update(s, Event::Insights(Ok(list)));
        let (s, fx) = press(s, Key::Char('y'));
        assert!(fx.is_empty(), "{fx:?}");
        let text = &s.status.as_ref().unwrap().text;
        assert!(text.contains("not copied"), "{text}");
        assert!(text.chars().all(|c| !c.is_control()), "{text:?}");
    }

    #[test]
    fn y_copies_only_the_selected_insights_command() {
        let s = loaded();
        let (s, _) = press(s, Key::Char('i'));
        let (s, _) = update(s, Event::Insights(Ok(insights())));
        let s = press(s, Key::Down).0;
        let (s, fx) = press(s, Key::Char('y'));
        assert_eq!(fx, vec![Effect::CopyText("rm -rf /p1".into())]);
        assert!(s.status.as_ref().unwrap().text.contains("rm -rf /p1"));
        // nothing ever executes: copying and browsing are the only effects here
        for k in [Key::Char('y'), Key::Enter, Key::Right, Key::Down, Key::Up] {
            let (_, fx) = press(s.clone(), k);
            assert!(
                fx.iter()
                    .all(|e| matches!(e, Effect::CopyText(_) | Effect::Browse(_))),
                "{k:?}"
            );
        }
    }

    #[test]
    fn y_outside_insights_or_with_none_copies_nothing() {
        let (s, fx) = press(loaded(), Key::Char('y'));
        assert!(fx.is_empty() && s.status.is_some());
        let s = press(loaded(), Key::Char('i')).0;
        let (s, _) = update(s, Event::Insights(Ok(vec![])));
        let (s, fx) = press(s, Key::Char('y'));
        assert!(fx.is_empty());
        assert_eq!(s.status.unwrap().text, "no insight selected");
    }

    #[test]
    fn insight_selection_scrolls_by_whole_items() {
        let many: Vec<Insight> = (0..20)
            .map(|i| Insight {
                human: "1 B".into(),
                message: format!("m{i}"),
                path: "/p".into(),
                suggested_command: "x".into(),
            })
            .collect();
        let s = press(loaded(), Key::Char('i')).0;
        let (s, _) = update(s, Event::Insights(Ok(many)));
        let slots = s.list_rows() / INSIGHT_LINES;
        let s = keys(s, &[Key::Down; 12]);
        assert_eq!(s.insight_sel, 12);
        assert_eq!(s.insight_offset, 12 + 1 - slots);
    }

    #[test]
    fn r_starts_a_scan_once() {
        let (s, fx) = press(loaded(), Key::Char('r'));
        assert_eq!(fx, vec![Effect::Scan]);
        assert!(s.scanning());
        let (s, fx) = press(s, Key::Char('r'));
        assert!(fx.is_empty());
        assert_eq!(s.status.as_ref().unwrap().text, "a scan is already running");
    }

    #[test]
    fn no_snapshot_offers_scan_and_ignores_navigation() {
        let (s, _) = update(State::new("/data", (80, 24)), Event::Overview(Ok(None)));
        assert!(s.loaded && s.overview.is_none());
        for k in [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::Enter,
            Key::Home,
            Key::End,
        ] {
            let (s2, fx) = press(s.clone(), k);
            assert!(fx.is_empty());
            assert_eq!(s2.view, View::Overview);
        }
        let (s, fx) = press(s, Key::Char('i'));
        assert_eq!(s.view, View::Overview);
        assert!(fx.is_empty());
        let (_, fx) = press(s, Key::Char('r'));
        assert_eq!(fx, vec![Effect::Scan]);
    }

    #[test]
    fn progress_and_scan_done() {
        let s = press(loaded(), Key::Char('r')).0;
        let p = Progress {
            files: 5,
            dirs: 1,
            bytes: 9,
        };
        let (s, _) = update(s, Event::Progress(p));
        assert_eq!(s.scan, Scan::Running(p));
        let s = keys(s, &[Key::Enter]);
        let (s, _) = update(s, Event::Browsed(Ok(listing("/x", 2))));
        let (s, _) = update(s, Event::ScanDone(Ok(())));
        assert_eq!(s.scan, Scan::Idle);
        assert!(s.stack.is_empty() && s.insights.is_none());
        assert_eq!(s.view, View::Overview);
        let (s, _) = update(s, Event::ScanDone(Err("boom".into())));
        let st = s.status.unwrap();
        assert!(st.error && st.text.contains("boom"));
    }

    #[test]
    fn engine_errors_become_status_not_crashes() {
        let (s, _) = update(loaded(), Event::Browsed(Err("gone".into())));
        assert_eq!(s.view, View::Overview);
        assert!(s.status.as_ref().unwrap().error);
        let (s, _) = update(s, Event::Overview(Err("bad".into())));
        assert!(s.loaded && s.overview.is_some(), "old data is kept");
        let s = press(s, Key::Char('i')).0;
        let (s, _) = update(s, Event::Insights(Err("nope".into())));
        assert_eq!(s.insights, Some(vec![]));
    }

    #[test]
    fn reload_clamps_focus_and_handles_empty() {
        let mut s = loaded();
        s.focus = 2;
        let mut o = overview();
        o.cats.truncate(1);
        let (s, _) = update(s, Event::Overview(Ok(Some(o))));
        assert_eq!(s.focus, 0);
        let mut empty = overview();
        empty.cats.clear();
        let (s, _) = update(s, Event::Overview(Ok(Some(empty))));
        assert_eq!(s.focus, 0);
        for k in [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::Enter,
            Key::End,
        ] {
            assert_eq!(press(s.clone(), k).0.focus, 0);
        }
    }

    #[test]
    fn every_key_in_every_view_is_safe() {
        let all = [
            Key::Left,
            Key::Right,
            Key::Up,
            Key::Down,
            Key::PageUp,
            Key::PageDown,
            Key::Home,
            Key::End,
            Key::Enter,
            Key::Backspace,
            Key::Esc,
            Key::Char('h'),
            Key::Char('j'),
            Key::Char('k'),
            Key::Char('l'),
            Key::Char('i'),
            Key::Char('y'),
            Key::Char('r'),
            Key::Char('?'),
            Key::Char('z'),
            Key::Char('b'),
            Key::Char('/'),
            Key::Char('g'),
            Key::Char('~'),
            Key::Char('1'),
            Key::Char('9'),
        ];
        let views: Vec<State> = {
            let o = loaded();
            let paths = press(o.clone(), Key::Enter).0;
            let (browse, _) = update(paths.clone(), Event::Browsed(Ok(listing("/x", 5))));
            let (ins, _) = update(
                press(o.clone(), Key::Char('i')).0,
                Event::Insights(Ok(insights())),
            );
            let empty_browse = update(paths.clone(), Event::Browsed(Ok(listing("/x", 0)))).0;
            let (nosnap, _) = update(State::new("/d", (80, 24)), Event::Overview(Ok(None)));
            let filtering = press(browse.clone(), Key::Char('/')).0;
            let jumping = press(browse.clone(), Key::Char('g')).0;
            vec![
                o,
                paths,
                browse,
                ins,
                empty_browse,
                nosnap,
                filtering,
                jumping,
            ]
        };
        for v in views {
            for k in all {
                let (_, fx) = press(v.clone(), k);
                for e in fx {
                    assert!(matches!(
                        e,
                        Effect::Browse(_) | Effect::Scan | Effect::Insights | Effect::CopyText(_)
                    ));
                }
            }
        }
    }

    /// A listing at `path` whose children are `names` (folders), largest first.
    fn dir(path: &str, names: &[&str]) -> Listing {
        Listing {
            path: path.into(),
            human: "1 KiB".into(),
            entries: names
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    path: format!("{path}/{n}"),
                    is_dir: true,
                    bytes: 10,
                    human: "10 B".into(),
                    pct_of_parent: Some(10.0),
                })
                .collect(),
            remaining: 0,
        }
    }

    fn browsed(s: State, l: Listing) -> State {
        update(s, Event::Browsed(Ok(l))).0
    }

    #[test]
    fn b_browses_the_root_and_esc_returns_to_overview() {
        let (s, fx) = press(loaded(), Key::Char('b'));
        assert_eq!(fx, vec![Effect::Browse("/data".into())]);
        let s = browsed(s, dir("/data", &["a", "b"]));
        assert_eq!((s.view, s.stack.len()), (View::Browse, 1));
        // At the root, ← leaves Browse too.
        let (s2, fx) = press(s.clone(), Key::Left);
        assert!(fx.is_empty());
        assert_eq!((s2.view, s2.stack.len()), (View::Overview, 0));
        let s = press(s, Key::Esc).0;
        assert_eq!(s.view, View::Overview);
        // No snapshot: b explains instead of browsing.
        let (s, _) = update(State::new("/data", (80, 24)), Event::Overview(Ok(None)));
        let (s, fx) = press(s, Key::Char('b'));
        assert!(fx.is_empty() && s.status.is_some());
    }

    #[test]
    fn back_restores_the_row_and_scroll_you_left() {
        let s = browsed(press(loaded(), Key::Char('b')).0, listing("/data", 60));
        let s = keys(s, &[Key::Down; 40]);
        let (sel, offset) = (s.stack[0].sel, s.stack[0].offset);
        let (s, fx) = press(s, Key::Right);
        assert_eq!(
            fx,
            vec![Effect::Browse("/data/proj/node_modules/e40".into())]
        );
        let s = browsed(s, listing("/data/proj/node_modules/e40", 3));
        let s = press(s, Key::Left).0;
        assert_eq!(s.stack.len(), 1);
        assert_eq!((s.stack[0].sel, s.stack[0].offset), (sel, offset));
    }

    #[test]
    fn back_keeps_going_above_where_you_came_in() {
        // Enter at /data/proj/node_modules from the Developer category's paths.
        let s = keys(loaded(), &[Key::Enter]);
        let (s, fx) = press(s, Key::Enter);
        assert_eq!(fx, vec![Effect::Browse("/data/proj/node_modules".into())]);
        let s = browsed(s, dir("/data/proj/node_modules", &["x"]));
        // ← loads the parent and selects node_modules in it.
        let (s, fx) = press(s, Key::Left);
        assert_eq!(fx, vec![Effect::Browse("/data/proj".into())]);
        let s = browsed(s, dir("/data/proj", &["src", "target", "node_modules"]));
        assert_eq!(s.stack.len(), 1);
        assert_eq!(s.stack[0].sel, 3, "row 0 is ../");
        assert_eq!(
            s.current().unwrap().selected().unwrap().name,
            "node_modules"
        );
        // Up again to the root, which selects proj.
        let (s, fx) = press(s, Key::Left);
        assert_eq!(fx, vec![Effect::Browse("/data".into())]);
        let s = browsed(s, dir("/data", &["pics", "proj"]));
        assert_eq!(s.stack[0].sel, 1);
        // Above the root: back to where Browse was entered from.
        let (s, fx) = press(s, Key::Left);
        assert!(fx.is_empty());
        assert_eq!(s.view, View::Paths);
    }

    #[test]
    fn back_never_leaves_the_scan_root() {
        let mut s = State::new("/data", (80, 24));
        s.view = View::Browse;
        s.browse_from = View::Overview;
        let s = browsed(s, dir("/elsewhere/deep", &["x"]));
        let (s, fx) = press(s, Key::Left);
        assert!(fx.is_empty(), "{fx:?}");
        assert_eq!(s.view, View::Overview);
    }

    #[test]
    fn filter_narrows_keeps_and_clears() {
        let s = browsed(
            press(loaded(), Key::Char('b')).0,
            dir("/data", &["@next", "react", "next", "next-auth", "zod"]),
        );
        let s = press(s, Key::Char('/')).0;
        assert_eq!(s.input, Some(Input::Filter));
        // q and ? are text while typing, not commands.
        let (s, fx) = press(s, Key::Char('q'));
        assert!(!s.quit && fx.is_empty() && s.stack[0].filter == "q");
        let s = keys(
            s,
            &[
                Key::Backspace,
                Key::Char('N'),
                Key::Char('e'),
                Key::Char('x'),
            ],
        );
        let names: Vec<_> = s.stack[0]
            .visible()
            .iter()
            .map(|&i| s.stack[0].listing.entries[i].name.clone())
            .collect();
        assert_eq!(names, ["@next", "next", "next-auth"]);
        let s = press(s, Key::Down).0;
        assert_eq!(s.current().unwrap().selected().unwrap().name, "next");
        // Enter keeps the filter; keys are commands again.
        let s = press(s, Key::Enter).0;
        assert_eq!((s.input, s.stack[0].filter.as_str()), (None, "Nex"));
        let (s, fx) = press(s, Key::Right);
        assert_eq!(fx, vec![Effect::Browse("/data/next".into())]);
        let s = browsed(s, dir("/data/next", &["dist"]));
        assert!(
            s.stack[1].filter.is_empty(),
            "a new level starts unfiltered"
        );
        // Back to the filtered level, then Esc clears its filter before leaving Browse.
        let s = press(s, Key::Left).0;
        assert_eq!(s.stack[0].filter, "Nex");
        let s = press(s, Key::Esc).0;
        assert!(s.stack[0].filter.is_empty() && s.view == View::Browse);
        assert_eq!(s.stack[0].visible().len(), 5);
        // Esc while typing clears too.
        let s = keys(s, &[Key::Char('/'), Key::Char('z'), Key::Esc]);
        assert!(s.stack[0].filter.is_empty() && s.input.is_none());
    }

    #[test]
    fn g_jumps_to_a_breadcrumb_level() {
        // Stack: /data, /data/a, /data/a/b, /data/a/b/c.
        let mut s = browsed(press(loaded(), Key::Char('b')).0, dir("/data", &["a"]));
        for p in ["/data/a", "/data/a/b", "/data/a/b/c"] {
            s = press(s, Key::Right).0;
            let name = &p[p.rfind('/').unwrap() + 1..];
            let next = match name {
                "a" => "b",
                "b" => "c",
                _ => "d",
            };
            s = browsed(s, dir(p, &[next]));
            let _ = name;
        }
        // Compared as paths: crumbs are joined with the platform separator.
        let got: Vec<PathBuf> = crumbs("/data", "/data/a/b/c")
            .iter()
            .map(PathBuf::from)
            .collect();
        let want: Vec<PathBuf> = ["/data", "/data/a", "/data/a/b", "/data/a/b/c"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(got, want);
        let s = press(s, Key::Char('g')).0;
        assert_eq!(s.input, Some(Input::Jump));
        // Level 2 is on the stack: restored with no load.
        let (s, fx) = press(s, Key::Char('2'));
        assert!(fx.is_empty());
        assert_eq!((s.stack.len(), s.input), (2, None));
        assert_eq!(s.current().unwrap().listing.path, "/data/a");
        // A level that does not exist says so.
        let s = keys(s, &[Key::Char('g'), Key::Char('7')]);
        assert!(s.status.as_ref().unwrap().text.contains("no level 7"));
        // ~ goes to the root.
        let s = press(s, Key::Char('~')).0;
        assert_eq!(s.stack.len(), 1);
        // Any other key cancels.
        let s = keys(s, &[Key::Char('g'), Key::Char('x')]);
        assert!(s.input.is_none());
    }

    #[test]
    fn g_loads_ancestors_above_the_stack() {
        let s = keys(loaded(), &[Key::Enter, Key::Enter]);
        let s = browsed(s, dir("/data/proj/node_modules", &["x"]));
        let (s, fx) = press(keys(s, &[Key::Char('g')]), Key::Char('1'));
        assert_eq!(fx, vec![Effect::Browse("/data".into())]);
        let s = browsed(s, dir("/data", &["pics", "proj"]));
        assert_eq!(s.stack.len(), 1);
        assert_eq!(
            s.stack[0].sel, 1,
            "the row on the way back down is selected"
        );
    }

    #[test]
    fn parent_row_goes_up_and_hides_while_filtering() {
        // The root has no ../ row: the cursor starts on the first entry.
        let s = press(loaded(), Key::Char('b')).0;
        let s = browsed(s, dir("/data", &["proj", "pics"]));
        assert!(!s.stack[0].up_row());
        assert_eq!(s.stack[0].sel, 0);
        // A child has one, and the cursor starts below it.
        let s = press(s, Key::Enter).0;
        let s = browsed(s, dir("/data/proj", &["src", "target"]));
        let l = s.current().unwrap();
        assert!(l.up_row());
        assert_eq!((l.sel, l.row_count()), (1, 3));
        assert_eq!(l.selected().unwrap().name, "src");
        // ↑ reaches ../, which selects no entry; ↵ on it goes back like ←.
        let s = press(s, Key::Up).0;
        assert!(s.current().unwrap().on_up());
        assert!(s.current().unwrap().selected().is_none());
        let (s2, fx) = press(s.clone(), Key::Enter);
        assert!(fx.is_empty());
        assert_eq!(s2.stack.len(), 1);
        assert_eq!(s2.current().unwrap().listing.path, "/data");
        // A filter hides ../ and puts the cursor on the first match.
        let s = keys(s, &[Key::Char('/'), Key::Char('t')]);
        let l = s.current().unwrap();
        assert!(!l.up_row());
        assert_eq!(l.selected().unwrap().name, "target");
        // Clearing it brings ../ back, cursor on the first entry again.
        let s = keys(s, &[Key::Esc]);
        let l = s.current().unwrap();
        assert!(l.up_row());
        assert_eq!(l.selected().unwrap().name, "src");
    }

    #[test]
    fn empty_folder_selects_the_parent_row() {
        let s = press(loaded(), Key::Char('b')).0;
        let s = browsed(s, dir("/data", &["proj"]));
        let s = press(s, Key::Enter).0;
        let s = browsed(s, dir("/data/proj", &[]));
        assert!(s.current().unwrap().on_up());
        let s = press(s, Key::Enter).0;
        assert_eq!(s.current().unwrap().listing.path, "/data");
    }

    #[test]
    fn insights_open_the_path_inside_its_folder() {
        let mut list = insights();
        list[1].path = "/data/proj/node_modules".into();
        let s = press(loaded(), Key::Char('i')).0;
        let s = update(s, Event::Insights(Ok(list))).0;
        let s = press(s, Key::Down).0;
        let (s, fx) = press(s, Key::Enter);
        assert_eq!(fx, vec![Effect::Browse("/data/proj".into())]);
        let s = browsed(s, dir("/data/proj", &["target", "node_modules"]));
        assert_eq!(s.stack[0].sel, 2, "row 0 is ../");
        // Leaving Browse returns to the insights list.
        let s = press(s, Key::Esc).0;
        assert_eq!(s.view, View::Insights);
    }

    #[test]
    fn y_copies_the_selected_path_in_browse_and_paths() {
        let s = keys(loaded(), &[Key::Enter]);
        let (_, fx) = press(s.clone(), Key::Char('y'));
        assert_eq!(fx, vec![Effect::CopyText("/data/proj/node_modules".into())]);
        let s = browsed(
            press(s, Key::Enter).0,
            dir("/data/proj/node_modules", &["a", "b"]),
        );
        let (s, fx) = press(s, Key::Down);
        assert!(fx.is_empty());
        let (s, fx) = press(s, Key::Char('y'));
        assert_eq!(
            fx,
            vec![Effect::CopyText("/data/proj/node_modules/b".into())]
        );
        assert!(s.status.unwrap().text.contains("copied path"));
        // Never a path with control characters.
        let mut l = dir("/data", &["x"]);
        l.entries[0].path = "/data/\x1b]52;c;AA\x07".into();
        let s = browsed(press(loaded(), Key::Char('b')).0, l);
        let (s, fx) = press(s, Key::Char('y'));
        assert!(fx.is_empty());
        assert!(s.status.unwrap().text.contains("not copied"));
    }

    #[test]
    fn progress_text_groups_digits() {
        let p = Progress {
            files: 1_234_567,
            dirs: 12,
            bytes: 2048,
        };
        assert_eq!(progress_text(&p), "1,234,567 files, 12 dirs, 2.0 KiB");
    }

    #[test]
    fn adjust_offset_cases() {
        assert_eq!(adjust_offset(0, 0, 5), 0);
        assert_eq!(adjust_offset(4, 0, 5), 0);
        assert_eq!(adjust_offset(5, 0, 5), 1);
        assert_eq!(adjust_offset(2, 7, 5), 2);
        assert_eq!(adjust_offset(9, 3, 0), 9);
    }
}
