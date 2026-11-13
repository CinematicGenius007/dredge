//! The pure state machine: `update(state, event) -> (state, effects)`. No terminal, no I/O, no
//! engine. Every number shown comes from data the runner fetched from `Engine` and delivered as
//! an [`Event`]; effects are requests the runner carries out.

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

/// One level of the browse stack.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub listing: Listing,
    pub sel: usize,
    pub offset: usize,
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
            s.stack.push(Level {
                listing,
                sel: 0,
                offset: 0,
            });
            s.view = View::Browse;
            s.status = None;
        }
        Event::Browsed(Err(e)) => s.err(e),
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
    let rows = s.list_rows();
    let slots = s.insight_slots();
    for l in &mut s.stack {
        l.offset = adjust_offset(l.sel, l.offset, rows);
    }
    s.insight_offset = adjust_offset(s.insight_sel, s.insight_offset, slots);
}

fn key(s: &mut State, k: Key, fx: &mut Vec<Effect>) {
    if k == Key::CtrlC || k == Key::Char('q') {
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
            copy_command(s, fx);
            return;
        }
        _ => {}
    }
    match s.view {
        View::Overview => overview_key(s, k),
        View::Paths => paths_key(s, k, fx),
        View::Browse => browse_key(s, k, fx),
        View::Insights => insights_key(s, k),
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

fn copy_command(s: &mut State, fx: &mut Vec<Effect>) {
    if s.view != View::Insights {
        s.note("y copies an insight's command: open insights with i");
        return;
    }
    match s.insights.as_ref().and_then(|l| l.get(s.insight_sel)) {
        Some(i) => {
            let cmd = i.suggested_command.clone();
            s.note(format!("copied (review before running): {cmd}"));
            fx.push(Effect::CopyText(cmd));
        }
        None => s.note("no insight selected"),
    }
}

fn overview_key(s: &mut State, k: Key) {
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
            if let Some(p) = s.focused().and_then(|c| c.top.get(s.paths_sel)) {
                fx.push(Effect::Browse(p.clone()));
            }
        }
        Key::Backspace | Key::Esc | Key::Left | Key::Char('h') => s.view = View::Overview,
        k if is_nav(k) => s.paths_sel = step(s.paths_sel, len, k, 1),
        _ => {}
    }
}

fn browse_key(s: &mut State, k: Key, fx: &mut Vec<Effect>) {
    let rows = s.list_rows();
    match k {
        Key::Enter | Key::Right | Key::Char('l') => {
            let Some(level) = s.stack.last() else { return };
            match level.listing.entries.get(level.sel) {
                Some(e) if e.is_dir => fx.push(Effect::Browse(e.path.clone())),
                Some(_) => s.note("not a directory"),
                None => {}
            }
        }
        Key::Backspace | Key::Esc | Key::Left | Key::Char('h') => {
            s.stack.pop();
            if s.stack.is_empty() {
                s.view = View::Paths;
            }
            s.status = None;
        }
        k if is_nav(k) => {
            if let Some(l) = s.stack.last_mut() {
                l.sel = step(l.sel, l.listing.entries.len(), k, rows);
                l.offset = adjust_offset(l.sel, l.offset, rows);
            }
        }
        _ => {}
    }
}

fn insights_key(s: &mut State, k: Key) {
    let len = s.insights.as_ref().map_or(0, Vec::len);
    let slots = s.insight_slots();
    match k {
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
        assert_eq!(s.status.as_ref().unwrap().text, "not a directory");
        // back out: browse level, browse level, paths, overview
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
        let rows = s.list_rows();
        assert_eq!(rows, 21);
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
        assert!(l.sel >= l.offset && l.sel < l.offset + s.list_rows());
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
    fn y_copies_only_the_selected_insights_command() {
        let s = loaded();
        let (s, _) = press(s, Key::Char('i'));
        let (s, _) = update(s, Event::Insights(Ok(insights())));
        let s = press(s, Key::Down).0;
        let (s, fx) = press(s, Key::Char('y'));
        assert_eq!(fx, vec![Effect::CopyText("rm -rf /p1".into())]);
        assert!(s.status.as_ref().unwrap().text.contains("rm -rf /p1"));
        // nothing ever executes: the only effects the machine can emit are these
        for k in [Key::Char('y'), Key::Enter, Key::Right, Key::Down, Key::Up] {
            let (_, fx) = press(s.clone(), k);
            assert!(fx.iter().all(|e| matches!(e, Effect::CopyText(_))), "{k:?}");
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
            vec![o, paths, browse, ins, empty_browse, nosnap]
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
