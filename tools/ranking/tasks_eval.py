#!/usr/bin/env python3
"""Score the task-level set: real questions, minimally sufficient evidence, and the ones the base cannot answer.

`eval.py` scores `labelled-queries.json`, whose queries were extracted from the notes' own filenames — it measures
rank stability. This scores `tasks.json`, written by hand: exact identifiers, paraphrases, a Russian question about
English material, multi-note questions, an ambiguous query, and three the base should NOT answer.

  python3 tools/ranking/tasks_eval.py            # per-class report
  python3 tools/ranking/tasks_eval.py --json     # for a before/after diff

The `absent` class has no clean automatic judge: the only honest signal the wire carries is `matched_terms`, so a
question counts as correctly unanswered when the top hit matched at most one query term. That is a weak test, and
it is labelled weak rather than dressed up.
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


def search(client: StdioMcp, query: str, limit: int = 10) -> list[dict]:
    reply = client.call("tb_search", {"query": query, "limit": limit})
    result = reply.get("result", reply)
    if result.get("isError"):
        return []
    block = (result.get("structuredContent") or {}).get("search") or {}
    root = str(VAULT) + "/"
    return [
        {"path": match["path"].removeprefix(root), "terms": match.get("matched_terms", 0),
         "score": round(match.get("score", 0.0), 2)}
        for match in block.get("matches", [])
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--set", type=Path, default=REPO / "tools/ranking/tasks.json")
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    tasks = json.loads(args.set.read_text())["rows"]
    env = dict(os.environ, HOME=os.environ.get("BRAIN_HOME", "/root"))
    rows = []
    with StdioMcp([ADAPTER, "mcp", str(VAULT)], name="tasks-eval", env=env) as client:
        for task in tasks:
            hits = search(client, task["query"], args.limit)
            paths = [hit["path"] for hit in hits]
            if task["expect"] == "none":
                passed = not hits or hits[0]["terms"] <= 1
                rank = None
            else:
                wanted = task.get("accept") or task["evidence"]
                rank = next((position for position, path in enumerate(paths, 1) if path in wanted), None)
                passed = rank is not None and (task["expect"] == "any" or rank == 1)
            rows.append({"query": task["query"], "class": task["class"], "passed": passed, "rank": rank,
                         "top": paths[:3], "top_terms": hits[0]["terms"] if hits else 0})

    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=2))
        return 0

    width = max(len(row["class"]) for row in rows)
    for row in rows:
        mark = "ok  " if row["passed"] else "MISS"
        where = f"rank {row['rank']}" if row["rank"] else f"terms {row['top_terms']}"
        print(f"{mark} {row['class']:<{width}}  {where:<10} {row['query'][:60]}")
        if not row["passed"]:
            print(f"        top: {', '.join(row['top'])}")

    classes = {}
    for row in rows:
        entry = classes.setdefault(row["class"], [0, 0])
        entry[1] += 1
        entry[0] += 1 if row["passed"] else 0
    print()
    for name, (passed, total) in sorted(classes.items()):
        print(f"{name:<12} {passed}/{total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
