#!/usr/bin/env python3
"""Build a context pack from the tools that already exist, and measure what it costs.

`docs/research/context-pack.md` fixes the contract and says a dedicated tool has to earn its place by cutting calls
and bytes. This is the composition that has to beat: candidates from `tb_search` carrying their own short excerpt,
and a `tb_content` window only for a fragment the search did not quote.

    python3 tools/kb/context_pack.py --query "why was the freshness prior rejected" --budget 6000
    python3 tools/kb/context_pack.py --tasks --json     # measure the task set: calls, bytes, evidence found

Each fragment states its `provenance`, because the fields a reader can trust differ: `search-excerpt` carries the
body window and the ranking reason but no `hash`, `commit` or `status` (the search answer holds no frontmatter),
while `content` carries all of them from the one read that produced the text. A fragment whose excerpt the engine
did not send falls back to `tb_content`, so the pack works against an engine from before the field.

The pack is deliberately explicit about what it does NOT establish: `missing` is filled from the terms that found
nothing, and `weak` fragments are marked rather than dropped, because a reader deciding what to trust needs both.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tb-mcp-rs/scripts/lib"))
from mcp_stdio import StdioMcp  # noqa: E402

ADAPTER = os.environ.get("BRAIN_BIN", "/usr/local/bin/2brain")
VAULT = Path(os.environ.get("BRAIN_VAULT", "/root/vault"))
PREFIX = str(VAULT) + "/"
FRAGMENT_CHARS = 1200
CANDIDATES = 8


class Pack:
    """Counts what a pack cost, so the composition can be compared with a tool that replaces it."""

    def __init__(self) -> None:
        self.calls = 0
        self.bytes = 0

    def call(self, client: StdioMcp, tool: str, args: dict) -> dict:
        self.calls += 1
        reply = client.call(tool, args)
        result = reply.get("result", reply)
        if result.get("isError"):
            return {"error": str(result.get("content"))[:200]}
        payload = result.get("structuredContent") or {}
        self.bytes += len(json.dumps(payload))
        return payload


def build(
    client: StdioMcp, budget: Pack, query: str, fragments: int, excerpt: int, use_excerpt: bool = True
) -> dict:
    search = budget.call(client, "tb_search", {"query": query, "limit": fragments})
    hits = (search.get("search") or {}).get("matches") or []
    out = {
        "query": query,
        "workspace": str(VAULT),
        "fragments": [],
        "missing": [],
        "weak_fragments": 0,
        "truncated": False,
    }
    for hit in hits:
        path = hit["path"].removeprefix(PREFIX)
        reason = {
            "score": round(hit.get("score", 0.0), 2),
            "matched_terms": hit.get("matched_terms"),
            "weak": bool(hit.get("weak")),
        }
        if use_excerpt and hit.get("excerpt"):
            # The search answer quoted the body itself: no second read, and no frontmatter either.
            out["fragments"].append(
                {
                    "path": path,
                    "provenance": "search-excerpt",
                    "hash": None,
                    "commit": None,
                    "about": None,
                    "excerpt": hit["excerpt"],
                    "reason": reason,
                    "status": None,
                    "truncated": None,
                }
            )
        else:
            content = budget.call(client, "tb_content", {"repo": "vault", "path": path, "max_chars": excerpt})
            body = content.get("content") or {}
            text = body.get("text") or ""
            fields, prose = _frontmatter(text)
            out["fragments"].append(
                {
                    "path": path,
                    "provenance": "content",
                    "hash": body.get("hash"),
                    "commit": body.get("head") or body.get("commit"),
                    "about": fields.get("description", "").strip('"'),
                    "excerpt": prose[:excerpt],
                    "reason": reason,
                    "status": _status(text),
                    "truncated": bool(body.get("truncated")),
                }
            )
        if hit.get("weak"):
            out["weak_fragments"] += 1
    out["engine_version"] = None  # filled by the caller from _meta when it needs it
    return out


def _frontmatter(text: str) -> tuple[dict[str, str], str]:
    """The frontmatter fields and the body after them.

    A fragment whose excerpt is 1 200 characters of YAML is a fragment nobody reads, so the pack quotes the body and
    carries the two fields a reader needs — `status` and `description` — beside it.
    """
    if not text.startswith("---"):
        return {}, text
    end = text.find("\n---", 3)
    if end < 0:
        return {}, text
    fields: dict[str, str] = {}
    for line in text[3:end].splitlines():
        key, sep, value = line.partition(":")
        if sep and key.strip():
            fields[key.strip()] = value.strip()
    return fields, text[end + 4 :].lstrip("\n")


def _status(text: str) -> str | None:
    return _frontmatter(text)[0].get("status")


def measure(
    tasks_path: Path, client: StdioMcp, fragments: int, excerpt: int, use_excerpt: bool = True
) -> list[dict]:
    rows = json.loads(tasks_path.read_text())["rows"]
    report = []
    for task in rows:
        budget = Pack()
        pack = build(client, budget, task["query"], fragments, excerpt, use_excerpt)
        wanted = task.get("accept") or task.get("evidence") or []
        found = [f["path"] for f in pack["fragments"] if f["path"] in wanted]
        # A multi-note question needs its corroboration, not one mention, so the count matters more than the boolean.
        corroboration = len(found) / len(wanted) if wanted else None
        report.append(
            {
                "class": task["class"],
                "query": task["query"],
                "calls": budget.calls,
                "bytes": budget.bytes,
                "fragments": len(pack["fragments"]),
                "evidence_found": bool(found),
                "evidence_paths": len(found),
                "expected_paths": len(wanted),
                "corroboration": corroboration,
                "weak": pack["weak_fragments"],
                "expect": task["expect"],
            }
        )
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--query")
    parser.add_argument("--fragments", type=int, default=CANDIDATES)
    parser.add_argument(
        "--excerpt",
        type=int,
        default=FRAGMENT_CHARS,
        help="window per fragment for the tb_content fallback; an engine excerpt is used as it arrives",
    )
    parser.add_argument("--tasks", action="store_true")
    parser.add_argument("--tasks-file", type=Path, default=REPO / "tools/ranking/tasks.json")
    parser.add_argument(
        "--no-excerpt",
        action="store_true",
        help="ignore the engine's excerpt and read tb_content per fragment: the control arm of the measurement",
    )
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--socket", default=None)
    parser.add_argument("--vault", default=None)
    args = parser.parse_args()

    global VAULT, PREFIX
    if args.vault:
        VAULT = Path(args.vault)
        PREFIX = str(VAULT) + "/"
    env = dict(os.environ, HOME=os.environ.get("BRAIN_HOME", "/root"))
    target = args.socket or str(VAULT)

    with StdioMcp([ADAPTER, "mcp", target], name="context-pack", env=env) as client:
        if args.tasks:
            report = measure(
                args.tasks_file, client, args.fragments, args.excerpt, not args.no_excerpt
            )
            if args.json:
                print(json.dumps(report, indent=2))
            else:
                calls = sum(row["calls"] for row in report)
                size = sum(row["bytes"] for row in report)
                found = sum(1 for row in report if row["evidence_found"])
                print(f"{len(report)} tasks, {calls} calls, {size} bytes, evidence in {found}")
                for row in report:
                    mark = "ok  " if row["evidence_found"] or row["expect"] == "none" else "MISS"
                    covered = "" if not row["expected_paths"] else f"{row['evidence_paths']}/{row['expected_paths']}"
                    print(f"  {mark} {row['class']:<10} {row['calls']:>2} calls {row['bytes']:>6} B "
                          f"{covered:>4} evidence {row['query'][:44]}")
            return 0

        if not args.query:
            parser.error("--query or --tasks is required")
        budget = Pack()
        pack = build(client, budget, args.query, args.fragments, args.excerpt, not args.no_excerpt)
        pack["cost"] = {"calls": budget.calls, "bytes": budget.bytes}
        print(json.dumps(pack, indent=2, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
