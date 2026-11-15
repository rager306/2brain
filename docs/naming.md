# Naming: the product is 2brain

Decision (2026-11-16): the consolidated system is called **2brain**, short form **2b** — used in headers,
commits and conversation, never as an identifier that crosses a process boundary.

The rebrand ran in tiers (`docs/rebrand-plan.md`). This page states what is true **now**, measured on
2026-11-16 against the installed binary and the live daemon, not what was planned.

| surface | state | where it lives |
|---|---|---|
| product, prose, docs | **2brain** | everywhere |
| repository | `github.com/rager306/2brain`, checkout `/root/2brain` | — |
| directories | all `tb-*`; upstream documentation in `upstream/` | — |
| packages and crates | `tb-engine`, `tb-mcp-rs`, … / `tb_engine`, `tb_mcp_rs`, … | `Cargo.toml` |
| the MCP binary | **`2brain`** — the only executable; `2brain mcp <target>` is the server | `/usr/local/bin/2brain` |
| server identifier clients see | `2brain` | `initialize` result |
| **resource URI scheme** | **`2brain://<member>/<path>`**; `arsumbris://` is still *accepted* on parse for one release | `src/resources.rs` (`RESOURCE_SCHEME`, `LEGACY_RESOURCE_SCHEME`) |
| **state and socket paths** | **`~/.2brain/…`** (`$2BRAIN_HOME`, else `<home>/.2brain`, else legacy `<home>/.arsumbris`); `~/.arsumbris` is a symlink to it | `repo.rs` (`state_dir_in`) |
| **systemd unit** | **`2brain-engine.service`**, `ExecStart=/root/.2brain/bin/au daemon start /root/vault` | `/etc/systemd/system` |
| the engine binary | `au` (unchanged: the unit and every script name it; a device layout, not a repository directory) | `~/.2brain/bin/au` |
| tool names | `au_*` — 52 tools, a wire contract | `src/capability.rs` |
| engine file format | `.arsumbris/repo.yaml`, `au.engine.*` types | every member repo |
| member identity (`name:` in `repo.yaml`) | unchanged (`au-engine`, `au-mcp-sdk`, …): content cites these names | each `repo.yaml` |

## What is still open, with the numbers

**Tier C — the engine file format.** `.arsumbris/` as a directory name (79 literals in 12 files, five
crates), `au.engine.*` as a schema-name prefix (52 occurrences in 28 files), and 308 tracked `repo.yaml` files, of
which 304 declare `type: au.engine.repo::au-engine` (1 the wrong floor, 3 no written type) plus 30
`workspace.yaml` and 11 `extends: au.engine.meta::au-engine` lines. The type namespace *is* the vault contract: a partial rename
leaves a type system that cannot open its own content, which is why the shape is two releases —
`identity_dir()` and `schema_type_matches()` first (accept both), then `au migrate identity --dry-run`,
then the data move, then dropping the legacy spelling. Inventory: `docs/research/tier-c-inventory.md`.

**Tool names — a client contract, not a brand.** `au_content` reads as the old product name, and it is the
one visible surface that still does. Renaming it is a contract change, not a sweep: 52 names, 618 string
occurrences in 81 files (101 in `src`, 448 in tests, 7 in docs, 5 in the engine), 34 fixture files named
after them, plus 91 mentions in vault content. The safe form is `tb_*` with `au_*` accepted as a deprecated
alias for one release. Until that decision is made the old names stay, and this row is the honest record of
it.

**Content.** 2 274 occurrences of the word `arsumbris` remain in the tree, almost all inside member content
(`tb-weave`, `tb-tree-research`, engine scenarios, `.gitignore` entries naming `.arsumbris/` paths). That is
data, not code: it moves with `au_rename` and a vault decision, not with a `sed`.

## Why the identifiers that moved, moved this way

A Rust identifier and a Cargo package name must start with a letter or an underscore, so `2b_` is not a
legal prefix. `tb` — the same two letters in the order a compiler accepts — is the spelling used for
packages, crates and directories. A tool name is a *string*, so `2b_` would be legal there; it was rejected
because several clients require tool names to start with a letter.

A rename that reaches the filesystem layout is not a rename: `.arsumbris/au-engine/` is the device root the
daemon recreates on start, and rewriting it in `repo.rs` alone (`ca4846d`) would have given the next binary
an empty state root. Tier B exists as a separate step for exactly that reason.

## What the name does not decide

It says nothing about the architecture: the engine stays a typed knowledge graph with files as the source of
truth, the MCP surface stays fail-closed, and the judgement layer stays outside the serving path
(`docs/agents-and-decisions-assessment.md`). A new name is not a new design.
