#!/usr/bin/env python3
"""Reproducible G5 paired experiments for Backlog P1-6 (offline enrichment) & U14 (hybrid-rerank).

Creates an isolated temporary 2brain workspace (never touches /root/vault) containing:
- Multi-family technical notes (security, networking, storage, error-fix, freshness/negation controls)
- Paraphrase queries where lexical BM25 misses or ranks poorly without enrichment / semantic rerank
- SEM-48 negation & qualifier adversarial controls where opposite-polarity notes must NEVER rank #1

Runs:
1. Offline enrichment arm (`2brain enrich heuristic` -> `2brain enrich evaluate` -> `2brain experiment evaluate-arm`)
2. Hybrid static-embedding reranker arm (`2brain experiment hybrid-eval` -> `2brain experiment evaluate-arm`)
3. Ablation without SEM-48 negation guard (`--no-negation-guard`) to verify the guard actually protects negation controls.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


CORPUS_NOTES: dict[str, str] = {
    "notes/socket-lock.md": """---
type: pattern
description: "Prevent Unix socket race conditions during daemon startup."
status: current
freshness: durable
---

# Socket Lock Startup Discipline

Acquire an exclusive flock via fs4 on `<socket>.lock` before unlinking any stale Unix domain socket path.
Set directory permissions to 0700 and bound socket mode to 0600 so concurrent startups never clobber a live listener.
""",
    "notes/backoff-jitter.md": """---
type: pattern
description: "Prevent thundering herds on transient upstream timeouts."
status: current
freshness: durable
---

# Exponential Backoff with Full Jitter

When upstream RPC endpoints return 429 or 503, sleep for a randomized delay drawn uniformly from `[0, min(cap, base * 2^attempt)]`.
Jitter decorrelates clients after an outage so synchronized waves do not knock the service back down.
""",
    "notes/dirfd-confinement.md": """---
type: pattern
description: "Walk workspace paths component-by-component with openat O_NOFOLLOW."
status: current
freshness: durable
---

# Zero-TOCTOU Directory Descriptor Confinement

Never validate a path with canonicalize and reopen by pathname.
Workspace confinement must never follow symlink escapes outside the member root; open every segment with openat O_NOFOLLOW and verify fstat on the open descriptor.
""",
    "notes/symlink-guide-positive.md": """---
type: guide
description: "Overview of symlink path handling and symlink traversal mechanics."
status: current
freshness: durable
---

# Symlink Traversal Reference

Symlink traversal symlink escape symlink path resolution notes covering POSIX symlink behavior and directory entries.
""",
    "notes/audit-chain.md": """---
type: pattern
description: "Append-only SHA-256 hash-chained audit trail for write actions."
status: current
freshness: durable
---

# Tamper-Evident Hash-Chained Audit Log

Every mutating action appends a JSONL entry containing prev_hash and entry_hash computed over canonical fields.
Validate inode identity and trailing newline before dispatching any mutation; fail closed if tampered.
""",
    "notes/error-fix-eaddrinuse.md": """---
type: error-fix
description: "Error-fix atom for EADDRINUSE during daemon restart."
status: current
freshness: durable
error_signature: "error failed to bind unix socket eaddrinuse"
occurrences: 4
---

# Error Fix: Daemon Bind EADDRINUSE

## Symptom
Daemon fails on restart with EADDRINUSE after unclean crash.

## Root Cause
Stale socket file left on disk without exclusive lock check.

## Fix
Hold an exclusive fs4 lock on `<socket>.lock` prior to unlinking stale socket paths.
""",
    "notes/allow-external-egress-forbidden.md": """---
type: rule
description: "External egress boundary policy when allow_external_egress is disabled."
status: current
freshness: durable
---

# Egress Boundary Rule

When `allow_external_egress` is false, the server must never transmit vault fragments to non-local cloud destinations.
Any tool, skill, prompt, or resource subscription targeting a blocked destination fails closed.
""",
    "notes/cloud-egress-guide.md": """---
type: guide
description: "Cloud egress destination configuration guide for cloud telemetry."
status: current
freshness: durable
---

# Cloud Egress Configuration

Cloud egress destination labels describe external destinations such as cloud telemetry endpoints and cloud providers.
""",
    "notes/stale-retry-draft.md": """---
type: inbox
description: "Old draft note about retry intervals."
status: superseded
freshness: transient
date: 2024-01-01
---

# Old Retry Draft

Superseded draft mentioning fixed retry intervals without jitter.
""",
}

SUITE = {
    "experiment_id": "u14-g5-retrieval-and-enrich-v1",
    "rollback_ref": "main",
    "min_tasks": 6,
    "min_families": 3,
    "min_utility_gain": 0.0,
    "max_token_overhead_ratio": 1.25,
    "synonym_clusters": [
        ["retry", "backoff", "jitter", "reattempt", "thundering", "429", "503"],
        ["daemon", "socket", "ipc", "flock", "eaddrinuse", "listener", "startup"],
        ["tamper", "audit", "hash", "chain", "ledger", "immutable", "integrity"],
        ["symlink", "confinement", "traversal", "escape", "openat", "nofollow"],
        ["egress", "cloud", "exfiltration", "external", "boundary", "local"],
    ],
    "tasks": [
        {
            "task_id": "t-backoff-paraphrase",
            "family": "resilience",
            "query": "reattempt 429 outage wave",
            "expected_paths": ["notes/backoff-jitter.md"],
            "forbidden_top1_paths": ["notes/stale-retry-draft.md"],
            "budget_cap_tokens": 4000,
        },
        {
            "task_id": "t-socket-race",
            "family": "concurrency",
            "query": "daemon startup listener race flock",
            "expected_paths": ["notes/socket-lock.md"],
            "forbidden_top1_paths": [],
            "budget_cap_tokens": 4000,
        },
        {
            "task_id": "t-audit-tamper",
            "family": "security",
            "query": "tamper evident ledger integrity",
            "expected_paths": ["notes/audit-chain.md"],
            "forbidden_top1_paths": [],
            "budget_cap_tokens": 4000,
        },
        {
            "task_id": "t-error-fix-bind",
            "family": "operations",
            "query": "eaddrinuse crash restart bind",
            "expected_paths": ["notes/error-fix-eaddrinuse.md"],
            "forbidden_top1_paths": [],
            "budget_cap_tokens": 4000,
        },
        {
            "task_id": "t-sem48-neg-symlink",
            "family": "negation-control",
            "query": "never follow symlink escape",
            "expected_paths": ["notes/dirfd-confinement.md"],
            "forbidden_top1_paths": ["notes/symlink-guide-positive.md"],
            "budget_cap_tokens": 4000,
        },
        {
            "task_id": "t-sem48-neg-egress",
            "family": "negation-control",
            "query": "never transmit external cloud destination",
            "expected_paths": ["notes/allow-external-egress-forbidden.md"],
            "forbidden_top1_paths": ["notes/cloud-egress-guide.md"],
            "budget_cap_tokens": 4000,
        },
    ],
}


def run_json(cmd: list[str]) -> dict:
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(
            f"command failed ({proc.returncode}): {' '.join(cmd)}\n{proc.stderr.strip()}"
        )
    return json.loads(proc.stdout)


def evaluate_via_cli(bin_path: str, eval_payload: dict) -> dict:
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8") as fh:
        json.dump(
            {
                "policy": eval_payload["policy"],
                "outcomes": eval_payload["outcomes"],
                "null_z_score": None,
            },
            fh,
        )
        arm_path = Path(fh.name)
    try:
        return run_json([bin_path, "experiment", "evaluate-arm", str(arm_path), "--json"])
    finally:
        arm_path.unlink(missing_ok=True)


def main() -> int:
    parser = argparse.ArgumentParser(description="Run isolated U14 & enrichment G5 experiments")
    parser.add_argument("--bin", required=True, help="Path to 2brain binary (built with --features hybrid-rerank)")
    args = parser.parse_args()

    with tempfile.TemporaryDirectory(prefix="2brain-u14-") as tmp:
        root = Path(tmp)
        ws = root / "vault"
        (ws / ".2brain").mkdir(parents=True)
        (ws / ".2brain" / "repo.yaml").write_text("name: vault\n", encoding="utf-8")
        for rel, body in CORPUS_NOTES.items():
            p = ws / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(body, encoding="utf-8")

        suite_path = root / "suite.json"
        suite_path.write_text(json.dumps(SUITE, indent=2), encoding="utf-8")
        cache_path = root / "enrichment.jsonl"

        # 1. Offline Enrichment (heuristic + G5 evaluate-arm)
        run_json(
            [
                args.bin,
                "enrich",
                "heuristic",
                str(ws),
                "--cache",
                str(cache_path),
                "--model",
                "heuristic-v1",
                "--prompt-version",
                "v1",
                "--json",
            ]
        )
        enrich_eval = run_json(
            [
                args.bin,
                "enrich",
                "evaluate",
                str(ws),
                "--cache",
                str(cache_path),
                "--model",
                "heuristic-v1",
                "--prompt-version",
                "v1",
                "--suite",
                str(suite_path),
                "--json",
            ]
        )
        enrich_arm = evaluate_via_cli(args.bin, enrich_eval)

        # 2. Hybrid Reranker (with SEM-48 negation guard)
        hybrid_eval = run_json(
            [
                args.bin,
                "experiment",
                "hybrid-eval",
                str(ws),
                "--suite",
                str(suite_path),
                "--alpha",
                "0.30",
                "--json",
            ]
        )
        hybrid_arm = evaluate_via_cli(args.bin, hybrid_eval)

        # 3. Hybrid Reranker ablation WITHOUT SEM-48 negation guard
        hybrid_noguard_eval = run_json(
            [
                args.bin,
                "experiment",
                "hybrid-eval",
                str(ws),
                "--suite",
                str(suite_path),
                "--alpha",
                "0.30",
                "--no-negation-guard",
                "--json",
            ]
        )
        hybrid_noguard_arm = evaluate_via_cli(args.bin, hybrid_noguard_eval)

        summary = {
            "enrich_heuristic": {
                "enriched_notes": enrich_eval["enriched_notes_in_simulation"],
                "verdict": enrich_arm["verdict"],
                "baseline_fulfillment_rate": enrich_arm["baseline_fulfillment_rate"],
                "candidate_fulfillment_rate": enrich_arm["candidate_fulfillment_rate"],
                "utility_gain": enrich_arm["utility_gain"],
                "baseline_mean_rank": enrich_arm["baseline_mean_rank"],
                "candidate_mean_rank": enrich_arm["candidate_mean_rank"],
                "token_overhead_ratio": enrich_arm["token_overhead_ratio"],
                "notes": enrich_arm["notes"],
            },
            "hybrid_rerank_guarded": {
                "verdict": hybrid_arm["verdict"],
                "dim": hybrid_eval["dim"],
                "estimated_ram_bytes": hybrid_eval["estimated_ram_bytes"],
                "vector_build_ms": round(hybrid_eval["vector_build_ms"], 3),
                "baseline_mean_query_us": round(hybrid_eval["baseline_mean_query_us"], 1),
                "candidate_mean_query_us": round(hybrid_eval["candidate_mean_query_us"], 1),
                "baseline_fulfillment_rate": hybrid_arm["baseline_fulfillment_rate"],
                "candidate_fulfillment_rate": hybrid_arm["candidate_fulfillment_rate"],
                "utility_gain": hybrid_arm["utility_gain"],
                "baseline_mean_rank": hybrid_arm["baseline_mean_rank"],
                "candidate_mean_rank": hybrid_arm["candidate_mean_rank"],
                "token_overhead_ratio": hybrid_arm["token_overhead_ratio"],
                "notes": hybrid_arm["notes"],
            },
            "hybrid_rerank_no_guard_ablation": {
                "verdict": hybrid_noguard_arm["verdict"],
                "baseline_fulfillment_rate": hybrid_noguard_arm["baseline_fulfillment_rate"],
                "candidate_fulfillment_rate": hybrid_noguard_arm["candidate_fulfillment_rate"],
                "baseline_mean_rank": hybrid_noguard_arm["baseline_mean_rank"],
                "candidate_mean_rank": hybrid_noguard_arm["candidate_mean_rank"],
                "notes": hybrid_noguard_arm["notes"],
            },
        }
        print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
