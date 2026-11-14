# RFC 0003: A readable view on the terminal

Status: Accepted

## Summary

When stdout is a terminal, data commands print a short readable view instead of JSON: the answer
first, one table, the snapshot's age and the next command to run. Pipes, redirects, `--json` and
`--pretty` still get the JSON document, byte for byte. The TUI overview and scan screens are
redesigned under the same rules. The rules live in [docs/UX.md](../UX.md).

## Motivation

A person who types `dredge browse ~` reads a single line of JSON. They have to pipe it through
`jq` or add `--pretty` to answer "what is big here?", and still convert bytes in their head.
Each command also leaves them guessing what to run next. The TUI showed a bar but not what was
inside a category or what could be cleared, and its scan screen showed a percentage it could not
know.

## Design

- The CLI renders the human view only when stdout is a terminal and neither `--json` (new global
  flag) nor `--pretty` is given. Errors go to stderr as `✗ message` and a fix line.
- `crates/dredge-render/src/human.rs` renders each `Output` kind from the same records the JSON
  carries. It is pure (strings in, lines out) and does no queries.
- The eight rules and the per-command skeleton are in docs/UX.md. A test
  (`crates/dredge-render/tests/human_every_command.rs`) runs every command through a real engine
  at 100, 72 and 50 columns and checks the skeleton.
- TUI: a header with the volume, file system and snapshot age (amber when older than 7 days); totals; the
  category bar with a pointer under the focused segment; the focused category's largest paths;
  up to five cleanup hints, where `c` copies the first one's command; and an honest scan screen
  that shows a percentage only against the volume's used space, and `Esc` to cancel (nothing is saved).
- `Engine::sizes_within(root, paths)` reads sizes of paths from one root's snapshot, so the
  overview does not load other snapshots. Library only: no query, no CLI or MCP surface.

## Output change

None to the contract. JSON documents, kinds and fields are unchanged and `schema_version` stays
4. Anything that reads dredge's output through a pipe sees exactly what it saw before. A script
that ran dredge with stdout on a real terminal and scraped the screen would now see the human
view; `--json` restores JSON there.

## Read-only and local impact

None. No new files, no network. The TUI copies a command to the clipboard (OSC 52) as before
and never runs it.

## Alternatives

- **Keep JSON everywhere and point people at `--pretty`.** Indented JSON is still not an answer.
- **A `--human` flag.** Every person would need to know it; a TTY check is what `ls`, `git`
  and `cargo` do, and pipes stay safe.
- **Render the human view in each CLI subcommand.** It would drift; one renderer over the output
  types keeps one look.

## Test plan

- `human_every_command.rs`: every command at three widths, plain and truecolor; leads; errors.
- Renderer unit tests in `human.rs` for path fitting, the lead, ages and widths.
- TUI tests: the 80x24 overview matches the board, the pointer, hints, stale header, the scan
  screen, `Esc` cancel, and `c` copy.
- `Engine::sizes_within` reads the root's snapshot and returns `None` for unknown paths.
