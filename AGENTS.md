# AGENTS.md

dredge is a minimalist, read-only disk-space explorer for humans and AI agents: a JSON CLI and a
stdio MCP server over one shared engine. Windows first; macOS and Linux work through the same code.

## Commands

```
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --release -p dredge-scan --example bench_scan -- <path>   # files/sec + peak memory
```

All three of fmt, clippy and test must be clean before a commit. CI runs them on Windows, macOS
and Linux.

## Layout

- `crates/dredge-core`: index model and builder, snapshots and store, queries, output types,
  `Engine`. No filesystem walking, no platform-specific code, `#![forbid(unsafe_code)]`.
- `crates/dredge-scan`: scanner backends. All platform code lives in `src/platform/`.
- `crates/dredge-render`: pure terminal rendering for `dredge bar` (strings in, strings out). No I/O,
  no terminal queries, no async, `#![forbid(unsafe_code)]`; depends on `dredge-core` output types only.
- `crates/dredge-tui`: `dredge tui`, a `ratatui` front end. `state.rs` is a pure
  `update(state, event) -> (state, effects)`, `view.rs` draws, `runner.rs` is the thin shell that calls
  `Engine`. No query logic. Optional in the CLI behind the `tui` feature (on by default).
- `crates/dredge-cli`: the `dredge` binary. Parses arguments, calls `Engine`, prints JSON.
- `crates/dredge-mcp`: the MCP server (`dredge mcp`). Tool handlers call `Engine`; background scans
  live in `jobs.rs`.
- `docs/`: `ARCHITECTURE.md`, `OUTPUT_SCHEMA.md` (the contract), `BENCHMARKS.md`, `INSTALL.md`
  (install, MCP registration, upgrading while sessions hold the exe open).
- `scripts/`: `update.ps1` (Windows) and `update.sh` (macOS, Linux) reinstall dredge from the repo.

## Rules

- One engine, thin frontends. A feature is a new `Query` plus a record type in `output.rs`, then a
  CLI subcommand and an MCP tool that only translate arguments. If the CLI and MCP outputs could
  differ, the logic is in the wrong place.
- "stdout is one JSON document" applies to data commands. `dredge bar` and `dredge tui` are
  presentation commands: they print text for humans, never emit escape codes unless the output is a
  terminal or `--color always`, and still send errors to stderr with a nonzero exit. `dredge tui`
  draws only on a terminal and refuses to start when stdout is not one.
- Output is compact by default: sorted by size, `limit` defaults to 10, one envelope shape
  (`schema_version`, `kind`, `snapshot`, `records`, `summary`). Changing a field is a
  `schema_version` change; update `docs/OUTPUT_SCHEMA.md` in the same commit.
- Sizes are allocated bytes, not logical. Hard links count once. Symlinks and junctions are never
  followed. Cloud placeholders are never hydrated (a dehydrated cloud directory is flagged, not
  entered).
- Read-only. The only thing dredge writes is its own snapshots in the data directory
  (`DREDGE_DATA_DIR`, else the OS local data directory). There is no delete or cleanup feature.
- Suggest, never act. dredge may suggest cleanup commands as text (insight records, the TUI's
  copy text) but never executes them, and never deletes or moves anything.
- Local only: no network access, no telemetry.
- MCP stdout carries the protocol only; log to stderr.
- Tests build fixtures in `tempfile` directories and never touch anything else. Keep scanning
  tests independent of the host's block size (assert relationships, not exact allocated bytes).
- Add dependencies with `cargo add` and a one-line justification comment in the root `Cargo.toml`.

## Configuration

- `DREDGE_FOCUS_ROOTS`: path list (`;` on Windows, `:` elsewhere) of roots agents mostly work in.
  `volumes` reports whether each is scanned and how old the snapshot is; `scan` with no path scans
  them.
- `DREDGE_DATA_DIR`: snapshot directory.
- `DREDGE_MCP_IDLE_SECS`: seconds before the MCP server drops its loaded index (default 120).

## Scope and phases

Phases and their contents are in `docs/ROADMAP.md`. `dredge bar` (phase 2, `dredge-render`) and
`dredge tui` (phase 3, `dredge-tui`, feature-gated in `dredge-cli`) are done.

Out of scope for now: deleting or moving files, an NTFS MFT backend, HTML reports, a GUI.
