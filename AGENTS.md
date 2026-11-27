# AGENTS.md — 2brain v2 (Clean-Room MIT Monorepo)

## Clean-Room Boundary (MANDATORY)

This repository is an independent **MIT-licensed** implementation built strictly from `spec/` and the Black-Box TCK.
- **NEVER** read, copy, or translate source files from any legacy SUL-1.0 `tb-engine/crates/*` tree into clean-room crates (`tb-syntax`, `tb-schema`, `tb-graph`, `tb-fs-git`, `tb-ipc`).
- Only pure `CLEAN_POST_FORK` modules authored from scratch (`tb-mcp`, `tb-search`, `tb-journal`) may be ported cleanly with verified provenance.
- Every third-party dependency MUST satisfy `deny.toml` (MIT / Apache-2.0 / BSD / ISC / Zlib only).

## Layer Rules

Enforced automatically by `python3 scripts/check_layer_deps.py`:
1. `crates/domain/*` MUST NOT depend on `tb-app`, `crates/adapters/*`, `two-brain`, `tokio`, `rmcp`, `gix`, `notify`, `fs4`, or `clap`.
2. `tb-app` depends ONLY on `crates/domain/*` and exposes ports (traits).
3. Only `crates/bin/two-brain` wires concrete adapters into `tb-app`.

## Dev Loop

Run `just gate-fast`, `just gate`, and `just miri tb-contracts`.
