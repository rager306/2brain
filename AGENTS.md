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

## Dev Loop & Verification Tiers

- **Inner edit loop:** Run `just check <crate>` (`cargo check -p <crate> --all-targets`) and `just test <crate> [filter]` during edits. Do not run full workspace `just gate` after every small edit.
- **Pre-commit / milestone gate:** Run `just gate-fast` (`fmt`, layer check, workspace `clippy -D warnings`) and `just gate` (adds `nextest --workspace` and `cargo deny --offline -L error`) once before committing.
- **Pure domain UB check:** `just miri [crate]` (default `tb-contracts`).
- **Release build:** `just build-release`.

## AGY & Compound Engineering Workflow

- Read `CODING_STANDARDS.md` and `REVIEW.md` before writing or reviewing code; update them in the same change when adding or retiring a rule.
- Use `codebase-memory-mcp` (`project: root-2brain-v2`, `get_file_outline`, `get_code_snippet`, `search_graph`) for Rust symbol/caller exploration before falling back to shell `rg`/`grep`.
- Batch independent reads/searches/edits in one turn; read whole files (<=800 lines) or coarse spans, and re-read after `cargo fmt` before `edit_file`.
- Never poll running background commands or subagents with `manage_task(status)` before `NotificationTimeoutSeconds`; wait for AGY reactive wakeup.
- Keep CLI output compact: run `codex exec` with `-o /tmp/codex-<tag>.md >/dev/null 2>&1 && cat /tmp/codex-<tag>.md`, and use `gh run watch --compact --interval 15` or `gh run view --json status,conclusion` (never bare `gh run watch`).
- Use `ce-compound` (`mode:non-interactive`) when verified work corrects a wrong belief, `ce-noslop` for user-facing reports, and `/compact` or `ce-handoff` after closing a major milestone.
