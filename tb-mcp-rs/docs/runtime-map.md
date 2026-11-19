# Runtime map: what runs, and what is left outside Rust

Measured 2026-11-16, after the TypeScript and Python layers were deleted. The earlier version of this
file (2026-11-14) mapped 26 100 lines of TypeScript and 293 of Python inside the served path; that map
is in git history, and this one records what replaced it.

## What runs

| module | language | role | in the served path |
|---|---|---|---|
| `tb-engine` | Rust | the engine: parser, type graph, diagnostics, daemon, wire, the BM25 index | **yes — the authority** |
| `tb-mcp-rs` | Rust | MCP stdio server over the engine socket; `scripts/install.sh` builds it as `/usr/local/bin/2brain` | **yes — the only front end** |
| `tb-mcp-sdk` | type definitions | the `mcp.*` vocabulary that content notes are written in (`mcp.skill`, `mcp.inject`, …) | no — it is a contract, and `tb-weave`/`tb-rules` depend on it |
| `tb-type-system`, `tb-base-types`, `tb-rules`, `tb-skills`, `tb-agent-guides`, `tb-writing-style`, `tb-competency`, `tb-defaults`, `tb-govern` | YAML + Markdown | vocabulary, rules, guides, templates, audits | content, mounted as members |
| `tb-ingest` | YAML contracts + one fixture script | source → markdown contracts | offline toolchain |

## What is gone (2026-11-16)

`tb-mcp` (Node daemon), `tb-mcp-core` (49 tool definitions and their implementations), both client
adapters, `au-mcp-bridge` (the Python bridge), `au-engine-sdk` (the Node client) and `au-type-codegen`
(all TypeScript), plus `arsumbris-mcp.service`. Reasons and measurements:
`docs/internal/serving-parity.md`, `docs/repo-inventory.md`, `docs/internal/detach-and-consolidate.md`.

The consequences worth knowing:

- **nothing outside Rust runs in the served path** — no Node, no Python, no bridge;
- the client spawns one binary and gets MCP directly, so there is no protocol translation layer to
  reconnect or pin;
- what the client loses is listed and decided in `docs/internal/serving-parity.md` §2b: the four `au_host_*`
  relays (dead: no `au-host` socket here), the inert `bash` stub, `glob`/`grep_files`, and
  `au_type_system`.

## Tool surface: 52 Rust tools

33 read, 16 mutate, 2 discover, 1 watch — the split is `src/capability.rs`, and
`docs/verification.md` records how it is checked against the router. The daemon's 49 tools are not a
reference any more: 41 were ported (three under clearer names — `au_list_imports` → `tb_imports`,
`au_validate` → `tb_validate_value`, `grep_files` → `tb_grep`), 8 were dropped with reasons, and the
Rust surface added what the daemon never had (`tb_batch`, `tb_search`, `tb_graph_health`,
`tb_rollback_preview`, `tb_watch`, `tb_reconcile`, `tb_preview_mutation`, `tb_content`, and the two
discovery tools).

`references/tool-map.md` in `skills/2brain/` carries the full generated list.
