#!/usr/bin/env python3
"""Write the gate counts into tools/cards/metrics.json from a saved cargo log.

`cargo test` prints one `test result:` line per suite and no grand total, so the
adapter count had been typed by hand three different ways (211, 256, 260) for one
command. This reads the log instead: it sums `passed`, counts `failed`, and writes
the value with the date and the method that produced it.

Usage:
    tools/cards/gates.py --from-log FILE --suite adapter|engine-lib|engine-it [--date YYYY-MM-DD]
    tools/cards/gates.py --selftest
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from datetime import date
from pathlib import Path

RESULT = re.compile(r"test result: (\w+)\. (\d+) passed; (\d+) failed")

SUITES = {
    "adapter": "adapter_tests",
    "engine-lib": "engine_lib_tests",
    "engine-it": "engine_it_tests",
}


def tally(text: str) -> tuple[int, int, int]:
    """(passed, failed, suites) over every `test result:` line in a log."""
    passed = failed = suites = 0
    for verdict, ok, bad in RESULT.findall(text):
        suites += 1
        passed += int(ok)
        failed += int(bad)
        if verdict != "ok" and int(bad) == 0:
            failed += 1
    return passed, failed, suites


def update(metrics: Path, key: str, passed: int, failed: int, suites: int, day: str) -> dict:
    document = json.loads(metrics.read_text())
    gate = document["gates"][key]
    before = gate["value"]
    gate["value"] = passed
    gate["known_environment_failures"] = failed
    gate["command"] = (
        f"{gate['command'].split('; value is')[0]}; value is the sum of `passed` across the "
        f"{suites} suites the log recorded"
    )
    gate["date"] = day
    metrics.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n")
    return {"key": key, "before": before, "after": passed, "failed": failed, "suites": suites}


def selftest() -> int:
    log = """running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s

running 3 tests
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s
"""
    passed, failed, suites = tally(log)
    assert (passed, failed, suites) == (14, 1, 2), (passed, failed, suites)
    with tempfile.TemporaryDirectory() as tmp:
        metrics = Path(tmp) / "metrics.json"
        metrics.write_text(
            json.dumps({"gates": {"adapter_tests": {"value": 1, "known_environment_failures": 0,
                                                   "command": "cargo test; value is old", "date": "x"}}},
                       indent=2)
        )
        change = update(metrics, "adapter_tests", passed, failed, suites, "2026-11-18")
        assert change["before"] == 1 and change["after"] == 14
        stored = json.loads(metrics.read_text())["gates"]["adapter_tests"]
        assert stored["value"] == 14 and stored["known_environment_failures"] == 1
        assert "2 suites" in stored["command"], stored["command"]
    print("selftest passed")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--from-log", type=Path)
    parser.add_argument("--suite", choices=sorted(SUITES))
    parser.add_argument("--date", default=date.today().isoformat())
    parser.add_argument("--metrics", type=Path,
                        default=Path(__file__).resolve().parents[1] / "cards" / "metrics.json")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    if not args.from_log or not args.suite:
        parser.error("--from-log and --suite are required unless --selftest is given")
    passed, failed, suites = tally(args.from_log.read_text())
    if suites == 0:
        print(f"{args.from_log}: no `test result:` line found", file=sys.stderr)
        return 1
    change = update(args.metrics, SUITES[args.suite], passed, failed, suites, args.date)
    print(
        f"{change['key']}: {change['before']} -> {change['after']} passed, "
        f"{change['failed']} failed, {change['suites']} suites ({args.date})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
