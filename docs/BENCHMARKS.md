# Benchmarks

No numbers are recorded yet. The harness exists; the first measurements have to be taken on a
machine with a Rust toolchain.

## Scan throughput and memory

```
cargo run --release -p dredge-scan --example bench_scan -- C:\
```

Prints files scanned, elapsed time, files/sec and peak memory (peak working set on Windows,
`ru_maxrss` on Unix). The scan result is discarded, so this measures the scanner and index build
only, not snapshot writing.

## What to record

| Machine | Path | Files | Elapsed | Files/sec | Peak memory | Notes |
| :- | :- | -: | -: | -: | -: | :- |
| Windows 11, NTFS on SSD, Defender on (2026-10-27) | `C:\` | 3,262,514 files, 657,976 dirs (3,920,490 nodes) | 135.1 s | 24,144 | 634 MiB | Release `bench_scan`; 22 system dirs denied. |
| Same machine, `dredge scan` (installed release) | `C:\` | 3,255,297 files | 84.8 s (walk only) | 38,373 | not measured | Ran before the 135 s run; cause of the difference not established. |
| Same machine, `dredge scan` | `C:\server` | 403,862 files | 0.8 s (walk only) | 494,928 | not measured | Warm cache. |

Run-to-run variation on `C:\` is large (85 s and 135 s for the same tree), which points to
the OS file cache and Defender rather than the scanner. Treat one number as a rough figure.

The 634 MiB peak is the scan itself: the transient hard-link ID list (about 32 B per file),
interned names, the per-column vectors and the finalize pass all exist at once. A loaded
snapshot is smaller (about 40 B per node plus names), and the MCP server holds only that.

Run each case twice and record the second (warm cache) result as well as the first. Targets from
the plan: a warm scan of a ~1M-file tree in single-digit seconds, and the index at about 40 bytes
per node plus names (see `ARCHITECTURE.md`).

## Query latency

Queries run on an in-memory index, so they should be effectively instant. Loading a snapshot is the
expensive step; time `dredge browse <root>` right after `dredge scan <root>` in a fresh process to
measure the load path.
