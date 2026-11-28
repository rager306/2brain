#!/usr/bin/env python3
"""Offline note enrichment driver (Tooling layer, strictly outside MCP serving path).

Workflow (Backlog P1-6):
1. Calls `2brain enrich plan` to identify notes whose body SHA-256 has no cache entry
   for `(note_hash, model, prompt_version)`.
2. Populates missing cache entries via EITHER:
   - `--mode llm`: OpenAI-compatible chat completions endpoint (`OPENAI_BASE_URL` / `OPENAI_API_KEY`)
     returning strict JSON `{"keywords": [...], "search_questions": [...]}`.
   - `--mode heuristic`: delegates to `2brain enrich heuristic` (zero-API deterministic baseline).
3. If `--suite <queries.json>` is supplied, runs paired G5 evaluation via `2brain enrich evaluate`
   AND pipes the paired payload through `2brain experiment evaluate-arm`; refuses `--apply`
   fail-closed unless the verdict is `promote_candidate`.
4. Applies cached enrichment via `2brain enrich apply` (dry-run by default; pass `--apply` to write).
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

SCHEMA_VERSION = "2brain.enrichment-cache/v1"
SYSTEM_PROMPT = (
    "You enrich technical Markdown knowledge notes for BM25 retrieval. "
    "Return ONLY valid JSON with two string arrays: "
    '{"keywords": ["5 to 8 concise domain terms"], '
    '"search_questions": ["2 to 4 natural questions this note answers"]}. '
    "Do not invent facts absent from the note."
)


def run_cmd(cmd: list[str]) -> str:
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(
            f"command failed ({proc.returncode}): {' '.join(cmd)}\n{proc.stderr.strip()}"
        )
    return proc.stdout


def call_openai_compatible(
    base_url: str, api_key: str, model: str, item: dict[str, object], timeout: float
) -> tuple[list[str], list[str]]:
    url = base_url.rstrip("/") + "/chat/completions"
    user_msg = (
        f"Title: {item.get('title', '')}\n"
        f"Description: {item.get('description') or ''}\n\n"
        f"Body excerpt:\n{item.get('body_excerpt', '')}"
    )
    payload = {
        "model": model,
        "temperature": 0.0,
        "response_format": {"type": "json_object"},
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_msg},
        ],
    }
    req = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {api_key}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = json.loads(resp.read().decode("utf-8"))
    except urllib.error.URLError as exc:
        raise RuntimeError(f"LLM API request failed for {item.get('path')}: {exc}") from exc

    content = body["choices"][0]["message"]["content"]
    parsed = json.loads(content)
    if not isinstance(parsed, dict):
        raise RuntimeError(f"LLM response for {item.get('path')} was not a JSON object")
    raw_kw = parsed.get("keywords", [])
    raw_sq = parsed.get("search_questions", [])
    if not isinstance(raw_kw, list) or not isinstance(raw_sq, list):
        raise RuntimeError(
            f"LLM response for {item.get('path')} must return JSON arrays for keywords and search_questions"
        )
    keywords = [str(x).strip() for x in raw_kw if isinstance(x, str) and x.strip()]
    questions = [str(x).strip() for x in raw_sq if isinstance(x, str) and x.strip()]
    return keywords[:16], questions[:8]


def main() -> int:
    parser = argparse.ArgumentParser(description="Offline 2brain note enrichment driver")
    parser.add_argument("workspace", type=Path, help="Path to 2brain workspace")
    parser.add_argument("--bin", default="2brain", help="Path to 2brain binary")
    parser.add_argument(
        "--cache",
        type=Path,
        default=None,
        help="Path to enrichment JSONL cache (default: <workspace>/.2brain/cache/enrichment.jsonl)",
    )
    parser.add_argument("--mode", choices=["heuristic", "llm"], default="heuristic")
    parser.add_argument("--model", default="heuristic-v1")
    parser.add_argument("--prompt-version", default="v1")
    parser.add_argument("--repo", default=None)
    parser.add_argument("--only-missing", action="store_true")
    parser.add_argument("--overwrite", action="store_true")
    parser.add_argument("--suite", type=Path, default=None, help="Paired G5 evaluation suite JSON")
    parser.add_argument("--top-k", type=int, default=5)
    parser.add_argument("--apply", action="store_true", help="Write changes (default is dry-run)")
    parser.add_argument("--max-items", type=int, default=100)
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()

    cache_path = args.cache or (args.workspace / ".2brain" / "cache" / "enrichment.jsonl")
    cache_path.parent.mkdir(parents=True, exist_ok=True)

    if args.mode == "heuristic":
        cmd = [
            args.bin,
            "enrich",
            "heuristic",
            str(args.workspace),
            "--cache",
            str(cache_path),
            "--model",
            args.model,
            "--prompt-version",
            args.prompt_version,
            "--json",
        ]
        if args.repo:
            cmd.extend(["--repo", args.repo])
        if args.only_missing:
            cmd.append("--only-missing")
        heur_out = json.loads(run_cmd(cmd))
        print(
            f"[enrich] heuristic cache ready: generated={heur_out['generated']} "
            f"hits={heur_out['cache_hits']} total={heur_out['total_cache_entries']}"
        )
    else:
        api_key = os.environ.get("OPENAI_API_KEY", "").strip()
        base_url = os.environ.get("OPENAI_BASE_URL", "https://api.openai.com/v1").strip()
        if not api_key:
            print("error: OPENAI_API_KEY is required for --mode llm", file=sys.stderr)
            return 2
        plan_cmd = [
            args.bin,
            "enrich",
            "plan",
            str(args.workspace),
            "--cache",
            str(cache_path),
            "--model",
            args.model,
            "--prompt-version",
            args.prompt_version,
            "--json",
        ]
        if args.repo:
            plan_cmd.extend(["--repo", args.repo])
        if args.only_missing:
            plan_cmd.append("--only-missing")
        plan = json.loads(run_cmd(plan_cmd))
        pending = [i for i in plan["items"] if i["status"] == "pending"][: args.max_items]
        print(
            f"[enrich] llm plan: total={plan['total_notes']} cached={plan['cached_hits']} "
            f"pending={plan['pending_misses']} processing={len(pending)}"
        )
        if pending:
            needs_leading_nl = False
            if cache_path.exists() and cache_path.stat().st_size > 0:
                with cache_path.open("rb") as rf:
                    rf.seek(-1, os.SEEK_END)
                    needs_leading_nl = rf.read(1) != b"\n"
            with cache_path.open("a", encoding="utf-8") as fh:
                if needs_leading_nl:
                    fh.write("\n")
                for item in pending:
                    kws, qs = call_openai_compatible(
                        base_url, api_key, args.model, item, args.timeout
                    )
                    rec = {
                        "schema_version": SCHEMA_VERSION,
                        "repo": item["repo"],
                        "path": item["path"],
                        "note_hash": item["note_hash"],
                        "model": args.model,
                        "prompt_version": args.prompt_version,
                        "keywords": kws,
                        "search_questions": qs,
                    }
                    fh.write(json.dumps(rec, ensure_ascii=False) + "\n")

    if args.suite is not None:
        eval_cmd = [
            args.bin,
            "enrich",
            "evaluate",
            str(args.workspace),
            "--cache",
            str(cache_path),
            "--model",
            args.model,
            "--prompt-version",
            args.prompt_version,
            "--suite",
            str(args.suite),
            "--top-k",
            str(args.top_k),
            "--json",
        ]
        eval_payload = json.loads(run_cmd(eval_cmd))
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8") as tf:
            json.dump(
                {
                    "policy": eval_payload["policy"],
                    "outcomes": eval_payload["outcomes"],
                    "null_z_score": None,
                },
                tf,
            )
            arm_input_path = Path(tf.name)
        try:
            arm_out = json.loads(
                run_cmd([args.bin, "experiment", "evaluate-arm", str(arm_input_path), "--json"])
            )
        finally:
            arm_input_path.unlink(missing_ok=True)
        verdict = arm_out["verdict"]
        print(
            f"[enrich] G5 evaluate-arm verdict={verdict} tasks={arm_out['task_count']} "
            f"base_rate={arm_out['baseline_fulfillment_rate']} "
            f"cand_rate={arm_out['candidate_fulfillment_rate']} "
            f"base_mrr={arm_out['baseline_mean_rank']} cand_mrr={arm_out['candidate_mean_rank']}"
        )
        if args.apply and verdict != "promote_candidate":
            print(
                f"error: refusing --apply because G5 evaluate-arm verdict is '{verdict}'",
                file=sys.stderr,
            )
            return 1

    apply_cmd = [
        args.bin,
        "enrich",
        "apply",
        str(args.workspace),
        "--cache",
        str(cache_path),
        "--model",
        args.model,
        "--prompt-version",
        args.prompt_version,
        "--json",
    ]
    if args.repo:
        apply_cmd.extend(["--repo", args.repo])
    if args.overwrite:
        apply_cmd.append("--overwrite")
    if not args.apply:
        apply_cmd.append("--dry-run")
    apply_out = json.loads(run_cmd(apply_cmd))
    print(
        f"[enrich] apply dry_run={apply_out['dry_run']} planned={apply_out['planned_writes']} "
        f"written={apply_out['written']} errors={len(apply_out['errors'])}"
    )
    return 0 if apply_out["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
