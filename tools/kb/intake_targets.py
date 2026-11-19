#!/usr/bin/env python3
"""Propose inbound links for content notes nothing points at.

The graph read `graph_shape { orphan_paths: true }` names every file with no inbound edge. Most of
those are not content (session dumps, type defs, templates), so this tool keeps the note corpus
(f/, knowledge/, manual/) and asks the same `search` the serving path uses: for each orphan, the
notes nearest its file stem are the candidates that could link to it.

It proposes; it never writes. A proposal is a note whose ranking put the orphan in its top three,
which is evidence the two discuss the same thing, not proof that a link belongs there.

    python3 tools/kb/intake_targets.py --socket <state root>/engine/run/<hash>.sock
    python3 tools/kb/intake_targets.py --health /tmp/graph_shape.json     # offline
    python3 tools/kb/intake_targets.py --selftest
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import importlib.util

PROBE = Path(__file__).resolve().parents[2] / "tb-engine" / "scripts" / "engine_probe.py"
CORPUS = ("/f/", "/knowledge/", "/manual/")
TOP = 3


def probe_module():
    spec = importlib.util.spec_from_file_location("engine_probe", PROBE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def stem_query(path: str) -> str:
    return Path(path).stem.replace("-", " ").replace("_", " ")


def content_orphans(shape: dict) -> list[str]:
    return [
        row["path"]
        for row in shape["orphans"]["paths"]
        if row["kind"] == "no_inbound" and any(part in row["path"] for part in CORPUS)
    ]


def proposals(orphans: list[str], search) -> list[dict]:
    out = []
    for orphan in orphans:
        hits = [hit for hit in search(stem_query(orphan)) if hit["path"] != orphan][:TOP]
        out.append({"orphan": orphan, "candidates": hits})
    return out


def live_search(socket: str, probe) -> callable:
    def search(query: str) -> list[dict]:
        response = probe.exchange(
            socket, {"read": "search", "id": 1, "query": query, "limit": TOP + 1}
        )
        return response.get("result", {}).get("search", {}).get("matches", [])
    return search


def selftest() -> int:
    orphans = ["/v/f/patterns/pattern-alpha.md", "/v/f/patterns/pattern-beta.md"]

    def fake(query: str) -> list[dict]:
        if "alpha" in query:
            return [
                {"path": "/v/f/patterns/pattern-alpha.md", "score": 9.0, "matched_terms": 2},
                {"path": "/v/knowledge/hub.md", "score": 5.0, "matched_terms": 2},
            ]
        return []

    rows = proposals(orphans, fake)
    assert rows[0]["candidates"] == [{"path": "/v/knowledge/hub.md", "score": 5.0, "matched_terms": 2}], rows
    assert rows[1]["candidates"] == [], rows
    assert stem_query("/v/f/patterns/pattern-alpha.md") == "pattern alpha"
    print("selftest ok")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--socket")
    parser.add_argument("--health")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    if args.health:
        shape = json.loads(Path(args.health).read_text())["result"]["graph_shape"]
        probe = probe_module()
    elif args.socket:
        probe = probe_module()
        shape = probe.exchange(args.socket, {"read": "graph_shape", "id": 1, "orphan_paths": True})
        shape = shape["result"]["graph_shape"]
    else:
        parser.error("--socket or --health is required")
    orphans = content_orphans(shape)
    rows = proposals(orphans, live_search(args.socket, probe)) if args.socket else []
    if not rows:
        print(f"{len(orphans)} content notes with no inbound edge; --socket needed for proposals")
        for orphan in orphans:
            print(orphan)
        return 0
    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=1))
        return 0
    print(f"{len(rows)} content notes with no inbound edge")
    for row in rows:
        print(f"\n{row['orphan'].replace('/root/vault/', '')}")
        if not row["candidates"]:
            print("  (no candidate)")
        for hit in row["candidates"]:
            print(f"  <- {hit['path'].replace('/root/vault/', '')}  {hit['score']:.1f}  t{hit['matched_terms']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
