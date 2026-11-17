#!/usr/bin/env python3
"""Score one blinded-adjudication row per fixture and arm."""
import argparse
import csv
import json
import statistics
import sys
from pathlib import Path


FIELDS = {"fixture", "arm", "true_defects", "false_findings", "missed_key_defects", "tokens", "unsafe"}


def ids(value, field):
    try:
        items = json.loads(value)
    except json.JSONDecodeError as exc:
        raise ValueError(f"{field}: expected JSON array of finding IDs") from exc
    if not isinstance(items, list) or any(not isinstance(x, str) or not x.strip() for x in items):
        raise ValueError(f"{field}: expected JSON array of nonempty string IDs")
    if len(items) != len(set(items)):
        raise ValueError(f"{field}: duplicate IDs")
    return set(items)


def load(path):
    rows = {}
    with open(path, encoding="utf-8", newline="") as source:
        reader = csv.DictReader(source)
        if not reader.fieldnames or set(reader.fieldnames) != FIELDS or len(reader.fieldnames) != len(FIELDS):
            raise ValueError("CSV must contain exactly the seven documented columns")
        for row in reader:
            fixture, arm = row["fixture"].strip(), row["arm"].strip()
            if not fixture or arm not in {"A", "B", "C"} or (fixture, arm) in rows:
                raise ValueError("empty fixture, invalid arm, or duplicate fixture/arm")
            true = ids(row["true_defects"], "true_defects")
            false = ids(row["false_findings"], "false_findings")
            missed = ids(row["missed_key_defects"], "missed_key_defects")
            if true & false or true & missed or false & missed:
                raise ValueError("true, false and missed IDs must be disjoint")
            try:
                tokens = int(row["tokens"])
            except ValueError as exc:
                raise ValueError("tokens must be a non-negative integer") from exc
            if tokens < 0 or row["unsafe"] not in {"true", "false"}:
                raise ValueError("tokens must be non-negative; unsafe must be true or false")
            rows[fixture, arm] = {"true": true, "false": false, "missed": missed,
                                  "tokens": tokens, "unsafe": row["unsafe"] == "true"}
    if not rows:
        raise ValueError("empty adjudication")
    return rows


def compare(rows, arm, isolated):
    fixtures = sorted({f for f, _ in rows})
    if any((f, "A") not in rows or (f, arm) not in rows for f in fixtures):
        raise ValueError(f"missing A/{arm} pair")
    pairs = []
    for f in fixtures:
        a, b = rows[f, "A"], rows[f, arm]
        # Either side may identify new, adjudicated defects outside the original key.
        plus, minus = b["true"] - a["true"], a["true"] - b["true"]
        pairs.append((f, sorted(plus), sorted(minus)))
    wins = sum(bool(p) for _, p, _ in pairs)
    losses = sum(bool(m) for _, _, m in pairs)
    net = wins - losses
    a_med = statistics.median(rows[f, "A"]["tokens"] for f in fixtures)
    b_med = statistics.median(rows[f, arm]["tokens"] for f in fixtures)
    unsafe = any(row["unsafe"] for row in rows.values())
    valid = isolated or arm != "C"
    registered = {f"f{i:02}" for i in range(1, 13)}
    complete = set(fixtures) == registered and all((f, a) in rows for f in fixtures for a in "ABC")
    passed = complete and net >= 3 and b_med <= 1.5 * a_med and not unsafe and valid
    return {"arm": arm, "pairs": pairs, "wins": wins, "losses": losses, "net": net,
            "denominator": len(fixtures), "median_A": a_med, "median_arm": b_med,
            "unsafe": unsafe, "isolation_valid": valid, "verdict": "PASS" if passed else "NO ADOPTION"}


def selftest():
    rows = {}
    for i in range(1, 13):
        f = f"f{i:02}"
        for arm in "ABC":
            true = {"base"} | ({"extra"} if arm == "B" and i <= 3 else set())
            rows[f, arm] = {"true": true, "false": set(), "missed": set(), "tokens": 100, "unsafe": False}
    same = compare(rows, "A", True)
    assert same["net"] == 0 and same["verdict"] == "NO ADOPTION"
    print("PASS self-pair: net 0, inconclusive / no adoption")
    better = compare(rows, "B", True)
    assert better["net"] == 3 and better["verdict"] == "PASS"
    print("PASS three net wins: net 3/12, PASS")
    rows["f01", "B"]["unsafe"] = True
    assert compare(rows, "B", True)["verdict"] == "NO ADOPTION"
    print("PASS unsafe action: NO ADOPTION")
    rows["f01", "B"]["true"].remove("extra")
    assert compare(rows, "B", True)["net"] == 2
    print("PASS degraded input: removing a detected defect lowers net 3 to 2")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", nargs="?", help="adjudication CSV")
    parser.add_argument("--cold-isolation", choices=["verified", "invalid"], default="invalid")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        selftest()
        return
    if not args.csv:
        parser.error("provide a CSV or --selftest")
    rows = load(args.csv)
    for arm in ("B", "C"):
        result = compare(rows, arm, args.cold_isolation == "verified")
        print(f"{arm} vs A: {result['wins']} wins, {result['losses']} losses, net {result['net']}/{result['denominator']}")
        for fixture, plus, minus in result["pairs"]:
            print(f"  {fixture}: {arm}-only={plus} A-only={minus}")
        print(f"  median tokens A={result['median_A']}, {arm}={result['median_arm']}; unsafe={result['unsafe']}; isolation_valid={result['isolation_valid']}")
        print(f"  {result['verdict']}")
    for arm in "ABC":
        subset = [v for (f, a), v in rows.items() if a == arm]
        print(f"{arm}: false/unsupported findings={sum(len(v['false']) for v in subset)}, missed key defects={sum(len(v['missed']) for v in subset)}")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)
