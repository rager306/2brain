# Changelog

All notable changes to `2brain` (clean-room MIT single-binary typed knowledge graph and MCP 2.0 server) are documented in this file.

## [Unreleased]

## [v0.5.3] - 2026-11-28

### Highlights
- **Sparse-field 4-field BM25 normalization & `.2brain/repo.yaml` search config (`tb-search`, `tb-fs-git`, `tb-app`):**
  - Normalized BM25 field average lengths by `non_empty_docs[field]` instead of corpus-wide `live_docs`, eliminating the ~35x length-normalization penalty on partially enriched corpora (verified on a 40-task, 6-family bilingual live-vault holdout: `fulfillment 0.900 -> 0.950`, `MRR 0.847 -> 0.9375`, `paraphrase_ru +28.6 pp` / `+0.215 MRR`, `0` critical violations).
  - Split frontmatter enrichment into separate `keywords`/`aliases` (`2.2`) and `search_questions`/`questions` (`1.4`) fields behind configurable `SearchWeights` and `FreshnessPolicy::sanitized()`.
  - Added optional `search.weights` and `search.freshness` configuration in `.2brain/repo.yaml` (honored in single-repo, multi-member `workspace.yaml` `own` repos, and historical Git snapshots) with strict finite/range diagnostics (`invalid-repo-config` / `duplicate-search-config`) and non-destructive `deps:` splicing in `tb_add_dep` / `tb_remove_dep`.
- **Shared metadata projection, typed ingest actions & honest G5 token accounting (`tb-app`, `tb-fs-git`, `two-brain`):**
  - Extracted `tb_app::search_doc` as the single source of truth for BM25 enrichment fields, `FreshnessMeta`, and hybrid document text across `tb-fs-git`, `tb-app::enrich`, and `two-brain`.
  - Replaced `IngestGateReport.action: String` with the typed `IngestAction` enum (`create` / `refresh` / `conflict`).
  - Consolidated G5 paired evaluation into `two-brain/src/eval_common.rs` with real excerpt/metadata token accounting (`ceil(chars / 4)` instead of flat per-hit estimates), `--min-should-match`, and `repo::path` matching across `enrich evaluate` and `hybrid-eval`.
  - Aligned `build_simulated_enriched_index` with `build_enrichment_apply_plan`, refused explicit `--repo <dependency>` across ingest paths, and added Russian stopwords to `heuristic_enrich`.
- **`error-fix` schema compatibility & YAML key safety (`tb-app`):**
  - Replaced string-prefix merge checks in `ErrorFixAtom` with `Option<String>`, preserved safe unconsumed workspace schema keys (`extra_fields`) fail-closed (`[A-Za-z_][A-Za-z0-9_-]{0,63}`), and hardened `write_yaml_field` against frontmatter breakout.
- **Cross-platform prebuilt releases, SLSA provenance & tooling (`scripts/`, `tools/`, `.github/workflows/release.yml`):**
  - Expanded `.github/workflows/release.yml` to build both `x86_64-unknown-linux-gnu` (`ubuntu-latest`) and `aarch64-apple-darwin` (`macos-14`) with `--target` CycloneDX 1.5 SBOMs, `COPYFILE_DISABLE=1` tarballs, portable Python SHA-256 manifests, and per-target SLSA provenance (`actions/attest-build-provenance@v4.2.2`).
  - Hardened `scripts/enrich_llm.py` (`max_tokens: 4096`, bounded `--retries`, bilingual EN+RU prompt, `--system-prompt-file`), hardened `tools/eval/export_model2vec.py`, and tightened `tools/check_capabilities.py` to require `#[test]`/`#[tokio::test]` attributes.

### Prebuilt Assets (`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`)
- `2brain-<target>.tar.gz` — standalone `cargo-auditable` release binary.
- `2brain-<target>.cdx.json` — CycloneDX 1.5 Software Bill of Materials (SBOM).
- `SHA256SUMS.txt` / `2brain-<target>.sha256` — SHA-256 checksums + GitHub SLSA build provenance attestations.

**Full Changelog**: https://github.com/rager306/2brain/compare/v0.5.2...v0.5.3

---

## [v0.5.2] - 2026-11-28

### Highlights
- **3-field BM25 enrichment & type-aware freshness (`tb-search`, `tb-app`):** Index `keywords`, `search_questions`, and `aliases` alongside body text, with configurable type-aware freshness decay (`durable` types do not decay; transient notes use half-life decay).
- **Offline note enrichment & deterministic cache (`2brain enrich`, `scripts/enrich_llm.py`):** Added `2brain enrich plan|heuristic|apply|evaluate` backed by a fail-closed `(note_hash, model, prompt_version)` JSONL cache, safe frontmatter splicing (`O_CREAT|O_EXCL|O_NOFOLLOW` + `renameat` + `fsync`), and an external tooling driver supporting both OpenAI-compatible and Anthropic-compatible endpoints gated by `2brain experiment evaluate-arm`.
- **`error-fix` ingest atoms (`tb-app`, `tb-mcp`):** Added canonical `--format error-fix` ingestion with normalized error-signature deduplication, `occurrences` accumulation, and `freshness: "durable"` output.
- **MCP `resources/subscribe` push notifications — in-process & daemon Unix-socket (`tb-ipc`, `tb-mcp`, `two-brain`):**
  - Added `ResourceUpdateHub` supporting both legacy `resources/subscribe` and MCP 2.0 `2026-07-28` `subscriptions/listen` over `2brain://<repo>/<path>` URIs, bounded per-session fan-out (`Weak` + `DropGuard`), and full `EgressPolicy` revocation enforcement.
  - Added daemon-side `IpcEventHub`, `serve_unix_socket_with_events`, monotonic `last_published` snapshot-delta publishing across watcher reloads and non-replayed mutations, and reconnecting `spawn_resource_event_bridge_with_initial` with upfront capability probing and envelope isolation.
- **Feature-gated U14 hybrid reranker (`--features hybrid-rerank`, `tb-search`, `tools/eval/`):** Pure-domain, zero-I/O Miri-clean static token embedding reranker (`StaticEmbeddingModel`, `HybridDocVectors`, `rerank_hits`) with `SEM-48` negation/contraction polarity guard, strict RAM ceiling enforcement, zero-dependency Model2Vec `safetensors` exporter (`tools/eval/export_model2vec.py`), and paired G5 evaluation (`2brain experiment hybrid-eval`).
- **Prebuilt auditable release pipeline (`.github/workflows/release.yml`):** Automated `cargo-auditable` release binary packaging, CycloneDX 1.5 SBOM generation, and SHA-256 verification manifests.
- **Semantic & security verification:** Expanded the black-box semantic suite to 21 E0/G1–G5 cases (110 assertions + `SEM-48` control), rejected `/` and `..` in `Locator::new` repository slugs, and recorded defect classes #21–#22 in `CODING_STANDARDS.md`.

### Prebuilt Assets (`x86_64-unknown-linux-gnu`)
- `2brain-x86_64-unknown-linux-gnu.tar.gz` — standalone `cargo-auditable` release binary.
- `2brain-x86_64-unknown-linux-gnu.cdx.json` — CycloneDX 1.5 Software Bill of Materials (SBOM).
- `SHA256SUMS.txt` / `2brain-x86_64-unknown-linux-gnu.sha256` — SHA-256 checksums.

```sh
sha256sum -c SHA256SUMS.txt
tar -xzf 2brain-x86_64-unknown-linux-gnu.tar.gz
./2brain --version
```

**Full Changelog**: https://github.com/rager306/2brain/compare/v0.5.1...v0.5.2

---

## [v0.5.1] - 2026-11-28

### Highlights
- **Description quality audit & inline `tb_ingest`:** Added `2brain check --audit-descriptions` (`missing`, `weak`, `stale` descriptions) and the opt-in fail-closed `tb_ingest` MCP tool (`--allow-ingest`) with SEM-02 prior-read enforcement.
- **Revocation, egress boundaries & hash-chained audit log (`--policy`, `--audit-log`):** Enforced `EgressPolicy` (`allowed_repos`, `revoked_repos`, `allow_external_egress`) across tools, resources, prompts, skills, and `tb_context`, backed by a hash-chained JSONL audit log and `2brain outcomes` CLI.
- **SHA-256 backup/restore & black-box semantic harness:** Added `2brain backup` / `2brain restore` (symlink-safe, atomic staging, manifest verification), `tools/check_capabilities.py`, and `tools/semantic/run_semantic_suite.py`.
- **Durable cross-client workflows (`--allow-workflows`):** Added D08 `TaskPacket` (`2brain task`), D09/D10 declarative `ProcedureSpec` + `RunReceipt` (`2brain procedure`), and bounded investigation sessions.
- **G5 evaluation gates (`2brain experiment`):** Added U13 experience promotion (`evaluate-lesson`), U14 paired arm evaluation (`evaluate-arm`), and diagnostic origin-deduplicated Personalized PageRank (`ppr`).

**Full Changelog**: https://github.com/rager306/2brain/compare/v0.5.0...v0.5.1

---

## [v0.5.0] - 2026-11-28

### Highlights
- **Clean-room `2brain ingest` pipeline:** Fail-closed conversion of `.md`, `.txt`, `.html`, `.csv`, `.json`, and `.jsonl` into typed Markdown notes with zero-TOCTOU `openat(O_NOFOLLOW)` reads, schema/link validation, BM25 near-duplicate detection, and atomic multi-note Git commits (`FsWorkspaceStore::commit_ingest_batch`).
- **Hardened Streamable HTTP deployment template:** Added production-ready `systemd` unit templates and documentation for loopback/authenticated Streamable HTTP MCP 2.0 serving.

**Full Changelog**: https://github.com/rager306/2brain/compare/v0.4.0...v0.5.0

---

## [v0.4.0] - 2026-11-28

### Highlights
- **Streamable HTTP MCP 2.0 transport (`--http`):** Added stateless MCP `2026-07-28` + optional legacy SSE sessions (`--http-legacy-sessions`), Bearer auth, DNS-rebinding `Host`/`Origin` validation, and bounded POST body timeouts.
- **Incremental search & gap-detection speedup:** Added incremental `SearchIndex` updates with deterministic `Locator` tie-breaking and tombstone compaction (~8x faster incremental reload, ~3.2x faster structural gap detection).
- **Read-only benchmark CLI:** Added `2brain bench` measuring cold load, incremental refresh, BM25 query latency, cognitive analytics suite, and RSS.

**Full Changelog**: https://github.com/rager306/2brain/compare/v0.3.0...v0.4.0

---

## [v0.3.0] - 2026-11-28

### Highlights
- **Full 54-tool MCP catalog & multi-member `workspace.yaml`:** Completed all 54 core tools (38 read-only, 16 gated mutating) plus multi-repository workspace composition.
- **Persistent Negative Memory & SEP-2663 elicitation:** Added fail-closed `review-decisions.jsonl` persistence (`tb_record_decision`, `tb_list_decisions`), full tool schemas (`--tools full`), structured error recovery hints, and interactive elicitation.
- **Incremental watcher refresh & overflow safety:** Added single-file incremental snapshot reload (`tb_fs_git::refresh_snapshot_incremental`), overflow-safe `notify` channels with atomic full-reload fallback, and Miri-clean randomized property tests in `tb-syntax` and `tb-graph`.

**Full Changelog**: https://github.com/rager306/2brain/compare/v0.2.1...v0.3.0

---

## [v0.2.1] - 2026-11-28

### Highlights
- **Inotify watcher self-trigger fix:** Filtered `notify::recommended_watcher` strictly to mutating filesystem events (`Create`, `Remove`, `Modify(Data|Name|Any|Other)`, `Access(Close(Write))`), ignoring `Access(Open/Read/Close(NoWrite))` and `Modify(Metadata)` (`atime`) updates that caused a 100% CPU reload loop.

**Full Changelog**: https://github.com/rager306/2brain/compare/v0.2.0...v0.2.1

---

## [v0.2.0] - 2026-11-28

### Highlights
- **Clean-room MIT hexagonal monorepo (13 crates, 1 binary):** Pure zero-I/O Miri-clean domain crates (`tb-contracts`, `tb-syntax`, `tb-schema`, `tb-graph`, `tb-search`, `tb-cognition`), application core (`tb-app`), zero-TOCTOU dirfd-confined Git/FS store (`tb-fs-git`), durable outcome journal (`tb-journal`), SEP-2640 skills (`tb-skills`), Unix-socket IPC daemon (`tb-ipc`), and MCP 2.0 server (`tb-mcp`).
- **Cognitive & epistemic analytics:** Pure CSR Louvain/betweenness topology (`2brain analyze`), D03/R14 temporal claims & contradiction auditing (`2brain audit-claims`), hub fragility (`2brain fragility`), and historical Git diff graphs (`2brain diff`).
- **Fail-closed write safety:** SEM-02 full-read gate (stateful `ReadLedger` + stateless CSPRNG HMAC `read_receipt`), interprocess `write.lock`, CAS SHA-256 verification, and durable outcome registry.
