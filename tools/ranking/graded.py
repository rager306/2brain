#!/usr/bin/env python3
"""Grade the task set: MRR, nDCG@10 and rank distance over the same eighteen questions.

    python3 tools/ranking/graded.py run.json                  # one run
    python3 tools/ranking/graded.py before.json after.json    # two runs, and their difference
    python3 tools/ranking/graded.py --json run.json
    python3 tools/ranking/graded.py --selftest

`tasks_eval.py` answers one question per task — did the evidence reach the rank the task asks for — so a
change either flips a task or is invisible. This reads the same runs and says how far the evidence moved,
which is the number most ranking changes actually change.

Three views of one observation, kept apart on purpose:

- **MRR**: 1/rank of the first acceptable path, 0 when no acceptable path is in the page;
- **nDCG@10**: the same position under a logarithmic discount, truncated at ten;
- **rank distance**: rank − 1, linear, so a move from 8 to 3 reads as five places rather than as a ratio.

Gains are binary. The labels are a *set* of acceptable paths, not grades of relevance: a path is either
evidence for the question or it is not. Because the labels carry no grades, any graded metric over them is a
function of the first acceptable hit's rank, and the ideal ranking is that hit at rank 1, so IDCG = 1. For a
task whose expectation is "any of these", counting every alternative as required would penalise a ranker for
a choice the label itself calls equivalent; the number of acceptable paths the page holds is printed beside
the score instead of being folded into it.

The aggregate is over the classes that have an expected path. `absent` is excluded because there is nothing
to rank — the base should answer nothing — and its pass rule stays in `tasks_eval.py`, which is a different
measurement. A task's `min_total` is likewise not a rank: it pins how wide a flagged candidate set must be,
and `search.rs` can collapse the set while leaving this note first.

Deterministic, standard library only.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
LIMIT = 10


def refuse(message: str):
    """Input that cannot be scored is not a score; exit code 2 says so, as in bootstrap.py."""
    print(message, file=sys.stderr)
    raise SystemExit(2)


def load_tasks(path: Path) -> list[dict]:
    data = json.loads(path.read_text())
    rows = data["rows"] if isinstance(data, dict) else data
    if not isinstance(rows, list):
        refuse(f"{path}: expected a `rows` array")
    return rows


def load_run(path: Path) -> list[dict]:
    data = json.loads(path.read_text())
    if not isinstance(data, list):
        refuse(f"{path}: expected the JSON array `tasks_eval.py --json` prints")
    for index, row in enumerate(data):
        missing = [key for key in ("query", "class", "page") if key not in row]
        if missing:
            refuse(f"{path}: row {index} lacks {', '.join(missing)}; regenerate it with this tree's "
                   f"`tasks_eval.py --json`, whose record carries the whole page")
    return data


def pair(tasks: list[dict], run: list[dict]) -> list[tuple[dict, dict]]:
    """Pair the labels with one run by position, after checking that the sequences agree.

    Position is the right key and `(query, class)` is not: `mixed-language` is two rows with the same
    query and different `args`, and the eval record does not carry `args`. The sequence is verified so a
    reordered file is refused rather than silently mislabelled.
    """
    if len(tasks) != len(run):
        refuse(f"the run does not match the set: {len(run)} rows against {len(tasks)} tasks")
    keys_tasks = [(task["query"], task["class"]) for task in tasks]
    keys_run = [(row["query"], row["class"]) for row in run]
    if keys_tasks != keys_run:
        first = next(i for i, (a, b) in enumerate(zip(keys_tasks, keys_run)) if a != b)
        refuse(f"row {first} is {keys_tasks[first]!r} in the set and {keys_run[first]!r} in the run")
    return list(zip(tasks, run))


def acceptable(task: dict) -> list[str]:
    """The paths that answer the task: `accept` when several are equivalent, else the minimal `evidence`."""
    return list(task.get("accept") or task.get("evidence") or [])


def score_task(task: dict, row: dict, limit: int = LIMIT) -> dict:
    wanted = acceptable(task)
    page = list(row["page"])[:limit]
    score = {
        "query": task["query"],
        "class": task["class"],
        "scored": bool(wanted),
        "page": page,
        "page_len": len(page),
        "passed": bool(row.get("passed")),
        "rank": None,
        "mrr": None,
        "ndcg10": None,
        "distance": None,
        "found": 0,
        "expected": len(wanted),
    }
    if not wanted:
        return score
    positions = [position for position, path in enumerate(page, 1) if path in wanted]
    rank = positions[0] if positions else None
    score.update(
        rank=rank,
        mrr=(1.0 / rank) if rank else 0.0,
        ndcg10=(1.0 / math.log2(rank + 1)) if rank else 0.0,
        distance=(rank - 1) if rank else None,
        found=len(positions),
    )
    return score


def mean(values: list[float]) -> float | None:
    return sum(values) / len(values) if values else None


def summarise(tasks: list[dict], run: list[dict], limit: int = LIMIT) -> dict:
    rows = [score_task(task, row, limit) for task, row in pair(tasks, run)]
    scored = [row for row in rows if row["scored"]]
    hits = [row for row in scored if row["rank"]]
    classes: dict[str, list[dict]] = {}
    for row in scored:
        classes.setdefault(row["class"], []).append(row)
    per_class = {
        name: {
            "n": len(group),
            "mrr": mean([row["mrr"] for row in group]),
            "ndcg10": mean([row["ndcg10"] for row in group]),
            "top1": sum(1 for row in group if row["rank"] == 1),
            "zero": sum(1 for row in group if not row["rank"]),
            "hits": sum(1 for row in group if row["rank"]),
            "mean_distance": mean([row["distance"] for row in group if row["rank"]]),
        }
        for name, group in sorted(classes.items())
    }
    excluded: dict[str, int] = {}
    for row in rows:
        if not row["scored"]:
            excluded[row["class"]] = excluded.get(row["class"], 0) + 1
    return {
        "n": len(rows),
        "limit": limit,
        "scored_n": len(scored),
        "excluded": excluded,
        "mrr": mean([row["mrr"] for row in scored]) or 0.0,
        "ndcg10": mean([row["ndcg10"] for row in scored]) or 0.0,
        "passed_n": sum(1 for row in rows if row["passed"]),
        "top1": sum(1 for row in scored if row["rank"] == 1),
        "zero_hits": sum(1 for row in scored if not row["rank"]),
        "hits": len(hits),
        "mean_distance": mean([row["distance"] for row in hits]),
        "macro_mrr": mean([entry["mrr"] for entry in per_class.values()]) or 0.0,
        "macro_ndcg10": mean([entry["ndcg10"] for entry in per_class.values()]) or 0.0,
        "classes": per_class,
        "tasks": rows,
    }


def _fmt(value: float | None, width: int = 6, digits: int = 3) -> str:
    return f"{'-':>{width}}" if value is None else f"{value:>{width}.{digits}f}"


def report(result: dict, source: str) -> None:
    excluded = ", ".join(f"{name} {count}" for name, count in sorted(result["excluded"].items())) or "none"
    print(f"graded ranking over {result['n']} tasks, page of {result['limit']}, binary gains   {source}")
    print(f"  {result['scored_n']} carry an expected path; {result['n'] - result['scored_n']} excluded "
          f"({excluded}: nothing to rank)")
    print()
    print(f"  MRR        {result['mrr']:.3f}")
    print(f"  nDCG@{result['limit']:<3}  {result['ndcg10']:.3f}")
    print(f"  macro over classes  MRR {result['macro_mrr']:.3f}   nDCG {result['macro_ndcg10']:.3f}")
    print(f"  first evidence at rank 1  {result['top1']}/{result['scored_n']}")
    print(f"  tasks_eval pass count     {result['passed_n']}/{result['n']}   (the metric this replaces: a "
          f"page that is right for the wrong reason passes, one at rank 2 does not)")
    print(f"  mean rank distance  {_fmt(result['mean_distance']).strip()}   over {result['hits']} tasks "
          f"whose evidence is in the page")
    print(f"  zero expected hits  {result['zero_hits']}/{result['scored_n']}")
    print()
    print(f"  {'class':<16}{'n':>3}{'MRR':>8}{'nDCG@10':>9}{'top1':>6}{'zero':>6}{'dist':>7}")
    for name, entry in result["classes"].items():
        dist = _fmt(entry["mean_distance"]).strip() if entry["hits"] else "-"
        print(f"  {name:<16}{entry['n']:>3}{entry['mrr']:>8.3f}{entry['ndcg10']:>9.3f}"
              f"{entry['top1']:>6}{entry['zero']:>6}{dist:>7}")


def compare(before: dict, after: dict) -> dict:
    if before["n"] != after["n"]:
        refuse(f"the runs do not pair: {before['n']} tasks against {after['n']}")
    left = [(row["query"], row["class"]) for row in before["tasks"]]
    right = [(row["query"], row["class"]) for row in after["tasks"]]
    if left != right:
        first = next(i for i, (a, b) in enumerate(zip(left, right)) if a != b)
        refuse(f"the runs do not pair: row {first} is {left[first]!r} and {right[first]!r}")
    scores_identical = all(
        (a["scored"], a["rank"], a["found"]) == (b["scored"], b["rank"], b["found"])
        for a, b in zip(before["tasks"], after["tasks"]))
    pages_identical = all(a["page"] == b["page"] for a, b in zip(before["tasks"], after["tasks"]))
    d_mrr = after["mrr"] - before["mrr"]
    d_ndcg = after["ndcg10"] - before["ndcg10"]
    if scores_identical:
        verdict = "identical"
    elif d_mrr > 0 and d_ndcg > 0:
        verdict = "better"
    elif d_mrr < 0 and d_ndcg < 0:
        verdict = "worse"
    else:
        verdict = "mixed"
    return {
        "n": before["n"],
        "scored_n": before["scored_n"],
        "scores_identical": scores_identical,
        "pages_identical": pages_identical,
        "before": {key: before[key] for key in ("mrr", "ndcg10", "passed_n", "top1", "zero_hits",
                                                  "mean_distance")},
        "after": {key: after[key] for key in ("mrr", "ndcg10", "passed_n", "top1", "zero_hits",
                                                 "mean_distance")},
        "delta_mrr": d_mrr,
        "delta_ndcg10": d_ndcg,
        "delta_passed": after["passed_n"] - before["passed_n"],
        "delta_top1": after["top1"] - before["top1"],
        "delta_zero_hits": after["zero_hits"] - before["zero_hits"],
        "verdict": verdict,
        "moved": [
            {"class": a["class"], "query": a["query"], "before": a["rank"], "after": b["rank"]}
            for a, b in zip(before["tasks"], after["tasks"])
            if a["scored"] and (a["rank"] != b["rank"] or a["found"] != b["found"])
        ],
        "classes": {
            name: {
                "n": entry["n"],
                "before_mrr": entry["mrr"],
                "after_mrr": after["classes"][name]["mrr"],
                "before_ndcg10": entry["ndcg10"],
                "after_ndcg10": after["classes"][name]["ndcg10"],
                "before_top1": entry["top1"],
                "after_top1": after["classes"][name]["top1"],
            }
            for name, entry in before["classes"].items()
        },
    }


def report_compare(result: dict, before_path: Path, after_path: Path) -> None:
    b, a = result["before"], result["after"]
    print(f"two runs over {result['n']} tasks; {result['scored_n']} carry an expected path")
    print(f"  before  {before_path}")
    print(f"  after   {after_path}")
    print(f"  identical scores  {'yes' if result['scores_identical'] else 'no'}"
          f"   identical pages  {'yes' if result['pages_identical'] else 'no'}")
    print()
    print(f"  {'metric':<22}{'before':>8}{'after':>8}{'delta':>9}")
    for label, key in (("MRR", "mrr"), ("nDCG@10", "ndcg10"), ("tasks_eval passes", "passed_n"),
                       ("first evidence rank 1", "top1"), ("zero expected hits", "zero_hits"),
                       ("mean rank distance", "mean_distance")):
        left, right = b[key], a[key]
        if key in ("passed_n", "top1", "zero_hits"):
            print(f"  {label:<22}{left:>8}{right:>8}{right - left:>+9}")
        else:
            shown_delta = "-" if left is None or right is None else f"{right - left:+.3f}"
            print(f"  {label:<22}{_fmt(left).strip():>8}{_fmt(right).strip():>8}{shown_delta:>9}")
    relative = (result["delta_mrr"] / b["mrr"]) if b["mrr"] else None
    print()
    print(f"  verdict on the graded view  {result['verdict'].upper()}")
    if relative is not None:
        print(f"  MRR moved {result['delta_mrr']:+.3f} ({relative:+.1%} of the before value); "
              f"nDCG moved {result['delta_ndcg10']:+.3f}")
    if result["moved"]:
        print(f"  {len(result['moved'])} task(s) moved their evidence inside the page:")
        for row in result["moved"]:
            was = f"rank {row['before']}" if row["before"] else "absent"
            now = f"rank {row['after']}" if row["after"] else "absent"
            print(f"    {row['class']:<16} {was:>9} -> {now:<9} {row['query'][:50]}")
    else:
        print("  no task moved its evidence inside the page")
    print()
    print(f"  {'class':<16}{'n':>3}{'MRR b/a':>16}{'nDCG b/a':>16}{'top1 b/a':>10}")
    for name, entry in result["classes"].items():
        print(f"  {name:<16}{entry['n']:>3}{entry['before_mrr']:>8.3f}/{entry['after_mrr']:<7.3f}"
              f"{entry['before_ndcg10']:>8.3f}/{entry['after_ndcg10']:<7.3f}"
              f"{entry['before_top1']:>4}/{entry['after_top1']:<5}")
    print()
    print("  what the graded view adds: every row that kept its verdict but moved in rank now enters the "
          "aggregate.\n  What it does not add: `absent` has no path to rank, so its 0/3 is invisible here; "
          "a page that\n  improves `min_total` without moving this note scores the same; and a note that "
          "answers the question but\n  is not in the label set cannot raise the score.")


def make_case(ranks: list[int | None], limit: int = LIMIT) -> tuple[list[dict], list[dict]]:
    """A run whose single acceptable path sits at each rank, or nowhere when the rank is None."""
    tasks, run = [], []
    for index, rank in enumerate(ranks):
        tasks.append({"query": f"q{index}", "class": "c", "evidence": ["wanted.md"], "expect": "hit"})
        page = [f"other{position}.md" for position in range(1, limit + 1)]
        if rank:
            page[rank - 1] = "wanted.md"
        run.append({"query": f"q{index}", "class": "c", "page": page, "passed": rank == 1})
    return tasks, run


def selftest() -> int:
    """The arithmetic, on shapes whose answer is known by hand."""
    tasks, run = make_case([1, 1, 1])
    perfect = summarise(tasks, run)
    tasks, run = make_case([10, 10, 10])
    last = summarise(tasks, run)
    tasks, run = make_case([None, None])
    nowhere = summarise(tasks, run)
    tasks, run = make_case([1, 3])
    mixed = summarise(tasks, run)
    absent_tasks = [{"query": "q", "class": "absent", "evidence": [], "expect": "none"}]
    absent_run = [{"query": "q", "class": "absent", "page": ["a.md"], "passed": True}]
    excluded = summarise(absent_tasks, absent_run)
    checks = [
        ("rank 1 on every task is a perfect score",
         perfect,
         lambda r: abs(r["mrr"] - 1.0) < 1e-12 and abs(r["ndcg10"] - 1.0) < 1e-12
         and r["top1"] == 3 and r["zero_hits"] == 0 and r["mean_distance"] == 0.0),
        ("rank 10 is 0.100 MRR and 1/log2(11) nDCG over nine places",
         last,
         lambda r: abs(r["mrr"] - 0.1) < 1e-12 and abs(r["ndcg10"] - 1 / math.log2(11)) < 1e-12
         and r["mean_distance"] == 9.0 and r["top1"] == 0),
        ("evidence outside the page scores zero and counts as a zero hit",
         nowhere,
         lambda r: r["mrr"] == 0.0 and r["ndcg10"] == 0.0 and r["zero_hits"] == 2
         and r["mean_distance"] is None),
        ("a set of ranks aggregates by the mean",
         mixed,
         lambda r: abs(r["mrr"] - (2 / 3)) < 1e-12 and abs(r["ndcg10"] - 0.75) < 1e-12
         and r["top1"] == 1 and abs(r["mean_distance"] - 1.0) < 1e-12),
        ("a class with no expected path is excluded, not scored as zero",
         excluded,
         lambda r: r["scored_n"] == 0 and r["excluded"] == {"absent": 1} and r["mrr"] == 0.0),
        ("a record without the page is refused",
         _refused(lambda: load_run_without_page()),
         lambda r: r is True),
        ("a reordered run is refused",
         _refused(lambda: pair(make_case([1, 2])[0], list(reversed(make_case([1, 2])[1])))),
         lambda r: r is True),
        ("a self-pair is identical and a reversed page is worse",
         reversed_case(),
         lambda r: r["self"]["scores_identical"] and r["self"]["verdict"] == "identical"
         and r["reversed"]["verdict"] == "worse" and r["reversed"]["delta_mrr"] < 0
         and r["reversed"]["delta_ndcg10"] < 0),
    ]
    failed = 0
    for label, result, check in checks:
        ok = check(result)
        failed += 0 if ok else 1
        print(f"{'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            print(f"       {result!r}")
    print(f"\n{len(checks) - failed}/{len(checks)} arithmetic checks pass")
    return 0 if failed == 0 else 1


def load_run_without_page() -> list[dict]:
    import tempfile

    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
        json.dump([{"query": "q", "class": "c", "top": ["a.md"]}], handle)
        path = Path(handle.name)
    return load_run(path)


def reversed_case() -> dict:
    tasks, run = make_case([1, 2, 3, 10, None])
    flipped = [dict(row, page=list(reversed(row["page"]))) for row in run]
    return {"self": compare(summarise(tasks, run), summarise(tasks, run)),
            "reversed": compare(summarise(tasks, run), summarise(tasks, flipped))}


def _refused(action) -> bool:
    try:
        action()
    except SystemExit:
        return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("before", type=Path, nargs="?")
    parser.add_argument("after", type=Path, nargs="?")
    parser.add_argument("--set", type=Path, default=REPO / "tools/ranking/tasks.json",
                        help="the labelled task set the run was made from")
    parser.add_argument("--limit", type=int, default=LIMIT)
    parser.add_argument("--json", action="store_true", help="print the record, for a doc or a gate")
    parser.add_argument("--selftest", action="store_true", help="check the arithmetic on known shapes")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if args.limit < 1:
        parser.error("--limit must be at least 1")
    if not args.before:
        parser.error("a `tasks_eval.py --json` file is required (or --selftest)")
    tasks = load_tasks(args.set)
    first = summarise(tasks, load_run(args.before), args.limit)
    if not args.after:
        if args.json:
            print(json.dumps(first, indent=2, ensure_ascii=False))
        else:
            report(first, str(args.before))
        return 0
    second = summarise(tasks, load_run(args.after), args.limit)
    result = compare(first, second)
    if args.json:
        print(json.dumps(result, indent=2, ensure_ascii=False))
    else:
        report_compare(result, args.before, args.after)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
