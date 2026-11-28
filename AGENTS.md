# AGENTS.md — 2brain (MIT Monorepo)

## Licensing & Dependencies

This repository is an independent **MIT-licensed** implementation built from `spec/`.
- Every third-party dependency MUST satisfy `deny.toml` (MIT / Apache-2.0 / BSD / ISC / Zlib / Unicode-3.0 / CDLA-Permissive-2.0 only).
- Do not copy non-MIT source code into any crate.

## Layer Rules

Enforced automatically by `python3 scripts/check_layer_deps.py`:
1. `crates/domain/*` MUST NOT depend on `tb-app`, `crates/adapters/*`, `two-brain`, `tokio`, `rmcp`, `notify`, `fs4`, or `clap`.
2. `tb-app` depends ONLY on `crates/domain/*` and exposes ports (traits).
3. Only `crates/bin/two-brain` wires concrete adapters into `tb-app`.

## Dev Loop

Run `just gate-fast`, `just gate`, and `just miri`.
