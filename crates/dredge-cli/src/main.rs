//! `dredge`: thin CLI over `dredge-core`. Every command prints one JSON document on stdout;
//! progress and error text go to stderr.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand};
use dredge_core::output::schema_json;
use dredge_core::util::human_bytes;
use dredge_core::{Engine, EngineConfig, Error, Output, Query, ScanOptions, ScanProgress};
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
    /// List saved snapshots.
    Snapshots,
    /// Scan paths and save a snapshot. With no path, scans $DREDGE_FOCUS_ROOTS.
    /// Several paths print one JSON document per line.
    Scan {
        paths: Vec<String>,
        /// Worker threads (default: logical cores).
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
    /// Everything known about one path.
    Detail { path: String },
    /// Print the JSON Schema of every output document.
    Schema,
    /// Run the read-only MCP server on stdin/stdout.
    Mcp {
        /// Drop the loaded index after this many idle seconds (default 120, or $DREDGE_MCP_IDLE_SECS).
        #[arg(long)]
        idle_timeout_secs: Option<u64>,
    },
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
        Command::Mcp { idle_timeout_secs } => return run_mcp(cfg, idle_timeout_secs),
        Command::Volumes => vec![engine.run(&Query::Volumes)],
        Command::Snapshots => vec![engine.run(&Query::Snapshots)],
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
