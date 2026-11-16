# au-mcp-sdk — the mcp.* vocabulary

This repository is a **contract**, not a library. It owns the type definitions that agent-facing
content is written in: `mcp.tool`, `mcp.skill`, `mcp.inject`, `mcp.hook`, `mcp.adapter`,
`agent-profile`, and the runtime metadata types those extend (`plugin-runtime-meta`,
`tool-access-meta`, `tool-precondition-meta`, `retention-config`, …).

## What it contains now

| path | what |
|---|---|
| `type/*.type.yaml` | 17 type definitions — the vocabulary |
| `.arsumbris/repo.yaml` | the repo identity that makes `type: mcp.skill::au-mcp-sdk` resolve |

## What it used to contain, and why that went

Until 2026-11-16 this repository also carried the TypeScript implementation of those types
(`src/`, `tests/`, `skills/`, a package, a lockfile). It was the vocabulary's reference
implementation for the Node MCP daemon.

The serving path is now Rust end to end — `/usr/local/bin/2brain` (the adapter in `au-mcp-rs`) over
the engine socket — so the TypeScript implementation had no consumer. It was deleted with the tools
it described (`docs/internal/detach-and-consolidate.md`, and the measurement in `docs/repo-inventory.md`).

The **definitions stayed** because notes are written in them: `au-weave` declares
`type: mcp.skill::au-mcp-sdk` in six files, `au-rules` mixes in `mcp.inject::au-mcp-sdk`, and the
`mcp-bundle` aggregator that the vault depends on resolves through this repository. Deleting the code
removed an implementation; deleting the types would have invalidated content.

## Using it

A note declares its type by the qualified name:

```yaml
---
type: mcp.skill::au-mcp-sdk
---
```

A repository that wants those names lists this one in its `.arsumbris/repo.yaml` deps, or depends on
`mcp-bundle` (the aggregator in `au-defaults`), which now resolves to this repository alone.
