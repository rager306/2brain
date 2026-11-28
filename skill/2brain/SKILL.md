---
name: 2brain
description: Navigate, search, audit, enrich, ingest, and safely mutate a typed Markdown knowledge graph using the 2brain CLI and MCP 2.0 server. Includes a done-when workflow checklist and SEM-02/SEM-09 safety rules.
---

# 2brain Agent Guide

`2brain` is a clean-room MIT single-binary typed knowledge graph and MCP 2.0 server for Markdown vaults with YAML frontmatter, `[[wiki-links]]`, `^block-ids`, and fail-closed write safety.

Offline, run `2brain guide` for this overview or `2brain guide <topic>` (`search`, `mutate`, `enrich`, `ingest`, `workflows`, `eval`). Pass `--json` to any CLI command whose output you parse.

## Core Invariants (Never Violate)

1. **SEM-02 (Read before write):** Partial `tb_content` reads NEVER arm mutations. Always read a note completely (getting a `read_receipt` + SHA-256) before `tb_write_file`, `tb_edit_file`, `tb_delete_file`, or `tb_rename`.
2. **SEM-09 (BM25 vs literal grep):** Use `tb_search` for ranked multi-term/conceptual lookup (4-field BM25 over `body`, `description`, `keywords`, `search_questions` + freshness). Use `tb_grep` for exact literal substrings, identifiers, or error codes.
3. **Fail-closed manifests:** `workspace.yaml` owns multi-member composition and workspace-wide `search:` weights/freshness; `.2brain/repo.yaml` is the optional per-repo identity (`name`, `deps`, and single-repo `search:` fallback).
4. **Offline enrichment & G5 gates:** Never run LLMs on the MCP serving path. Enrich notes offline (`2brain enrich plan` + `scripts/enrich_llm.py`), gate changes with paired `2brain experiment evaluate-arm`, and apply atomically via `2brain enrich apply`.

## Agent Workflow Checklist (with `done-when`)

- [ ] **1. Orient (`2brain wakeup <ws>` or `tb_wakeup` / `tb_overview`)** — *done when* you know note count, top clusters, hubs, and whether diagnostics/broken links exist.
- [ ] **2. Retrieve (`tb_search` + `tb_grep` + `tb_neighborhood`)** — *done when* candidate paths are verified and cross-checked with exact grep for identifiers.
- [ ] **3. Read Whole (`tb_content`)** — *done when* `truncated == false` and you hold `sha256` + `read_receipt` for every note you plan to modify.
- [ ] **4. Mutate or Ingest (`tb_edit_file` / `2brain ingest --dry-run` first)** — *done when* dry-run reports `0` schema/link errors and apply commits cleanly.
- [ ] **5. Verify (`2brain check <ws> --audit-descriptions --json`)** — *done when* `schema == 0`, `utf8_errors == 0`, and no new broken links or weak descriptions were introduced.
