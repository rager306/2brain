# 2brain

Clean-room, **MIT-licensed**, single-binary typed knowledge graph and cognitive **MCP 2.0** server written in Rust (Edition 2024).

## Highlights

- **Single binary (`2brain`)**: combines the workspace daemon (`2brain daemon`), stdio & Streamable HTTP MCP 2.0 server (`2brain mcp`), durable outcome registry management (`2brain outcomes`), read-only benchmark profiler (`2brain bench`), and standalone graph/epistemic analytics CLI (`check`, `analyze`, `audit-claims`, `fragility`, `diff`).
- **Hexagonal / Onion monorepo (13 crates)** with CI-enforced compile-time layer boundaries (`scripts/check_layer_deps.py`):
  - **`crates/domain/*`** (zero I/O, zero `tokio`, `miri`-verified):
    - `tb-contracts`: 5 distinct identities (`NodeId`, `TermId`, `TypeId`, `Generation`, `RevisionDigest`), `Assessment<T>`, typestate `WriteIntent<Unchecked -> ReadVerified -> CasVerified>`, `OutcomeState`.
    - `tb-syntax`: strict UTF-8 decoding (SEM-04), `saphyr` YAML 1.2 frontmatter, `memchr` `[[wikilink]]` scanner, headings + `^block-ids`, EN/RU tokenizer.
    - `tb-schema`: `*.type.yaml` schema parser, inheritance, cross-repo qualification, deterministic validation diagnostics.
    - `tb-graph`: `u32` Compressed Sparse Row (`TypedLinkGraph`), iterative Tarjan SCC/bridges/articulation points, exact triangles/transitivity, bounded N-hop neighborhood.
    - `tb-search`: delta-varint BM25 + body proximity + rare-IDF title lift + word forms + slice-safe excerpts, separated from literal `grep_text` (SEM-09).
    - `tb-cognition`: CSR Brandes betweenness & Conceptual Gateways (`bc/deg`), Peel-the-Onion hub masking, 5-seed Leiden-connected modularity + NMI + Maslov-Sneppen null-model Z-score, Callon quadrants, Dual-Layer (`TypedLinkGraph ⊕ Rare-IDF`) structural gaps with anti-tautology & negative memory, LLM Bias Audit, `D03`/`R14` Claim Layer & Polysingularity Contradiction Audit, Git Difference Graph + Change-Driven Decay, and Hub Fragility analysis.
  - **`crates/app/tb-app`**: hexagonal ports, immutable `VaultSnapshot` behind `ArcSwap`, generation-fenced cognitive caches, release-admission SHA-256 verification on `tb_context`, and idempotent mutation orchestration.
  - **`crates/adapters/*`**:
    - `tb-fs-git`: zero-TOCTOU dirfd-relative (`openat`/`O_NOFOLLOW`) filesystem confinement (SEM-18), interprocess `fs4` `write.lock`, atomic CAS/Git mutations with rollback, and read-only Git object snapshot loader (`ls-tree -z` + batched `cat-file --batch`).
    - `tb-journal`: CRC-32 framed durable outcome journal, crash-safe `usage.json` + `usage.dirty`, and workspace `registry.lock`.
    - `tb-skills`: **SEP-2640** `skill://` catalog (5 built-in skills + dynamic vault projection for `mcp.skill` and promoted `pattern` notes) with verified SHA-256 digests.
    - `tb-ipc`: big-endian `u32` framed Unix-socket daemon with exclusive `<socket>.lock` and `0700`/`0600` permissions.
    - `tb-mcp`: **MCP 2.0 (`2026-07-28` stateless core + legacy `2024-11-05..2025-11-25`)** over **stdio** and **Streamable HTTP** (`--http`, fail-closed non-loopback Bearer auth, DNS-rebinding `Host` & `Origin` guards, `/healthz`), **SEP-2640 Skills**, **SEP-2663 Tasks + Elicitation** (`io.modelcontextprotocol/tasks` via `rmcp::TaskManager` + atomic `Semaphore` concurrency cap), fail-closed 38/54 tool surface (+9 opt-in cognitive tools via `--allow-cognitive`, including persistent `D07/D11` Negative Memory), full JSON schemas (`--tools concise|full`), structured `reason_code`/`recovery_hint` errors, and stateful `ReadLedger` + stateless CSPRNG HMAC-SHA256 `read_receipt` write gate (SEM-02).
  - **`crates/bin/two-brain`**: composition root producing the single `2brain` binary.

## Quick Start

```bash
# Build release binary
cargo build --release -p two-brain --bin 2brain

# Inspect build and protocol surface
2brain info

# Validate syntax, schemas, links, and CSR graph health
2brain check /path/to/vault

# Run topology, gateways, communities, Peel-the-Onion, and Dual-Layer gaps
2brain analyze /path/to/vault

# Audit D03 claims, R14 temporal validity, and contradictions
2brain audit-claims /path/to/vault --as-of 2026-11-28

# Analyze articulation-point hub fragility and overloaded MOC splits
2brain fragility /path/to/vault

# Compare historical Git snapshot against working tree (+ Change-Driven Decay)
2brain diff /path/to/vault HEAD~5

# Run the Unix-socket daemon
2brain daemon start /path/to/vault
2brain daemon status /path/to/vault

# Serve MCP 2.0 over stdio (connecting to daemon OR standalone --in-process)
2brain mcp /path/to/vault
2brain mcp --in-process /path/to/vault --allow-cognitive

# Serve MCP 2.0 over Streamable HTTP (loopback-only by default; non-loopback requires TWOBRAIN_HTTP_TOKEN or --http-token-file)
2brain mcp /path/to/vault --http 127.0.0.1:8787 --allow-cognitive

# Run read-only workspace performance benchmark (cold load, incremental reload, BM25, cognitive suite, RSS)
2brain bench /path/to/vault --iterations 5
```

## Verification & Quality Gates

```bash
# Formatting, 13-crate layer boundaries, clippy -D warnings, tests, MIT license audit
just gate

# Miri UB check across all 6 pure-domain crates
just miri
```

Architecture and protocol specifications live in [`spec/`](spec/). Engineering invariants learned from adversarial review live in [`CODING_STANDARDS.md`](CODING_STANDARDS.md).

## License

Licensed under the [MIT License](LICENSE).
