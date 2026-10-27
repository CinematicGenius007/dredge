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
- `crates/dredge-cli`: the `dredge` binary. Parses arguments, calls `Engine`, prints JSON.
- `crates/dredge-mcp`: the MCP server (`dredge mcp`). Tool handlers call `Engine`; background scans
  live in `jobs.rs`.
- `docs/`: `ARCHITECTURE.md`, `OUTPUT_SCHEMA.md` (the contract), `BENCHMARKS.md`.

## Rules

- One engine, thin frontends. A feature is a new `Query` plus a record type in `output.rs`, then a
  CLI subcommand and an MCP tool that only translate arguments. If the CLI and MCP outputs could
  differ, the logic is in the wrong place.
- Output is compact by default: sorted by size, `limit` defaults to 10, one envelope shape
  (`schema_version`, `kind`, `snapshot`, `records`, `summary`). Changing a field is a
  `schema_version` change; update `docs/OUTPUT_SCHEMA.md` in the same commit.
- Sizes are allocated bytes, not logical. Hard links count once. Symlinks and junctions are never
  followed. Cloud placeholders are never hydrated (a dehydrated cloud directory is flagged, not
  entered).
- Read-only. The only thing dredge writes is its own snapshots in the data directory
  (`DREDGE_DATA_DIR`, else the OS local data directory). There is no delete or cleanup feature.
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

## Out of scope for phase 1

Deleting or moving files, an NTFS MFT backend, a TUI, HTML reports, a GUI.
