# ADRs — 2Brain fork architectural decisions

## Format

Records use MADR-lite with YAML front matter, following the Allspark fork's
`doc/adr/README.md`: `id`, `title`, `status`, `lifecycle`, `date`,
`supersedes`, and `related`. Status is `Proposed`, `Accepted`, or `Superseded`.
Lifecycle tags distinguish `[validated]`, `[bounded]`, `[smoke]`,
`[proposed]`, and `[deferred]` claims. Decisions are proposed until explicitly
accepted; they do not authorize production changes on their own.

## Current ADRs

| id | title | status | lifecycle |
|---|---|---|---|
| [ADR-0001](0001-rust-native-agent-mcp.md) | Rust-native, agent-neutral MCP serving for 2Brain | Accepted (2026-11-16) | [validated] |
| [ADR-0002](0002-promote-with-checks.md) | Promote-with-checks refuses on ERROR diagnostics, as an optional rider | Proposed | [proposed] |
| [ADR-0003](0003-rename-type-grant-binding.md) | A cascade mutation carries the grant it must respect, checked under the write lock | Proposed | [proposed] |

An ADR's companion documents — the plan and the tool inventory that argue rather than decide — live in
[`../plans/`](../plans/). A file in `adr/` is a record: it carries an id, a status and a date.
