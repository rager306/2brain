# 2brain (v2 — Clean-Room MIT Architecture)

Single-binary, MIT-licensed typed knowledge graph and cognitive MCP 2.0 server written in Rust (Edition 2024).

## Architecture

Hexagonal / Onion monorepo with strict compile-time layer enforcement (`scripts/check_layer_deps.py`):

- **`crates/domain/*`** — pure Rust core (0 I/O, 0 `tokio`, 0 `fs`, `miri`-verified):
  `tb-contracts`, `tb-syntax`, `tb-schema`, `tb-graph` (CSR + `u32` interning), `tb-search` (BM25), `tb-cognition` (Brandes/Gateways, Leiden+NMI, Peel-the-Onion, Callon, Dual-Layer Gaps, DiffGraph).
- **`crates/app/tb-app`** — use cases, ports (traits), context assembly, release admission, generation-fenced analytics.
- **`crates/adapters/*`** — driven and driving adapters:
  `tb-fs-git`, `tb-journal` (CRC-32 framed journal + crash-safe `usage.json` + `registry.lock`), `tb-skills` (SEP-2640 `skill://` catalog), `tb-ipc` (daemon Unix socket), `tb-mcp` (MCP `2026-07-28` + legacy stdio/HTTP).
- **`crates/bin/two-brain`** — composition root producing the single `2brain` binary.

Specifications live in `spec/` (`01-workspace-and-syntax.md`, `02-type-system-and-diagnostics.md`, `03-mcp-2.0-skills-and-contracts.md`).
