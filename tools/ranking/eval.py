#!/usr/bin/env python3
"""Score a ranking change against the labelled query set.

Reads `tools/ranking/labelled-queries.json` and reports, per class:

  hit@1, hit@5, MRR   — did the expected note come back, and how high

    python3 tools/ranking/eval.py                     # against the live adapter
    python3 tools/ranking/eval.py --json              # machine-readable, for a before/after diff

The set is synthetic (`build_queries.py` says how), so this measures *rank stability and ordering*, not
user satisfaction. A change that lowers hit@1 on the `literal` class is a defect regardless of what it
does to the topical class.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tb-mcp-rs/scripts/lib"))
from mcp_stdio import StdioMcp, is_error, tool_payload, tool_text  # noqa: E402

ADAPTER = os.environ.get("BRAIN_BIN", "/usr/local/bin/2brain")
VAULT = os.environ.get("BRAIN_VAULT", "/root/vault")
PREFIX = None  # set from the vault being served, in main()


def rank_of(payload: dict, expected: str) -> int | None:
    matches = payload.get("search", payload).get("matches", [])
    wanted = PREFIX + expected
    for position, match in enumerate(matches, 1):
        if match.get("path") == wanted:
            return position
    return None


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--set", type=Path, default=REPO / "tools/ranking/labelled-queries.json")
    parser.add_argument("--limit", type=int, default=10)
    parser.add_argument("--json", action="store_true")
    parser.add_argument(
        "--socket",
        default=None,
        help="engine socket to serve instead of the live vault (tools/ranking/candidate_engine.py)",
    )
    parser.add_argument(
        "--vault",
        default=VAULT,
        help="root the served engine answers from; the set's expected paths are relative to it",
    )
    args = parser.parse_args(argv[1:])
    global PREFIX
    PREFIX = args.vault.rstrip("/") + "/"

    data = json.loads(args.set.read_text())
    rows = data["rows"]
    totals = {}
    details = []
    with StdioMcp([ADAPTER, "mcp", args.socket or args.vault], name="eval") as client:
        for row in rows:
            reply = client.call("tb_search", {"query": row["query"], "limit": args.limit})
            if is_error(reply):
                print(f"search refused for {row['query']!r}: {tool_text(reply)}", file=sys.stderr)
                return 1
            position = rank_of(tool_payload(reply), row["expected"])
            bucket = totals.setdefault(row["class"], {"n": 0, "hit1": 0, "hit5": 0, "rr": 0.0})
            bucket["n"] += 1
            bucket["hit1"] += 1 if position == 1 else 0
            bucket["hit5"] += 1 if position and position <= 5 else 0
            bucket["rr"] += (1.0 / position) if position else 0.0
            details.append({**row, "rank": position})

    report = {}
    for name, bucket in sorted(totals.items()):
        n = bucket["n"]
        report[name] = {
            "n": n,
            "hit@1": round(bucket["hit1"] / n, 3),
            "hit@5": round(bucket["hit5"] / n, 3),
            "mrr": round(bucket["rr"] / n, 3),
        }
    if args.json:
        print(json.dumps({"report": report, "details": details}, indent=1))
    else:
        for name, stats in report.items():
            print(f"{name:<8} n={stats['n']:<3} hit@1={stats['hit@1']:<5} "
                  f"hit@5={stats['hit@5']:<5} mrr={stats['mrr']}")
        misses = [d for d in details if d["rank"] is None]
        if misses:
            print(f"missing from top-{args.limit}: {len(misses)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
