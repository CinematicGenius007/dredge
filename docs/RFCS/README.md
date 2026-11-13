# RFCs

An RFC is a short written proposal, agreed before any code. Write one before a change that:

- changes the output schema: any field, kind or envelope change (ground rule 4);
- adds a query (see CONTRIBUTING.md, "Adding a query");
- adds a new class of dependency, such as a TUI stack or any networking crate.

Changes that do not touch these need no RFC. A bug fix does not need one.

## Process

1. Open an issue with the feature form and say that it needs an RFC. Discuss the problem and
   the approach there. Add the `rfc` label.
2. When the approach is agreed, open a PR that adds `docs/RFCS/NNNN-short-title.md`, where
   `NNNN` is the next free number. Use the sections below.
3. Set the status line at the top of the file to one of `Draft`, `Accepted`, `Rejected` or
   `Superseded by NNNN`.
4. Implement only after the RFC is `Accepted`. Link the RFC from the implementation PR.

## Sections

- **Summary**: the change in two or three sentences.
- **Motivation**: the problem, and who has it.
- **Design**: the query, record type, fields and CLI/MCP arguments.
- **Output change**: the new or changed fields, the `schema_version` effect, and the
  `docs/OUTPUT_SCHEMA.md` diff.
- **Read-only and local impact**: what new files, links or network access, if any, this needs.
  The answer should be none.
- **Alternatives**: what else was considered, and why it was not chosen.
- **Test plan**: which fixtures and parity tests prove it.

- [0001](0001-lazy-storage-and-honest-freshness.md): lazy snapshot storage, reuse of recent scans, honest freshness (Draft)
