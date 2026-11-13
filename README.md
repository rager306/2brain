# arsumbris (fork monorepo)

One repository for the whole arsumbris fork: the typed knowledge-graph engine,
its MCP layers, the Rust MCP adapter, and the content/knowledge repositories.
Each former repository is a top-level directory with its own history preserved
by `git subtree`.

Origin: `github.com/rager306/arsumbris` (private).

## Layout

| path | role |
|---|---|
| `au-engine/` | Rust core: `au-cli`, `au-engine`, `au-core`, `au-parser`, `au-grammar`, `au-references`, `au-diagnostics`, `au-testkit` |
| `au-mcp-rs/` | experimental Rust MCP stdio adapter over the engine socket |
| `au-mcp-bridge/` | Python stdio MCP bridge |
| `au-mcp/`, `au-mcp-core/`, `au-mcp-sdk/`, `au-mcp-adapter-*` | MCP serving layers, SDK, and client adapters |
| `au-ingest/`, `au-govern/` | ingest pipeline, workspace composition |
| `au-type-system/`, `au-type-codegen/`, `au-base-types/` | type-system contract and generated types |
| `au-skills/`, `au-agent-guides/`, `au-rules/`, `au-writing-style/`, `au-competency/`, `au-defaults/` | content and knowledge repositories |
| `au-tree-research/`, `au-weave/` | domain repositories |
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
