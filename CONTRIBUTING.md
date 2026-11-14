# Contributing to dredge

Read [AGENTS.md](AGENTS.md) (commands, layout, rules) and [docs/ROADMAP.md](docs/ROADMAP.md)
(phases) before you start. Each phase ends with fmt, clippy and test clean, and CI green on
Windows, macOS and Linux.

## Ground rules

1. fmt, clippy and test are green on all three OSes.
2. Read-only is a security boundary: no writes outside the data dir, no following links, no
   hydrating cloud files, no network.
3. A new query is a Query, a record type, a CLI subcommand, an MCP tool, a schema doc entry and
   a CLI/MCP parity test.
4. An output change needs an RFC issue first, then a `schema_version` bump.
5. A new dependency is added with `cargo add` plus a one-line justification comment in the root
   `Cargo.toml`. TUI and terminal dependencies never enter `dredge-core`.
6. Tests use `tempfile` only and never assert host block sizes.
7. `unsafe` appears only in `dredge-scan/src/platform/`, and each block has a `// SAFETY:`
   comment.
8. Conventional commits, one concern per PR.
9. AI-assisted PRs are welcome. The author must understand the change and have run it, and must
   disclose the tool used.
10. Category rules and cleanup hints need a source link and a test. A wrong "safe to remove"
    hint does real harm.
11. DCO: `git commit -s` adds a `Signed-off-by:` line. See [Sign-off](#sign-off-dco).

## Dev loop

```
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run --release -p dredge-scan --example bench_scan -- <path>   # files/sec + peak memory
```

Run all three of fmt, clippy and test before you commit. Run `cargo fmt --all` to format.
Run the benchmark when a change could affect scan speed or memory.

`dredge tui` is behind the `tui` cargo feature of `dredge-cli`, on by default. Anything in
`dredge-cli` that touches it needs `#[cfg(feature = "tui")]`, and the build without it must stay
clean and keep `bar` and `mcp`:

```
cargo clippy -p dredge-cli --no-default-features -- -D warnings
```

TUI logic goes in `crates/dredge-tui/src/state.rs` as a pure `update(state, event)` with a test;
`view.rs` and `runner.rs` stay thin, and every number comes from `Engine`.

## Adding a query

A query is an engine function that the CLI and MCP only translate to and from. Logic does not
go in the frontends (AGENTS.md, first rule).

1. Open an RFC issue (see [docs/RFCS/README.md](docs/RFCS/README.md)). A new query needs one.
2. Add a variant to `Query` in `crates/dredge-core/src/engine.rs`, and handle it in `Engine`.
3. Add the record type to `crates/dredge-core/src/output.rs`, with serde and schemars derives.
4. Add the CLI subcommand in `crates/dredge-cli/src/main.rs`. It parses arguments, calls
   `engine.run`, and prints.
5. Add the MCP tool in `crates/dredge-mcp/src/server.rs`. It translates arguments only.
   Add the human view in `crates/dredge-render/src/human.rs` following [docs/UX.md](docs/UX.md)
   (lead, table, age, Next), a row in its coverage table, and a case in
   `crates/dredge-render/tests/human_every_command.rs`.
6. Document the new kind and fields in `docs/OUTPUT_SCHEMA.md`, in the same commit. Changing an
   existing field is a `schema_version` change; say in the RFC whether the version moves.
7. Add tests with fixture trees in `tempfile`. Assert relationships, not exact sizes. Add a
   parity test that the CLI and MCP outputs are identical for the same input.
8. Run the dev loop.

## Adding a category rule

Category rules (`RULES`) and cleanup hints (`HINTS`) are static tables in
`crates/dredge-core/src/categories.rs`, matched on names and path components only.

1. Open a category-rule issue (the issue form asks for the source). Rules and hints need a
   source link: vendor documentation, an OS convention, or the documentation of the tool that
   creates the files.
2. Add the rule as data in the table: category, OS scope, a `source` URL, and a matcher:
   `Extension`, `DirName`, `PathSuffix` or `RootPath` (`*` matches one path component).
3. Path and directory rules go before extension rules. The first match wins, and a matched
   directory claims its whole subtree. Anchor OS rules to their documented location (for
   example `Library/Caches`, not any `Caches`).
4. Add a comment above the rule with its source link.
5. Add a test that shows the rule matches the intended path and nothing it should not match.
   The tests in `categories.rs` build in-memory indexes with `IndexBuilder`, so they touch no
   files.
6. Do not add a hint without a source that says the contents are re-created (or, for Trash,
   already deleted by the user). A wrong "safe to remove" hint does real harm; a wrong
   category does not. dredge suggests and never acts; a suggestion is still text the user may
   run.

## Adding a category color

The bar's palette lives in `crates/dredge-render/src/lib.rs`, on `Ink`. A new category needs a
variant in `dredge-core`'s `Category` first, then in render:

1. Add an `Ink` variant, put it in `Ink::ALL` (palette order is fixed), and handle it in
   `from_category`, `name`, `label`, `rgb` (the saturated ink; the pale ground is derived) and
   `sgr16` (the 16-color fallback; 256-color is derived from `rgb`).
2. Pick a hue that no other ink uses and that reads on a light tint.
3. Regenerate the golden files with `DREDGE_UPDATE_GOLDEN=1 cargo test -p dredge-render --test golden`
   and review the diff.

## Commits and pull requests

- Use Conventional Commit titles, such as `feat:`, `fix:`, `docs:`, `test:` or `chore:`.
- Keep one concern per PR. Link the issue or RFC.
- Fill in the pull request template. Its checklist mirrors the ground rules.
- Do not commit generated output or the `target/` directory.

### Sign-off (DCO)

Add a sign-off to every commit with `git commit -s`. Git writes a `Signed-off-by:` line from
your `user.name` and `user.email` settings. The line is the Developer Certificate of Origin
sign-off, a statement that you have the right to submit the change. It is free text. It is not
code signing and it needs no paid certificate. Keep `user.name` and `user.email` set before you
commit.

## AI-assisted contributions

AI-assisted contributions are welcome. Before you open a PR, you must understand the change and
have run it yourself. State the tool you used in the PR description.

## Security

Do not report vulnerabilities in public issues. See [SECURITY.md](SECURITY.md).

## Code of conduct and license

Participation is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). The project is licensed
under the [MIT license](LICENSE).

## Releases and dependency checks

`publish = false` remains set in the workspace pending an owner decision. To make a binary release,
bump the workspace version in `Cargo.toml`, update `Cargo.lock`, then create and push the matching
`vX.Y.Z` tag. The tag workflow verifies the tag against `dredge-cli` metadata and publishes Windows
x64, macOS x64/arm64, and Linux x64 archives with SHA-256 checksums. The workflow cannot be run
locally; validate workflow syntax and the build commands before tagging.

CI runs `cargo deny check` and `cargo audit` on every event, alongside the Windows/macOS/Linux
matrix. The Ubuntu benchmark floor runs only for pushes to `main` and manual dispatches. To refresh
it, use the 30-directory, 3,000-file fixture recipe in `docs/BENCHMARKS.md`, take several local
release measurements, set the floor to about 25% of a representative local rate, and update
`.github/bench-floor.txt` plus the benchmark documentation.
