#!/usr/bin/env python3
"""Measure whether the ranker prefers durable knowledge, on note pairs that share their subject.

The freshness prior (P1-5 of the vault backlog) was measured once, cost 1.9 points of topical hit@1, and was
reverted. The reason recorded then was that the labelled set asks "which note holds these words", and the
expected note does not change when a note goes stale. This builds the missing class: pairs of notes that talk
about the same subject but declare different `freshness`, where the durable note is the expected answer.

    python3 tools/ranking/durable_probe.py
    python3 tools/ranking/durable_probe.py --json

It is a *policy* probe, not ground truth: it asserts that the durable note answers better, which is the claim
P1-5 makes. If the ranker already prefers it, the prior is unnecessary. If it does not, that is the evidence
P1-5 needs — and the size of the class is the confidence in it.

The pair set is derived, not labelled by hand: two notes with different `freshness` whose descriptions share
at least `--min-shared` rare words. Pairs are the unit, so a note can appear more than once.
"""

from __future__ import annotations

import argparse
import collections
import itertools
import json
import os
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tb-mcp-rs/scripts/lib"))
from mcp_stdio import StdioMcp, is_error, tool_payload  # noqa: E402

ADAPTER = os.environ.get("BRAIN_BIN", "/usr/local/bin/2brain")
VAULT = os.environ.get("BRAIN_VAULT", "/root/vault")
DURABLE = {"stable", "evergreen", "durable", "slow"}
STOP = {
    "this", "that", "with", "from", "into", "over", "than", "then", "they", "their", "there", "these",
    "those", "which", "while", "when", "where", "what", "have", "note", "about", "using", "does", "each",
}
WORD = re.compile(r"[a-z]{4,}")


def frontmatter(path: Path) -> dict[str, str]:
    text = path.read_text(errors="ignore")
    if not text.startswith("---"):
        return {}
    end = text.find("\n---", 3)
    if end < 0:
        return {}
    fields: dict[str, str] = {}
    for line in text[3:end].splitlines():
        match = re.match(r"([a-z_]+):\s*(.*)", line.strip())
        if match:
            fields[match.group(1)] = match.group(2).strip()
    return fields


def pairs(min_shared: int) -> list[dict]:
    rows = []
    for path in VAULT and Path(VAULT).rglob("*.md"):
        fields = frontmatter(path)
        description = fields.get("description", "")
        freshness = fields.get("freshness", "")
        if not description or not freshness:
            continue
        words = {word for word in WORD.findall(description.lower()) if word not in STOP}
        if words:
            rows.append((path, freshness, words))

    index: dict[str, list[tuple[Path, str]]] = collections.defaultdict(list)
    for path, freshness, words in rows:
        for word in words:
            index[word].append((path, freshness))

    shared: collections.Counter[tuple[str, str]] = collections.Counter()
    for word, holders in index.items():
        # A word in almost every description carries no subject information.
        if len(holders) > 6:
            continue
        for (left, left_fresh), (right, right_fresh) in itertools.combinations(holders, 2):
            if (left_fresh in DURABLE) == (right_fresh in DURABLE):
                continue
            shared[tuple(sorted([str(left), str(right)]))] += 1

    out = []
    for (left, right), count in shared.items():
        if count < min_shared:
            continue
        left_fresh = frontmatter(Path(left)).get("freshness", "")
        durable, transient = (left, right) if left_fresh in DURABLE else (right, left)
        words = sorted(
            set(WORD.findall(frontmatter(Path(durable)).get("description", "").lower()))
            & set(WORD.findall(frontmatter(Path(transient)).get("description", "").lower()))
            - STOP
        )
        out.append(
            {
                "query": " ".join(words[:6]),
                "expected": durable,
                "rival": transient,
                "shared_words": count,
            }
        )
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--min-shared", type=int, default=3)
    parser.add_argument("--limit", type=int, default=200)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--socket", default=None, help="engine socket, instead of the vault")
    parser.add_argument("--vault", default=VAULT, help="root the served engine answers from")
    args = parser.parse_args()

    rows = pairs(args.min_shared)[: args.limit]
    if not rows:
        print("no pairs: the vault has no two notes with different freshness sharing a subject")
        return 1

    target = args.socket or args.vault
    launch = [ADAPTER, "mcp", target]
    prefix = args.vault.rstrip("/") + "/"
    hits_at_1 = hits_at_5 = 0
    reciprocal = 0.0
    rival_first = 0
    with StdioMcp(launch, name="durable-probe") as client:
        for row in rows:
            reply = client.call("au_search", {"query": row["query"], "limit": 10})
            if is_error(reply):
                print("search failed:", reply["result"]["content"][0]["text"][:200])
                return 2
            matches = tool_payload(reply)["search"]["matches"]
            paths = [match["path"].replace(prefix, "") for match in matches]
            # The pair was built from the vault this script reads on disk; the engine may serve a copy of
            # it, so both sides are reduced to the path relative to their own root.
            expected = row["expected"].replace(VAULT.rstrip("/") + "/", "")
            rival = row["rival"].replace(VAULT.rstrip("/") + "/", "")
            rank = paths.index(expected) + 1 if expected in paths else None
            rival_rank = paths.index(rival) + 1 if rival in paths else None
            if rank:
                reciprocal += 1.0 / rank
                if rank == 1:
                    hits_at_1 += 1
                if rank <= 5:
                    hits_at_5 += 1
            if rival_rank and (rank is None or rival_rank < rank):
                rival_first += 1
            if args.json:
                print(
                    json.dumps(
                        {
                            "query": row["query"],
                            "expected": expected,
                            "rival": rival,
                            "rank": rank,
                            "rival_rank": rival_rank,
                        }
                    )
                )

    count = len(rows)
    summary = {
        "pairs": count,
        "hit_at_1": round(hits_at_1 / count, 3),
        "hit_at_5": round(hits_at_5 / count, 3),
        "mrr": round(reciprocal / count, 3),
        "transient_beats_durable": rival_first,
    }
    print(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
