# Naming: the product is 2brain

Decision (2026-11-16): the consolidated system is called **2brain**, short form **2b** — used in
headers, commits and conversation, never as an identifier that crosses a process boundary. The name belongs to the whole —
the Rust knowledge engine, the MCP surface agents talk to, and the judgement layer that decides small
things cheaply. "arsumbris" is not deleted; it becomes the working name of the parts that already
carry it, and every rename is a migration rather than a find-and-replace.

## What is named now, and what is not

| surface | today | under 2brain |
|---|---|---|
| the product, in prose and docs | arsumbris | **2brain**, from now |
| this repository's path and GitHub slug | `arsumbris`, `github.com/rager306/arsumbris` | unchanged; a GitHub rename is a one-off action with its own redirect consequences |
| crates and binary | `au-engine`… `au-mcp-rs`, `au`, `au-mcp-rs` | unchanged |
| MCP server identifiers | `arsumbris-rs` (Rust adapter), `arsumbris` (Python bridge, retiring) | unchanged until N2 |
| resource URI scheme | `arsumbris://<member>/<path>` | unchanged: it is in fixtures, clients and the WIRE contract |
| socket and state paths | `~/.arsumbris/au-engine/run/*.sock`, `~/.arsumbris/bin/au` | unchanged |
| systemd units | `arsumbris-engine`, `arsumbris-mcp` | unchanged |
| the vault | `/root/vault` | unchanged; a vault is content, not a product name |

## The rule that keeps this from becoming a sweep

**A name that crosses a process boundary is a migration, not a rename.** It needs a compatibility
step, a rollback, and a test that the old name still answers — because the moment a socket path or a
resource URI changes, every client, fixture and cached session disagrees at once.

Names that do *not* cross a boundary (a heading, a doc title, a comment, a plan's prose) are free to
say 2brain today, and they should: the point of a name is that the docs agree on one.

## Phases

| phase | change | gate |
|---|---|---|
| **N1** (now) | identity only: `README.md`, `AGENTS.md`, the plans and `docs/` say 2brain; nothing technical moves | `git grep -c 2brain` matches the documents, `cargo test` unchanged |
| **N2** | the MCP server identifier the clients see: `arsumbris-rs` → `2brain` in Hermes' config, the adapter's `--info` name and `docs/` | one live session works on the new id; rollback is a config edit |
| **N3** | crates and directories (`au-*` → `brain-*`?) — **only if worth it** | a pure-Rust mechanical change with no path in it; every `use` and `Cargo.toml` moves together |
| **N4** | wire-visible strings: the resource URI scheme, socket paths, unit names | a versioned alias: the new scheme answers, the old one keeps answering for one release |

N3 is deliberately optional. Renaming 24 crates buys a consistent vocabulary and costs a day of
mechanical churn plus every `docs/` reference; the product can carry the name 2brain with none of it,
which is what N1 does.

## What the name does not decide

It says nothing about the architecture: the engine stays a typed knowledge graph with files as the
source of truth, the MCP surface stays fail-closed, and the judgement layer stays outside the serving
path (`docs/agents-and-decisions-assessment.md`). A new name is not a new design.
