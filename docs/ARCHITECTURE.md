# dredge architecture (phase 1)

Status: **draft for approval**. No code exists yet. Items marked ⚑ are decisions I'd like
you to look at; everything else under "Decisions" is a simplest-option call I'll proceed
with unless you object.

## 1. Goal and non-goals

`dredge` answers "what is using my disk space, and what is safe to clean up?" for humans
(CLI JSON now; TUI/HTML later) and AI agents (same JSON, plus a read-only MCP server).

Phase 1 builds: scanner, in-memory index, snapshots, JSON CLI, MCP server, tests/CI/docs.
Phase 1 does **not** build: deletion, NTFS raw-MFT backend, TUI, HTML report,
macOS-specific optimizations, GUI. Extension points (section 10) exist as traits and
module boundaries only. No stubs.

## 2. Crates and dependency direction

```
dredge-cli ──► dredge-mcp ──► dredge-core ◄── dredge-scan
     │                            ▲               ▲
     └────────────────────────────┴───────────────┘   (cli also depends on dredge-scan)
```

| Crate | Owns | Does not own |
|---|---|---|
| `dredge-core` | `Index` + `IndexBuilder`, size accounting rules, snapshot IO, `Store` (data dir), `Engine` (query functions), output types (serde + schemars), `Scanner` trait, `volumes()` | any filesystem walking, any `cfg(windows)` code |
| `dredge-scan` | `WalkScanner`, platform directory enumeration (`cfg` modules + portable fallback) | queries, JSON, snapshots |
| `dredge-cli` | binary `dredge`: clap parsing, printing one JSON doc, exit codes, `dredge mcp` entry | any query logic |
| `dredge-mcp` | rmcp stdio server, job manager, idle-unload cache, tool descriptions | any query logic |

**One engine, thin frontends.** `dredge_core::Engine` exposes `browse`, `largest`,
`types`, `detail`, `snapshots`, `volumes`, and `scan(&dyn Scanner, ...)`. Each returns the
typed `Envelope` that is serialized verbatim by both the CLI and the MCP tools. Frontends
only parse arguments, call one Engine method, and print. A test asserts CLI and MCP
output are byte-identical for the same inputs.

The `Scanner` trait lives in core (not in `dredge-scan`) so the dependency arrow points
scan → core and a future `MftScanner` crate can slot in without core changing.

## 3. Index (in-memory model)

Struct-of-arrays, one entry per filesystem object (file, dir, link). Nodes are laid out in
**DFS pre-order with siblings sorted by allocated size descending**. Consequences:

- A node's subtree is the contiguous range `[i, subtree_end[i])`. `largest` and `types`
  are a linear slice scan, no pointer chasing.
- Children of `i` are `i+1`, then `subtree_end[i+1]`, then `subtree_end[...]`, and so on.
  Because they are size-sorted, `browse --limit N` touches N children. This replaces the
  CSR child arrays suggested in the plan: pre-order gives the same O(1) child access with
  zero extra arrays. (⚑ deviation from the plan's wording; same intent.)
- Children always have a higher index than their parent, so directory totals are one
  reverse pass: `for i in (0..n).rev() { total[parent[i]] += total[i] }`.

| Column | Type | Bytes | Notes |
|---|---|---|---|
| `parent` | u32 | 4 | root's parent = itself |
| `name_off` | u32 | 4 | into the name arena |
| `name_len` | u16 | 2 | max 255 UTF-16 units = < 1 KiB UTF-8 |
| `flags` | u16 | 2 | low 3 bits `kind`, remaining bits flags |
| `allocated` | u64 | 8 | on-disk bytes; dirs hold subtree total |
| `logical` | u64 | 8 | apparent bytes; dirs hold subtree total |
| `mtime` | u32 | 4 | unix seconds ⚑ (see Decisions D1) |
| `subtree_end` | u32 | 4 | exclusive end of subtree in pre-order |
| `files` | u32 | 4 | dirs: file count in subtree. files: 1 |

≈ 40 bytes/node plus names. **Names are interned** into one UTF-8 arena (hash-deduped
while building, so `index.js`, `.git`, `Cargo.toml` are stored once). Expect ~50 B per
file on typical trees, so ~500 MB for 10M files. Not optimized further in phase 1.

`kind`: `file`, `dir`, `symlink`, `junction`, `reparse` (other reparse points).

`flags`: `cloud_placeholder`, `denied`, `hardlink_alias`, `estimated` (allocated size is
rounded from logical because the platform couldn't report it), `name_lossy` (name was not
valid Unicode and was converted lossily), `mount_point` (Unix: different filesystem, not
entered).

Rare per-node data lives in **sparse side tables** keyed by node index, not columns:
`hard_links: HashMap<u32, u32>` (links seen in scan for owner and aliases).

Paths are never stored. They are rebuilt by walking `parent` and joining names with the
platform separator. Path → node resolution walks components from the root, matching
siblings by name: exact match first, then case-folded fallback on Windows/macOS
(case-insensitive filesystems); Linux is exact only.

## 4. Size accounting rules

- **Allocated** (on-disk) is the primary size. **Logical** is recorded alongside.
  Sums and percentages use allocated only.
- **Hard links count once.** Within one scan, entries sharing a file ID are one group; the
  owner is the member with the lexicographically smallest full path (deterministic,
  independent of thread timing). Other members get `allocated = logical = 0` and flag
  `hardlink_alias`. The owner shows `hard_links: N` = links *seen inside the scan root*
  (may be fewer than the true link count if other links are outside the root).
- **Symlinks, junctions, other reparse points are never followed.** They appear as their
  own entries with their own (tiny) size and a `kind` saying what they are. Link targets
  are not recorded in phase 1.
- **Cloud placeholders (OneDrive etc.) are never hydrated.** Detected from file
  attributes during enumeration; no file is ever opened for data. Flagged
  `cloud_placeholder`; allocated is whatever the filesystem reports (usually ~0) and
  `logical_bytes` is surfaced so the agent sees "400 MB online-only". A *dehydrated cloud
  directory* (recall-on-open / recall-on-data-access / offline attributes) is flagged the
  same way and not entered, because listing it can make the sync engine fetch its
  contents; a populated cloud-tag directory or a pinned file is read normally.
- A directory's own metadata size is included in its allocated total.
- A scan is **single-volume**: Windows mount points are reparse points (not followed);
  on Unix, a directory whose `st_dev` differs from the root's is recorded as `mount_point`
  and not entered. Because of this, file IDs only need to be unique within one volume, so
  dedupe keys are the file ID alone and the volume ID is stored once in the snapshot
  header.
- Scan totals will not equal the volume's "used" figure (MFT, journal, VSS, files in
  unreadable directories). The scan summary reports `volume_used_bytes` and
  `unaccounted_bytes` when the root is a volume root so this gap is explicit.

## 5. Scanner

```rust
pub trait Scanner: Send + Sync {
    fn name(&self) -> &'static str;
    fn scan(&self, root: &Path, opts: &ScanOptions, progress: &ScanProgress)
        -> Result<Index, ScanError>;
}
```

`ScanProgress` is a struct of atomics (files, dirs, bytes, errors, cancel flag). The CLI
prints it to stderr when stderr is a TTY; MCP reads it for `disk_job_status`. A scanner
checks `progress.cancelled()` between directories. Nothing in phase 1 sets the flag: the
CLI has no Ctrl-C handler (that would need an extra dependency) and snapshots are written
atomically, so an interrupted scan leaves no partial file.

### WalkScanner

- Work-stealing parallelism with `rayon::scope`: one task per directory. A task lists its
  directory, builds `RawEntry` records, inserts the batch into a `Mutex<IndexBuilder>`
  (short critical section: intern names, append records) which returns child directory
  ids, then spawns one task per subdirectory. If profiling shows contention, replace the
  mutex with a channel to a single builder thread; the `IndexBuilder` API doesn't change.
- Default threads = logical cores (directory enumeration is I/O-latency-bound, so this is
  tuned by benchmark, not guessed; `ScanOptions::threads` overrides).
- Platform enumeration is one function behind `cfg`:
  `fn read_dir(path, &mut Vec<RawEntry>) -> Result<(), DirError>`.

| Platform | Implementation | Allocated size source |
|---|---|---|
| Windows (primary) | `windows-sys`: open directory handle (`FILE_FLAG_BACKUP_SEMANTICS`, full sharing, no admin), enumerate with `GetFileInformationByHandleEx(FileIdExtdDirectoryInfo)` in large buffers. One call returns, per entry: `AllocationSize`, `EndOfFile`, attributes, reparse tag, 128-bit file ID, mtime. Files are never opened. | `AllocationSize` (exact, handles compression/sparse) |
| Unix | `std::fs` + `MetadataExt` | `st_blocks * 512`; `nlink`, `(dev, ino)` |
| Fallback (any other case, and Windows filesystems that reject the extended class) | `std::fs::read_dir` + metadata | logical size rounded up to 4096, flag `estimated` |

The plan suggested `FindFirstFileExW`. I'm using `FileIdExtdDirectoryInfo` instead
because `FindFirstFileExW` returns no allocation size and no file ID, which would force
one `CreateFile` per file (slow, and risks touching placeholders). This is the same
"windows-sys, bulk fetch" approach, with the right information class. `FindFirstFileExW`
with `FIND_FIRST_EX_LARGE_FETCH` is the planned fallback only if the extended class proves
unreliable in testing.

`FILE_ID_EXTD_DIR_INFO` has no link count, so on Windows every file's ID is recorded in a
transient list and grouped at finalize (≈ 32 B/file: a `u128` ID plus a `u32`, freed after build). On Unix only
files with `nlink > 1` are recorded.

Cloud detection (Windows): `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`,
`FILE_ATTRIBUTE_RECALL_ON_OPEN`, `FILE_ATTRIBUTE_OFFLINE`. Cloud-tagged reparse points
(OneDrive) are treated as ordinary files/dirs plus the flag, **not** as links. Because we
only enumerate and never open for data, nothing hydrates. Directory placeholders are
entered (enumerating an unhydrated OneDrive folder does not download its files; this is
verified manually before the phase is declared done, see Testing).

Long paths: internal Win32 calls use the `\\?\` prefix; it is stripped for everything
that reaches output or snapshots.

### Finalize pipeline (`IndexBuilder::finish`)

1. Collect: unordered nodes with scanner-assigned ids.
2. Lay out a name-sorted tree (deterministic).
3. Resolve hard links using the name-sorted order: pick owners, zero the aliases.
4. Compute directory totals (reverse pass).
5. Re-sort siblings by allocated desc (ties: name), emit DFS pre-order, fill `parent`,
   `subtree_end`, `files`. The index is immutable from here.

`IndexBuilder` is public in core so a future `MftScanner` feeds it directly.

### Errors

Per-directory and per-file failures (access denied, sharing violation, vanished during
scan, name decode problems) are **counted by category** and never abort the scan. A
denied directory becomes a `dir` node with flag `denied` and size 0. The index keeps
totals per category plus the first 20 sample paths. Only a failure on the root itself
(doesn't exist, not a directory, can't be opened) is a `ScanError`.

## 6. Snapshot

One file per scanned root, atomically replaced on rescan (write temp, fsync, rename).

```
magic    8 B   "DREDGE\0\1"
version  u32   format version (starts at 1)
hdr_len  u32
header   JSON (uncompressed, UTF-8): root, taken_at (RFC 3339), volume_id, scanner,
         dredge_version, node_count, allocated_total, case_insensitive, error totals,
         error samples, volume_total/used (if root is a volume root)
body     zstd frame (content checksum on): name arena, then each column as raw
         little-endian bytes, then side tables
```

- The header is uncompressed so `dredge snapshots` lists entries without decompressing.
- Columnar layout compresses far better than row records.
- Unknown `version` → `snapshot_version_unsupported` error telling the user to rescan.
  No migrations in phase 1; the version field is there so a later phase can add them.
- Corrupt/truncated file → `snapshot_corrupt`, never a panic.
- Location: `--data-dir`, else `DREDGE_DATA_DIR`, else the OS local data dir
  (`%LOCALAPPDATA%\dredge`, `~/Library/Application Support/dredge`, `$XDG_DATA_HOME/dredge`).
  Filename: `<sanitized-root>-<fnv1a64-hex>.dredge`.
- **Lookup for queries:** the snapshots whose root is an ancestor of (or equal to) the
  query path are candidates; the **newest** wins. No covering snapshot → `no_snapshot`
  error. Queries never touch the live filesystem, so answers are deterministic and the
  snapshot's `age_secs` is the honest staleness signal.

## 7. Engine and queries

All queries take an `&Index` + path + options and return typed rows. `Engine` adds
snapshot resolution, envelope building, and the optional single-slot index cache (used by
MCP; the CLI loads once per process).

- `browse(path, limit)`: immediate children, size-sorted. O(limit) to produce rows.
  Summary includes entry count and bytes not shown, so percentages add up.
- `largest(path, ext?, min_bytes?, limit)`: files only, linear scan of the subtree slice
  with a bounded min-heap.
- `types(path, limit)`: aggregates files by lowercased extension (no dot; `""` reported
  as `(none)`), linear slice scan with a hash map.
- `detail(path)`: one node: sizes, counts, flags, mtime, hard-link info.
- `volumes()`: portable via `sysinfo` (disk feature only); no `cfg` needed.
- `snapshots()`: header-only listing.

Progressive disclosure is the contract: volumes → browse one level → largest/types →
detail. No query returns a tree. `limit` defaults to 10, is clamped to 1000, and every
list is sorted by allocated size descending.

## 8. CLI

`dredge <cmd>`; stdout is exactly one JSON document (compact by default, `--pretty` for
humans); progress and diagnostics go to stderr; failure is a nonzero exit **and** a JSON
`error` document on stdout so agents can parse failures. Exit codes: `0` ok, `1`
runtime error, `2` usage error (clap). `dredge schema` prints the JSON Schema generated
by `schemars` from the same Rust output types the engine returns, so it cannot drift from
the real output. Full shapes in `OUTPUT_SCHEMA.md`.

`dredge mcp [--idle-timeout-secs N]` runs the MCP server on stdio. In that mode stdout is
reserved for the protocol; all logging goes to stderr.

## 9. MCP server (`dredge-mcp`, rmcp, stdio)

Tools map 1:1 to Engine methods and return the same envelope as compact JSON text.

| Tool | Args | Description shown to the model |
|---|---|---|
| `disk_volumes` | none | "List volumes with used/free space. Start here to see which drive is full." |
| `disk_scan` | `path` | "Index a folder or drive (slow; returns job_id). Needed once before browse/largest/types/detail." |
| `disk_job_status` | `job_id`, `wait_seconds` (≤60) | "Check a scan job; set wait_seconds to wait instead of polling." |
| `disk_browse` | `path`, `limit` | "Largest entries directly inside a folder. Use to drill down one level at a time." |
| `disk_largest` | `path`, `ext`, `min_bytes`, `limit` | "Largest individual files anywhere under a folder. Use to find big files fast." |
| `disk_types` | `path`, `limit` | "Space used per file extension under a folder. Use to see what kind of data dominates." |
| `disk_detail` | `path` | "Everything known about one file or folder. Use after browse/largest on a candidate." |

- **Jobs:** `disk_scan` spawns the scan on a blocking thread and returns
  `{kind:"job", job_id, state:"running"}` immediately. `disk_job_status` long-polls up to
  `wait_seconds` (a tokio `watch` channel) and returns state, live counters (files, dirs,
  bytes, errors), and on completion one scan summary per root. The server stays
  responsive for other tool calls meanwhile. Jobs are in-memory and die with the process
  (snapshots persist); the last 8 are kept. One scan runs at a time per process; a second
  `disk_scan` returns the running job. `path` is optional: omitted, the job scans the
  focus roots in order.
- **Single loaded index:** the Engine cache holds at most one `Arc<Index>`. Loading a
  different snapshot evicts the current one. A timer unloads it after the idle timeout
  (default 120 s; `--idle-timeout-secs` or `DREDGE_MCP_IDLE_SECS`). An in-flight query
  holds its own `Arc`, so eviction never invalidates a running query. A scan under way
  builds its own transient index, so peak memory can briefly be one loaded + one building.
- **Annotations:** every tool sets `openWorldHint = false`. All tools except `disk_scan`
  set `readOnlyHint = true`. `disk_scan` writes dredge's own snapshot file in the data
  directory and nothing else, so it is annotated truthfully: `readOnlyHint = false`,
  `destructiveHint = false`, `idempotentHint = true`, and its description says what it
  writes. (Deviation from the earlier draft, which marked it read-only; a client that
  auto-approves read-only tools will therefore ask before the first scan.)
- **Concurrent processes:** each agent session runs its own MCP process, so several may
  share one data directory. Snapshot writes are atomic (temp file named with the pid, then
  rename), reads see either the old or the new snapshot, and each process reloads when it
  sees a newer `taken_at` for the snapshot it has cached.

### Focus roots

`DREDGE_FOCUS_ROOTS` (a platform path list: `;` on Windows, `:` elsewhere; nothing is
hard-coded) names the roots agents mostly work in, e.g. `C:\server;C:\`. It changes three
things: `volumes` reports each root's scan state and snapshot age (`summary.focus`);
`disk_scan` / `dredge scan` with no path scans them in order; and the MCP server
instructions point agents at `disk_volumes` first. Queries are not restricted to focus
roots, and the newest snapshot that covers a path answers it (so scanning `C:\server`
after `C:\` gives that subtree fresher data without a full rescan).

## 10. Extension points (interfaces only)

- **`MftScanner`** (NTFS raw MFT, needs admin): implement `Scanner`, feed `IndexBuilder`.
  Core and all frontends are untouched.
- **Cleanup rules** (`rules/*.toml`): the directory exists, empty. Rules will be a
  read-only matcher over `Index` returning candidate nodes plus a reason. Nothing is
  parsed or executed in phase 1.
- **TUI / HTML report / GUI:** new frontends over `Engine`. Because the Engine returns
  typed envelopes, they need no new query logic.
- **Deletion:** a future `Cleaner` would live in its own crate and consume candidates;
  `dredge-core` stays read-only forever.

## 11. Testing, CI, benchmarks

- Fixture builder creates trees under `tempfile::TempDir` only: nested dirs, hard links,
  symlinks, empty files, unicode names, very deep paths (> 260 chars on Windows), and
  permission-denied dirs (`chmod 000` on Unix; ACL deny via `icacls` on Windows, in a
  `cfg(windows)` test). Nothing outside the temp dir is touched.
- Windows-only tests (`cfg(windows)`): allocated size from `AllocationSize`, NTFS hard
  link dedupe, junction not followed, deep paths via `\\?\`.
- Property/invariant tests on any scanned fixture: dir total = sum of children; hard link
  groups counted once; `subtree_end` nesting is well-formed; snapshot round-trip yields a
  byte-identical index; CLI and MCP outputs identical.
- Cloud placeholders can't be fabricated in CI. Placeholder flag detection is unit-tested
  on attribute bit patterns; the "scan does not hydrate" claim is verified manually on
  the user's real OneDrive folder before phase 1 is signed off (check that file sizes on
  disk and OneDrive status icons don't change).
- Benchmark: `crates/dredge-scan/examples/bench_scan.rs` scans a path and prints
  files/sec, nodes, elapsed, and peak memory (Windows `PeakWorkingSetSize`, Unix
  `getrusage`). Baselines recorded in `docs/BENCHMARKS.md`. No optimizing before that.
- CI: GitHub Actions, matrix `windows-latest`, `macos-latest`, `ubuntu-latest`; steps:
  `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.

## 12. Dependencies (all via `cargo add`; each gets a one-line Cargo.toml justification)

| Crate | Where | Why |
|---|---|---|
| `serde`, `serde_json` | core, cli, mcp | output envelope, snapshot header |
| `schemars` | core | generate `dredge schema` from the real types |
| `thiserror` | libs | typed library errors |
| `anyhow` | cli only | binary-level error plumbing |
| `clap` (derive) | cli | argument parsing |
| `rayon` | scan | work-stealing directory tasks |
| `zstd` | core | snapshot compression (C library, builds on all three CI OSes) |
| `windows-sys` | scan (target-gated) | directory enumeration, attributes, volume serial |
| `libc` | scan (target-gated) | `getrusage` in bench; unix helpers if std lacks them |
| `sysinfo` (disk only) | core | cross-platform volume listing |
| `dirs` | core | OS data directory |
| `rustc-hash` | core | fast hasher for name interning and file-id grouping |
| `jiff` | core | RFC 3339 timestamps and ages |
| `rmcp` | mcp | official Rust MCP SDK |
| `tokio` | mcp | async runtime required by rmcp, jobs, timers |
| `tempfile` | dev | fixture trees |

Snapshot filename hash is a 10-line inline FNV-1a, not a dependency.

## 13. Decisions

- **D1 ⚑ Store `mtime` per node** (u32 unix seconds, +4 B/node). The plan's record list
  omits it, but "is this safe to clean up?" depends heavily on age, and adding a column
  later is a snapshot-format change. Shown in `detail` and `--verbose` only in phase 1.
  Easy to drop now; I'd keep it.
- **D2** Pre-order + size-sorted siblings instead of CSR child arrays (section 3).
- **D3** `Scanner` trait lives in `dredge-core`, not `dredge-scan` (dependency direction).
- **D4** Windows enumeration uses `FileIdExtdDirectoryInfo`, not `FindFirstFileExW`
  (section 5).
- **D5** Hard link owner = smallest path; aliases zeroed and flagged. Deterministic across
  runs and thread counts.
- **D6** Snapshots are one-per-root, newest-covering-snapshot wins for queries, and
  queries never read the live filesystem.
- **D7** Errors are also emitted as a JSON document on stdout (plus stderr text) so agents
  get structured failures; exit code is still nonzero.
- **D8** JSON is compact by default (agent context is expensive); `--pretty` opts in.
- **D9** Names are stored as lossy UTF-8 with a `name_lossy` flag. Such entries can be
  reported accurately but not addressed by path in a query. Rare; revisit with WTF-8 if
  it ever matters.
- **D10** Byte counts are JSON numbers (u64). Exact up to 2^53 (≈ 9 PB), which is beyond
  any single volume this tool will see.
- **D11** No cancellation tool in MCP phase 1 (not in the plan). The scanner supports
  cancellation internally, but nothing exposes it yet (no CLI Ctrl-C handler, no MCP
  tool); adding either later is additive.
- **D12** Human sizes use binary units (KiB, MiB, GiB), one decimal.
- **D13** `Engine` takes the scanner as an argument to `scan()`, so core never depends on
  `dredge-scan`.
