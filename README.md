# 2brain

The product is **2brain**: one Rust knowledge engine over a typed graph of notes, the MCP surface
agents talk to, and a judgement layer for the small decisions. The repository keeps its working name
and paths (`arsumbris`, `au-*`) — see `docs/naming.md` for what is renamed when, and why a name that
crosses a process boundary is a migration rather than a find-and-replace.

One repository for the whole system: the typed knowledge-graph engine, its MCP layers, the Rust MCP
adapter, and the content/knowledge repositories. Each former repository is a top-level directory with
its own history preserved by `git subtree` (upstream is detached: `archive/upstream-0.0.5`).

Origin: `github.com/rager306/arsumbris` (private).

## Layout

| path | role |
|---|---|
| `au-engine/` | Rust core: `au-cli`, `au-engine`, `au-core`, `au-parser`, `au-grammar`, `au-references`, `au-diagnostics`, `au-testkit` |
| `au-mcp-rs/` | **the serving path**: the Rust MCP adapter behind `/usr/local/bin/2brain`, built and installed by `scripts/install.sh` |
| ~~`au-mcp-bridge/`~~ | retired Python bridge, deleted with the Node daemon (2026-11-16) |
| `au-mcp-sdk/` | the `mcp.*` type vocabulary (contract); the TypeScript MCP layer it belonged to was deleted 2026-11-16 |
| `au-ingest/`, `au-govern/` | ingest pipeline, workspace composition |
| `au-type-system/`, `au-base-types/` | the type-system contract |
| `au-skills/`, `au-agent-guides/`, `au-rules/`, `au-writing-style/`, `au-competency/`, `au-defaults/` | content and knowledge repositories |
| `au-tree-research/`, `au-weave/` | domain repositories |
| `au-engine-sdk/`, `au-type-codegen/` | the remaining TypeScript (engine client, generator) — off the serving path, no JS consumer today |
| `arsumbris/` | upstream bundle documentation (release notes, installation, safety) |
| `AGENTS.md` | working instructions for coding agents |

## History model

Each directory entered this repository through `git subtree add`, so its
original commits are reachable. `git log -- <dir>` shows the import commit;
`git log --follow -- <dir>/<file>` follows a file across it.

The pre-monorepo sibling checkouts under `/root/arsumbris/` are untouched and
remain usable as read-only references until they are retired.

## Build notes

`target/` and `node_modules/` are ignored at every level. The Rust toolchain is
pinned by `au-engine/rust-toolchain.toml`; on this host the complete 1.99.0
toolchain lives under `RUSTUP_HOME=/root/.rustup`.
