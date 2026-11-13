# Security policy

## What is in scope

dredge is read-only by design. That guarantee is the security boundary. Report as a security
issue any behaviour that:

- writes, deletes, moves or modifies a file outside the snapshot directory;
- follows a symlink or junction, or downloads or reads the content of a cloud placeholder;
- opens a network connection, or sends telemetry;
- runs a command that dredge only suggested in text;
- lets input from an MCP client cause any of the above;
- is memory-unsafe in the `unsafe` code under `crates/dredge-scan/src/platform/`.

Ordinary bugs, such as wrong sizes or a crash on bad arguments, are regular issues. Use the bug
form.

## Reporting a vulnerability

Do not open a public issue. Use GitHub's private vulnerability reporting for this repository:
the Security tab, then "Report a vulnerability". If that option is not available, contact
dredge@cinematicgenius007.com.

Include:

- the dredge version (`dredge --version`) or commit SHA;
- your OS and version;
- the exact commands or MCP calls that show the problem;
- what you expected and what happened.

Redact personal paths from any output you attach.

## Supported versions

dredge has no release yet. Only the `main` branch is supported. Report against `main`.
