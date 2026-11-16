#!/usr/bin/env python3
"""Paired bootstrap over two `tasks_eval.py --json` runs: is a pass-count difference bigger than noise?

    python3 tools/ranking/bootstrap.py before.json after.json
    python3 tools/ranking/bootstrap.py --json before.json after.json
    python3 tools/ranking/bootstrap.py --selftest

Two runs of `tools/ranking/tasks_eval.py --json` over the same queries and the same vault give one
pass/fail per task. The difference between them is a vector of +1, 0 and -1 over tasks, and this
tool resamples *tasks* to ask how much of that difference a different draw of the same size could
have produced. The verdict is one of three words and never a fourth:

    BETTER        the 95% interval's lower bound is above zero;
    WORSE         the upper bound is below zero;
    INCONCLUSIVE  the interval contains zero.

The mechanism is the one in `docs/research/ruvector-crates-2026-11-16.md` entry 2, whose shape comes
from `crates/ruvector-sota-bench/harness/src/statistics.ts:20-47`: paired deltas, 2 000 resamples, a
percentile interval, and an RNG seeded by hashing the data so the same inputs always give the same
verdict. Three things are printed beside the interval rather than left implicit:

- `resolution`: the smallest net gain this n could certify. Per-task values are 0 or 1, so the
  resampled mean can only be a multiple of 1/n; at n=18 that coarseness is the whole answer to
  "what can this tool conclude", and the number is computed, not assumed;
- `mcnemar`: the exact paired test for binary outcomes. The bootstrap is an approximation and
  McNemar is not, so a disagreement between them is visible instead of hidden;
- `changed`: the tasks whose pass/fail flipped, named. A pass count is a thresholded metric; rows
  that only moved in rank or candidate count are counted separately.

No dependency beyond the standard library. Exit code is 0 whenever a verdict was computed, 2 on
input that cannot be paired; a `verdict` field in `--json` is what a gate should read.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import sys
from pathlib import Path

ALPHA = 0.05            # two-sided; each tail is 2.5%
SAMPLES = 2_000
MIN_EFFECT = 0.0        # a rate; the reference used 0.005, which is below one task at any n we use


def refuse(message: str):
    """Input that cannot be paired is not a verdict; exit code 2 says so."""
    print(message, file=sys.stderr)
    raise SystemExit(2)


def load(path: Path) -> list[dict]:
    data = json.loads(path.read_text())
    if not isinstance(data, list):
        refuse(f"{path}: expected the JSON array `tasks_eval.py --json` prints")
    for index, row in enumerate(data):
        if not all(key in row for key in ("query", "class", "passed")):
            refuse(f"{path}: row {index} lacks query/class/passed")
    return data


def pair_rows(before: list[dict], after: list[dict]) -> list[dict]:
    """Pair by position, after checking that the two runs describe the same task set in the same order.

    Position is the right key here and `(query, class)` is not: the mixed-language class has two rows
    with the same query and different `args`, and the JSON does not carry `args`. So the sequence is
    verified (a reordered file is refused, not silently mispaired) and the pairing is positional.
    """
    if len(before) != len(after):
        refuse(f"the runs do not pair: {len(before)} tasks against {len(after)}")
    keys_before = [(row["query"], row["class"]) for row in before]
    keys_after = [(row["query"], row["class"]) for row in after]
    if keys_before != keys_after:
        first = next(i for i, (a, b) in enumerate(zip(keys_before, keys_after)) if a != b)
        refuse(
            f"the runs do not pair: row {first} is {keys_before[first]!r} in the first file and "
            f"{keys_after[first]!r} in the second. Same task set, same order, or the difference "
            "means nothing.")
    return [{"query": b["query"], "class": b["class"], "before": bool(b["passed"]),
             "after": bool(a["passed"]), "before_row": b, "after_row": a}
            for b, a in zip(before, after)]


def seed_for(deltas: list[int], samples: int) -> int:
    payload = json.dumps({"deltas": deltas, "samples": samples}, sort_keys=True, separators=(",", ":"))
    return int(hashlib.sha256(payload.encode()).hexdigest()[:8], 16)


class Lcg:
    """The generator the reference uses: a 32-bit LCG seeded from a hash, not a library RNG."""

    def __init__(self, seed: int) -> None:
        self.state = seed & 0xFFFFFFFF

    def next(self) -> float:
        self.state = (1_664_525 * self.state + 1_013_904_223) & 0xFFFFFFFF
        return self.state / 0x1_0000_0000


def quantile(sorted_values: list[float], probability: float) -> float:
    return sorted_values[min(len(sorted_values) - 1, int(probability * len(sorted_values)))]


def decide(deltas: list[int], samples: int = SAMPLES, min_effect: float = MIN_EFFECT) -> dict:
    """The mean paired delta, its percentile interval, and the three-valued verdict."""
    n = len(deltas)
    if n < 2:
        return {"n": n, "mean": 0.0, "lower": -math.inf, "upper": math.inf,
                "verdict": "inconclusive", "samples": 0, "seed": "0x0"}
    seed = seed_for(deltas, samples)
    rng = Lcg(seed)
    means = []
    for _ in range(samples):
        total = 0
        for _ in range(n):
            total += deltas[int(rng.next() * n)]
        means.append(total / n)
    means.sort()
    mean = sum(deltas) / n
    lower = quantile(means, ALPHA / 2)
    upper = quantile(means, 1 - ALPHA / 2)
    verdict = ("better" if lower > min_effect
               else "worse" if upper < -min_effect
               else "inconclusive")
    return {"n": n, "mean": mean, "lower": lower, "upper": upper, "verdict": verdict,
            "samples": samples, "seed": f"0x{seed:08x}"}


def mcnemar_exact(gained: int, lost: int) -> float:
    """Two-sided exact McNemar p-value over the discordant pairs."""
    discordant = gained + lost
    if discordant == 0:
        return 1.0
    tail = sum(math.comb(discordant, i) for i in range(min(gained, lost) + 1)) / 2 ** discordant
    return min(1.0, 2 * tail)


def resolution(n: int, samples: int = SAMPLES, min_effect: float = MIN_EFFECT) -> int | None:
    """The smallest number of net wins, all in the better direction, whose interval clears zero.

    All-wins is the most favourable shape a difference of that size can take, so this is the best
    case, not a typical one. The starting point comes from P(a resample contains no win) = (1-k/n)^n,
    which must fall below the lower tail; the simulation confirms it for the interval rule in use.
    """
    if n < 2:
        return None
    start = max(1, math.ceil(n * (1 - (ALPHA / 2) ** (1 / n))) - 1)
    for k in range(start, n + 1):
        if decide([1] * k + [0] * (n - k), samples, min_effect)["verdict"] == "better":
            return k
    return None


def summarise(rows: list[dict], samples: int, min_effect: float) -> dict:
    deltas = [int(row["after"]) - int(row["before"]) for row in rows]
    n = len(rows)
    decision = decide(deltas, samples, min_effect)
    gained = [row for row in rows if row["after"] and not row["before"]]
    lost = [row for row in rows if row["before"] and not row["after"]]
    moved = [
        row for row in rows
        if row["before"] == row["after"] and (
            row["before_row"].get("rank") != row["after_row"].get("rank")
            or row["before_row"].get("total") != row["after_row"].get("total")
            or row["before_row"].get("top") != row["after_row"].get("top"))
    ]
    identical = all(
        (row["before_row"].get("passed"), row["before_row"].get("rank"),
         row["before_row"].get("total"), row["before_row"].get("top"))
        == (row["after_row"].get("passed"), row["after_row"].get("rank"),
            row["after_row"].get("total"), row["after_row"].get("top"))
        for row in rows)
    smallest = resolution(n, samples, min_effect)
    mcnemar_p = mcnemar_exact(len(gained), len(lost))
    return {
        "n": n,
        "before_passed": sum(int(row["before"]) for row in rows),
        "after_passed": sum(int(row["after"]) for row in rows),
        "delta_tasks": sum(deltas),
        "delta_rate": decision["mean"],
        "changed": len(gained) + len(lost),
        "gained": [f"{row['class']}: {row['query']}" for row in gained],
        "lost": [f"{row['class']}: {row['query']}" for row in lost],
        "moved_without_flip": len(moved),
        "identical_pages": identical,
        "samples": decision["samples"],
        "seed": decision["seed"],
        "lower": decision["lower"],
        "upper": decision["upper"],
        "lower_tasks": _tasks(decision["lower"], n),
        "upper_tasks": _tasks(decision["upper"], n),
        "verdict": decision["verdict"],
        "mcnemar_p": mcnemar_p,
        "mcnemar_agrees": (decision["verdict"] == "inconclusive"
                           or mcnemar_p < ALPHA),
        "resolution_tasks": smallest,
        "resolution_rate": (smallest / n) if smallest else None,
        "duplicate_keys": len(rows) - len({(row["query"], row["class"]) for row in rows}),
    }


def _tasks(rate: float, n: int) -> int | None:
    if rate in (math.inf, -math.inf):
        return None
    return round(rate * n)


def report(result: dict, before: Path, after: Path, min_effect: float) -> None:
    n = result["n"]
    print(f"paired bootstrap over {n} tasks")
    print(f"  before  {result['before_passed']}/{n} passed   {before}")
    print(f"  after   {result['after_passed']}/{n} passed   {after}")
    print(f"  delta   {result['delta_tasks']:+d} task ({result['delta_rate']:+.3f} of the set)")
    print()
    print(f"  {result['changed']} of {n} tasks changed verdict: "
          f"{len(result['gained'])} fail -> pass, {len(result['lost'])} pass -> fail")
    for row in result["gained"]:
        print(f"    + {row}")
    for row in result["lost"]:
        print(f"    - {row}")
    if result["moved_without_flip"]:
        print(f"  ({result['moved_without_flip']} more rows kept their verdict but moved in rank or "
              f"candidate count; a pass count cannot see them)")
    if result["duplicate_keys"]:
        print(f"  ({result['duplicate_keys']} row(s) share a query and class with another row; the pairing "
              f"is positional and those rows are not independent)")
    print()
    print(f"  bootstrap  {result['samples']} resamples of the {n} tasks, seed {result['seed']}, "
          f"minimum effect {min_effect:.3f}")
    print(f"  95% interval  rate {result['lower']:+.3f} to {result['upper']:+.3f}   "
          f"tasks {result['lower_tasks']:+d} to {result['upper_tasks']:+d}")
    print()
    if result["verdict"] == "inconclusive":
        reading = "the interval contains zero"
    else:
        reading = "the interval excludes zero"
    print(f"  verdict  {result['verdict'].upper()} at the 95% interval "
          f"[{result['lower']:+.3f}, {result['upper']:+.3f}] — {reading}")
    if result["identical_pages"]:
        print("           the two runs returned the same page on every task; an interval over a zero "
              "delta vector is [0, 0], which contains zero")
    print()
    print(f"  exact McNemar (two-sided)  p = {result['mcnemar_p']:.3f}")
    if not result["mcnemar_agrees"]:
        print("           the exact paired test does not reach this level: the percentile interval is an "
              "interval on the mean delta and is the more liberal of the two. A gate should require both.")
    if result["resolution_tasks"]:
        print(f"  resolution  this n can certify a net gain of {result['resolution_tasks']} tasks "
              f"({result['resolution_rate']:.1%}) at this level, and nothing smaller")
    print()
    print("  what the interval covers: these tasks, not questions in general. Rows in one class are "
          "correlated and a class with two rows counts twice; a pass/fail metric ignores how far a "
          "rank moved; and a bootstrap run after the change cannot supply the hypothesis it tests.")


SELFTEST_ROWS = [{"query": f"q{i}", "class": f"c{i % 4}", "passed": False, "rank": None,
                  "total": 10, "top": []} for i in range(18)]


def selftest() -> int:
    """The arithmetic, not the instrument: four shapes with a known right answer."""
    def rows_with(passed_indexes: set[int]) -> list[dict]:
        return [dict(row, passed=(i in passed_indexes)) for i, row in enumerate(SELFTEST_ROWS)]

    base = rows_with({0, 1, 2, 3, 4})
    checks = [
        ("a self-pair is inconclusive",
         summarise(pair_rows(base, base), SAMPLES, MIN_EFFECT),
         lambda r: r["verdict"] == "inconclusive" and r["lower"] == 0.0 and r["upper"] == 0.0),
        ("one gain in 18 is inconclusive, interval touching zero",
         summarise(pair_rows(base, rows_with({0, 1, 2, 3, 4, 5})), SAMPLES, MIN_EFFECT),
         lambda r: r["verdict"] == "inconclusive" and r["delta_tasks"] == 1
         and r["lower"] == 0.0 and r["upper_tasks"] >= 1),
        ("four net gains in 18 is better",
         summarise(pair_rows(base, rows_with(set(range(9)))), SAMPLES, MIN_EFFECT),
         lambda r: r["verdict"] == "better" and r["delta_tasks"] == 4 and r["lower"] > 0),
        ("six net losses in 18 is worse",
         summarise(pair_rows(rows_with(set(range(11))), base), SAMPLES, MIN_EFFECT),
         lambda r: r["verdict"] == "worse" and r["delta_tasks"] == -6 and r["upper"] < 0),
        ("a reordered pair is refused",
         _refused(list(reversed(base)), base), lambda r: r is True),
    ]
    failed = 0
    for label, result, check in checks:
        ok = check(result)
        failed += 0 if ok else 1
        print(f"{'ok  ' if ok else 'FAIL'} {label}")
    print(f"\n{len(checks) - failed}/{len(checks)} arithmetic checks pass")
    return 0 if failed == 0 else 1


def _refused(before: list[dict], after: list[dict]) -> bool:
    try:
        pair_rows(before, after)
    except SystemExit:
        return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("before", type=Path, nargs="?")
    parser.add_argument("after", type=Path, nargs="?")
    parser.add_argument("--json", action="store_true", help="print the record, for a doc or a gate")
    parser.add_argument("--samples", type=int, default=SAMPLES)
    parser.add_argument("--min-effect", type=float, default=MIN_EFFECT,
                        help="a rate the lower bound must clear; the smallest nonzero delta at n tasks "
                             "is 1/n, so a value below that changes nothing")
    parser.add_argument("--selftest", action="store_true", help="check the arithmetic on known shapes")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if not args.before or not args.after:
        parser.error("two result files are required (or --selftest)")
    rows = pair_rows(load(args.before), load(args.after))
    if len(rows) < 2:
        refuse(f"{args.before}: a paired bootstrap needs at least two tasks")
    result = summarise(rows, args.samples, args.min_effect)
    if args.json:
        print(json.dumps(result, indent=2, ensure_ascii=False))
    else:
        report(result, args.before, args.after, args.min_effect)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
