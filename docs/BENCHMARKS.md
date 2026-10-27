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
| | | | | | | |

Run each case twice and record the second (warm cache) result as well as the first. Targets from
the plan: a warm scan of a ~1M-file tree in single-digit seconds, and the index at about 40 bytes
per node plus names (see `ARCHITECTURE.md`).

## Query latency

Queries run on an in-memory index, so they should be effectively instant. Loading a snapshot is the
expensive step; time `dredge browse <root>` right after `dredge scan <root>` in a fresh process to
measure the load path.
