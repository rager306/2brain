#!/usr/bin/env python3
"""Measure what a query-time alias map would cost, before any mechanism exists.

`docs/research/alias-field-2026-11-17.md` sized the class and refused both a third index field
and a query-time expansion without a measurement. This is that measurement, and it is a MODEL: it
runs two queries and compares them, it changes no engine behaviour.

For each verified pair (an acronym and the expansion the corpus itself states beside it), it asks
the served `search` twice and reports:

- `alias_total` / `expansion_total`: the candidate-set width each surface yields, so the inflation
  an expansion would cause is a number rather than a worry;
- `alias_only`: notes the acronym reaches and the expansion does not (a note that writes only the
  short form);
- `expansion_only`: notes the spelled-out form reaches and the acronym does not — the class the
  alias row measures;
- `shared`: notes both reach.

The map is hand-verified and deliberately short. A pair earns a row only when the corpus states
both surfaces in one note, which is what `tools/kb/alias_audit.py` ranks; that tool proposes, a
reader decides, and this one measures.

    python3 tools/ranking/alias_expansion.py --socket <sock>
    python3 tools/ranking/alias_expansion.py --socket <sock> --json
    python3 tools/ranking/alias_expansion.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path

PROBE = Path(__file__).resolve().parents[2] / "tb-engine" / "scripts" / "engine_probe.py"
PAGE = 200

PAIRS = [
    ("SoT", "source of truth"),
    ("KG", "knowledge graph"),
    ("ER", "entity resolution"),
    ("CC", "claude code"),
    ("RSI", "recursive self improvement"),
    ("AS", "ai systems"),
    ("US", "user stories"),
    ("MT", "multi turn"),
    ("IO", "i o"),
]


def probe_module():
    spec = importlib.util.spec_from_file_location("engine_probe", PROBE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def live_search(socket: str, probe):
    def search(query: str) -> tuple[int, list[str]]:
        response = probe.exchange(
            socket, {"read": "search", "id": 1, "query": query, "limit": PAGE}
        )
        answer = response.get("result", {}).get("search", {})
        return answer.get("total", 0), [m["path"] for m in answer.get("matches", [])]

    return search


def compare(short: str, long: str, search) -> dict:
    alias_total, alias_paths = search(short)
    expansion_total, expansion_paths = search(long)
    alias_set, expansion_set = set(alias_paths), set(expansion_paths)
    # A page holds at most PAGE notes, so a total larger than that is reported
    # rather than silently read as a complete set.
    complete = alias_total <= PAGE and expansion_total <= PAGE
    return {
        "alias": short,
        "expansion": long,
        "alias_total": alias_total,
        "expansion_total": expansion_total,
        "alias_only": len(alias_set - expansion_set) if complete else None,
        "expansion_only": len(expansion_set - alias_set) if complete else None,
        "shared": len(alias_set & expansion_set) if complete else None,
        "complete": complete,
        "inflation": round(expansion_total / alias_total, 2) if alias_total else None,
    }


def selftest() -> int:
    pages = {
        "SoT": (3, ["/v/a.md", "/v/b.md", "/v/c.md"]),
        "source of truth": (4, ["/v/b.md", "/v/c.md", "/v/d.md", "/v/e.md"]),
    }

    def fake(query: str):
        return pages[query]

    row = compare("SoT", "source of truth", fake)
    assert row["alias_only"] == 1, row
    assert row["expansion_only"] == 2, row
    assert row["shared"] == 2, row
    assert row["inflation"] == 1.33, row
    print("selftest ok")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--socket")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    if not args.socket:
        parser.error("--socket is required")
    search = live_search(args.socket, probe_module())
    rows = [compare(short, long, search) for short, long in PAIRS]
    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=1))
        return 0
    print(f"{'alias':6} {'exp':>5} {'acr':>5} {'exp':>5} {'a-only':>6} {'e-only':>6} {'shared':>6}  expansion")
    for row in rows:
        print(
            f"{row['alias']:6} {row['inflation'] if row['inflation'] is not None else '?':>5} "
            f"{row['alias_total']:5} {row['expansion_total']:5} "
            f"{row['alias_only'] if row['alias_only'] is not None else '?':>6} "
            f"{row['expansion_only'] if row['expansion_only'] is not None else '?':>6} "
            f"{row['shared'] if row['shared'] is not None else '?':>6}  {row['expansion']}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
