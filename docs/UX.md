# dredge terminal UX

How every dredge command looks on a terminal. The JSON documents are the contract
([OUTPUT_SCHEMA.md](OUTPUT_SCHEMA.md)); this file is the contract for the human view and the TUI.
The designs are drawn in the project's Paper file (boards 01 to 12). RFC 0003 records the
decision.

## When the human view is used

stdout is a terminal, and neither `--json` nor `--pretty` was given. Anything else (a pipe, a
redirect, `--json`) gets the JSON document byte for byte, so scripts never see this view.
Errors go to stderr in either mode, with a nonzero exit.

## The eight rules

1. **Lead with the answer.** The first line says what the command found, in a sentence:
   `~/Development uses 42.1 GiB`, not a header row.
2. **One grid.** Sizes are right-aligned on the left, then the share, a 10-cell bar, then the path. Every
   table in every command uses the same columns, so the eye learns one shape.
3. **Paths are readable.** The parent is faint and the leaf is bright. The home directory is `~`.
   A long path is cut in the middle (`~/…/node_modules/`), never in the leaf. Folders end in `/`.
4. **Color means one thing.** Orange `◆` is a cleanup hint (reclaimable space). Amber `!` is a
   warning (stale data, unreadable folders). Red `✗` is an error, green `✓` a finished scan. Category colors appear only in
   the bar. Nothing else is colored. With `NO_COLOR` or a dumb terminal, the glyphs still carry
   the meaning.
5. **Say how old the data is.** Every query footer gives the snapshot's age. Older than 7 days,
   it turns amber with `!`.
6. **End with the next command.** The last line is `Next  dredge …`: the command a person
   would most likely run next, with the real path filled in. dredge prints it; it never runs it.
7. **Pipes get JSON.** See above. Never parse the human view.
8. **Fit the terminal.** Lines fit `$COLUMNS` (else the terminal width, else 80). Under 60
   columns the bar column is dropped and the lead may wrap onto an indented second line.
   Commands are the one exception: a `Next` line or a suggested `$ command` is never cut, so it
   stays pasteable even if it wraps.

## The skeleton

Every data command prints, in order:

```
<lead sentence>                                   rule 1

      SIZE   SHARE              NAME              faint header
   3.1 GiB   73.8%  ━━━━━━━───  node_modules/     rules 2, 3 (up to the limit)
   …
  + N more                                        when the list was cut

◆ <one cleanup hint, if any>                      at most one

snapshot 3 hours old · <optional note>           rule 5
Next  dredge <command> <path>  <why>              rule 6
      dredge <another command>  <why>             optional second suggestion
```

Errors print two lines on stderr: `✗ what failed` and the fix (`run dredge scan ~/x first`).

## Coverage

Each command has a human renderer in `crates/dredge-render/src/human.rs`, and
`crates/dredge-render/tests/human_every_command.rs` checks every one against this file at 100, 72
and 50 columns: the first line is the answer, no line overflows (commands excepted), no escape
codes without color, a `Next` line is present, and errors carry a fix.

| Command | Lead (example) | Body | Next |
| :- | :- | :- | :- |
| `scan` | `✓ 6.8 GiB in ~/x/ · 29,045 files · 812 folders · 3.1s` | none | `dredge categories <root>`, `dredge tui <root>` |
| `volumes` | `357.6 GiB free on Macintosh HD · 61% used · 2 volumes` | volumes, with snapshot age | `dredge scan <mount>` or `dredge categories <mount>` |
| `snapshots` | `1.1 MiB in 2 snapshots · <data dir>` | snapshots, prunable marked | `dredge forget --prune --dry-run` when any are prunable |
| `doctor` | `8 ok · 2 to look at` | one row per check | the first warning's fix |
| `browse` | `6.8 GiB in ~/x/ · 17 items, largest first` | children | `dredge browse <largest folder>` |
| `largest` | `6.8 GiB in 29,045 files under ~/x/` | files | `dredge detail <first>` |
| `types` | `1.8 GiB in .rlib files, 27.2% of ~/x/ · 28 types` | extensions | `dredge largest <path> --ext <top>` |
| `categories` | `6.8 GiB in ~/x/ · 3 categories` | categories | `dredge insights <path>` |
| `insights` | reclaimable total, or `Nothing to suggest under ~/x/` | hints with `◆` and `$ command` | `dredge detail <first path>`, else `dredge categories <path>` |
| `detail` | `708.0 KiB · folder` | facts (path, modified, category, …) | `dredge browse <path>` |
| `forget` | `1.8 KiB would be freed · 1 file · dry run, nothing removed` | files with reason | `dredge snapshots` |

`bar`, `tui`, `schema`, `completions` and `mcp` are not data commands and keep their own output.

A new data command adds a renderer, a row in this table, and a case in `human_every_command.rs`.

## The TUI (`dredge tui`)

The same rules, on one screen. Layout at 80x24, top to bottom:

- **Header:** `dredge › <root>` on the left; on the right the volume label, file system and
  `scanned X ago` (amber `! scanned N days ago` when stale), or `scanning · <progress>`.
- **Totals:** `X used of T (P%) · F free`, or `X in this folder` when the root is not a volume.
- **Bar:** space by category. A pointer under the bar (`└ Developer 12.3 GiB` or `… ┘`) names
  the focused segment, and a legend follows.
- **Inside <category>:** the focused category's largest paths, numbered, each with a mini bar,
  size, path, and `◆ reclaimable` when a hint covers it.
- **Cleanup hints:** up to five, path first and the reason faint, with `c copy the first` and
  `i all N` on the right. On a short terminal this collapses to one line.
- **Footer:** the keys that work on this screen; `? help  q quit` is always kept.

The scan screen is honest about what it knows: bytes read so far, files, folders, elapsed time
and rate, unreadable folders in amber, and a percentage only when the volume's used space gives a
real denominator (capped at 99%, labelled "measured against the volume's used space"). Otherwise it
says that a folder's size is known only once it has been read. While a scan runs, the previous
snapshot stays in use.

| Key | Action |
| :- | :- |
| arrows or `h` `j` `k` `l` | move category focus; in lists, move the selection |
| `Enter` | open: category, then its largest paths, then folders |
| `Esc` or `Backspace` | up one level; during a scan, cancel it (the last snapshot is unchanged) |
| `c` | copy the top cleanup hint's command (OSC 52); never run |
| `i` | all cleanup hints |
| `y` | copy the selected hint's command |
| `/` `g` | filter, jump to a path |
| `r` | rescan in the background at low priority |
| `?` | help |
| `q` or `Ctrl-C` | quit |
