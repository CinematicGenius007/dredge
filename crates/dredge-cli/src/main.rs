//! `dredge`: thin CLI over `dredge-core`. Every data command prints one JSON document on
//! stdout; progress and error text go to stderr. `bar` and `tui` are presentation commands:
//! `bar` prints text and `tui` draws on the terminal (errors still go to stderr with a nonzero
//! exit).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use dredge_core::output::schema_json;
use dredge_core::util::human_bytes;
use dredge_core::{Engine, EngineConfig, Error, Forget, Output, Query, ScanOptions, ScanProgress};
use dredge_render::{Caps, ColorTier, Glyphs, Ink, Report, Segment};
use dredge_scan::WalkScanner;

#[derive(Parser)]
#[command(
    name = "dredge",
    version,
    about = "Find what is using your disk space. Output is JSON."
)]
struct Cli {
    /// Indent the JSON output.
    #[arg(long, global = true)]
    pretty: bool,
    /// Include every extra field on each row.
    #[arg(long, global = true)]
    verbose: bool,
    /// Snapshot directory (default: $DREDGE_DATA_DIR or the OS local data directory).
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List volumes with free space, snapshot age, and the configured focus roots.
    Volumes,
    /// List saved snapshots, their total size, and which ones no query uses.
    Snapshots,
    /// Remove dredge's own snapshot files. Touches nothing outside the data directory.
    #[command(group(clap::ArgGroup::new("what").required(true).args(["roots", "prune", "all"])))]
    Forget {
        /// Remove the snapshot rooted at each path (exactly that root, not nested ones).
        roots: Vec<String>,
        /// Remove snapshots no query uses (shadowed by a newer covering snapshot, root gone,
        /// unreadable) and temp files left by interrupted writes.
        #[arg(long)]
        prune: bool,
        /// Remove every snapshot.
        #[arg(long)]
        all: bool,
        /// List what would be removed and remove nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Check snapshot storage, focus roots, and available disk space.
    Doctor,
    /// Print shell completion scripts.
    Completions { shell: CompletionShell },
    /// Scan paths and save a snapshot. With no path, scans $DREDGE_FOCUS_ROOTS.
    /// Several paths print one JSON document per line.
    Scan {
        paths: Vec<String>,
        /// Worker threads (default: logical cores, at most 4 on macOS).
        #[arg(long)]
        threads: Option<usize>,
        /// Do not print progress to stderr.
        #[arg(long)]
        quiet: bool,
    },
    /// List the children of a path, largest first.
    Browse {
        path: String,
        /// Rows to return (default 10).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Largest files under a path.
    Largest {
        path: String,
        /// Only files with this extension, e.g. `log`.
        #[arg(long)]
        ext: Option<String>,
        /// Only files at least this many allocated bytes.
        #[arg(long, value_name = "BYTES")]
        min_bytes: Option<u64>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Space by file extension under a path.
    Types {
        path: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Space by category (developer, caches, photos, ...) under a path.
    Categories {
        path: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Cleanup hints with suggested commands as text. dredge never runs them.
    Insights {
        path: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Everything known about one path.
    Detail { path: String },
    /// Print a dithered space-by-category bar as text (not JSON).
    Bar {
        path: String,
        /// Bar width in cells (default: $COLUMNS, else 80).
        #[arg(long)]
        width: Option<usize>,
        /// Use color: `auto` only on a terminal and without NO_COLOR.
        #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
        color: ColorMode,
        /// Draw the segment of this category densely (e.g. `developer`).
        #[arg(long, value_name = "CATEGORY")]
        focus: Option<String>,
        /// Dot glyphs for the color tiers.
        #[arg(long, value_enum, default_value_t = GlyphMode::Half)]
        glyphs: GlyphMode,
    },
    /// Browse space interactively in the terminal (not JSON). Needs a terminal on stdout.
    Tui {
        /// Root to explore (default: the first of $DREDGE_FOCUS_ROOTS, else the current directory).
        path: Option<String>,
    },
    /// Print the JSON Schema of every output document.
    Schema,
    /// Run the read-only MCP server on stdin/stdout.
    Mcp {
        /// Drop the loaded index after this many idle seconds (default 120, or $DREDGE_MCP_IDLE_SECS).
        #[arg(long)]
        idle_timeout_secs: Option<u64>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, ValueEnum)]
enum GlyphMode {
    Half,
    Braille,
    Ascii,
}

#[derive(Clone, Copy, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Powershell,
    Elvish,
}

/// Decides the color tier. `--color never` and `always` are explicit and win; `auto` means
/// plain text when `NO_COLOR` is set or stdout is not a terminal.
fn resolve_color(
    mode: ColorMode,
    no_color: bool,
    is_tty: bool,
    colorterm: Option<&str>,
    term: Option<&str>,
) -> ColorTier {
    match mode {
        ColorMode::Never => return ColorTier::None,
        ColorMode::Auto if no_color || !is_tty => return ColorTier::None,
        _ => {}
    }
    if matches!(colorterm, Some("truecolor" | "24bit")) {
        ColorTier::TrueColor
    } else if term.is_some_and(|t| t.contains("256color")) {
        ColorTier::Ansi256
    } else {
        ColorTier::Ansi16
    }
}

fn detect_caps(mode: ColorMode, glyphs: GlyphMode) -> Caps {
    let env = |k: &str| std::env::var(k).ok();
    let color = resolve_color(
        mode,
        std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()),
        std::io::stdout().is_terminal(),
        env("COLORTERM").as_deref(),
        env("TERM").as_deref(),
    );
    let glyphs = match glyphs {
        GlyphMode::Half => Glyphs::HalfBlock,
        GlyphMode::Braille => Glyphs::Braille,
        GlyphMode::Ascii => Glyphs::Ascii,
    };
    Caps { color, glyphs }
}

fn bar_error(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("dredge: {msg}");
    ExitCode::from(1)
}

/// `dredge bar`: categories (and the top insight) rendered as text.
fn run_bar(
    engine: &Engine,
    path: &str,
    width: Option<usize>,
    mode: ColorMode,
    focus: Option<String>,
    glyphs: GlyphMode,
) -> ExitCode {
    let focus_ink = match focus.as_deref().map(|f| (f, Ink::from_name(f))) {
        Some((f, None)) => {
            let names = Ink::ALL
                .iter()
                .filter(|i| **i != Ink::Free)
                .map(|i| i.name())
                .collect::<Vec<_>>()
                .join(", ");
            return bar_error(format!(
                "unknown category `{f}` for --focus; valid categories: {names}"
            ));
        }
        Some((_, Some(Ink::Free))) => return bar_error("--focus takes a category, not `free`"),
        Some((_, ink)) => ink,
        None => None,
    };
    let cats = match engine.categories(path, Some(100)) {
        Ok(c) => c,
        Err(e) => return bar_error(&e),
    };
    let mut segments: Vec<Segment> = cats
        .records
        .iter()
        .filter(|r| r.allocated_bytes > 0)
        .map(Segment::from_record)
        .collect();
    let used = cats.summary.allocated_bytes;

    // Free space and capacity are known only when the path is a whole volume.
    let mut total = None;
    if let Output::Volumes(v) = engine.run(&Query::Volumes) {
        let canon = std::fs::canonicalize(path).ok();
        let vol = v.records.iter().find(|r| {
            canon
                .as_deref()
                .is_some_and(|c| c == std::path::Path::new(&r.mount))
        });
        if let Some(r) = vol {
            total = Some(r.total_bytes);
            if r.free_bytes > 0 {
                segments.push(Segment::new("Free", r.free_bytes, Ink::Free));
            }
        }
    }

    let insight = engine
        .insights(path, Some(1))
        .ok()
        .and_then(|i| i.records.into_iter().next())
        .map(|r| format!("{} ({})  {}", r.message, r.human, r.suggested_command));
    let report = Report {
        title: cats.summary.path.clone(),
        used_bytes: used,
        total_bytes: total,
        segments,
        insight,
    };
    let focus = focus_ink.and_then(|i| {
        let found = report.segments.iter().position(|s| s.ink == i);
        if found.is_none() {
            eprintln!(
                "dredge: category `{}` has no space under this path; showing all categories",
                i.name()
            );
        }
        found
    });
    let width = width
        .or_else(|| std::env::var("COLUMNS").ok()?.trim().parse().ok())
        .filter(|w| *w > 0)
        .unwrap_or(80);
    let caps = detect_caps(mode, glyphs);
    let mut stdout = std::io::stdout().lock();
    for line in dredge_render::render_report(&report, width, focus, caps) {
        // A closed pipe (`| head`) is not an error worth reporting.
        if writeln!(stdout, "{line}").is_err() {
            break;
        }
    }
    ExitCode::SUCCESS
}

/// Root for `dredge tui` when no path is given: the first focus root, else the cwd.
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
fn tui_root(path: Option<String>, focus_roots: &[String]) -> Result<String, String> {
    if let Some(p) = path {
        return Ok(p);
    }
    if let Some(first) = focus_roots.first() {
        return Ok(first.clone());
    }
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| format!("cannot read the current directory: {e}"))
}

#[cfg(feature = "tui")]
fn run_tui(cfg: EngineConfig, path: Option<String>) -> ExitCode {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return bar_error("`dredge tui` needs a terminal on stdin and stdout; use `dredge bar` or the JSON commands in scripts");
    }
    let root = match tui_root(path, &cfg.focus_roots) {
        Ok(r) => r,
        Err(e) => return bar_error(e),
    };
    let engine = std::sync::Arc::new(Engine::new(cfg));
    match dredge_tui::run(engine, std::sync::Arc::new(WalkScanner), root) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => bar_error(e),
    }
}

#[cfg(not(feature = "tui"))]
fn run_tui(_cfg: EngineConfig, _path: Option<String>) -> ExitCode {
    bar_error("this build of dredge has no terminal UI (built without the `tui` feature); reinstall with default features")
}

const DEFAULT_IDLE_SECS: u64 = 120;

fn run_mcp(cfg: EngineConfig, idle_secs: Option<u64>) -> ExitCode {
    let idle_secs = idle_secs
        .or_else(|| {
            std::env::var("DREDGE_MCP_IDLE_SECS")
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .unwrap_or(DEFAULT_IDLE_SECS);
    match dredge_mcp::serve(cfg, Duration::from_secs(idle_secs)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dredge: mcp server failed: {e}");
            ExitCode::from(1)
        }
    }
}

fn emit(out: &Output, pretty: bool) {
    let mut stdout = std::io::stdout().lock();
    // A closed pipe (`| head`) is not an error worth reporting.
    let _ = writeln!(stdout, "{}", out.render(pretty));
    if let Output::Error(e) = out {
        eprintln!("dredge: {}", e.error.message);
        if let Some(h) = &e.error.hint {
            eprintln!("hint: {h}");
        }
    }
}

fn scan_one(engine: &Engine, path: &str, threads: Option<usize>, show_progress: bool) -> Output {
    let progress = ScanProgress::default();
    let opts = ScanOptions { threads };
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|s| {
        if show_progress {
            s.spawn(|| {
                while !done.load(Ordering::Relaxed) {
                    let p = progress.snapshot();
                    eprint!(
                        "\rscanning {path}: {} files, {} dirs, {}   ",
                        p.files,
                        p.dirs,
                        human_bytes(p.allocated_bytes)
                    );
                    std::thread::sleep(Duration::from_millis(100));
                }
                eprint!("\r{:width$}\r", "", width = 100);
            });
        }
        let r = engine.scan(&WalkScanner, path, &opts, &progress);
        done.store(true, Ordering::Relaxed);
        r
    });
    match result {
        Ok(out) => Output::Scan(out),
        Err(e) => Output::from_error(&e),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg = EngineConfig::from_env(cli.data_dir, cli.verbose);
    let engine = Engine::new(cfg.clone());

    let outputs: Vec<Output> = match cli.command {
        Command::Schema => {
            println!("{}", schema_json(cli.pretty));
            return ExitCode::SUCCESS;
        }
        Command::Completions { shell } => {
            let shell = match shell {
                CompletionShell::Bash => clap_complete::Shell::Bash,
                CompletionShell::Zsh => clap_complete::Shell::Zsh,
                CompletionShell::Fish => clap_complete::Shell::Fish,
                CompletionShell::Powershell => clap_complete::Shell::PowerShell,
                CompletionShell::Elvish => clap_complete::Shell::Elvish,
            };
            clap_complete::generate(shell, &mut Cli::command(), "dredge", &mut std::io::stdout());
            return ExitCode::SUCCESS;
        }
        Command::Mcp { idle_timeout_secs } => return run_mcp(cfg, idle_timeout_secs),
        Command::Tui { path } => return run_tui(cfg, path),
        Command::Bar {
            path,
            width,
            color,
            focus,
            glyphs,
        } => return run_bar(&engine, &path, width, color, focus, glyphs),
        Command::Volumes => vec![engine.run(&Query::Volumes)],
        Command::Snapshots => vec![engine.run(&Query::Snapshots)],
        Command::Forget {
            roots,
            prune,
            all,
            dry_run,
        } => {
            let what = if all {
                Forget::All
            } else if prune {
                Forget::Prune
            } else {
                Forget::Roots(roots)
            };
            vec![engine
                .forget(&what, dry_run)
                .map(Output::Forget)
                .unwrap_or_else(|e| Output::from_error(&e))]
        }
        Command::Doctor => vec![engine.run(&Query::Doctor)],
        Command::Browse { path, limit } => vec![engine.run(&Query::Browse { path, limit })],
        Command::Largest {
            path,
            ext,
            min_bytes,
            limit,
        } => vec![engine.run(&Query::Largest {
            path,
            ext,
            min_bytes,
            limit,
        })],
        Command::Types { path, limit } => vec![engine.run(&Query::Types { path, limit })],
        Command::Categories { path, limit } => vec![engine.run(&Query::Categories { path, limit })],
        Command::Insights { path, limit } => vec![engine.run(&Query::Insights { path, limit })],
        Command::Detail { path } => vec![engine.run(&Query::Detail { path })],
        Command::Scan {
            paths,
            threads,
            quiet,
        } => {
            let targets: Vec<String> = if paths.is_empty() {
                engine.focus_roots().to_vec()
            } else {
                paths
            };
            if targets.is_empty() {
                vec![Output::from_error(&Error::InvalidArgument(
                    "no path given and DREDGE_FOCUS_ROOTS is not set".into(),
                ))]
            } else {
                let show = !quiet && std::io::stderr().is_terminal();
                targets
                    .iter()
                    .map(|p| scan_one(&engine, p, threads, show))
                    .collect()
            }
        }
    };

    let mut failed = false;
    for o in &outputs {
        failed |= o.is_error();
        emit(o, cli.pretty);
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_root_prefers_arg_then_focus_then_cwd() {
        let roots = vec!["/f1".to_string(), "/f2".to_string()];
        assert_eq!(tui_root(Some("/x".into()), &roots).unwrap(), "/x");
        assert_eq!(tui_root(None, &roots).unwrap(), "/f1");
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(tui_root(None, &[]).unwrap(), cwd);
    }

    #[test]
    fn color_detection() {
        use ColorMode::*;
        let t = |m, nc, tty, ct, term| resolve_color(m, nc, tty, ct, term);
        assert_eq!(
            t(Auto, true, true, Some("truecolor"), None),
            ColorTier::None
        );
        assert_eq!(
            t(Auto, false, false, Some("truecolor"), None),
            ColorTier::None
        );
        assert_eq!(
            t(Always, true, false, Some("24bit"), None),
            ColorTier::TrueColor
        );
        assert_eq!(
            t(Never, false, true, Some("truecolor"), None),
            ColorTier::None
        );
        assert_eq!(
            t(Auto, false, true, None, Some("xterm-256color")),
            ColorTier::Ansi256
        );
        assert_eq!(t(Auto, false, true, None, Some("xterm")), ColorTier::Ansi16);
        assert_eq!(t(Auto, false, true, None, None), ColorTier::Ansi16);
    }
}
