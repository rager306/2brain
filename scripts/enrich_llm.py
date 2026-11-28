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
DEFAULT_SYSTEM_PROMPT = (
    "You enrich technical Markdown knowledge notes for bilingual BM25 retrieval. "
    "Return ONLY valid JSON with two string arrays: "
    '{"keywords": ["6 to 10 concise domain terms and plain-word synonyms"], '
    '"search_questions": ["2 to 4 natural questions this note answers"]}. '
    "If the note is written in a non-English language (e.g. Russian) or mixes languages, "
    "include concise equivalents in BOTH the note's language and English so cross-lingual queries match. "
    "Preserve negations/prohibitions accurately and never invent facts absent from the note."
)


def run_cmd(cmd: list[str]) -> str:
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(
            f"command failed ({proc.returncode}): {' '.join(cmd)}\n{proc.stderr.strip()}"
        )
    return proc.stdout


def _extract_json_object(text: str, note_path: object) -> dict[str, object]:
    cleaned = text.strip()
    if cleaned.startswith("```"):
        lines = cleaned.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        cleaned = "\n".join(lines).strip()
    try:
        parsed = json.loads(cleaned)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"LLM response for {note_path} was not valid JSON: {exc}") from exc
    if not isinstance(parsed, dict):
        raise RuntimeError(f"LLM response for {note_path} was not a JSON object")
    return parsed


def _parse_enrichment_fields(parsed: dict[str, object], note_path: object) -> tuple[list[str], list[str]]:
    raw_kw = parsed.get("keywords", [])
    raw_sq = parsed.get("search_questions", [])
    if not isinstance(raw_kw, list) or not isinstance(raw_sq, list):
        raise RuntimeError(
            f"LLM response for {note_path} must return JSON arrays for keywords and search_questions"
        )
    keywords = [str(x).strip() for x in raw_kw if isinstance(x, str) and x.strip()]
    questions = [str(x).strip() for x in raw_sq if isinstance(x, str) and x.strip()]
    return keywords[:16], questions[:8]


def _http_post_json(
    url: str, payload: dict[str, object], headers: dict[str, str], timeout: float, label: str
) -> dict[str, object]:
    req = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        with exc:
            err_body = ""
            try:
                err_body = exc.read(512).decode("utf-8", errors="replace").strip()
            except OSError:
                pass
        raise RuntimeError(f"{label} HTTP {exc.code}: {err_body or exc.reason}") from exc
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        raise RuntimeError(f"{label} request failed: {exc}") from exc

    try:
        body = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"{label} returned invalid JSON body: {exc}") from exc
    if not isinstance(body, dict):
        raise RuntimeError(f"{label} returned non-object JSON payload")
    return body


def call_openai_compatible(
    base_url: str,
    api_key: str,
    model: str,
    item: dict[str, object],
    timeout: float,
    system_prompt: str,
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
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_msg},
        ],
    }
    body = _http_post_json(
        url,
        payload,
        {"Content-Type": "application/json", "Authorization": f"Bearer {api_key}"},
        timeout,
        f"OpenAI-compatible API ({item.get('path')})",
    )
    choices = body.get("choices")
    if not isinstance(choices, list) or not choices or not isinstance(choices[0], dict):
        raise RuntimeError(f"OpenAI-compatible response for {item.get('path')} missing choices[0]")
    msg = choices[0].get("message")
    content = msg.get("content") if isinstance(msg, dict) else None
    if not isinstance(content, str) or not content.strip():
        raise RuntimeError(f"empty assistant message content for {item.get('path')}")
    parsed = _extract_json_object(content, item.get("path"))
    return _parse_enrichment_fields(parsed, item.get("path"))


def call_anthropic_compatible(
    base_url: str,
    api_key: str,
    model: str,
    item: dict[str, object],
    timeout: float,
    system_prompt: str,
) -> tuple[list[str], list[str]]:
    clean_base = base_url.rstrip("/")
    url = f"{clean_base}/messages" if clean_base.endswith("/v1") else f"{clean_base}/v1/messages"
    user_msg = (
        f"Title: {item.get('title', '')}\n"
        f"Description: {item.get('description') or ''}\n\n"
        f"Body excerpt:\n{item.get('body_excerpt', '')}"
    )
    payload = {
        "model": model,
        "max_tokens": 1024,
        "stream": False,
        "temperature": 0.0,
        "system": system_prompt,
        "messages": [{"role": "user", "content": user_msg}],
    }
    body = _http_post_json(
        url,
        payload,
        {
            "Content-Type": "application/json",
            "x-api-key": api_key,
            "Authorization": f"Bearer {api_key}",
            "anthropic-version": "2023-06-01",
        },
        timeout,
        f"Anthropic-compatible API ({item.get('path')})",
    )
    blocks = body.get("content")
    if not isinstance(blocks, list):
        raise RuntimeError(f"Anthropic-compatible response for {item.get('path')} missing content list")
    text_parts = [
        b["text"]
        for b in blocks
        if isinstance(b, dict) and b.get("type") == "text" and isinstance(b.get("text"), str)
    ]
    content = "\n".join(text_parts).strip()
    if not content:
        raise RuntimeError(f"empty text content from LLM for {item.get('path')}")
    parsed = _extract_json_object(content, item.get("path"))
    return _parse_enrichment_fields(parsed, item.get("path"))


def _open_cache_append_nofollow(cache_path: Path):
    flags = os.O_RDWR | os.O_CREAT | os.O_APPEND
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd = os.open(str(cache_path), flags, 0o600)
    return os.fdopen(fd, "r+b")


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
    parser.add_argument(
        "--provider",
        choices=["auto", "openai", "anthropic"],
        default="auto",
        help="LLM API protocol for --mode llm (auto detects OPENAI_API_KEY vs ANTHROPIC_AUTH_TOKEN/ANTHROPIC_API_KEY)",
    )
    parser.add_argument("--model", default="heuristic-v1")
    parser.add_argument("--prompt-version", default="v1")
    parser.add_argument("--repo", default=None)
    parser.add_argument(
        "--path-prefix",
        action="append",
        default=[],
        help="Optional path prefix filter for --mode llm (repeatable)",
    )
    parser.add_argument("--only-missing", action="store_true")
    parser.add_argument("--overwrite", action="store_true")
    parser.add_argument("--suite", type=Path, default=None, help="Paired G5 evaluation suite JSON")
    parser.add_argument("--top-k", type=int, default=5)
    parser.add_argument(
        "--min-should-match",
        type=int,
        default=None,
        help="Optional explicit BM25 min_should_match passed to `2brain enrich evaluate`",
    )
    parser.add_argument(
        "--system-prompt-file",
        type=Path,
        default=None,
        help="Optional external system prompt text file (overrides built-in bilingual default)",
    )
    parser.add_argument("--apply", action="store_true", help="Write changes (default is dry-run)")
    parser.add_argument("--max-items", type=int, default=100)
    parser.add_argument("--timeout", type=float, default=30.0)
    args = parser.parse_args()
    if args.max_items < 0:
        print("error: --max-items must be >= 0", file=sys.stderr)
        return 2
    system_prompt = (
        args.system_prompt_file.read_text(encoding="utf-8").strip()
        if args.system_prompt_file is not None
        else DEFAULT_SYSTEM_PROMPT
    )

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
        openai_key = os.environ.get("OPENAI_API_KEY", "").strip()
        anthropic_key = (
            os.environ.get("ANTHROPIC_AUTH_TOKEN", "").strip()
            or os.environ.get("ANTHROPIC_API_KEY", "").strip()
        )
        provider = args.provider
        if provider == "auto":
            if openai_key:
                provider = "openai"
            elif anthropic_key:
                provider = "anthropic"
            else:
                print(
                    "error: set OPENAI_API_KEY or ANTHROPIC_AUTH_TOKEN/ANTHROPIC_API_KEY for --mode llm",
                    file=sys.stderr,
                )
                return 2
        if provider == "openai":
            if not openai_key:
                print("error: OPENAI_API_KEY is required for --provider openai", file=sys.stderr)
                return 2
            api_key = openai_key
            base_url = os.environ.get("OPENAI_BASE_URL", "").strip() or "https://api.openai.com/v1"
            caller = call_openai_compatible
        else:
            if not anthropic_key:
                print(
                    "error: ANTHROPIC_AUTH_TOKEN or ANTHROPIC_API_KEY is required for --provider anthropic",
                    file=sys.stderr,
                )
                return 2
            api_key = anthropic_key
            base_url = os.environ.get("ANTHROPIC_BASE_URL", "").strip() or "https://api.anthropic.com"
            caller = call_anthropic_compatible

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
        pending = [i for i in plan["items"] if i["status"] == "pending"]
        if args.path_prefix:
            pending = [
                i
                for i in pending
                if any(str(i.get("path", "")).startswith(p) for p in args.path_prefix)
            ]
        pending = pending[: args.max_items]
        print(
            f"[enrich] llm({provider}/{args.model}) plan: total={plan['total_notes']} "
            f"cached={plan['cached_hits']} pending={plan['pending_misses']} processing={len(pending)}"
        )
        if pending:
            with _open_cache_append_nofollow(cache_path) as fh:
                fh.seek(0, os.SEEK_END)
                needs_leading_nl = False
                if fh.tell() > 0:
                    fh.seek(-1, os.SEEK_END)
                    needs_leading_nl = fh.read(1) != b"\n"
                for item in pending:
                    kws, qs = caller(
                        base_url, api_key, args.model, item, args.timeout, system_prompt
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
                    prefix = b"\n" if needs_leading_nl else b""
                    needs_leading_nl = False
                    fh.write(prefix + (json.dumps(rec, ensure_ascii=False) + "\n").encode("utf-8"))
                    fh.flush()

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
        if args.overwrite:
            eval_cmd.append("--overwrite")
        if args.min_should_match is not None:
            eval_cmd.extend(["--min-should-match", str(args.min_should_match)])
        eval_payload = json.loads(run_cmd(eval_cmd))
        arm_input_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8") as tf:
                arm_input_path = Path(tf.name)
                json.dump(
                    {
                        "policy": eval_payload["policy"],
                        "outcomes": eval_payload["outcomes"],
                        "null_z_score": None,
                    },
                    tf,
                )
            arm_out = json.loads(
                run_cmd([args.bin, "experiment", "evaluate-arm", str(arm_input_path), "--json"])
            )
        finally:
            if arm_input_path is not None:
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
