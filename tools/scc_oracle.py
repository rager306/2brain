#!/usr/bin/env python3
"""Compare SCC labels against a small-graph reachability oracle.

Usage: python3 tools/scc_oracle.py fixture.json  # or --selftest

Fixture: {"nodes": ["a", "b"], "edges": [["a", "b"]],
          "labels": {"a": "component-1", "b": "component-2"}}.
This checks the supplied graph and labels only; it does not resolve a workspace.
"""

import argparse
import json
import sys
from pathlib import Path


def compare(data):
    nodes = data["nodes"]
    edges = data["edges"]
    labels = data["labels"]
    if not isinstance(nodes, list) or not all(isinstance(n, str) for n in nodes):
        raise ValueError("nodes must be a list of strings")
    if len(nodes) != len(set(nodes)) or not isinstance(labels, dict) or set(labels) != set(nodes):
        raise ValueError("nodes must be unique and labels must cover them exactly")
    if not all(isinstance(label, str) for label in labels.values()):
        raise ValueError("labels must be strings")
    adjacency = {n: set() for n in nodes}
    if not isinstance(edges, list):
        raise ValueError("edges must be a list")
    for edge in edges:
        if not isinstance(edge, list) or len(edge) != 2 or not all(isinstance(n, str) and n in adjacency for n in edge):
            raise ValueError("each edge must contain two known nodes")
        adjacency[edge[0]].add(edge[1])

    reachable = {}
    for node in nodes:
        seen = {node}
        pending = [node]
        while pending:
            current = pending.pop()
            for neighbor in adjacency[current] - seen:
                seen.add(neighbor)
                pending.append(neighbor)
        reachable[node] = seen
    return [(a, b) for i, a in enumerate(nodes) for b in nodes[i + 1:]
            if (b in reachable[a] and a in reachable[b]) != (labels[a] == labels[b])]


def selftest():
    fixture = {"nodes": ["a", "b", "c", "d"],
               "edges": [["a", "b"], ["b", "a"], ["b", "c"], ["d", "d"]],
               "labels": {"a": "x", "b": "x", "c": "y", "d": "z"}}
    assert compare(fixture) == []
    fixture["labels"]["c"] = "x"
    assert set(compare(fixture)) == {("a", "c"), ("b", "c")}
    fixture["labels"]["c"] = "y"
    fixture["labels"]["b"] = "different"
    assert compare(fixture) == [("a", "b")]
    fixture["edges"].append(["c", "missing"])
    try:
        compare(fixture)
    except ValueError:
        pass
    else:
        raise AssertionError("unknown endpoint accepted")
    assert compare({"nodes": [], "edges": [], "labels": {}}) == []
    print("selftest passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", nargs="?", help="JSON graph and candidate component labels")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest and not args.fixture:
        selftest()
        return 0
    if not args.fixture or args.selftest:
        parser.error("provide one fixture or --selftest")
    try:
        mismatches = compare(json.loads(Path(args.fixture).read_text(encoding="utf-8")))
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"invalid fixture: {exc}", file=sys.stderr)
        return 2
    if mismatches:
        print(json.dumps({"result": "mismatch", "pairs": mismatches}))
        return 1
    print(json.dumps({"result": "match"}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
