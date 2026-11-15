#!/usr/bin/env python3
"""Ask the staleness question as a choice instead of a yes/no (W4.1, second experiment).

Why: the yes/no question scored recall 0.333 on the 50 labelled notes
(`docs/w4.1-judgement-run-2026-11-16.md`). A yes/no gives the judge no way to say "the description
is vague", which is most of what the hand labels called stale. This asks a five-way choice whose
first option is "supported" and whose other four are kinds of failure, then reports the binary
detection (any failure option) against the same hand labels, so the numbers are comparable with the
0.333 baseline.

Cost: one request per note, ~$0.002 for the 50. Exit codes: 0 ran, 2 bad labels, 3 no key.
"""

import argparse
import json
import os
import sys

from audit import (
    DEFAULT_MODEL,
    ValidationError,
    load_fixture,
    load_project_env,
    post_systemone,
)

OPTIONS = {
    "supported": "every substantive claim in the description is present in the body",
    "vague": "the description states no checkable claim (generic or too abstract to be true or false)",
    "missing": "the description states a claim the body does not contain",
    "contradicted": "the description states something the body contradicts",
    "another_subject": "the description is about a different note's subject",
}

INSTRUCTIONS = (
    "The NOTE is one vault note: a DESCRIPTION and a BODY_EXCERPT. Decide which single statement "
    "describes the relation between them. Choose 'supported' only when every substantive claim of "
    "the description is present in the body. Choose 'vague' when the description states no checkable "
    "claim. Choose 'missing' when it states a claim the body does not contain, 'contradicted' when "
    "the body contradicts it, and 'another_subject' when it is about a different subject. "
    "The CANDIDATE is irrelevant here."
)


def question() -> dict:
    return {
        "description_body": {
            "type": "choice",
            "instructions": INSTRUCTIONS,
            "criteria": OPTIONS,
        }
    }


def state(note: dict) -> dict:
    return {
        "note": {
            "description": note["description"],
            "body_excerpt": note["excerpt"],
        }
    }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--labels", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "labelled.json"))
    parser.add_argument("--report", default="/tmp/stale-choice-report.json")
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--threshold", type=float, default=0.5, help="below this confidence the answer counts as uncertain")
    arguments = parser.parse_args(argv)

    load_project_env(__file__)
    key = os.environ.get("SUBQ_API_KEY")
    endpoint = os.environ.get("JUDGEMENT_ENDPOINT") or "https://api.subq.ai"
    if not key and "subq.ai" in endpoint:
        print("stale-choice: no SUBQ_API_KEY and no local endpoint; refusing to run", file=sys.stderr)
        return 3

    try:
        fixture = load_fixture(arguments.labels)
    except ValidationError as error:
        print("stale-choice: the labels are invalid:", file=sys.stderr)
        for problem in getattr(error, "problems", [str(error)]):
            print("  -", problem, file=sys.stderr)
        return 2
    notes = fixture["notes"]

    rows = []
    for note in notes:
        body = {"model": arguments.model, "state": state(note), "questions": question()}
        try:
            answer = post_systemone(endpoint, body, key, 90.0)
        except Exception as error:  # one bad note must not end the pass
            rows.append({"path": note["path"], "error": str(error)[:200]})
            continue
        cell = (answer.get("answers") or {}).get("description_body") or {}
        choice = cell.get("choice")
        confidence = cell.get("confidence")
        detected = None if choice is None else (choice != "supported")
        if confidence is not None and confidence < arguments.threshold:
            detected = None
        rows.append({
            "path": note["path"],
            "choice": choice,
            "confidence": confidence,
            "detected_stale": detected,
            "gold_stale": note["labels"]["stale_description"]["value"] == "yes",
        })

    tp = sum(1 for r in rows if r.get("detected_stale") is True and r["gold_stale"])
    fp = sum(1 for r in rows if r.get("detected_stale") is True and not r["gold_stale"])
    fn = sum(1 for r in rows if r.get("detected_stale") is False and r["gold_stale"])
    tn = sum(1 for r in rows if r.get("detected_stale") is False and not r["gold_stale"])
    uncertain = sum(1 for r in rows if r.get("detected_stale") is None)
    errors = sum(1 for r in rows if "error" in r)
    precision = tp / (tp + fp) if tp + fp else None
    recall = tp / (tp + fn) if tp + fn else None
    print(f"stale-choice: {len(rows)} notes, {errors} error(s), {uncertain} uncertain")
    print(f"  detected {tp} of {tp + fn} stale, {fp} false positive(s)  ->  "
          f"precision {'n/a' if precision is None else f'{precision:.3f}'}, "
          f"recall {'n/a' if recall is None else f'{recall:.3f}'}")
    from collections import Counter
    print("  choices:", dict(Counter(r.get("choice") for r in rows)))
    with open(arguments.report, "w", encoding="utf-8") as handle:
        json.dump({"model": arguments.model, "threshold": arguments.threshold, "rows": rows}, handle, indent=2)
    print(f"  report: {arguments.report}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
