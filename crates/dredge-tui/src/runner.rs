//! The thin shell: terminal setup and restore, the event loop, and effect execution. Every
//! number it hands to the state machine comes from `Engine`.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::panic;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableMouseCapture, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use dredge_core::output::{CategoriesOut, EntryRecord};
use dredge_core::{Engine, Error, Output, Query, ScanOptions, ScanProgress, Scanner};
use dredge_render::Ink;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::state::{
    update, Cat, Effect, Entry, Event, Insight, Key, Listing, Overview, Progress, State,
    BROWSE_LIMIT,
};
use crate::view::{draw, Theme};

#[derive(Debug)]
pub enum RunError {
    /// stdout is not a terminal.
    NotATerminal,
    Io(io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotATerminal => {
                f.write_str("`dredge tui` needs a terminal on stdin and stdout; use `dredge bar` or the JSON commands in scripts")
            }
            RunError::Io(e) => write!(f, "terminal error: {e}"),
        }
    }
}

impl std::error::Error for RunError {}

impl From<io::Error> for RunError {
    fn from(e: io::Error) -> Self {
        RunError::Io(e)
    }
}

// ---------------------------------------------------------------------------------------------
// Data: map Engine output to the state machine's plain types. Mapping only, no queries of our own.

pub fn load_overview(engine: &Engine, root: &str) -> Result<Option<Overview>, String> {
    let cats: CategoriesOut = match engine.categories(root, Some(100)) {
        Ok(c) => c,
        Err(Error::NoSnapshot { .. }) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let (mut total, mut free) = (None, None);
    if let Output::Volumes(v) = engine.run(&Query::Volumes) {
        let canon = std::fs::canonicalize(root).ok();
        if let Some(r) = v.records.iter().find(|r| {
            canon
                .as_deref()
                .is_some_and(|c| c == std::path::Path::new(&r.mount))
        }) {
            total = Some(r.total_bytes);
            free = Some(r.free_bytes);
        }
    }
    Ok(Some(Overview {
        path: cats.summary.path.clone(),
        used_bytes: cats.summary.allocated_bytes,
        used_human: cats.summary.human.clone(),
        total_bytes: total,
        free_bytes: free,
        age_secs: cats.snapshot.as_ref().map_or(0, |s| s.age_secs),
        cats: cats
            .records
            .iter()
            .filter(|r| r.allocated_bytes > 0)
            .map(|r| Cat {
                ink: Ink::from_category(r.category),
                bytes: r.allocated_bytes,
                human: r.human.clone(),
                share: r.share,
                files: r.files,
                top: r.top.clone(),
            })
            .collect(),
    }))
}

fn entry(r: &EntryRecord) -> Entry {
    Entry {
        name: r.name.clone(),
        path: r.path.clone(),
        is_dir: r.kind == "dir",
        bytes: r.allocated_bytes,
        human: r.human.clone(),
        pct_of_parent: r.pct_of_parent,
    }
}

pub fn load_browse(engine: &Engine, path: &str) -> Result<Listing, String> {
    let q = Query::Browse {
        path: path.to_string(),
        limit: Some(BROWSE_LIMIT),
    };
    match engine.try_run(&q) {
        Ok(Output::Browse(b)) => Ok(Listing {
            path: b.summary.path.clone(),
            human: b.summary.human.clone(),
            entries: b.records.iter().map(entry).collect(),
            remaining: b.summary.remaining_entries,
        }),
        Ok(_) => Err("unexpected engine output".into()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn load_insights(engine: &Engine, root: &str) -> Result<Vec<Insight>, String> {
    engine
        .insights(root, Some(50))
        .map(|i| {
            i.records
                .iter()
                .map(|r| Insight {
                    human: r.human.clone(),
                    message: r.message.clone(),
                    path: r.path.clone(),
                    suggested_command: r.suggested_command.clone(),
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------------
// Terminal

/// Leaves raw mode and the alternate screen and shows the cursor. Safe to call twice.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
}

/// Restores the terminal when dropped, on every exit path including unwinding.
struct Restore;

impl Drop for Restore {
    fn drop(&mut self) {
        restore_terminal();
    }
}

type PanicHook = dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync + 'static;

struct PanicHookGuard(Arc<PanicHook>);

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let previous = self.0.clone();
            panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

fn install_panic_hook() -> PanicHookGuard {
    let previous: Arc<PanicHook> = Arc::from(panic::take_hook());
    let chained = previous.clone();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        chained(info);
    }));
    PanicHookGuard(previous)
}

/// Standard base64 (RFC 4648), for OSC 52.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The OSC 52 sequence that asks the terminal to put `text` on the system clipboard.
const CLIPBOARD_LIMIT: usize = 100_000;

pub fn osc52(text: &str, tmux: bool) -> Option<String> {
    if text.len() > CLIPBOARD_LIMIT {
        return None;
    }
    let osc = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    Some(if tmux {
        format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b"))
    } else {
        osc
    })
}

fn translate(ev: TermEvent) -> Option<Event> {
    match ev {
        TermEvent::Resize(w, h) => Some(Event::Resize(w, h)),
        TermEvent::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) if kind != KeyEventKind::Release => translate_key(code, modifiers).map(Event::Key),
        _ => None,
    }
}

pub fn translate_key(code: KeyCode, mods: KeyModifiers) -> Option<Key> {
    if mods.contains(KeyModifiers::CONTROL) {
        return matches!(code, KeyCode::Char('c' | 'C')).then_some(Key::CtrlC);
    }
    Some(match code {
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Esc => Key::Esc,
        KeyCode::Char(c) => Key::Char(c),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Scan job

struct ScanJob {
    progress: Arc<ScanProgress>,
    done: Receiver<Result<(), String>>,
}

impl Drop for ScanJob {
    fn drop(&mut self) {
        self.progress.cancel();
    }
}

/// Rescans use a quarter of the cores (at least one) so the machine stays usable.
fn low_priority_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).max(1))
}

fn spawn_scan(engine: Arc<Engine>, scanner: Arc<dyn Scanner>, root: String) -> ScanJob {
    let progress = Arc::new(ScanProgress::default());
    let (tx, rx) = mpsc::channel();
    let p = progress.clone();
    std::thread::spawn(move || {
        let opts = ScanOptions {
            threads: Some(low_priority_threads()),
        };
        let r = engine
            .scan(scanner.as_ref(), &root, &opts, &p)
            .map(|_| ())
            .map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    ScanJob { progress, done: rx }
}

fn progress_of(p: &ScanProgress) -> Progress {
    let s = p.snapshot();
    Progress {
        files: s.files,
        dirs: s.dirs,
        bytes: s.allocated_bytes,
    }
}

// ---------------------------------------------------------------------------------------------
// The loop

/// Runs the TUI until the user quits. The terminal is restored on every path out, including a
/// panic.
pub fn run(engine: Arc<Engine>, scanner: Arc<dyn Scanner>, root: String) -> Result<(), RunError> {
    if !io::IsTerminal::is_terminal(&io::stdout()) || !io::IsTerminal::is_terminal(&io::stdin()) {
        return Err(RunError::NotATerminal);
    }
    let _panic_hook = install_panic_hook();
    let _restore = Restore;
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let theme = Theme::detect();
    let size = terminal.size()?;
    let mut state = State::new(root.clone(), (size.width, size.height));
    let mut job: Option<ScanJob> = None;
    let mut queue: VecDeque<Event> = VecDeque::new();
    queue.push_back(Event::Overview(load_overview(&engine, &root)));
    let mut dirty = true;
    let mut quit = false;
    let mut last_progress = Instant::now();

    while !quit {
        // Apply everything queued, collecting effects and follow-up events.
        while let Some(ev) = queue.pop_front() {
            let before = state.clone();
            let (next, effects) = update(state, ev);
            state = next;
            dirty |= state != before;
            for fx in effects {
                match fx {
                    Effect::Quit => {
                        quit = true;
                        break;
                    }
                    Effect::Browse(p) => queue.push_back(Event::Browsed(load_browse(&engine, &p))),
                    Effect::Insights => {
                        queue.push_back(Event::Insights(load_insights(&engine, &state.root)));
                    }
                    Effect::Scan => {
                        if job.is_none() {
                            job = Some(spawn_scan(
                                engine.clone(),
                                scanner.clone(),
                                state.root.clone(),
                            ));
                            last_progress = Instant::now();
                        }
                    }
                    Effect::CopyText(t) => {
                        let mut out = io::stdout();
                        if let Some(sequence) = osc52(&t, std::env::var_os("TMUX").is_some()) {
                            let _ = out.write_all(sequence.as_bytes());
                        } else {
                            state.status = Some(crate::state::Status {
                                text: "command too long to copy".into(),
                                error: true,
                            });
                            dirty = true;
                        }
                        let _ = out.flush();
                    }
                }
            }
            if quit {
                break;
            }
        }
        if quit {
            break;
        }
        if dirty {
            terminal.draw(|f| draw(f, &state, theme))?;
            dirty = false;
        }

        // Block on input; while a scan runs, wake every 100 ms for progress and completion.
        let input = if job.is_some() {
            if event::poll(Duration::from_millis(100))? {
                Some(event::read()?)
            } else {
                None
            }
        } else {
            Some(event::read()?)
        };
        if let Some(ev) = input.and_then(translate) {
            queue.push_back(ev);
        }
        if let Some(j) = &job {
            match j.done.try_recv() {
                Ok(r) => {
                    let ok = r.is_ok();
                    queue.push_back(Event::ScanDone(r));
                    if ok {
                        queue.push_back(Event::Overview(load_overview(&engine, &root)));
                    }
                    job = None;
                }
                Err(TryRecvError::Disconnected) => {
                    return Err(RunError::Io(io::Error::other("scan thread stopped")));
                }
                Err(TryRecvError::Empty) => {
                    if last_progress.elapsed() >= Duration::from_millis(100) {
                        queue.push_back(Event::Progress(progress_of(&j.progress)));
                        last_progress = Instant::now();
                    }
                }
            }
        }
    }
    if let Some(j) = &job {
        j.progress.cancel();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn osc52_wraps_the_text() {
        assert_eq!(
            osc52("rm -rf x", false).unwrap(),
            "\x1b]52;c;cm0gLXJmIHg=\x07"
        );
        assert_eq!(
            osc52("f", true).unwrap(),
            "\x1bPtmux;\x1b\x1b]52;c;Zg==\x07\x1b\\"
        );
        assert!(osc52(&"a".repeat(CLIPBOARD_LIMIT + 1), false).is_none());
    }

    #[test]
    fn dropping_scan_job_cancels_it() {
        let progress = Arc::new(ScanProgress::default());
        let (_tx, done) = mpsc::channel();
        drop(ScanJob {
            progress: progress.clone(),
            done,
        });
        assert!(progress.cancelled());
    }

    #[test]
    fn keys_translate() {
        let none = KeyModifiers::NONE;
        assert_eq!(
            translate_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Key::CtrlC)
        );
        assert_eq!(
            translate_key(KeyCode::Char('r'), KeyModifiers::CONTROL),
            None
        );
        assert_eq!(
            translate_key(KeyCode::Char('q'), none),
            Some(Key::Char('q'))
        );
        assert_eq!(
            translate_key(KeyCode::Char('?'), KeyModifiers::SHIFT),
            Some(Key::Char('?'))
        );
        assert_eq!(
            translate_key(KeyCode::Backspace, none),
            Some(Key::Backspace)
        );
        assert_eq!(translate_key(KeyCode::F(1), none), None);
    }

    #[test]
    fn missing_snapshot_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = Engine::new(dredge_core::EngineConfig {
            data_dir: tmp.path().join("data"),
            focus_roots: vec![],
            verbose: false,
        });
        let root = tmp.path().to_string_lossy().into_owned();
        assert_eq!(load_overview(&engine, &root), Ok(None));
        assert!(load_browse(&engine, &root).is_err());
        assert!(load_insights(&engine, &root).is_err());
    }
}
