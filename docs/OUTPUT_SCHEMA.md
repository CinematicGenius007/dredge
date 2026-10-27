# dredge output schema (version 1)

Status: **draft for approval**. `dredge schema` prints the machine-readable JSON Schema,
generated from the same Rust types the commands return. This file is the human guide.

## Conventions

- stdout is **exactly one JSON document**. Diagnostics and progress go to stderr.
- Compact by default; `--pretty` indents. MCP tools always return compact JSON.
- Nonzero exit on failure, and stdout then carries a `kind: "error"` document.
- All sizes are bytes (JSON numbers). **Allocated** = space on disk, the number to
  reason about. **Logical** = file length as apps see it.
- `human` = allocated bytes in binary units, one decimal (`"1.2 GiB"`).
- Timestamps are RFC 3339 UTC (`"2026-10-27T09:14:02Z"`). `age_secs` is seconds since the
  snapshot was taken.
- Paths are in the platform's native form (`C:\Users\me\x`, `/home/me/x`), never with a
  `\\?\` prefix. Path arguments accept `/` or `\` on Windows and match case-insensitively
  on Windows/macOS (exact match preferred).
- Every list is sorted by allocated size descending and takes `limit` (default 10,
  max 1000; larger values are clamped).
- Optional fields are **omitted**, never `null`, unless stated.

## Envelope

```json
{
  "schema_version": 1,
  "kind": "browse",
  "snapshot": { "root": "C:\\Users\\me", "taken_at": "2026-10-27T09:14:02Z", "age_secs": 5231 },
  "records": [ ... ],
  "summary": { ... }
}
```

| Field | Meaning |
|---|---|
| `schema_version` | Integer, bumped on breaking change. |
| `kind` | `volumes`, `scan`, `snapshots`, `browse`, `largest`, `types`, `detail`, `job`, `error`. |
| `snapshot` | Present for `browse`, `largest`, `types`, `detail`, `scan`. The snapshot the answer came from. `age_secs` is the staleness signal. Includes `scan_errors` when the scan hit problems (see below). |
| `records` | The rows. Shape depends on `kind`. Empty array when there are none. |
| `summary` | Aggregate facts about the query. Shape depends on `kind`. |

`snapshot.scan_errors` (only when nonzero): `{ "denied": 12, "locked": 3, "other": 0 }`.
Tells the reader that some directories were unreadable, so totals may be low. The denied
directories themselves appear in `browse` with `"flags": ["denied"]`.

## Row fields shared by `browse`, `largest`, `detail`

Default rows are compact. Always present:

| Field | Type | Meaning |
|---|---|---|
| `name` | string | Final path component. |
| `path` | string | Full path. |
| `kind` | string | `file`, `dir`, `symlink`, `junction`, `reparse`. Links are never followed. |
| `allocated_bytes` | int | On-disk size. For `dir`, the whole subtree. Hard-linked data counted once. |
| `human` | string | `allocated_bytes` formatted. |

Position share (one of, depending on kind):

| Field | Used in | Meaning |
|---|---|---|
| `pct_of_parent` | `browse`, `detail` | `allocated_bytes` as % of the parent directory's total. One decimal. |
| `pct_of_scope` | `largest`, `types` | % of the queried path's total. |

Present **only when relevant** (or always with `--verbose` / `verbose: true`):

| Field | Shown by default when | Meaning |
|---|---|---|
| `flags` | non-empty | Array, any of: `cloud_placeholder` (online-only, not hydrated), `denied` (could not read), `hardlink_alias` (counted at another path; this path is 0), `estimated` (platform couldn't report allocated size), `name_lossy`, `mount_point` (Unix: other filesystem, not entered). |
| `hard_links` | file has more than one link in the scan | Links seen within the scan root. |
| `logical_bytes` | allocated < ½ of logical and logical ≥ 1 MiB (cloud placeholder, sparse, compressed) | Apparent size. |
| `files` | `--verbose`; always in `detail` for dirs | Files in the subtree. |
| `dirs` | `--verbose`; always in `detail` for dirs | Directories in the subtree. |
| `modified` | `--verbose`; always in `detail` | Last write time (RFC 3339). |

## `kind: "volumes"`

`dredge volumes` / `disk_volumes`. No `snapshot` object. Records, one per mounted volume:

```json
{ "mount": "C:\\", "label": "OS", "fs": "NTFS",
  "total_bytes": 1023212785664, "used_bytes": 612403699712, "free_bytes": 410809085952,
  "used_pct": 59.9, "used_human": "570.4 GiB", "total_human": "953.0 GiB",
  "snapshot_age_secs": 5231 }
```

`label` omitted if empty. `snapshot_age_secs` present only if a snapshot rooted exactly at
the mount exists.

`summary`: `{ "volumes": 2, "data_dir": "C:\\Users\\me\\AppData\\Local\\dredge", "focus": [...] }`

`focus` is present only when `DREDGE_FOCUS_ROOTS` is set: one entry per configured root,
`{ "path": "C:\\work", "scanned": true, "snapshot_age_secs": 5231, "covered_by": "C:\\" }`.
`scanned` is true when any snapshot covers the path; `covered_by` names the covering
snapshot's root when it is not the focus root itself; `snapshot_age_secs` is omitted when
not scanned. This is how an agent learns, in one call, what is indexed and how stale it is.

## `kind: "scan"`

`dredge scan <path>`; also embedded in a finished `disk_job_status`.

```json
{
  "schema_version": 1, "kind": "scan",
  "snapshot": { "root": "C:\\Users\\me", "taken_at": "2026-10-27T09:14:02Z", "age_secs": 0 },
  "records": [],
  "summary": {
    "root": "C:\\Users\\me",
    "scanner": "walk",
    "files": 1204331, "dirs": 98210,
    "allocated_bytes": 183420186624, "human": "170.8 GiB",
    "logical_bytes": 179002331136,
    "elapsed_ms": 41230, "files_per_sec": 29210,
    "hard_link_groups": 4120, "reparse_points": 311, "cloud_placeholders": 8120,
    "errors": { "denied": 12, "locked": 3, "other": 0,
                "samples": ["C:\\Users\\me\\AppData\\Local\\Packages\\x"] },
    "snapshot_path": "C:\\Users\\me\\AppData\\Local\\dredge\\C--Users-me-9f3a1c0d7b21e455.dredge",
    "snapshot_bytes": 61203388,
    "volume_used_bytes": 612403699712, "unaccounted_bytes": 429000000000
  }
}
```

`elapsed_ms` is the wall-clock time of the whole scan (walk, index build and snapshot write),
and `files_per_sec` is `files` over that time, so both match what the caller waited. The walk
alone is faster; `bench_scan` measures that.

`errors.samples` holds at most 20 paths. `volume_used_bytes` and `unaccounted_bytes` appear
only when the root is a volume root: `unaccounted_bytes` is volume used space the scan
could not attribute (filesystem metadata, shadow copies, unreadable folders).

## `kind: "snapshots"`

`dredge snapshots`. No `snapshot` object. Header-only, cheap. Records:

```json
{ "root": "C:\\Users\\me", "taken_at": "2026-10-27T09:14:02Z", "age_secs": 5231,
  "nodes": 1302541, "allocated_bytes": 183420186624, "human": "170.8 GiB",
  "file_bytes": 61203388, "volume_id": "A4F2-91C0", "format_version": 1 }
```

`summary`: `{ "snapshots": 1, "data_dir": "C:\\Users\\me\\AppData\\Local\\dredge" }`

## `kind: "browse"`

`dredge browse <path> [--limit N]`: immediate children, largest first.

```json
{
  "schema_version": 1, "kind": "browse",
  "snapshot": { "root": "C:\\Users\\me", "taken_at": "2026-10-27T09:14:02Z", "age_secs": 5231 },
  "records": [
    { "name": "AppData", "path": "C:\\Users\\me\\AppData", "kind": "dir",
      "allocated_bytes": 98123456512, "human": "91.4 GiB", "pct_of_parent": 53.5 },
    { "name": "OneDrive", "path": "C:\\Users\\me\\OneDrive", "kind": "dir",
      "allocated_bytes": 4096, "human": "4.0 KiB", "pct_of_parent": 0.0,
      "flags": ["cloud_placeholder"], "logical_bytes": 42949672960 }
  ],
  "summary": {
    "path": "C:\\Users\\me", "allocated_bytes": 183420186624, "human": "170.8 GiB",
    "entries": 41, "shown": 10, "truncated": true,
    "remaining_entries": 31, "remaining_bytes": 2147483648
  }
}
```

`remaining_*` lets a reader confirm shown + remaining = total without a second call.
Browsing a file returns that single file as the only record.

## `kind: "largest"`

`dredge largest <path> [--ext EXT] [--min-bytes N] [--limit N]`: files only (never
directories), anywhere under `path`. `--ext` is case-insensitive, with or without the dot.

```json
{
  "schema_version": 1, "kind": "largest",
  "snapshot": { "root": "C:\\Users\\me", "taken_at": "2026-10-27T09:14:02Z", "age_secs": 5231 },
  "records": [
    { "name": "ubuntu.vhdx", "path": "C:\\Users\\me\\AppData\\Local\\wsl\\ubuntu.vhdx",
      "kind": "file", "allocated_bytes": 21474836480, "human": "20.0 GiB",
      "pct_of_scope": 11.7 }
  ],
  "summary": {
    "path": "C:\\Users\\me", "filters": { "ext": "vhdx", "min_bytes": 104857600 },
    "matched_files": 3, "matched_bytes": 26843545600,
    "shown": 3, "truncated": false
  }
}
```

`filters` lists only filters supplied. `matched_*` count every file passing the filters,
not just those shown.

## `kind: "types"`

`dredge types <path> [--limit N]`: files grouped by lowercased extension (no dot;
`"(none)"` for no extension).

```json
{
  "schema_version": 1, "kind": "types",
  "snapshot": { "root": "C:\\Users\\me", "taken_at": "2026-10-27T09:14:02Z", "age_secs": 5231 },
  "records": [
    { "ext": "vhdx", "files": 3, "allocated_bytes": 26843545600, "human": "25.0 GiB",
      "pct_of_scope": 14.6 }
  ],
  "summary": { "path": "C:\\Users\\me", "types": 412, "shown": 10, "truncated": true,
               "remaining_bytes": 9663676416 }
}
```

## `kind: "detail"`

`dredge detail <path>`: one item. Exactly one record. All extras that apply are included.

```json
{
  "schema_version": 1, "kind": "detail",
  "snapshot": { "root": "C:\\Users\\me", "taken_at": "2026-10-27T09:14:02Z", "age_secs": 5231 },
  "records": [
    { "name": "node_modules", "path": "C:\\Users\\me\\proj\\node_modules", "kind": "dir",
      "allocated_bytes": 1181116006, "human": "1.1 GiB", "pct_of_parent": 88.2,
      "logical_bytes": 1103872100, "files": 41022, "dirs": 5310,
      "modified": "2026-03-02T11:40:19Z" }
  ],
  "summary": { "path": "C:\\Users\\me\\proj\\node_modules", "parent": "C:\\Users\\me\\proj",
              "depth": 4 }
}
```

For a hard-linked file, the record has `hard_links` and, on an alias, flag
`hardlink_alias` with `allocated_bytes: 0`.

## `kind: "job"` (MCP only)

`disk_scan` returns immediately:

```json
{ "schema_version": 1, "kind": "job", "records": [],
  "summary": { "job_id": "scan-1", "state": "running", "roots": ["C:\\work", "C:\\"],
               "elapsed_ms": 3 } }
```

`disk_job_status(job_id, wait_seconds)` returns the same envelope; `state` is one of
`running`, `done`, `failed`. A job scans its `roots` in order. While running,
`summary.current_root` names the root being scanned and `summary.progress` is
`{ "files": 220113, "dirs": 18002, "allocated_bytes": 41000000000, "errors": 4 }` for that
root; `elapsed_ms` covers the whole job. `summary.results` holds one `kind: "scan"` summary
per finished root (omitted while empty). When `failed`, `summary.error` is the first error
object (below); roots that finished before or after the failure are still in `results`.
An unknown id is an `error` document with code `job_not_found`.

Only one scan runs at a time per server process: calling `disk_scan` while a job is running
returns that job (check `roots`) instead of starting another.

## `kind: "error"`

```json
{ "schema_version": 1, "kind": "error",
  "error": { "code": "no_snapshot",
             "message": "no snapshot covers C:\\Temp; run `dredge scan C:\\Temp` first",
             "hint": "dredge scan C:\\Temp" } }
```

`hint` is optional. Codes (stable, safe to branch on):

| Code | Meaning |
|---|---|
| `no_snapshot` | No snapshot covers the path. |
| `path_not_in_snapshot` | A snapshot covers it but the path isn't in the index (new since the scan, or never existed). |
| `invalid_argument` | Bad value (negative limit, unparseable size, empty path). |
| `scan_failed` | The scan root couldn't be read at all. |
| `snapshot_corrupt` | File unreadable or checksum mismatch; rescan. |
| `snapshot_version_unsupported` | Written by a different format version; rescan. |
| `job_not_found` | Unknown or expired `job_id`. |
| `io_error` | Other I/O failure (data dir not writable, etc.). |
| `internal` | Bug. Please report. |

CLI exit codes: `0` success, `1` any error document, `2` usage error from the argument
parser (no JSON document; clap's message goes to stderr).

## `dredge schema`

Prints one JSON Schema document describing the envelope and every record/summary type
above (`schema_version` 1). No snapshot needed. MCP does not expose it; the tool
descriptions carry what an agent needs.

## MCP tool inputs

| Tool | Input |
|---|---|
| `disk_volumes` | none |
| `disk_scan` | `path?` (string; omit to scan the configured focus roots) |
| `disk_job_status` | `job_id` (string), `wait_seconds` (int 0-60, default 0) |
| `disk_browse` | `path`, `limit` (default 10) |
| `disk_largest` | `path`, `ext?`, `min_bytes?`, `limit` (default 10) |
| `disk_types` | `path`, `limit` (default 10) |
| `disk_detail` | `path` |

Each tool returns the corresponding envelope as one compact JSON text block. `verbose`
is CLI-only; agents get the compact form. A relative `path` is resolved against the
server's (or CLI's) working directory, so an agent session can pass `.`; paths are
normalized lexically (`.`/`..` resolved, drive letter upper-cased on Windows).

On the CLI, `dredge scan` with several paths (or with none and `DREDGE_FOCUS_ROOTS` set)
prints one `scan` (or `error`) document per line.

## Stability

`schema_version` 1 promises: existing fields keep their names and meaning; new optional
fields may appear; consumers must ignore unknown fields. Removing or renaming a field, or
changing a unit, bumps `schema_version`.
