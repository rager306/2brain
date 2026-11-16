#!/usr/bin/env python3
"""Score the 160-question set, per class and per axis.

`tasks_eval.py` scores the eighteen hand-written tasks by pass/fail; this scores `questions.json`, which is bigger, carries
nine axes per row, and asks a different question: given a query written the way a reader who has not read the note would
ask it, where does the expected note land, and does the class or the axis explain the miss.

    python3 tools/ranking/run_questions.py                      # per class and per axis
    python3 tools/ranking/run_questions.py --json out.json      # the rows, for a diff
    python3 tools/ranking/run_questions.py --scope all --limit 20

A row passes when an `expect` path is inside the top `--k` (5 by default). `accept` is an alternative answer set, as in
`tasks.json`. Rows with no `expect` (the absent class) are reported separately: for them the honest signal is the page's
own width, and the report prints the matched-term count of the top hit rather than a rank.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tb-mcp-rs/scripts/lib"))
from mcp_stdio import StdioMcp  # noqa: E402

ADAPTER = os.environ.get("BRAIN_BIN", "/usr/local/bin/2brain")
VAULT = Path(os.environ.get("BRAIN_VAULT", "/root/vault"))
QUESTIONS = REPO / "tools/ranking/questions.json"


def run(rows, socket_path, vault, scope, limit):
    env = dict(os.environ, HOME=os.environ.get("BRAIN_HOME", "/root"))
    # Two adapter shapes are in play: the installed build takes `mcp <vault>` and finds its socket through HOME, the
    # repository build takes the socket positionally. Both are supported because a comparison against a running daemon
    # usually uses the first and a candidate build the second.
    args = [ADAPTER, "mcp", socket_path] if socket_path else [ADAPTER, "mcp", str(vault)]
    out = []
    with StdioMcp(args, name="questions", env=env) as client:
        for row in rows:
            request = {"repo": "vault", "query": row["query"], "limit": limit, "scope": scope}
            request.update(row.get("args") or {})
            reply = client.call("tb_search", request)
            payload = (reply.get("result", reply).get("structuredContent") or {}).get("search", {})
            paths = [m["path"].replace(str(vault) + "/", "") for m in (payload.get("matches") or [])]
            expected = list(row.get("expect") or []) + list(row.get("accept") or [])
            rank = next((i + 1 for i, p in enumerate(paths) if p in expected), None)
            out.append({
                "id": row["id"], "class": row["class"], "axes": row.get("axes") or {},
                "query": row["query"], "total": payload.get("total"), "rank": rank,
                "paths": paths[:limit], "matched_terms": (payload.get("matches") or [{}])[0].get("matched_terms"),
            })
    return out


def report(results, k):
    scored = [r for r in results if r["rank"] is not None or r["paths"]]
    with_expect = [r for r in results if r["class"] != "absent"]
    def line(rows, label):
        n = len(rows)
        if not n:
            return None
        at1 = sum(1 for r in rows if r["rank"] == 1)
        atk = sum(1 for r in rows if r["rank"] and r["rank"] <= k)
        none = sum(1 for r in rows if r["rank"] is None)
        mrr = sum(1.0 / r["rank"] for r in rows if r["rank"]) / n
        return f"| {label} | {n} | {at1} | {atk} | {none} | {mrr:.3f} |"
    print(f"# questions: hit@1 / hit@{k} / absent from page / MRR\n")
    print(f"| group | n | @1 | @{k} | none | MRR |")
    print("|---|---:|---:|---:|---:|---:|")
    for cls in sorted({r["class"] for r in results}):
        rows = [r for r in results if r["class"] == cls]
        if cls == "absent":
            continue
        print(line(rows, cls))
    print(line(with_expect, "**all answerable**"))
    print()
    print(f"| axis | value | n | @1 | @{k} | none | MRR |")
    print("|---|---|---:|---:|---:|---:|---:|")
    for axis in ("language", "length", "form", "hop", "entity", "lifecycle"):
        values = sorted({(r["axes"] or {}).get(axis) for r in with_expect if (r["axes"] or {}).get(axis)})
        for value in values:
            rows = [r for r in with_expect if (r["axes"] or {}).get(axis) == value]
            n = len(rows)
            at1 = sum(1 for r in rows if r["rank"] == 1)
            atk = sum(1 for r in rows if r["rank"] and r["rank"] <= k)
            none = sum(1 for r in rows if r["rank"] is None)
            mrr = sum(1.0 / r["rank"] for r in rows if r["rank"]) / n
            print(f"| {axis} | {value} | {n} | {at1} | {atk} | {none} | {mrr:.3f} |")
    absent = [r for r in results if r["class"] == "absent"]
    if absent:
        print(f"\nabsent class: {len(absent)} rows, top hit matched terms " +
              ", ".join(str(r["matched_terms"]) for r in absent))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--json", type=Path)
    parser.add_argument("--socket")
    parser.add_argument("--vault", type=Path, default=VAULT)
    parser.add_argument("--scope", default="own")
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--k", type=int, default=5)
    args = parser.parse_args()
    data = json.loads(QUESTIONS.read_text())
    rows = data["rows"] if isinstance(data, dict) else data
    results = run(rows, args.socket, args.vault, args.scope, args.limit)
    report(results, args.k)
    if args.json:
        args.json.write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()
