#!/usr/bin/env python3
"""Offline judgement harness for the arsumbris vault (W4.1 phase 1).

Reads labelled.json, builds one `systemone` request per note with three
questions (stale description, note type, duplicate claim) and compares the
answers with the hand labels. Standard library only: no vault access, no
vendor call without a key, and the only file it writes is the report.

Exit codes:
  0  run completed (including --dry-run); per-request errors are in report.json
  2  labelled.json failed validation
  3  no SUBQ_API_KEY and no explicit JUDGEMENT_ENDPOINT / --endpoint
"""

from __future__ import annotations

import argparse
import concurrent.futures
import http.client
import json
import os
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone

HERE = os.path.dirname(os.path.abspath(__file__))

NOTE_TYPES = (
    "claim",
    "decision",
    "error",
    "extract",
    "insight",
    "manual",
    "moc",
    "pattern",
    "principle",
)
DEFAULT_ENDPOINT = "https://api.subq.ai"
DEFAULT_MODEL = "subq-snap-1.2"
EXPECTED_NOTES = 50

# The id and the question must point the same way. Measured 2026-11-16: with the id
# `stale_description` and a "does the description still describe the body?" question, the
# model answered "yes" for good descriptions (mean probability 0.88 for accurate notes
# against 0.58 for stale ones), so precision sat at 0.12 at every threshold. The id was
# rephrased to match the question; the question itself is unchanged.
STALE_INSTRUCTIONS = (
    "The NOTE is one vault note: a DESCRIPTION and a BODY_EXCERPT. Decide whether the "
    "description still describes the body. Say true only if the description's "
    "substantive claims are supported by the body. Say false if it names a different "
    "subject, contradicts the body, or asserts a scope, status or count the body does "
    "not contain. The CANDIDATE is irrelevant here."
)
TYPE_INSTRUCTIONS = (
    "Which single note type does the NOTE's BODY_EXCERPT belong to? Choose by "
    "the role the text plays, not by its folder: claim (one asserted "
    "proposition), decision (a chosen option with rationale), error (a failure "
    "mode and its correction), extract (a source extraction), insight (a "
    "conclusion drawn from evidence), manual (user documentation), moc (a map "
    "of links), pattern (reusable implementation rules), principle (a general "
    "design rule)."
)
DUPLICATE_INSTRUCTIONS = (
    "Is the NOTE the same claim as the CANDIDATE? Answer yes only if both "
    "assert the same proposition (same mechanism, number, decision or "
    "conclusion), not merely the same topic."
)


class ValidationError(Exception):
    def __init__(self, problems: list[str]):
        self.problems = problems
        super().__init__(f"{len(problems)} problem(s)")


def load_fixture(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as fh:
            fixture = json.load(fh)
    except OSError as exc:
        raise ValidationError([f"cannot read {path}: {exc}"]) from exc
    except json.JSONDecodeError as exc:
        raise ValidationError([f"{path} is not JSON: {exc}"]) from exc

    problems: list[str] = []
    if not isinstance(fixture, dict):
        raise ValidationError(["top level must be an object"])
    if not isinstance(fixture.get("sampling"), (dict, str)):
        problems.append("missing `sampling` (the rule used to pick the notes)")

    notes = fixture.get("notes")
    if not isinstance(notes, list):
        raise ValidationError(problems + ["`notes` must be a list"])
    if len(notes) != EXPECTED_NOTES:
        problems.append(f"`notes` has {len(notes)} entries, expected {EXPECTED_NOTES}")

    seen: set[str] = set()
    for i, note in enumerate(notes):
        where = f"notes[{i}]"
        if not isinstance(note, dict):
            problems.append(f"{where}: must be an object")
            continue
        path_value = note.get("path")
        if not isinstance(path_value, str) or not path_value.endswith(".md"):
            problems.append(f"{where}.path: must be a .md path")
        elif path_value in seen:
            problems.append(f"{where}.path: duplicate path {path_value}")
        else:
            seen.add(path_value)

        note_type = note.get("type")
        if note_type not in NOTE_TYPES:
            problems.append(f"{where}.type: {note_type!r} is not one of the nine types")

        if not isinstance(note.get("description"), str):
            problems.append(f"{where}.description: must be a string")

        excerpt = note.get("excerpt")
        if not isinstance(excerpt, str):
            problems.append(f"{where}.excerpt: must be a string")
        elif not 400 <= len(excerpt) <= 800:
            problems.append(f"{where}.excerpt: {len(excerpt)} characters, expected 400-800")

        labels = note.get("labels")
        if not isinstance(labels, dict):
            problems.append(f"{where}.labels: must be an object")
            continue
        stale = labels.get("stale_description")
        if not isinstance(stale, dict) or stale.get("value") not in ("yes", "no"):
            problems.append(f"{where}.labels.stale_description.value: must be yes or no")
        elif not isinstance(stale.get("justification"), str) or not stale["justification"].strip():
            problems.append(f"{where}.labels.stale_description: missing justification")

        type_label = labels.get("type")
        if not isinstance(type_label, dict) or type_label.get("value") not in NOTE_TYPES:
            problems.append(f"{where}.labels.type.value: must be one of the nine types")
        elif type_label["value"] != note_type:
            problems.append(
                f"{where}.labels.type.value: {type_label['value']!r} does not match declared type {note_type!r}"
            )
        elif not isinstance(type_label.get("justification"), str) or not type_label["justification"].strip():
            problems.append(f"{where}.labels.type: missing justification")

        duplicate = labels.get("duplicate_of")
        if not isinstance(duplicate, dict):
            problems.append(f"{where}.labels.duplicate_of: must be an object")
        else:
            value = duplicate.get("value")
            if value is not None and (not isinstance(value, str) or not value.endswith(".md")):
                problems.append(f"{where}.labels.duplicate_of.value: must be a .md path or null")
            elif value is not None and value == path_value:
                problems.append(f"{where}.labels.duplicate_of.value: points at the note itself")
            if not isinstance(duplicate.get("justification"), str) or not duplicate["justification"].strip():
                problems.append(f"{where}.labels.duplicate_of: missing justification")

        candidate = note.get("duplicate_candidate")
        if not isinstance(candidate, dict):
            problems.append(f"{where}.duplicate_candidate: must be an object")
            continue
        candidate_path = candidate.get("path")
        if not isinstance(candidate_path, str) or not candidate_path.endswith(".md"):
            problems.append(f"{where}.duplicate_candidate.path: must be a .md path")
        elif candidate_path == path_value:
            problems.append(f"{where}.duplicate_candidate.path: candidates the note itself")
        if not isinstance(candidate.get("description"), str):
            problems.append(f"{where}.duplicate_candidate.description: must be a string")
        candidate_excerpt = candidate.get("excerpt")
        if not isinstance(candidate_excerpt, str):
            problems.append(f"{where}.duplicate_candidate.excerpt: must be a string")
        elif not 400 <= len(candidate_excerpt) <= 800:
            problems.append(f"{where}.duplicate_candidate.excerpt: must be 400-800 characters")

    if problems:
        raise ValidationError(problems)
    return fixture


def build_request(note: dict, model: str) -> dict:
    candidate = note["duplicate_candidate"]
    state = {
        "note": {
            "description": note["description"],
            "body_excerpt": note["excerpt"],
        },
        "candidate": {
            "description": candidate["description"],
            "body_excerpt": candidate["excerpt"],
        },
    }
    # The vendor's shape, measured 2026-11-16 against subq-snap-1.2: questions is an
    # object keyed by question id, and the type-specific parts live under `criteria` —
    # {true, false} for `noul`, {option: description-or-null} for `choice`.
    questions = {
        "stale_description": {
            "type": "noul",
            "instructions": STALE_INSTRUCTIONS,
            "criteria": {
                "true": "the description still describes the body",
                "false": "the description is stale, wrong, or about another subject",
            },
        },
        "type": {
            "type": "choice",
            "instructions": TYPE_INSTRUCTIONS,
            "criteria": {note_type: None for note_type in NOTE_TYPES},
        },
        "duplicate": {
            "type": "noul",
            "instructions": DUPLICATE_INSTRUCTIONS,
            "criteria": {"true": "the same claim", "false": "a different claim"},
        },
    }
    return {"model": model, "state": state, "questions": questions}


def dumps_body(body: dict) -> str:
    # The dry run prints this string, so it is byte-for-byte the body that is sent.
    return json.dumps(body, ensure_ascii=False, indent=2)


def post_systemone(endpoint: str, body: dict, key: str | None, timeout: float) -> dict:
    url = endpoint.rstrip("/") + "/v1/systemone"
    request = urllib.request.Request(
        url,
        data=dumps_body(body).encode("utf-8"),
        method="POST",
        headers={"Content-Type": "application/json", "Accept": "application/json"},
    )
    if key:
        request.add_header("Authorization", f"Bearer {key}")
    with urllib.request.urlopen(request, timeout=timeout) as response:
        payload = response.read().decode("utf-8")
    parsed = json.loads(payload)
    if not isinstance(parsed, dict):
        raise ValueError("response is not a JSON object")
    return parsed


def load_project_env(start: str) -> str | None:
    """Load KEY=VALUE pairs from the nearest `.env` above this file.

    The project keeps its key in `<repo>/.env` (gitignored), so the harness works from
    a checkout with no shell setup. A variable already in the environment wins: an
    explicit export is a stronger statement than a file, and a CI run must be able to
    override the file without editing it.
    """
    directory = os.path.dirname(os.path.abspath(start))
    while True:
        candidate = os.path.join(directory, ".env")
        if os.path.isfile(candidate):
            try:
                with open(candidate, encoding="utf-8") as handle:
                    for line in handle:
                        line = line.strip()
                        if not line or line.startswith("#") or "=" not in line:
                            continue
                        name, _, value = line.partition("=")
                        name = name.strip()
                        if name.startswith("export "):
                            name = name[len("export ") :].strip()
                        value = value.strip().strip('"').strip("'")
                        os.environ.setdefault(name, value)
            except OSError:
                pass
            return candidate
        parent = os.path.dirname(directory)
        if parent == directory:
            return None
        directory = parent


def pick_answer(payload: dict, question_id: str):
    answers = payload.get("answers")
    if answers is None:
        answers = payload.get("results")
    if answers is None:
        answers = payload
    if isinstance(answers, dict):
        return answers.get(question_id)
    if isinstance(answers, list):
        for item in answers:
            if isinstance(item, dict) and item.get("id") == question_id:
                return item
    return None


def noul_probability(raw) -> float | None:
    if isinstance(raw, (int, float)):
        return float(raw)
    if isinstance(raw, dict):
        for field in ("noul", "probability", "prob", "p"):
            value = raw.get(field)
            if isinstance(value, (int, float)):
                return float(value)
    return None


def choice_value(raw) -> str | None:
    if isinstance(raw, str):
        return raw
    if isinstance(raw, dict):
        for field in ("choice", "answer", "value"):
            value = raw.get(field)
            if isinstance(value, str):
                return value
        probabilities = raw.get("probabilities")
        if isinstance(probabilities, dict) and probabilities:
            numeric = {k: v for k, v in probabilities.items() if isinstance(v, (int, float))}
            if numeric:
                return max(numeric.items(), key=lambda kv: kv[1])[0]
    return None


def choice_probability(raw, answer: str | None) -> float | None:
    if isinstance(raw, dict):
        confidence = raw.get("confidence")
        if isinstance(confidence, (int, float)):
            return float(confidence)
        probabilities = raw.get("probabilities")
        if isinstance(probabilities, dict) and answer in probabilities:
            value = probabilities[answer]
            if isinstance(value, (int, float)):
                return float(value)
    return None


def noul_verdict(probability: float | None, threshold: float) -> str:
    if probability is None:
        return "error"
    if probability >= threshold:
        return "yes"
    if probability <= 1.0 - threshold:
        return "no"
    return "uncertain"


def score_note(note: dict, payload: dict, threshold: float) -> dict:
    gold_stale = note["labels"]["stale_description"]["value"]
    gold_type = note["labels"]["type"]["value"]
    gold_duplicate = "yes" if note["labels"]["duplicate_of"]["value"] else "no"
    candidate_path = note["duplicate_candidate"]["path"]

    rows: dict[str, dict] = {}

    raw = pick_answer(payload, "stale_description")
    probability = noul_probability(raw)
    # Polarity, and the bug this comment exists to prevent: the model answers "does the
    # description still describe the body?" (yes = accurate), while the hand label is
    # `stale_description` (yes = stale). Comparing the two directly scores a perfectly
    # calibrated judge at zero; measured 2026-11-16, that is exactly what happened and
    # precision sat at 0.12 while the answers were fine. `verdict` is reported in the
    # label's own direction.
    answers = noul_verdict(probability, threshold)
    verdict = {"yes": "no", "no": "yes"}.get(answers, answers)
    rows["stale_description"] = {
        "question": "does the description still describe the body? (verdict in the label's direction: yes = stale)",
        "answer": probability,
        "probability": probability,
        "threshold": threshold,
        "verdict": verdict,
        "gold": gold_stale,
        "matched": None if verdict in ("uncertain", "error") else verdict == gold_stale,
    }

    raw = pick_answer(payload, "type")
    answer = choice_value(raw)
    rows["type"] = {
        "question": "which of the nine note types is it?",
        "answer": answer,
        "probability": choice_probability(raw, answer),
        "threshold": None,
        "verdict": "error" if answer is None else answer,
        "gold": gold_type,
        "matched": None if answer is None else answer == gold_type,
    }

    raw = pick_answer(payload, "duplicate")
    probability = noul_probability(raw)
    verdict = noul_verdict(probability, threshold)
    rows["duplicate"] = {
        "question": f"is this the same claim as {candidate_path}?",
        "answer": probability,
        "probability": probability,
        "threshold": threshold,
        "verdict": verdict,
        "gold": gold_duplicate,
        "matched": None if verdict in ("uncertain", "error") else verdict == gold_duplicate,
    }

    return {"path": note["path"], "type": note["type"], "questions": rows}


def error_row(note: dict, message: str) -> dict:
    rows = {}
    for question_id, question in (
        ("stale_description", "does the description still describe the body?"),
        ("type", "which of the nine note types is it?"),
        ("duplicate", f"is this the same claim as {note['duplicate_candidate']['path']}?"),
    ):
        rows[question_id] = {
            "question": question,
            "answer": None,
            "probability": None,
            "threshold": None,
            "verdict": "error",
            "gold": None,
            "matched": None,
            "error": message,
        }
    return {"path": note["path"], "type": note["type"], "questions": rows}


def summarise(rows: list[dict]) -> dict:
    summary = {}
    for question_id in ("stale_description", "type", "duplicate"):
        decided = correct = uncertain = errors = 0
        for row in rows:
            cell = row["questions"][question_id]
            if "error" in cell or cell.get("verdict") == "error":
                errors += 1
            elif cell["matched"] is None:
                uncertain += 1
            else:
                decided += 1
                correct += int(bool(cell["matched"]))
        # Accuracy alone hides the base rate: 40 of the 50 notes are not stale, so
        # answering "no" every time scores 0.80 while finding nothing. For the yes/no
        # questions report the yes class's precision and recall beside it, which is what
        # "does this detector work" actually asks.
        tp = fp = fn = 0
        for row in rows:
            cell = row["questions"][question_id]
            if "error" in cell or cell.get("matched") is None:
                continue
            if cell["verdict"] == "yes" and cell["gold"] == "yes":
                tp += 1
            elif cell["verdict"] == "yes":
                fp += 1
            elif cell["gold"] == "yes":
                fn += 1
        summary[question_id] = {
            "decided": decided,
            "correct": correct,
            "accuracy": (correct / decided) if decided else None,
            # Kept for callers that read the old key; it was always accuracy.
            "precision": (correct / decided) if decided else None,
            "yes_precision": (tp / (tp + fp)) if (tp + fp) else None,
            "yes_recall": (tp / (tp + fn)) if (tp + fn) else None,
            "uncertain": uncertain,
            "errors": errors,
        }
    return summary


def print_precision(summary: dict) -> None:
    print()
    header = f"{'question':<20}{'decided':>8}{'accuracy':>9}{'yes_prec':>9}{'yes_rec':>8}{'uncertain':>10}{'errors':>7}"
    print(header)
    for question_id in ("stale_description", "type", "duplicate"):
        row = summary[question_id]

        def fmt(value):
            return "n/a" if value is None else f"{value:.3f}"

        print(
            f"{question_id:<20}{row['decided']:>8}{fmt(row['accuracy']):>9}"
            f"{fmt(row['yes_precision']):>9}{fmt(row['yes_recall']):>8}"
            f"{row['uncertain']:>10}{row['errors']:>7}"
        )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--labels", default=os.path.join(HERE, "labelled.json"))
    parser.add_argument("--report", default=os.path.join(HERE, "report.json"))
    parser.add_argument("--endpoint", default=None, help="systemone-compatible base URL (or JUDGEMENT_ENDPOINT)")
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--threshold", type=float, default=0.7, help="noul >= t means yes, <= 1-t means no")
    parser.add_argument("--price-per-mtok", type=float, default=0.04)
    parser.add_argument("--concurrency", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--dry-run", action="store_true", help="print the first request and the cost, send nothing")
    args = parser.parse_args(argv)

    if not 0.5 <= args.threshold <= 1.0:
        print("audit: --threshold must be between 0.5 and 1.0", file=sys.stderr)
        return 2
    if args.concurrency < 1:
        print("audit: --concurrency must be at least 1", file=sys.stderr)
        return 2

    try:
        fixture = load_fixture(args.labels)
    except ValidationError as exc:
        print(f"audit: {args.labels} failed validation:", file=sys.stderr)
        for problem in exc.problems:
            print(f"  - {problem}", file=sys.stderr)
        return 2

    notes = fixture["notes"]
    requests = [build_request(note, args.model) for note in notes]
    total_chars = sum(len(dumps_body(body)) for body in requests)
    tokens = total_chars / 4.0
    cost = tokens / 1_000_000.0 * args.price_per_mtok

    endpoint = args.endpoint or os.environ.get("JUDGEMENT_ENDPOINT") or DEFAULT_ENDPOINT
    env_file = load_project_env(__file__)
    key = os.environ.get("SUBQ_API_KEY") or None

    if args.dry_run:
        print(f"POST {endpoint.rstrip('/')}/v1/systemone")
        if key:
            print("Authorization: Bearer <SUBQ_API_KEY set>")
        else:
            print("Authorization: (none; --dry-run sends nothing)")
        print("Content-Type: application/json")
        print()
        print(dumps_body(requests[0]))
        print()
        print(
            f"cost: {len(requests)} requests, {total_chars:,} characters "
            f"≈ {tokens:,.0f} tokens ≈ ${cost:.6f} at ${args.price_per_mtok:g} per 1M tokens"
        )
        return 0

    explicit_endpoint = bool(args.endpoint or os.environ.get("JUDGEMENT_ENDPOINT"))
    if not key and not explicit_endpoint:
        print(
            "audit: SUBQ_API_KEY and JUDGEMENT_ENDPOINT are both unset; refusing to run the judge.",
            file=sys.stderr,
        )
        print(
            "audit: set SUBQ_API_KEY for the vendor (env or <repo>/.env), or JUDGEMENT_ENDPOINT "
            "(--endpoint) "
            "for a local systemone-compatible judge. Use --dry-run to inspect the request "
            "without a key.",
            file=sys.stderr,
        )
        return 3

    rows: list[dict] = []
    started = datetime.now(timezone.utc)

    def work(note: dict, body: dict) -> dict:
        try:
            payload = post_systemone(endpoint, body, key, args.timeout)
        except (urllib.error.URLError, ValueError, TimeoutError, OSError, http.client.HTTPException) as exc:
            return error_row(note, f"{type(exc).__name__}: {exc}")
        return score_note(note, payload, args.threshold)

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        futures = [pool.submit(work, note, body) for note, body in zip(notes, requests)]
        for future in concurrent.futures.as_completed(futures):
            rows.append(future.result())

    order = {note["path"]: i for i, note in enumerate(notes)}
    rows.sort(key=lambda row: order[row["path"]])
    summary = summarise(rows)
    failed_requests = sum(
        1
        for row in rows
        if any("error" in cell or cell.get("verdict") == "error" for cell in row["questions"].values())
    )

    report = {
        "generated_at": started.isoformat(timespec="seconds"),
        "endpoint": endpoint,
        "model": args.model,
        "threshold": args.threshold,
        "price_per_mtok": args.price_per_mtok,
        "notes": len(notes),
        "characters": total_chars,
        "tokens_estimated": round(tokens),
        "cost_estimated_usd": round(cost, 8),
        "errors": failed_requests,
        "precision": summary,
        "results": rows,
    }
    with open(args.report, "w", encoding="utf-8") as fh:
        json.dump(report, fh, ensure_ascii=False, indent=2)
        fh.write("\n")

    print(f"audit: {len(notes)} notes, {len(notes) * 3} questions, {failed_requests} failed requests")
    print(
        f"audit: {total_chars:,} characters ≈ {tokens:,.0f} tokens ≈ ${cost:.6f} "
        f"at ${args.price_per_mtok:g} per 1M tokens"
    )
    print(f"audit: report written to {args.report}")
    print_precision(summary)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
