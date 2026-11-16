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

A task may carry `args` (the tool arguments its class measures — `exact`, `min_should_match`) and `min_total`
(the candidate count the documented behaviour implies), so a class can pin a flag's semantics and not only a rank.
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
# A small English list, enough to answer "would dropping function words change the answer?". Not a claim that
# these are the right stopwords for a Russian-and-English corpus — that is what the experiment decides.
STOPWORDS = {
    "a", "an", "the", "of", "on", "in", "at", "to", "for", "and", "or", "is", "are", "was", "were",
    "be", "been", "it", "its", "this", "that", "these", "those", "with", "by", "as", "from", "how",
    "what", "which", "who", "when", "where", "why", "do", "does", "did", "can", "could", "should",
}
VAULT = Path(os.environ.get("BRAIN_VAULT", "/root/vault"))
PREFIX = str(VAULT) + "/"  # the served root; `--vault` re-points it for a candidate engine


def real_terms(query: str) -> list[str]:
    return [word for word in query.lower().split() if word and word not in STOPWORDS]


def search(client: StdioMcp, query: str, limit: int = 10,
           extra: dict | None = None) -> tuple[list[dict], int | None]:
    """One page of matches, plus the engine's total for the query.

    `extra` is the task's own arguments (`exact`, `min_should_match`, `scope`): the set has to measure
    the flags the tool documents, so a task can pin the behaviour of a non-default parameter.
    """
    args = {"query": query, "limit": limit, **(extra or {})}
    reply = client.call("tb_search", args)
    result = reply.get("result", reply)
    if result.get("isError"):
        return [], None
    block = (result.get("structuredContent") or {}).get("search") or {}
    root = PREFIX
    hits = [
        {"path": match["path"].removeprefix(root), "terms": match.get("matched_terms", 0),
         "score": round(match.get("score", 0.0), 2), "weak": bool(match.get("weak"))}
        for match in block.get("matches", [])
    ]
    return hits, block.get("total")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--set", type=Path, default=REPO / "tools/ranking/tasks.json")
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--vault", default=None, help="the served root the socket answers for, so paths can be "
                                                     "made workspace-relative")
    parser.add_argument("--socket", default=None, help="serve that engine socket instead of the live vault "
                                                      "(tools/ranking/candidate_engine.py)")
    parser.add_argument(
        "--require-real-terms",
        action="store_true",
        help="ask for min_should_match = the number of non-stopword query terms, i.e. treat function words as "
             "noise the engine should not count",
    )
    args = parser.parse_args()

    global PREFIX
    if args.vault:
        PREFIX = str(Path(args.vault)) + "/"
    tasks = json.loads(args.set.read_text())["rows"]
    env = dict(os.environ, HOME=os.environ.get("BRAIN_HOME", "/root"))
    rows = []
    target = args.socket or str(VAULT)
    with StdioMcp([ADAPTER, "mcp", target], name="tasks-eval", env=env) as client:
        for task in tasks:
            extra = dict(task.get("args") or {})
            if args.require_real_terms and "min_should_match" not in extra:
                extra["min_should_match"] = max(1, len(real_terms(task["query"])))
            hits, total = search(client, task["query"], args.limit, extra)
            paths = [hit["path"] for hit in hits]
            if task["expect"] == "none":
                # The engine never refuses to answer; it reports `weak` when every matched term is common in the
                # corpus. A base that says "these words occur here" has not answered, so that counts as unanswered.
                passed = not hits or hits[0]["terms"] <= 1 or hits[0]["weak"]
                rank = None
            else:
                wanted = task.get("accept") or task["evidence"]
                rank = next((position for position, path in enumerate(paths, 1) if path in wanted), None)
                passed = rank is not None and (task["expect"] == "any" or rank == 1)
            # A page's width is not a property of the question; a task that pins what a flag does to the
            # candidate SET states the count it expects, and a collapsed answer fails even when the right
            # note still ranks first (the `exact` AND is the case that needs this).
            if task.get("min_total") is not None:
                passed = passed and (total or 0) >= task["min_total"]
            rows.append({"query": task["query"], "class": task["class"], "passed": passed, "rank": rank,
                         "total": total, "top": paths[:3], "top_terms": hits[0]["terms"] if hits else 0,
                         "top_weak": hits[0]["weak"] if hits else None})

    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=2))
        return 0

    width = max(len(row["class"]) for row in rows)
    for row in rows:
        mark = "ok  " if row["passed"] else "MISS"
        where = f"rank {row['rank']}" if row["rank"] else f"terms {row['top_terms']}"
        if row.get("top_weak"):
            where += " weak"
        where += f" of {row['total']}"
        print(f"{mark} {row['class']:<{width}}  {where:<16} {row['query'][:60]}")
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
