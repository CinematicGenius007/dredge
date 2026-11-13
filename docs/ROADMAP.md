# dredge roadmap

Status: **approved plan**. Phases ship in order; each phase ends with fmt, clippy `-D warnings`
and test clean on the host, and CI green on Windows, macOS and Linux.

Rules that hold for every phase (from `AGENTS.md`): one engine and thin frontends, read-only,
local only, compact output, `schema_version` bump plus `docs/OUTPUT_SCHEMA.md` in the same
commit as any output change.

## Phase 0: contribution foundation (docs only)

Deliver:
- `README.md`: what it is, install, CLI examples, MCP registration, the read-only promise.
- `CONTRIBUTING.md`: ground rules (below), dev loop, how to add a query, how to add a category rule.
- `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), `SECURITY.md` (read-only guarantee is the
  security boundary; private reporting).
- `.github/`: issue forms (bug, feature, category-rule), PR template with the checklist,
  `CODEOWNERS`.
- `docs/RFCS/README.md`: when an RFC is needed (schema change, new query, new dependency class).
- `AGENTS.md`: move `dredge bar` and the TUI into scope; keep read-only; document the suggest-only
  policy.
- Remove the empty `rules/` directory or give it a purpose.

Ground rules (go into `CONTRIBUTING.md`):
1. fmt, clippy, test green on all three OSes.
2. Read-only is a security boundary: no writes outside the data dir, no following links, no
   hydrating cloud files, no network.
3. New query = Query + record type + CLI + MCP + schema doc + CLI/MCP parity test.
4. Output change = RFC issue first, then a `schema_version` bump.
5. New dependency = `cargo add` plus a one-line justification; TUI and terminal deps never enter
   `dredge-core`.
6. Tests use `tempfile` only and never assert host block sizes.
7. `unsafe` only in `dredge-scan/src/platform/`, each block with `// SAFETY:`.
8. Conventional commits, one concern per PR.
9. AI-assisted PRs welcome; author must understand and have run the change, and disclose the tool.
10. Category rules and cleanup hints need a source link and a test. A wrong "safe to remove" hint
    does real harm.
11. DCO: `git commit -s` adds a `Signed-off-by:` line. It is free text, not code signing and not
    a paid certificate.

Done when: every file above exists, links resolve, `AGENTS.md` and this file agree.

## Phase 1: cheap categorization and cleanup hints (core, CLI, MCP)

Categories and cleanup hints are available through the CLI and MCP.

Goal: an agent calling the MCP gets "what kind of space is this" in one cheap call, with no file
content reads and no model needed.

Deliver:
- `dredge-core/src/categories.rs`: rule table as data. Categories: Apps, Developer, Photos,
  Movies, Music, Documents, Caches, Trash, System, Other. Rules match on extension and on path
  globs, per OS (macOS `~/Library/Caches`, `~/.Trash`, Xcode simulators; Windows
  `%LOCALAPPDATA%\Temp`, Recycle Bin; Linux `~/.cache` (the `$XDG_CACHE_HOME` default), `~/.local/share/Trash`; everywhere
  `node_modules`, `target`, `.gradle`, `.cargo/registry`, `.npm`, `__pycache__`). First match wins,
  most specific path rule before extension rule.
- Classification is one linear pass over the index (pre-order layout), cached per snapshot.
- `Query::Categories` -> `CategoryRecord { category, allocated_bytes, human, share, files, top }`
  where `top` is up to 3 of the largest paths wholly in the category, none nested in another.
- `Query::Insights` -> `InsightRecord { id, category, path, allocated_bytes, human, message,
  suggested_command }`. Hints only; `suggested_command` is text for the user or agent to run
  themselves. dredge never runs it.
- Optional `category` field on `browse` and `largest` rows behind `--verbose`.
- CLI: `dredge categories <path>`, `dredge insights <path>`. MCP tools with the same arguments.
- Schema v2 plus `docs/OUTPUT_SCHEMA.md`.
- Tests: fixture trees in `tempfile`; assert relationships (a `node_modules` tree lands in
  Developer, sums of categories equal the root total, CLI and MCP output byte-identical).

Done when: `dredge categories ~ --pretty` and the MCP tool return identical JSON, category sums
reconcile to the snapshot root total, and every rule has a source comment and a test.

## Phase 2: dithered bar (`dredge bar`) - done

Shipped. Goldens are plain string files under `crates/dredge-render/tests/golden/` instead of
`insta` (no new dependency).

Goal: the stippled, per-category stacked bar from the reference screenshot, as plain terminal
output.

Deliver:
- New crate `dredge-render` (no tty or async deps, depends on `dredge-core` output types only).
  Pure function `render_bar(segments, width, focus, caps) -> Vec<Line>`.
- Look: each segment is a tinted ground with a regular dot grid in the category ink. Ordered
  dithering (Bayer 4x4) picks which cells carry a dot. Density per state: focused segment ~1.0,
  others ~0.2, free space ~0.06 neutral. Two pixel rows per cell via `▀` (fg/bg colors); a
  braille variant for terminals that render half blocks poorly.
- Color tiers: truecolor, 256, 16, and none (`NO_COLOR` or non-TTY falls back to plain
  `Apps 52.6 GB  ###...` text). Never emit escapes when stdout is not a TTY unless `--color always`.
- Legend grid under the bar: icon-free (ASCII or single glyph), name, size, same ink as the bar.
- `dredge bar <path>` prints header (used of total), bar, legend, and the top insight line.
- Tests: golden snapshots with `insta` per color tier, width 40 and 100, focus on and off.

Done when: output is deterministic (same input, same bytes), degrades cleanly on all four tiers,
and the render crate has no I/O.

## Phase 3: `dredge tui` - done

Deliver:
- New crate `dredge-tui` (`ratatui`, `crossterm`), feature-gated in `dredge-cli` so the MCP-only
  build stays small. Reuses `dredge-render` for the bar.
- Header (used/total, snapshot age), bar, legend grid, detail pane for the focused category.
- Keys: arrows move focus, Enter drills into `browse` for the category's top paths, Backspace
  goes up, `i` shows insights with the suggested command, `y` copies that command to the
  clipboard text (printed, never executed), `r` rescans with `ScanProgress`, `q` quits.
- Rescan runs at low priority; the bar repaints only on change and stops once the scan settles.
- All data comes from `Engine`; the TUI has no query logic.
- Tests: pure state-machine unit tests (key -> state), render-to-buffer tests with ratatui's
  `TestBackend`.

Done when: a full scan to browse to insights loop works on the host with no panics on resize,
tiny terminals, or Ctrl-C, and terminal state is always restored.

## Phase 4: release and distribution — implemented

Implemented: hand-written tag-triggered GitHub release workflow, `cargo-deny` and `cargo-audit`
in CI, a gated `bench_scan` regression floor, shell completions, `dredge doctor`, and install docs
with Homebrew/Scoop/winget templates. `publish = false` remains pending an owner decision.

The release workflow covers Windows x64, macOS x64/arm64, and Linux x64. Linux arm64 is skipped:
there is no native runner in the current simple matrix and introducing cross-compilation would add
reliability risk. The workflow has not run locally or on a tag; the first tagged CI release remains
the operational confirmation.

## Backlog (not scheduled)

`dredge diff` between snapshots (what grew), `--older-than` and mtime filters, `--exclude` and
`.dredgeignore`, ncdu-compatible export, NTFS MFT backend, USN/FSEvents incremental rescan,
snapshot parser fuzzing, MCP progress notifications and resources, treemap HTML.

## Execution

Sequential, one phase at a time, each handed to a fresh agent with only its phase section as the
brief. Work lands uncommitted in the working tree; each phase is reviewed by a second model
before the next starts.

| Phase | Implementer | Reviewer |
|---|---|---|
| 0 docs | Claude Haiku 5.5 (drafts) | Claude Sonnet 5.5 |
| 1 core | Codex GPT-6.1 Terra, high | Claude Opus 5.5 |
| 2 render | Codex GPT-6 Luna for rules/tests, Claude Sonnet 5.5 for the dither | Codex GPT-6.1 Terra |
| 3 tui | Claude Sonnet 5.5 | Codex GPT-6.1 Terra |
| 4 release | Codex GPT-6 Luna | Claude Sonnet 5.5 |
