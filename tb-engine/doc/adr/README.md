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
| [ADR-0001](0001-rust-native-agent-mcp.md) | Rust-native, agent-neutral MCP serving for 2Brain | Proposed | [proposed] |
