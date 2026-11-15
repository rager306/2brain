# Naming: the product is 2brain

Decision (2026-11-16): the consolidated system is called **2brain**, short form **2b** — used in
headers, commits and conversation, never as an identifier that crosses a process boundary.

The rebrand happened the same day, in tiers (`docs/rebrand-plan.md`). What is true now:

| surface | state |
|---|---|
| product, prose, docs | **2brain** |
| repository | `github.com/rager306/2brain`, checkout `/root/2brain` |
| directories | all `tb-*`; upstream documentation in `upstream/` |
| packages and crates | `tb-engine`, `tb-mcp-rs`, … / `tb_engine`, `tb_mcp_rs`, … |
| the MCP binary | **`2brain`** — the only executable; `2brain mcp <target>` is the server |
| the engine binary | `au` (unchanged: the systemd unit and every script name it) |
| server identifier clients see | `2brain` |
| tool names | `au_*` (unchanged: a wire contract, not a brand) |
| resource URI scheme | `arsumbris://<member>/<path>` — **open**, tier D |
| state and socket paths | `~/.arsumbris/…` — **open**, tier B |
| systemd unit | `arsumbris-engine.service` — **open**, tier B |
| engine file format | `.arsumbris/repo.yaml`, `au.engine.*` types — **open**, tier C |
| member identity (`name:` in `repo.yaml`) | unchanged (`au-engine`, `au-mcp-sdk`, …): content cites these names |

The four open rows are the ones that cannot be a rename: they are read by clients that exist, stored in
content that exists, or hardcoded in the engine. Each has a gate in `docs/rebrand-plan.md`.

## Why the identifiers that moved, moved this way

A Rust identifier and a Cargo package name must start with a letter or an underscore, so `2b_` is not a
legal prefix. `tb` — the same two letters in the order a compiler accepts — is the spelling used for
packages, crates and directories.

## What the name does not decide

It says nothing about the architecture: the engine stays a typed knowledge graph with files as the
source of truth, the MCP surface stays fail-closed, and the judgement layer stays outside the serving
path (`docs/agents-and-decisions-assessment.md`). A new name is not a new design.
