#!/usr/bin/env python3
"""Compare structural centrality with graph_health.central offline.

This is not a ranking input or a write gate, and never reads the live workspace.
It reads only a supplied JSON graph: {"nodes": ["a", "b"], "edges": [["a", "b"]]}.
PageRank always uses directed edges; --undirected applies only to betweenness.
Duplicate edges are collapsed. Betweenness scores are unnormalized.
"""

import argparse
from collections import deque
import json
import math
from pathlib import Path
import sys


def adjacency_from(data):
    nodes = data["nodes"]
    edges = data["edges"]
    if not isinstance(nodes, list) or not all(isinstance(n, str) for n in nodes):
        raise ValueError("nodes must be a list of strings")
    if len(nodes) != len(set(nodes)):
        raise ValueError("nodes must be unique")
    if not isinstance(edges, list):
        raise ValueError("edges must be a list")
    adjacency = {node: set() for node in sorted(nodes)}
    for edge in edges:
        if not isinstance(edge, list) or len(edge) != 2 or not all(
            isinstance(node, str) and node in adjacency for node in edge
        ):
            raise ValueError("each edge must contain two known nodes")
        adjacency[edge[0]].add(edge[1])
    return {node: sorted(neighbors) for node, neighbors in adjacency.items()}


def pagerank(adjacency, damping=0.85, tolerance=1e-10, max_iterations=200):
    """Return scores, iteration count and convergence based on L1 change."""
    count = len(adjacency)
    if not count:
        return {}, 0, True
    scores = dict.fromkeys(adjacency, 1.0 / count)
    for iteration in range(1, max_iterations + 1):
        dangling = sum(scores[node] for node, neighbors in adjacency.items() if not neighbors)
        base = (1.0 - damping + damping * dangling) / count
        updated = dict.fromkeys(adjacency, base)
        for node, neighbors in adjacency.items():
            if neighbors:
                share = damping * scores[node] / len(neighbors)
                for neighbor in neighbors:
                    updated[neighbor] += share
        delta = sum(abs(updated[node] - scores[node]) for node in adjacency)
        scores = updated
        if delta <= tolerance:
            return scores, iteration, True
    return scores, max_iterations, False


def betweenness(adjacency, undirected=False):
    """Calculate unweighted Brandes betweenness for a simple graph."""
    if undirected:
        neighbors = {node: set(targets) for node, targets in adjacency.items()}
        for node, targets in adjacency.items():
            for target in targets:
                neighbors[target].add(node)
        adjacency = {node: sorted(targets) for node, targets in neighbors.items()}
    scores = dict.fromkeys(adjacency, 0.0)
    for source in adjacency:
        stack = []
        predecessors = {node: [] for node in adjacency}
        paths = dict.fromkeys(adjacency, 0)
        paths[source] = 1
        distance = dict.fromkeys(adjacency, -1)
        distance[source] = 0
        queue = deque([source])
        while queue:
            node = queue.popleft()
            stack.append(node)
            for target in adjacency[node]:
                if distance[target] < 0:
                    distance[target] = distance[node] + 1
                    queue.append(target)
                if distance[target] == distance[node] + 1:
                    paths[target] += paths[node]
                    predecessors[target].append(node)
        dependency = dict.fromkeys(adjacency, 0.0)
        while stack:
            node = stack.pop()
            for predecessor in predecessors[node]:
                dependency[predecessor] += paths[predecessor] / paths[node] * (1.0 + dependency[node])
            if node != source:
                scores[node] += dependency[node]
    if undirected:
        scores = {node: score / 2.0 for node, score in scores.items()}
    return scores


def top(scores, limit):
    return [{"node": node, "score": score} for node, score in
            sorted(scores.items(), key=lambda item: (-item[1], item[0]))[:limit]]


def selftest():
    def graph(nodes, edges):
        return adjacency_from({"nodes": nodes, "edges": edges})

    def check(actual, expected):
        if actual.keys() != expected.keys() or any(
            not math.isclose(actual[node], score, abs_tol=1e-10, rel_tol=1e-10)
            for node, score in expected.items()
        ):
            raise AssertionError(f"expected {expected}, got {actual}")

    path = graph(["a", "b", "c"], [["a", "b"], ["b", "c"]])
    check(betweenness(path), {"a": 0, "b": 1, "c": 0})
    star = graph(["a", "b", "c", "d"], [["a", "b"], ["a", "c"], ["a", "d"]])
    check(betweenness(star, undirected=True), {"a": 3, "b": 0, "c": 0, "d": 0})
    cycle = graph(["a", "b", "c"], [["a", "b"], ["b", "c"], ["c", "a"]])
    check(betweenness(cycle, undirected=True), {"a": 0, "b": 0, "c": 0})
    pair = graph(["a", "b"], [["a", "b"], ["b", "a"]])
    check(pagerank(pair)[0], {"a": 0.5, "b": 0.5})
    empty = graph([], [])
    check(pagerank(empty)[0], {})
    check(betweenness(empty), {})
    isolated = graph(["a"], [])
    check(pagerank(isolated)[0], {"a": 1})
    check(betweenness(isolated), {"a": 0})
    print("selftest passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", nargs="?", help="JSON graph export")
    parser.add_argument("--top-k", type=int, default=10, help="number of results per metric (default: 10)")
    parser.add_argument("--undirected", action="store_true", help="collapse edge direction for betweenness")
    parser.add_argument("--json", action="store_true", help="output machine-readable JSON")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest and not args.fixture:
        selftest()
        return 0
    if not args.fixture or args.selftest:
        parser.error("provide one fixture or --selftest")
    if args.top_k < 1:
        parser.error("--top-k must be positive")
    try:
        adjacency = adjacency_from(json.loads(Path(args.fixture).read_text(encoding="utf-8")))
    except (OSError, ValueError, KeyError, TypeError) as exc:
        print(f"invalid fixture: {exc}", file=sys.stderr)
        return 2
    ranks, iterations, converged = pagerank(adjacency)
    result = {
        "pagerank": {"iterations": iterations, "converged": converged, "top": top(ranks, args.top_k)},
        "betweenness": {"undirected": args.undirected, "top": top(betweenness(adjacency, args.undirected), args.top_k)},
    }
    if args.json:
        print(json.dumps(result))
    else:
        print(f"PageRank (iterations: {iterations}, converged: {converged})")
        for row in result["pagerank"]["top"]:
            print(f"  {row['node']}\t{row['score']:.10f}")
        print(f"Betweenness ({'undirected' if args.undirected else 'directed'}, unnormalized)")
        for row in result["betweenness"]["top"]:
            print(f"  {row['node']}\t{row['score']:.10f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
