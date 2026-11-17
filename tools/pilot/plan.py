#!/usr/bin/env python3
"""Emit a randomized review plan without executing reviews."""
import argparse
import json
import random
from pathlib import Path


HERE = Path(__file__).resolve().parent
PROTOCOLS = {
    "A": "Use the current workflow to review this design for material, actionable defects.",
    "B": "Use a fresh reviewer. First review specification compliance; then separately review quality and failure paths. Report actionable defects with evidence.",
    "C": "Use an isolated fresh reviewer. First explain the design in your own words, then identify actionable gaps and cite evidence. Do not see the drafting conversation or other reviews.",
}


def make_plan(fixtures, repetitions, seed):
    runs = []
    for fixture in sorted(fixtures, key=lambda x: x["id"]):
        for repetition in range(1, repetitions + 1):
            for arm, protocol in PROTOCOLS.items():
                prompt = (f"{protocol}\nReview only; do not write or access any live system. "
                          "Read only the design and the allowed frozen source files below. "
                          "Cite evidence and distinguish uncertainty.\n\n"
                          f"Design:\n{fixture['design']}\n\nAllowed sources:\n" +
                          "\n".join(f"- {p}" for p in fixture["sources"]))
                runs.append({"fixture": fixture["id"], "class": fixture["class"],
                             "arm": arm, "repetition": repetition, "prompt": prompt})
    random.Random(seed).shuffle(runs)
    return runs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="emit JSON")
    parser.add_argument("--seed", type=int, default=20261117, help="order seed")
    parser.add_argument("--repetitions", type=int, default=1)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error("repetitions must be positive")
    fixtures = [json.loads(p.read_text(encoding="utf-8")) for p in sorted((HERE / "fixtures").glob("*.json"))]
    if not fixtures or len({f["id"] for f in fixtures}) != len(fixtures):
        parser.error("fixtures must exist and have unique IDs")
    runs = make_plan(fixtures, args.repetitions, args.seed)
    data = {"seed": args.seed, "fixture_count": len(fixtures), "runs": runs}
    if args.json:
        print(json.dumps(data, indent=2))
    else:
        print(f"Seed {args.seed}; {len(fixtures)} fixtures; {len(runs)} runs")
        for run in runs:
            print(f"\n--- {run['fixture']} {run['arm']} repetition {run['repetition']} ---\n{run['prompt']}")


if __name__ == "__main__":
    main()
