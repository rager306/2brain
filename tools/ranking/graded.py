#!/usr/bin/env python3
"""Grade the task set: MRR, nDCG@5 over graded gains, and rank distance over the same eighteen questions.

    python3 tools/ranking/graded.py run.json                  # one run
    python3 tools/ranking/graded.py before.json after.json    # two runs, and their difference
    python3 tools/ranking/graded.py --mde a.json a.json       # the noise floor, the cross-build spread, the MDE
    python3 tools/ranking/graded.py --json run.json
    python3 tools/ranking/graded.py --selftest

`tasks_eval.py` answers one question per task — did the evidence reach the rank the task asks for — so a
change either flips a task or is invisible. This reads the same runs and says how far the evidence moved,
which is the number most ranking changes actually change.

**Grades.** Every path in a task's `evidence` or `accept` is grade 2: the label calls it the answer, and an
`accept` alternative is a grade-2 answer by the label's own words. An optional `support` list names grade-1
paths — a note a reader needs beside the answer: the second hop, the map that states the rule, the general
model the note specialises. Any other path in the page is grade 0. A row with no `support` is a binary
label, and that is a statement about the task, not a gap in the tooling; `docs/research/task-set.md` says
which rows are binary and why. The grades never widen the answer set: `support` is a separate field, so
`tasks_eval.py`'s pass rules — pre-registered before this metric existed — are untouched.

**nDCG@5, linear gain, a true IDCG.** For one query and the page d_1 … d_5:

    DCG@5  = sum_{i=1..5}  g(d_i) / log2(i + 1)     g = 2, 1 or 0, linear in the grade
    IDCG@5 = sum_{i=1..5}  g*_i   / log2(i + 1)     g* = the task's grades, sorted descending
    nDCG@5 = DCG@5 / IDCG@5                          undefined when IDCG@5 = 0

Linear gain is trec_eval's default — the qrels relevance value is the gain — and the discount is
`log2(rank + 1)`. The exponential form `2^g - 1` is not used: with two grade-2 notes it weights the first
3:1 and hides the multi-hop miss that grades exist to expose. A task whose `IDCG@5` is 0 has nothing to
rank (every `absent` row) and is reported as excluded rather than scored as zero.

**Two companions, kept apart on purpose:**

- **MRR**: 1/rank of the first grade-2 path in the page — grade 1 is support, not an answer — and 0 when no
  answer is in the page. Its definition is the one the earlier table used, so those MRR numbers stay
  comparable;
- **rank distance**: rank − 1 of that same path, linear, so a move from 8 to 3 reads as five places rather
  than as a ratio.

The cutoff is 5 because the graded signal is the top of the page: a grade-2 note at rank 6 scores zero
nDCG@5 while MRR still gives it 1/6, and that difference is printed rather than hidden.

The aggregate is over the tasks that carry a grade (`IDCG@5 > 0`). A task's `min_total` is not a rank: it
pins how wide a flagged candidate set must be, and `search.rs` can collapse the set while leaving this note
first.

**σ and the MDE.** `--mde` reads two runs and reports the standard deviation of the per-task nDCG@5
deltas, the standard deviation of nDCG@5 across tasks, and the minimum detectable effect
`δ = 2.486·σ/√n` (80 % power, α = 0.05 one-sided; 2.486 = z_0.95 + z_0.80) at the number of tasks the runs
pair and at `--target-n`. Two runs of the same build are a self-pair: every delta is exactly zero because
the engine is deterministic, so that σ is the *noise floor*, not the σ the power formula needs — a σ of
zero would make every effect decidable — and the report says so. The headline MDE uses the cross-task σ,
the only estimate that does not come from the effect being measured.

Deterministic, standard library only.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PAGE = 10      # the page a `tasks_eval.py --json` record holds; MRR and rank distance read it
CUTOFF = 5     # nDCG@5
Z_POWER = 2.486  # z_0.95 + z_0.80: 80 % power at alpha = 0.05 one-sided (Webber, Moffat & Zobel 2008)


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


def graded_paths(task: dict) -> dict[str, int]:
    """The task's labels as path -> grade, with the answer set taken from the pass rule's own field.

    `accept` when several answers are equivalent, else `evidence`: that is exactly the set `tasks_eval.py`
    passes a run on, so grade 2 carries the label's existing claim instead of a new one. `support` is a
    separate field and an error if it repeats an answer, so a path can never be both.
    """
    labels: dict[str, int] = {}
    for path in task.get("accept") or task.get("evidence") or []:
        labels[path] = 2
    for path in task.get("support") or []:
        if path in labels:
            refuse(f"{task['query']!r}: {path!r} is labelled both an answer and support")
        labels[path] = 1
    return labels


def score_task(task: dict, row: dict, limit: int = PAGE, cutoff: int = CUTOFF) -> dict:
    labels = graded_paths(task)
    page = list(row["page"])[:limit]
    k = min(cutoff, limit)
    score = {
        "query": task["query"],
        "class": task["class"],
        "scored": False,
        "page": page,
        "page_len": len(page),
        "passed": bool(row.get("passed")),
        "labels": len(labels),
        "support": sum(1 for grade in labels.values() if grade == 1),
        "expected": sum(1 for grade in labels.values() if grade == 2),
        "rank": None,
        "mrr": None,
        "dcg5": 0.0,
        "idcg5": 0.0,
        "ndcg5": None,
        "distance": None,
        "answers_found": 0,
        "support_found": 0,
        "found": 0,
    }
    if not labels:
        return score
    ideal = sorted(labels.values(), reverse=True)[:k]
    idcg = sum(grade / math.log2(position + 1) for position, grade in enumerate(ideal, 1))
    dcg = sum(labels.get(path, 0) / math.log2(position + 1) for position, path in enumerate(page[:k], 1))
    positions = [position for position, path in enumerate(page, 1) if labels.get(path) == 2]
    rank = positions[0] if positions else None
    answers_found = sum(1 for path in page if labels.get(path) == 2)
    support_found = sum(1 for path in page if labels.get(path) == 1)
    score.update(
        scored=idcg > 0,
        rank=rank,
        mrr=(1.0 / rank) if rank else 0.0,
        dcg5=dcg,
        idcg5=idcg,
        ndcg5=(dcg / idcg) if idcg else None,
        distance=(rank - 1) if rank else None,
        answers_found=answers_found,
        support_found=support_found,
        found=answers_found + support_found,
    )
    return score


def mean(values: list[float]) -> float | None:
    return sum(values) / len(values) if values else None


def spread(values: list[float]) -> float | None:
    """The sample standard deviation (n − 1), which is what the power formula takes."""
    if len(values) < 2:
        return None
    centre = mean(values)
    return math.sqrt(sum((value - centre) ** 2 for value in values) / (len(values) - 1))


def summarise(tasks: list[dict], run: list[dict], limit: int = PAGE, cutoff: int = CUTOFF) -> dict:
    rows = [score_task(task, row, limit, cutoff) for task, row in pair(tasks, run)]
    scored = [row for row in rows if row["scored"]]
    hits = [row for row in scored if row["rank"]]
    classes: dict[str, list[dict]] = {}
    for row in scored:
        classes.setdefault(row["class"], []).append(row)
    per_class = {
        name: {
            "n": len(group),
            "mrr": mean([row["mrr"] for row in group]),
            "ndcg5": mean([row["ndcg5"] for row in group]),
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
        "cutoff": cutoff,
        "scored_n": len(scored),
        "excluded": excluded,
        "mrr": mean([row["mrr"] for row in scored]) or 0.0,
        "ndcg5": mean([row["ndcg5"] for row in scored]) or 0.0,
        "passed_n": sum(1 for row in rows if row["passed"]),
        "top1": sum(1 for row in scored if row["rank"] == 1),
        "zero_hits": sum(1 for row in scored if not row["rank"]),
        "hits": len(hits),
        "mean_distance": mean([row["distance"] for row in hits]),
        "labels": sum(row["labels"] for row in rows),
        "support_labels": sum(row["support"] for row in rows),
        "support_tasks": sum(1 for row in rows if row["support"]),
        "macro_mrr": mean([entry["mrr"] for entry in per_class.values()]) or 0.0,
        "macro_ndcg5": mean([entry["ndcg5"] for entry in per_class.values()]) or 0.0,
        "classes": per_class,
        "tasks": rows,
    }


def _fmt(value: float | None, width: int = 6, digits: int = 3) -> str:
    return f"{'-':>{width}}" if value is None else f"{value:>{width}.{digits}f}"


def report(result: dict, source: str) -> None:
    excluded = ", ".join(f"{name} {count}" for name, count in sorted(result["excluded"].items())) or "none"
    support = (f"{result['support_labels']} grade-1 label(s) on {result['support_tasks']} task(s)"
               if result["support_labels"] else "no grade-1 labels")
    print(f"graded ranking over {result['n']} tasks, page of {result['limit']}, nDCG@{result['cutoff']} "
          f"over {result['labels']} labels ({support})   {source}")
    print(f"  {result['scored_n']} carry a grade; {result['n'] - result['scored_n']} excluded "
          f"({excluded}: IDCG = 0, nothing to rank)")
    print()
    print(f"  nDCG@{result['cutoff']:<3}  {result['ndcg5']:.3f}")
    print(f"  MRR        {result['mrr']:.3f}   (first grade-2 path; grade 1 does not count as an answer)")
    print(f"  macro over classes  nDCG {result['macro_ndcg5']:.3f}   MRR {result['macro_mrr']:.3f}")
    print(f"  first answer at rank 1  {result['top1']}/{result['scored_n']}")
    print(f"  tasks_eval pass count     {result['passed_n']}/{result['n']}   (the metric this replaces: a "
          f"page that is right for the wrong reason passes, one at rank 2 does not)")
    print(f"  mean rank distance  {_fmt(result['mean_distance']).strip()}   over {result['hits']} tasks "
          f"whose answer is in the page")
    print(f"  no answer in the page  {result['zero_hits']}/{result['scored_n']}")
    print()
    print(f"  {'class':<16}{'n':>3}{'nDCG':>8}{'MRR':>8}{'top1':>6}{'zero':>6}{'dist':>7}")
    for name, entry in result["classes"].items():
        dist = _fmt(entry["mean_distance"]).strip() if entry["hits"] else "-"
        print(f"  {name:<16}{entry['n']:>3}{entry['ndcg5']:>8.3f}{entry['mrr']:>8.3f}"
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
        (a["scored"], a["ndcg5"], a["rank"], a["found"]) == (b["scored"], b["ndcg5"], b["rank"], b["found"])
        for a, b in zip(before["tasks"], after["tasks"]))
    pages_identical = all(a["page"] == b["page"] for a, b in zip(before["tasks"], after["tasks"]))
    pairs = [(a, b) for a, b in zip(before["tasks"], after["tasks"]) if a["scored"] and b["scored"]]
    deltas = [b["ndcg5"] - a["ndcg5"] for a, b in pairs]
    d_mrr = after["mrr"] - before["mrr"]
    d_ndcg = after["ndcg5"] - before["ndcg5"]
    if scores_identical:
        verdict = "identical"
    elif d_ndcg > 0:
        verdict = "better"
    elif d_ndcg < 0:
        verdict = "worse"
    else:
        verdict = "unchanged"
    return {
        "n": before["n"],
        "scored_n": before["scored_n"],
        "scores_identical": scores_identical,
        "pages_identical": pages_identical,
        "before": {key: before[key] for key in ("mrr", "ndcg5", "passed_n", "top1", "zero_hits",
                                                "mean_distance")},
        "after": {key: after[key] for key in ("mrr", "ndcg5", "passed_n", "top1", "zero_hits",
                                              "mean_distance")},
        "delta_mrr": d_mrr,
        "delta_ndcg5": d_ndcg,
        "delta_mean": mean(deltas),
        "delta_sd": spread(deltas),
        "delta_nonzero": sum(1 for value in deltas if value != 0.0),
        "delta_pairs": len(deltas),
        "delta_passed": after["passed_n"] - before["passed_n"],
        "delta_top1": after["top1"] - before["top1"],
        "delta_zero_hits": after["zero_hits"] - before["zero_hits"],
        "verdict": verdict,
        "mrr_disagrees": (d_ndcg > 0 > d_mrr) or (d_ndcg < 0 < d_mrr),
        "moved": [
            {"class": a["class"], "query": a["query"], "before": a["rank"], "after": b["rank"]}
            for a, b in zip(before["tasks"], after["tasks"])
            if a["scored"] and (a["rank"] != b["rank"] or a["found"] != b["found"])
        ],
        "graded_only": [
            {"class": a["class"], "query": a["query"], "rank": a["rank"],
             "before": a["ndcg5"], "after": b["ndcg5"]}
            for a, b in zip(before["tasks"], after["tasks"])
            if a["scored"] and a["rank"] == b["rank"] and a["ndcg5"] != b["ndcg5"]
        ],
        "classes": {
            name: {
                "n": entry["n"],
                "before_mrr": entry["mrr"],
                "after_mrr": after["classes"][name]["mrr"],
                "before_ndcg5": entry["ndcg5"],
                "after_ndcg5": after["classes"][name]["ndcg5"],
                "before_top1": entry["top1"],
                "after_top1": after["classes"][name]["top1"],
            }
            for name, entry in before["classes"].items()
        },
    }


def report_compare(result: dict, before_path: Path, after_path: Path) -> None:
    b, a = result["before"], result["after"]
    print(f"two runs over {result['n']} tasks; {result['scored_n']} carry a grade")
    print(f"  before  {before_path}")
    print(f"  after   {after_path}")
    print(f"  identical scores  {'yes' if result['scores_identical'] else 'no'}"
          f"   identical pages  {'yes' if result['pages_identical'] else 'no'}")
    print()
    print(f"  {'metric':<22}{'before':>8}{'after':>8}{'delta':>9}")
    for label, key in (("nDCG@5", "ndcg5"), ("MRR", "mrr"), ("tasks_eval passes", "passed_n"),
                       ("first answer rank 1", "top1"), ("zero expected hits", "zero_hits"),
                       ("mean rank distance", "mean_distance")):
        left, right = b[key], a[key]
        if key in ("passed_n", "top1", "zero_hits"):
            print(f"  {label:<22}{left:>8}{right:>8}{right - left:>+9}")
        else:
            shown_delta = "-" if left is None or right is None else f"{right - left:+.3f}"
            print(f"  {label:<22}{_fmt(left).strip():>8}{_fmt(right).strip():>8}{shown_delta:>9}")
    relative_ndcg = (result["delta_ndcg5"] / b["ndcg5"]) if b["ndcg5"] else None
    relative_mrr = (result["delta_mrr"] / b["mrr"]) if b["mrr"] else None
    print()
    print(f"  verdict on the graded view  {result['verdict'].upper()}")
    moved_ndcg = f"{relative_ndcg:+.1%} of the before value" if relative_ndcg is not None else "no base"
    moved_mrr = f"{relative_mrr:+.1%}" if relative_mrr is not None else "-"
    print(f"  nDCG@5 moved {result['delta_ndcg5']:+.3f} ({moved_ndcg}); MRR moved "
          f"{result['delta_mrr']:+.3f} ({moved_mrr})")
    sd = result["delta_sd"]
    if sd is not None:
        print(f"  per-task nDCG@5 deltas: {result['delta_nonzero']}/{result['delta_pairs']} nonzero, "
              f"mean {result['delta_mean']:+.3f}, sd {sd:.3f}")
    if result["mrr_disagrees"]:
        print("  the two metrics disagree about the direction; the graded number is the one this tool "
              "defines, and MRR is printed beside it")
    if result["moved"]:
        print(f"  {len(result['moved'])} task(s) moved their answer inside the page:")
        for row in result["moved"]:
            was = f"rank {row['before']}" if row["before"] else "absent"
            now = f"rank {row['after']}" if row["after"] else "absent"
            print(f"    {row['class']:<16} {was:>9} -> {now:<9} {row['query'][:50]}")
    else:
        print("  no task moved its answer inside the page")
    if result["graded_only"]:
        print(f"  {len(result['graded_only'])} task(s) kept the answer's rank and moved the graded score "
              f"(MRR cannot see these):")
        for row in result["graded_only"]:
            where = f"rank {row['rank']}" if row["rank"] else "answer absent"
            print(f"    {row['class']:<16} {where:>13}  nDCG {row['before']:.3f} -> {row['after']:.3f}  "
                  f"{row['query'][:44]}")
    print()
    print(f"  {'class':<16}{'n':>3}{'nDCG b/a':>17}{'MRR b/a':>17}{'top1 b/a':>10}")
    for name, entry in result["classes"].items():
        print(f"  {name:<16}{entry['n']:>3}{entry['before_ndcg5']:>8.3f}/{entry['after_ndcg5']:<8.3f}"
              f"{entry['before_mrr']:>8.3f}/{entry['after_mrr']:<8.3f}"
              f"{entry['before_top1']:>4}/{entry['after_top1']:<5}")
    print()
    print("  what the graded view adds: every row that keeps its verdict but moves its support, or moves an\n"
          "  answer inside the cutoff, enters the aggregate. What it does not add: `absent` has no path to\n"
          "  rank, so its 0/3 is invisible here; a page that improves `min_total` without moving this note\n"
          "  scores the same; and a note that answers the question but is not in the label set cannot raise\n"
          "  the score.")


def mde_record(before: dict, after: dict, target_n: int = 160) -> dict:
    """σ and the minimum detectable effect, from one pair of runs.

    Two σ estimates, because they answer different questions: the spread of the per-task deltas (how much
    this pair moves, which includes the effect being measured and is exactly zero for a self-pair), and the
    spread of nDCG@5 across tasks in one run (how much a question set varies, which is the only estimate
    that does not come from the effect). The headline MDE uses the second.
    """
    pairs = [(a, b) for a, b in zip(before["tasks"], after["tasks"]) if a["scored"] and b["scored"]]
    deltas = [b["ndcg5"] - a["ndcg5"] for a, b in pairs]
    levels = [row["ndcg5"] for row in after["tasks"] if row["scored"]]
    n = len(deltas)
    return {
        "n": n,
        "target_n": target_n,
        "mean_delta": mean(deltas),
        "sd_delta": spread(deltas),
        "nonzero": sum(1 for value in deltas if value != 0.0),
        "noise_floor": all(value == 0.0 for value in deltas),
        "mean_level": mean(levels),
        "sd_level": spread(levels),
        "mde": {str(size): (Z_POWER * spread(levels) / math.sqrt(size)) if spread(levels) is not None
                else None for size in (n, 18, target_n)},
        "mde_from_deltas": {str(size): (Z_POWER * spread(deltas) / math.sqrt(size))
                            if spread(deltas) is not None else None
                            for size in (n, 18, target_n)},
    }


def report_mde(result: dict, before_path: Path, after_path: Path) -> None:
    print(f"effect size over {result['n']} paired tasks")
    print(f"  before  {before_path}")
    print(f"  after   {after_path}")
    print()
    print(f"  mean nDCG@5 delta  {result['mean_delta']:+.3f}"
          f"   tasks that moved  {result['nonzero']}/{result['n']}")
    print(f"  sd of the per-task deltas  {_fmt(result['sd_delta'], digits=4).strip()}")
    if result["noise_floor"]:
        print("  every delta is exactly zero: this is a self-pair, so the number above is the NOISE FLOOR")
        print("  (the engine is deterministic), not the cross-build σ. Plugging 0 into the power formula")
        print("  would make every effect decidable, which is why it is not the headline.")
    print(f"  sd of nDCG@5 across tasks (the after run)  {_fmt(result['sd_level'], digits=4).strip()}")
    print()
    print("  MDE: δ = 2.486·σ/√n   (80 % power, α = 0.05 one-sided)")
    print(f"  {'n':>5}{'from the cross-task σ':>24}{'from the delta σ':>20}")
    for size in (result["n"], 18, result["target_n"]):
        head = result["mde"].get(str(size))
        tail = result["mde_from_deltas"].get(str(size)) if result["sd_delta"] else None
        print(f"  {size:>5}{_fmt(head, width=24, digits=4)}{_fmt(tail, width=20, digits=4)}")
    print()
    print("  the cross-task σ is the headline: it treats the questions as a sample, so it is the estimate")
    print("  a claim about a *future* set needs. The delta σ is the observed pair's own spread; it contains")
    print("  the effect being tested, so it is too small to judge that effect — and it is '-', not 0.000,")
    print("  when the pair is a self-pair.")


def make_case(ranks: list[int | None], limit: int = PAGE) -> tuple[list[dict], list[dict]]:
    """A run whose single grade-2 path sits at each rank, or nowhere when the rank is None."""
    tasks, run = [], []
    for index, rank in enumerate(ranks):
        tasks.append({"query": f"q{index}", "class": "c", "evidence": ["wanted.md"], "expect": "hit"})
        page = [f"other{position}.md" for position in range(1, limit + 1)]
        if rank:
            page[rank - 1] = "wanted.md"
        run.append({"query": f"q{index}", "class": "c", "page": page, "passed": rank == 1})
    return tasks, run


def page_with(*slots: tuple[int, str], limit: int = PAGE) -> list[str]:
    page = [f"other{position}.md" for position in range(1, limit + 1)]
    for position, path in slots:
        page[position - 1] = path
    return page


def graded_case(answer_rank: int | None, support_rank: int | None = None) -> tuple[dict, dict]:
    task = {"query": "q", "class": "c", "evidence": ["answer.md"], "expect": "hit"}
    if support_rank is not None:
        task["support"] = ["support.md"]
    slots = []
    if answer_rank:
        slots.append((answer_rank, "answer.md"))
    if support_rank:
        slots.append((support_rank, "support.md"))
    run = {"query": "q", "class": "c", "page": page_with(*slots), "passed": answer_rank == 1}
    return task, run


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

    def one(answer_rank, support_rank=None):
        task, row = graded_case(answer_rank, support_rank)
        return summarise([task], [row])

    support_second = one(1, 2)
    support_first = one(2, 1)
    support_third = one(2, 3)
    support_only = one(None, 1)
    answer_third = one(3, None)
    answer_sixth = one(6, None)
    both = [{"query": "q", "class": "c", "evidence": ["answer.md"], "support": ["answer.md"]}]
    checks = [
        ("rank 1 on every task is a perfect score",
         perfect,
         lambda r: abs(r["mrr"] - 1.0) < 1e-12 and abs(r["ndcg5"] - 1.0) < 1e-12
         and r["top1"] == 3 and r["zero_hits"] == 0 and r["mean_distance"] == 0.0),
        ("a grade-2 note at rank 10 scores 0.100 MRR and 0 nDCG@5, because the cutoff is five",
         last,
         lambda r: abs(r["mrr"] - 0.1) < 1e-12 and r["ndcg5"] == 0.0
         and r["mean_distance"] == 9.0 and r["top1"] == 0),
        ("a set of ranks aggregates by the mean",
         mixed,
         lambda r: abs(r["mrr"] - (2 / 3)) < 1e-12 and abs(r["ndcg5"] - 0.75) < 1e-12
         and r["top1"] == 1 and abs(r["mean_distance"] - 1.0) < 1e-12),
        ("a class with no grade is excluded, not scored as zero",
         excluded,
         lambda r: r["scored_n"] == 0 and r["excluded"] == {"absent": 1} and r["ndcg5"] == 0.0),
        ("the answer first, support second, is a perfect graded score",
         support_second,
         lambda r: abs(r["ndcg5"] - 1.0) < 1e-12 and abs(r["mrr"] - 1.0) < 1e-12),
        ("support above the answer lowers nDCG@5 below 1 and leaves the answer at rank 2",
         support_first,
         lambda r: abs(r["ndcg5"] - (1 + 2 / math.log2(3)) / (2 + 1 / math.log2(3))) < 1e-12
         and abs(r["mrr"] - 0.5) < 1e-12),
        ("MRR cannot tell two runs apart when the answer keeps its rank, but nDCG@5 can",
         {"before": support_first, "after": support_third},
         lambda r: abs(r["before"]["mrr"] - r["after"]["mrr"]) < 1e-12
         and r["before"]["ndcg5"] > r["after"]["ndcg5"] + 1e-12),
        ("MRR and nDCG@5 can disagree in DIRECTION: the answer first beats the answer second on MRR, "
         "and loses on nDCG@5 because the second run also holds the support note",
         {"answer_first": one(1, 9), "support_first": support_first},
         lambda r: r["answer_first"]["mrr"] > r["support_first"]["mrr"]
         and r["answer_first"]["ndcg5"] < r["support_first"]["ndcg5"]),
        ("finding only the support note scores below finding only the answer",
         {"support": support_only, "answer": answer_third},
         lambda r: r["support"]["ndcg5"] < r["answer"]["ndcg5"] and r["support"]["mrr"] == 0.0
         and r["support"]["ndcg5"] > 0.0),
        ("an answer at rank 6 is invisible to nDCG@5 while MRR still counts it",
         answer_sixth,
         lambda r: r["ndcg5"] == 0.0 and abs(r["mrr"] - 1 / 6) < 1e-12),
        ("a path labelled both an answer and support is refused",
         _refused(lambda: summarise(both, [{"query": "q", "class": "c", "page": ["answer.md"]}])),
         lambda r: r is True),
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
         and r["reversed"]["delta_ndcg5"] < r["reversed"]["delta_mrr"]),
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
    parser.add_argument("--limit", type=int, default=PAGE)
    parser.add_argument("--target-n", type=int, default=160,
                        help="the second size the MDE is printed at")
    parser.add_argument("--mde", action="store_true",
                        help="the noise floor, the cross-task σ, and the minimum detectable effect")
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
        if args.mde:
            parser.error("--mde needs two runs: the pair is where σ comes from")
        if args.json:
            print(json.dumps(first, indent=2, ensure_ascii=False))
        else:
            report(first, str(args.before))
        return 0
    second = summarise(tasks, load_run(args.after), args.limit)
    if args.mde:
        record = mde_record(first, second, args.target_n)
        if args.json:
            print(json.dumps(record, indent=2, ensure_ascii=False))
        else:
            report_mde(record, args.before, args.after)
        return 0
    result = compare(first, second)
    if args.json:
        print(json.dumps(result, indent=2, ensure_ascii=False))
    else:
        report_compare(result, args.before, args.after)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
