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
| crates and the engine binary | `au-engine`… `au-mcp-rs`, `au` | unchanged |
| the MCP binary | `au-mcp-rs`, `arsumbris` | **`2brain`**, installed at `/usr/local/bin/2brain` (phase N2) |
| MCP server identifiers | `arsumbris-rs` (Rust adapter), `arsumbris` (Python bridge, retiring) | **`2brain`** for the Rust server (N2); `arsumbris` disappears with the bridge |
| resource URI scheme | `arsumbris://<member>/<path>` | unchanged: it is in fixtures, clients and the WIRE contract |
| socket and state paths | `~/.arsumbris/au-engine/run/*.sock`, `~/.arsumbris/bin/au` | unchanged |
| systemd units | `arsumbris-engine`, `arsumbris-mcp` | unchanged |
| the vault | `/root/vault` | unchanged; a vault is content, not a product name |

## Identifiers: `tb_` is the legal spelling of 2b

`2b_` cannot be a Rust identifier or a Cargo package name: both must start with a letter or an
underscore, so a leading digit is a hard error (not a style choice). The rename therefore uses **`tb`**
— the same two letters, in the order a compiler accepts — while `2brain` stays the name a client sees and
`2b` stays the prose form.

Applied 2026-11-16 (commit `crates: rename au-* to tb-*`):

| surface | before | now |
|---|---|---|
| engine package names | `au-cli`, `au-core`, `au-engine`, `au-parser`, `au-grammar`, `au-diagnostics`, `au-references`, `au-testkit` | `tb-*` |
| engine crate paths (Rust) | `au_cli`, `au_core`, … | `tb_cli`, `tb_core`, … |
| adapter package and lib | `au-mcp-rs`, `au_mcp_rs` | `tb-mcp-rs`, `tb_mcp_rs` |
| crate directories | `crates/au-*` | `crates/tb-*` |
| engine binary | `au` | unchanged — `~/.arsumbris/bin/au`, the systemd unit and `au-health.sh` name it |
| adapter binary | `2brain` | unchanged |
| tool names, resource scheme, wire fields, socket paths, `au.engine.repo` | `au_*`, `arsumbris://` | **unchanged: they are the contract** |

The two rows that did not move are the point. A crate name is internal — nothing outside the build reads
it, so renaming is mechanical and verifiable with a compile. A tool name, a resource URI, a socket path
or a type identity in a vault's `repo.yaml` is read by clients and stored in content: `au.engine.repo` is
a *type name* that every member of the workspace declares, so renaming it is a vault migration, not a
rename (phase N4 material).

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
| **N2** | **done 2026-11-16**: the binary is `2brain` (one executable for both halves), the MCP server name is `2brain`, install is `/usr/local/bin/2brain`; Hermes' stanza is in `docs/hermes-2brain.md` | the installed binary answers `--version` with its commit, `info /root/vault` resolves the live socket, and a session on the new id does a read |
| **N3** | crates and directories (`au-*` → `brain-*`?) — **only if worth it** | a pure-Rust mechanical change with no path in it; every `use` and `Cargo.toml` moves together |
| **N4** | wire-visible strings: the resource URI scheme, socket paths, unit names | a versioned alias: the new scheme answers, the old one keeps answering for one release |

N3 is deliberately optional. Renaming 24 crates buys a consistent vocabulary and costs a day of
mechanical churn plus every `docs/` reference; the product can carry the name 2brain with none of it,
which is what N1 does.

## What the name does not decide

It says nothing about the architecture: the engine stays a typed knowledge graph with files as the
source of truth, the MCP surface stays fail-closed, and the judgement layer stays outside the serving
path (`docs/agents-and-decisions-assessment.md`). A new name is not a new design.
