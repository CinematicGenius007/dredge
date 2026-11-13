# dredge

dredge shows what is using your disk space. It is a read-only disk-space explorer with two
front ends over one engine: a JSON command-line tool for people and scripts, and a stdio MCP
server for AI agents.

Supported platforms: Windows, macOS and Linux. Windows is the primary development platform;
macOS and Linux run the same code. CI builds, lints and tests on all three.

Status: early development. There is no release yet, so install from source.

## Read-only

- dredge writes, and on request removes, only its own snapshot files, in `DREDGE_DATA_DIR` or
  the OS local data directory.
- It never deletes, moves or modifies any other file.
- It does not follow symlinks or junctions, and it does not download cloud placeholders.
- It makes no network connections and sends no telemetry.
- Sizes are allocated bytes (space on disk). Hard links count once.

Read [SECURITY.md](SECURITY.md) for what counts as a security issue.

## Install

Install from a clone of this repository with Cargo. Prebuilt GitHub Releases and package-manager templates are documented, but no release or package-manager listing is published yet:

```bash
cargo install --path crates/dredge-cli
dredge --version
```

[docs/INSTALL.md](docs/INSTALL.md) has the full steps for each OS, the Windows upgrade
procedure, MCP registration, and snapshot locations.

## Usage

Queries read saved snapshots, so run `dredge scan` on a path first.

```bash
dredge scan ~/Downloads              # scan and save a snapshot
dredge volumes                       # volumes, free space, snapshot age
dredge browse ~ --limit 5            # children of a path, largest first
dredge largest ~ --ext log --min-bytes 1048576 --limit 10
dredge types ~ --limit 10            # space by file extension
dredge categories ~                  # space by kind: developer, caches, photos, ...
dredge insights ~                    # regenerable space, with a command to review (never run)
dredge detail ~/Library              # everything known about one path
dredge bar ~ --focus developer       # dithered space-by-category bar (text, not JSON)
dredge --pretty browse ~             # indented JSON
```

Other commands: `dredge snapshots` lists saved snapshots, `dredge doctor` checks snapshot and environment health, `dredge completions bash` prints shell completions, and `dredge schema` prints the JSON
Schema of every output document.

### Space used by dredge itself

Snapshots take about 6 bytes per scanned file or folder on disk, so a home directory of 8 million
entries is close to 50 MB. `dredge snapshots` shows the total and marks snapshots no query will
use: `shadowed_by` names a newer snapshot that covers the same paths (you scanned `~/code/app`,
then `~`), and `root_exists: false` means the folder is gone.

```bash
dredge snapshots                     # each snapshot, the total, and what is prunable
dredge forget --prune --dry-run      # what --prune would remove; removes nothing
dredge forget --prune                # remove shadowed, deleted-root and unreadable snapshots
dredge forget ~/code/app             # remove the snapshot of one root
dredge forget --all                  # remove every snapshot
```

`forget` removes only files in the data directory that carry dredge's snapshot header (and temp
files from interrupted writes, older than an hour). A snapshot of a drive that is not plugged in
is kept by `--prune`. Set `DREDGE_MAX_DATA` (for example `500M`) to cap the data directory: after
each scan dredge removes unused snapshots, then the oldest, and never the one it just wrote. The
cap is off by default, and when set it applies to MCP `disk_scan` too. `forget` itself is not
available over MCP.

Global flags: `--pretty`, `--verbose` (every extra field on each row), and `--data-dir <DIR>`.
`dredge scan` also takes `--threads <N>` and `--quiet`.

`dredge bar` prints a human-readable bar instead of JSON. It uses color on a terminal and plain
text when piped or when `NO_COLOR` is set (`--color always|never` overrides, `--width N`):

```
$ dredge bar ~/proj --color never --width 40
/home/me/proj  4.2 GiB
#########################+++++++++++++++
Developer  3.1 GiB  ######..
Photos     1.1 GiB  ##......
```

`dredge tui [path]` opens an interactive view of the same data (needs a terminal on stdout; with no
path it uses the first of `DREDGE_FOCUS_ROOTS`, else the current directory). It never scans on its
own: with no snapshot it offers `r`. It is a default cargo feature; build the CLI with
`--no-default-features` to leave it out (`bar` and `mcp` stay).

| Key | Action |
|---|---|
| arrows or `h` `j` `k` `l` | move category focus (in lists: move the selection) |
| `Enter` | open: category, then its largest paths, then folders (`browse`) |
| `Esc` or `Backspace` | up one level, then back to the categories |
| `i` | toggle cleanup insights with the suggested command |
| `y` | copy the selected insight's command to the clipboard (OSC 52); never run |
| `r` | rescan the root in the background at low priority |
| `?` | help |
| `q` or `Ctrl-C` | quit |

Other output is one JSON document on stdout. Progress and error text go to stderr. A failed command
exits nonzero and prints a document with `kind: "error"`. The shape of each document is in
[docs/OUTPUT_SCHEMA.md](docs/OUTPUT_SCHEMA.md). Lists are sorted by allocated size, largest
first, and default to 10 rows.

## MCP server

`dredge mcp` runs a read-only MCP server on stdin and stdout. Its tools are `disk_volumes`,
`disk_scan`, `disk_job_status`, `disk_browse`, `disk_largest`, `disk_types`, `disk_categories`,
`disk_insights`, `disk_detail` and `disk_doctor`.
`disk_scan` runs a scan in the background, and `disk_job_status` reports on it. To register the
server with an agent client, see [docs/INSTALL.md](docs/INSTALL.md).

## Configuration

| Variable | Purpose |
|---|---|
| `DREDGE_FOCUS_ROOTS` | Roots you mostly work in, as a path list (`;` on Windows, `:` elsewhere). `dredge scan` with no path scans them. |
| `DREDGE_DATA_DIR` | Snapshot directory. |
| `DREDGE_MAX_DATA` | Optional size cap for the snapshot directory, such as `500M` or `2G` (binary units). Off by default. |
| `DREDGE_MCP_IDLE_SECS` | Seconds before the MCP server drops its loaded index (default 120). |

## Documentation

- [docs/ROADMAP.md](docs/ROADMAP.md): phases and what each one delivers.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): crates, data flow, and design decisions.
- [docs/OUTPUT_SCHEMA.md](docs/OUTPUT_SCHEMA.md): the output contract.
- [docs/BENCHMARKS.md](docs/BENCHMARKS.md): scan speed and memory.
- [docs/INSTALL.md](docs/INSTALL.md): install, upgrade and MCP registration.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md). Report vulnerabilities
privately, as described in [SECURITY.md](SECURITY.md). Participation is governed by the
[Code of Conduct](CODE_OF_CONDUCT.md).

## License

MIT. See [LICENSE](LICENSE).
