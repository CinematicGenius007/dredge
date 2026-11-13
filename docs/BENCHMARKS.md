# Benchmarks

Recorded numbers are below. Each is one machine on one day, so read them as rough figures and
not as guarantees.

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
| Same machine, `dredge scan` | a project directory (not `C:\`) | 403,862 files | 0.8 s (walk only) | 494,928 | not measured | Warm cache. |

| macOS, APFS on SSD, 10 cores (2026-11-14), `lstat` backend | a developer directory | 7,016,936 files, 788,259 dirs | 80.1 s real, 462.7 s sys | ~88,000 | 937 MB RSS | `dredge scan`, warm, 10 threads. |
| Same machine, `getattrlistbulk` backend | same directory | 7,026,214 files, 790,605 dirs | 44.3 s real, 279.9 s sys | 160,544 | 944 MB RSS | 10 threads. |
| Same machine, `getattrlistbulk` backend | same directory | same | 42.5 s real, 101.1 s sys | 165,540 | 934 MB RSS | 4 threads (the new macOS default). 6 threads: 44.2 s / 162.9 s sys; 8 threads: 45.4 s / 218.6 s sys. |
| Same machine, `lstat` backend | a 1.4M-file subtree | 1,385,168 files, 139,188 dirs | 12.6 s real, 70.2 s sys | 110,143 | 316 MB RSS | 10 threads, run back to back with the next row. |
| Same machine, `getattrlistbulk` backend | same subtree | same | 6.8 s real, 37.1 s sys | 173,950 | 297 MB RSS | 10 threads. 4 threads: 7.7 s / 16.6 s sys. 2 threads: 13.5 s / 12.8 s sys. |

On macOS the scan is bound by the kernel, not by dredge: user time is under 10 s for 7M files.
`getattrlistbulk` returns a whole buffer of entries per call, which removes the per-entry `lstat`.
Past about four workers, wall time stays flat while system time grows, so the default on macOS is
capped at four threads. Both backends were compared entry by entry on the 1.4M-file subtree
(1,524,731 entries, 0 differences in kind, allocated or logical size). Memory is unchanged: it is
the index build, not the listing.

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

## CI regression floor

The optional Ubuntu CI benchmark creates 3,000 tiny files in 30 directories, runs the release
`bench_scan` example, and fails below `.github/bench-floor.txt` (currently 150,000 files/sec).
This is about 23% of the local 645,416 files/sec measurement on the same fixture, deliberately
leaving substantial headroom for noisy hosted runners. It runs only on pushes to `main` and manual
dispatches. To refresh the floor, regenerate the same fixture shape, run
`cargo run --release -p dredge-scan --example bench_scan -- <fixture>`, repeat several times,
choose roughly 25% of the lowest representative local throughput, and update the floor file and
this note together. CI workflows cannot be run locally; the benchmark command itself is runnable.
