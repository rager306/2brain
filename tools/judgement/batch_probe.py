#!/usr/bin/env python3
"""Measure the judgement API's batch behaviour: N notes, one request each size.

The question this answers is the one that decides an architecture: is a judged pass over the vault
1000 calls or 20? The API takes `state` (the material) plus a `questions` object, so batching means
packing several notes into one `state` and asking one question per note — the questions stay
independent, the context is shared.

    python3 tools/judgement/batch_probe.py --sizes 1 5 20 --repeat 2

Prints, per size: wall time, per-question time, and whether any question came back unanswered.
Cost is not reported by the API in a comparable way, so it is not invented here; the previous run's
per-call cost is in docs/w4.1-judgement-run-2026-11-16.md.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from audit import DEFAULT_ENDPOINT, DEFAULT_MODEL, load_project_env, post_systemone  # noqa: E402

NOTE = """
Note {i}
description: "decision {i}: keep the engine the only writer"
body: |
  The engine owns every write. The MCP adapter never touches the filesystem; it forwards a mutation
  the engine validates, and the answer carries the commit. Read-then-write CAS uses the content hash.
"""

QUESTION = {
    "type": "noul",
    "instructions": "Does this note describe a decision (as opposed to a pattern, an error or an excerpt)?",
    "criteria": {"true": "a decision was taken", "false": "no decision in this note"},
}


def build(n: int, model: str) -> dict:
    state = "".join(NOTE.format(i=i) for i in range(n))
    questions = {f"note_{i}": dict(QUESTION) for i in range(n)}
    return {"model": model, "state": state, "questions": questions}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sizes", type=int, nargs="+", default=[1, 5, 20])
    parser.add_argument("--repeat", type=int, default=2)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--endpoint", default=os.environ.get("JUDGEMENT_ENDPOINT", DEFAULT_ENDPOINT))
    args = parser.parse_args()

    key = os.environ.get("SUBQ_API_KEY")
    if not key:
        # The key lives in <repo>/.env (gitignored); load_project_env exports it.
        load_project_env(__file__)
        key = os.environ.get("SUBQ_API_KEY")
    if not key:
        print("no SUBQ_API_KEY", file=sys.stderr)
        return 3

    print(f"model {args.model}, endpoint {args.endpoint}")
    for size in args.sizes:
        body = build(size, args.model)
        state_bytes = len(body["state"])
        times, answered = [], []
        for _ in range(args.repeat):
            started = time.monotonic()
            try:
                reply = post_systemone(args.endpoint, body, key, timeout=120.0)
            except Exception as error:  # noqa: BLE001
                print(f"  n={size:3d}: ERROR {error}")
                return 1
            times.append(time.monotonic() - started)
            answers = reply.get("answers") or reply.get("results") or {}
            answered.append(len(answers))
        best = min(times)
        print(f"  n={size:3d} state={state_bytes:6d}B  wall={best:6.2f}s  "
              f"per_question={best/size*1000:7.1f}ms  answered={answered[0]}/{size}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
