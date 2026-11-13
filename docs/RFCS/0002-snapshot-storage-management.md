# RFC 0002: Managing the space dredge itself uses

Status: Accepted

## Summary

Say how much space dredge's own snapshots take and which of them no query can ever use, and add
`dredge forget` to remove dredge's own snapshot files: by root, all of them, or only the dead
ones (`--prune`). Clean up temp files left by crashed writes. An optional size cap,
`DREDGE_MAX_DATA`, evicts snapshots after a scan. Everything here touches only files inside
the data directory that dredge itself wrote.

## Motivation

A disk-space tool should not be the thing filling the disk. Today:

- Snapshots are about 6 bytes per scanned node on disk. A home directory of 8M nodes is close
  to 50 MB, and every scanned root keeps its own file forever.
- `dredge snapshots` lists files one by one but gives no total, and nothing marks a snapshot
  that is dead weight.
- A snapshot nested inside a newer snapshot of an ancestor (scan `~/code/app`, later scan `~`)
  is never read again: lookup always takes the newest covering snapshot. It is "shadowed".
- A snapshot whose root no longer exists (a deleted project, an unplugged drive's mount path)
  answers nothing useful.
- A crash between writing `<file>.dredge.<pid>.tmp` and renaming it leaves the temp file
  behind. Nothing removes it.
- There is no command to delete a snapshot. The only way is to find the data directory and
  `rm` files by hand, which is exactly the kind of guesswork dredge exists to remove.

## Design

### 1. `dredge snapshots` says what it costs

Each record gains:

| Field | Meaning |
| :- | :- |
| `root_exists` | `false` when the snapshot's root path is gone. One `stat`; never hydrates a cloud placeholder. |
| `shadowed_by` | Root of a newer snapshot that covers this one's whole root, so no query uses this one. Absent otherwise. |

The summary gains `file_bytes` and `human` (all of dredge's files in the data directory,
including unreadable snapshots and stale temp files), `prunable` and `prunable_bytes`
(what `dredge forget --prune` would remove now), and `temp_files` and `temp_bytes` (stale temp
files, see 4).

### 2. `dredge forget`

```
dredge forget <root>...        remove the snapshot of each root (exact root, not nested ones)
dredge forget --prune          remove shadowed, root-missing, unreadable and stale temp files
dredge forget --all            remove every snapshot and stale temp file
             [--dry-run]       report what would be removed, remove nothing
```

New kind `forget`. One record per file:

```json
{ "root": "/Users/me/code/app", "file": "/…/dredge/Users-me-code-app-…dredge",
  "file_bytes": 812340, "reason": "shadowed", "shadowed_by": "/Users/me", "removed": true }
```

`reason` is one of `requested`, `all`, `shadowed`, `root_missing`, `unsupported_format`,
`corrupt`, `temp_file`. `root_missing` means the root is gone and its parent folder still exists;
a snapshot of a whole volume, or of a folder whose parent is gone too (a drive that is not plugged
in), is never pruned for a missing root. `root` is absent when the file's header cannot be read. `error`
appears (and `removed` is `false`) when the file could not be removed.

Summary: `{ "removed": 1, "freed_bytes": 812340, "dry_run": false, "remaining": 3,
"data_dir": "…" }`.

A root given by path that has no snapshot of its own is an error (`invalid_argument`, naming
the snapshot that covers it if one does) and nothing is removed, so a typo never removes half of
a list.

**Safety rules.** A file is removed only when all hold:

1. It is a direct child of the data directory (no recursion, no `..`).
2. `symlink_metadata` says it is a regular file (a symlink in the data directory is never
   followed or removed).
3. Its name ends in `.dredge` and its first 8 bytes are the dredge snapshot magic, or its
   name matches `*.dredge.<digits>.tmp` and it was last modified more than an hour ago.

A file with the `.dredge` extension that lacks the magic is someone else's file and is never
touched. `--prune` removes `unsupported_format` and `corrupt` files because no build that reads
the current format can use them; `doctor` already tells users to rescan those roots.

`forget` is CLI only. The MCP server stays read-only and gets no delete tool.

### 3. Shadowed, precisely

Snapshots are ordered newest first (ties by root, as `find_covering` does). Snapshot `s` is
shadowed by `t` when `t` comes before `s` in that order, `t`'s root covers `s`'s root, and
the roots differ. Every path under `s`'s root is then also under `t`'s root, and `find_covering`
returns `t` first, so `s` is never loaded. Rescanning `s`'s root makes it the newest again,
and it is no longer shadowed.

### 4. Stale temp files

`Store::save` removes `*.dredge.<digits>.tmp` files older than one hour before it writes. The
hour keeps clear of a temp file that another dredge process is writing right now (a write
touches the file continuously and takes seconds). `doctor` and `snapshots` report what is left.

### 5. `doctor`

New checks:

| Check | Status |
| :- | :- |
| `snapshot_storage` | `ok`: "N snapshot(s), X on disk". `warn` when `DREDGE_MAX_DATA` is set and exceeded, or is invalid. |
| `prunable` | `ok` when nothing is prunable, else `warn` with hint `dredge forget --prune`. |
| `temp_files` | `ok` when none, else `warn` with hint `dredge forget --prune`. |

### 6. Insight for the data directory

When the data directory lies inside the path `insights` is asked about and holds at least the
usual 100 MiB, an insight with id `dredge_snapshots` labels it: "dredge's own snapshots",
suggesting `dredge forget --prune` (text only, as for every insight). Its contents get no
second hint. The rule never suggests `rm` for the data directory.

### 7. Optional cap: `DREDGE_MAX_DATA`

Unset by default: nothing is evicted unless the user asks. When set to a size (`500M`, `2G`,
`1073741824`; suffixes `K`, `M`, `G`, `T` and `KiB`…`TiB` are binary), a scan that pushes the
data directory over the cap evicts, in order, shadowed snapshots, then root-missing ones, then
the oldest, never the snapshot just written, until the total fits. The `scan` summary gains
`evicted`, the roots removed (the file path for a file with no readable root), omitted when
empty. An invalid value fails the scan with
`invalid_argument` before walking. The cap applies to every scan, including an MCP `disk_scan`
(whose finished job reports `evicted` in its embedded scan summary): the user set it, and a cap
that only some scans honour would not be a cap.

## Output change

`schema_version` 3 → 4 for the new `forget` kind. Nothing is removed or renamed.

- `snapshots` records: `root_exists`, optional `shadowed_by`.
- `snapshots` summary: `file_bytes`, `prunable`, `prunable_bytes`, `temp_files`, `temp_bytes`.
- `scan` summary: optional `evicted`.
- `insights`: new id `dredge_snapshots`.
- `doctor`: new checks `snapshot_storage`, `prunable`, `temp_files`.
- New kind `forget`, as above.

RFC 0001's planned bump moves to 4 → 5.

## Read-only and local impact

dredge still writes and removes only its own files in the data directory, under the safety
rules in 2. It never deletes, moves or modifies anything else, and `insights` still only
suggests commands. README, AGENTS.md and CONTRIBUTING.md change "writes only its own snapshots"
to "writes and removes only its own snapshot files". No network.

## Alternatives

- **Automatic pruning on every scan.** Removing shadowed snapshots silently would surprise a
  user who scanned a folder on purpose. Reporting it, and offering `--prune` and an opt-in cap,
  keeps the user in charge.
- **An age-based TTL.** Age alone says nothing about use: a month-old snapshot of an archive
  drive is still the only one. A cap evicts by need instead.
- **Splicing nested scans into the parent** removes shadowing at the source, and is part of
  RFC 0001. It needs the v2 storage format; this RFC works with today's format and stays
  useful after it (missing roots, temp files, the cap).
- **An MCP `disk_forget` tool.** Agents would gain a delete tool, which breaks the read-only
  MCP promise. The CLI is enough.

## Test plan

- Store: shadowing (nested older is shadowed, nested newer is not, sibling is not, same-second
  ties follow lookup order), missing roots, temp files (young kept, old removed), the safety
  rules (symlink, file without magic, subdirectory, foreign extension never removed).
- Engine: `forget` by root, `--prune`, `--all`, `--dry-run` removes nothing, unknown root
  removes nothing and errors, cache dropped for a removed file, cap eviction order and that the
  new snapshot is kept, invalid cap fails before the walk.
- Insight: data directory inside a fixture tree gets `dredge_snapshots` and no nested hint.
- CLI: `forget` end to end against a temp data directory; `snapshots` totals; `schema_version`
  4 everywhere; `dredge schema` lists `forget`.
