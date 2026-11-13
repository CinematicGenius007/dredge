## Summary

<!-- What changes and why. One concern per PR. -->

## Linked issue or RFC

<!-- Closes #, or the RFC that approved an output change or new query. -->

## Checklist

- [ ] fmt, clippy (`-D warnings`) and test are green on my machine; CI on Windows, macOS and Linux is green (ground rule 1)
- [ ] Read-only holds: no writes outside the data dir, no link following, no cloud hydration, no network (ground rule 2)
- [ ] A new query adds a Query variant, a record type, CLI, MCP, a schema doc entry and a CLI/MCP parity test (ground rule 3)
- [ ] An output change has an RFC issue linked, a `schema_version` bump if needed, and `docs/OUTPUT_SCHEMA.md` updated in this PR (ground rule 4)
- [ ] A new dependency was added with `cargo add` and a one-line justification in the root `Cargo.toml`; no TUI or terminal dependency in `dredge-core` (ground rule 5)
- [ ] Tests use `tempfile` only and do not assert host block sizes (ground rule 6)
- [ ] `unsafe` appears only in `dredge-scan/src/platform/`, each block with a `// SAFETY:` comment (ground rule 7)
- [ ] Commit titles are Conventional Commits; commits are signed off with `git commit -s` (ground rules 8 and 11)
- [ ] Category rules and cleanup hints have a source link and a test (ground rule 10)
- [ ] Docs are updated where behaviour changed (README, AGENTS.md, CONTRIBUTING.md)
- [ ] dredge does not execute, delete or move anything; cleanup appears only as suggested text

## AI assistance

<!-- If AI tools helped write this change, name the tool. Confirm that you understand the change and ran it yourself (ground rule 9). Otherwise write "None". -->
