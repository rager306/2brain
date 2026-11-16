#!/usr/bin/env python3
"""Check an FSM run log: every transition carries the evidence that allowed it.

A run log is JSON Lines, one object per line. The schema is deliberately small, because a
log nobody writes is worth nothing:

    {"run": "<id>", "state": "ORIENT", "event": "enter", "at": "<iso>"}
    {"run": "<id>", "state": "ORIENT", "event": "guard", "guard": "overview_answered",
     "source": "tool", "call": "tb_overview", "ok": true}
    {"run": "<id>", "state": "ORIENT", "event": "exit", "to": "LOCATE", "at": "<iso>"}

Rules checked here:
  1. states are from the vocabulary in docs/research/meta-prompt-fsm.md;
  2. an `exit` must be preceded, in the same state, by at least one passing `guard`;
  3. a `guard` names where its answer came from: `tool`, `engine` or `judge`; a `judge` guard also
     carries `probability`, `threshold`, `model` and an explicit `comparison` (`gt` for "true means the
     good state", `lt` for an inverted question), so a threshold can be audited later;
  4. `judge` guards are only allowed in states where the FSM document permits judgment;
  5. the transition graph is the one the document declares (no invented edges);
  6. `outcome_unknown` must lead to RECOVER, never back to APPLY.

    python3 tools/fsm/check_run.py run.jsonl          # exit 0 clean, 1 violation, 2 unreadable
    python3 tools/fsm/check_run.py --selftest         # proves each rule can fail
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

STATES = (
    "ORIENT", "LOCATE", "READ", "PROPOSE", "VALIDATE", "APPLY", "RECOVER", "VERIFY", "CONSOLIDATE",
)

# Which states may have judged guards (docs/research/meta-prompt-fsm.md, "Transition guards").
JUDGED_STATES = {"ORIENT", "LOCATE", "PROPOSE", "CONSOLIDATE"}

# The declared transition graph. Anything else is an invented edge.
EDGES = {
    ("ORIENT", "LOCATE"), ("LOCATE", "READ"), ("READ", "PROPOSE"), ("PROPOSE", "VALIDATE"),
    ("VALIDATE", "APPLY"), ("VALIDATE", "PROPOSE"), ("APPLY", "VERIFY"), ("APPLY", "RECOVER"),
    ("RECOVER", "VERIFY"), ("VERIFY", "CONSOLIDATE"), ("CONSOLIDATE", "PROPOSE"),
    ("CONSOLIDATE", "VERIFY"), ("LOCATE", "ORIENT"), ("READ", "LOCATE"), ("PROPOSE", "READ"),
    ("VERIFY", "APPLY"),
}

SOURCES = {"tool", "engine", "judge"}


def parse(lines: list[str]) -> tuple[list[dict], list[str]]:
    events, problems = [], []
    for number, line in enumerate(lines, 1):
        line = line.strip()
        if not line or line.startswith("//"):
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            problems.append(f"line {number}: not JSON ({error})")
            continue
        if not isinstance(event, dict):
            problems.append(f"line {number}: not an object")
            continue
        event["_line"] = number
        events.append(event)
    return events, problems


def check(events: list[dict]) -> list[str]:
    problems: list[str] = []
    per_state_passed: dict[tuple[str, str], bool] = {}
    current: dict[str, str] = {}

    for event in events:
        line = event["_line"]
        run = event.get("run")
        state = event.get("state")
        kind = event.get("event")
        if not run or not state or kind not in {"enter", "guard", "exit", "note"}:
            problems.append(f"line {line}: needs run, state and a known event")
            continue
        if state not in STATES:
            problems.append(f"line {line}: unknown state {state!r}")
            continue

        if kind == "enter":
            current[run] = state
            per_state_passed[(run, state)] = False
        elif kind == "guard":
            source = event.get("source")
            if source not in SOURCES:
                problems.append(f"line {line}: guard needs source in {sorted(SOURCES)}")
                continue
            if source == "judge":
                if state not in JUDGED_STATES:
                    problems.append(f"line {line}: a judged guard is not allowed in {state}")
                for field in ("probability", "threshold", "model", "question"):
                    if field not in event:
                        problems.append(f"line {line}: a judged guard needs {field}")
                probability = event.get("probability")
                threshold = event.get("threshold")
                # Polarity is explicit: an inverted question ("is this stale?") passes when the
                # probability is *below* its threshold. The log must say which way it reads,
                # because a checker that assumes a direction silently blesses a wrong gate.
                comparison = event.get("comparison", "gt")
                if comparison not in ("gt", "lt"):
                    problems.append(f"line {line}: comparison must be 'gt' or 'lt'")
                elif isinstance(probability, (int, float)) and isinstance(threshold, (int, float)):
                    passed = probability >= threshold if comparison == "gt" else probability <= threshold
                    if not passed:
                        problems.append(
                            f"line {line}: guard failed ({probability} {comparison} {threshold}) but "
                            "the log states it as evidence")
                        continue
            if event.get("ok") is True:
                per_state_passed[(run, state)] = True
        elif kind == "exit":
            to = event.get("to")
            if state != current.get(run):
                problems.append(f"line {line}: exit from {state} but the run is in {current.get(run)}")
            if not per_state_passed.get((run, state)):
                problems.append(f"line {line}: exit from {state} without a passing guard")
            if (state, to) not in EDGES:
                problems.append(f"line {line}: undeclared transition {state} -> {to}")
            if event.get("outcome") == "outcome_unknown" and to != "RECOVER":
                problems.append(f"line {line}: outcome_unknown must go to RECOVER, not {to}")
            current[run] = to
            per_state_passed[(run, to)] = False
    return problems


def selftest() -> int:
    def run(*events: dict) -> list[str]:
        return check([dict(event, _line=i + 1) for i, event in enumerate(events)])

    ok = run(
        {"run": "r", "state": "ORIENT", "event": "enter"},
        {"run": "r", "state": "ORIENT", "event": "guard", "guard": "g", "source": "tool", "ok": True},
        {"run": "r", "state": "ORIENT", "event": "exit", "to": "LOCATE"},
    )
    cases = {
        "exit without a guard": run(
            {"run": "r", "state": "ORIENT", "event": "enter"},
            {"run": "r", "state": "ORIENT", "event": "exit", "to": "LOCATE"},
        ),
        "undeclared edge": run(
            {"run": "r", "state": "ORIENT", "event": "enter"},
            {"run": "r", "state": "ORIENT", "event": "guard", "guard": "g", "source": "tool", "ok": True},
            {"run": "r", "state": "ORIENT", "event": "exit", "to": "APPLY"},
        ),
        "judge outside a judged state": run(
            {"run": "r", "state": "APPLY", "event": "enter"},
            {"run": "r", "state": "APPLY", "event": "guard", "guard": "g", "source": "judge",
             "probability": 0.9, "threshold": 0.8, "model": "m", "question": "q", "ok": True},
        ),
        "failed judge guard used as evidence": run(
            {"run": "r", "state": "LOCATE", "event": "enter"},
            {"run": "r", "state": "LOCATE", "event": "guard", "guard": "g", "source": "judge",
             "probability": 0.3, "threshold": 0.8, "model": "m", "question": "q", "ok": True},
            {"run": "r", "state": "LOCATE", "event": "exit", "to": "READ"},
        ),
        "outcome_unknown outside RECOVER": run(
            {"run": "r", "state": "APPLY", "event": "enter"},
            {"run": "r", "state": "APPLY", "event": "guard", "guard": "g", "source": "engine", "ok": True},
            {"run": "r", "state": "APPLY", "event": "exit", "to": "VERIFY", "outcome": "outcome_unknown"},
        ),
        "inverted guard recorded as failed": run(
            {"run": "r", "state": "CONSOLIDATE", "event": "enter"},
            {"run": "r", "state": "CONSOLIDATE", "event": "guard", "guard": "g", "source": "judge",
             "probability": 0.9, "threshold": 0.2, "comparison": "lt", "model": "m", "question": "q",
             "ok": True},
            {"run": "r", "state": "CONSOLIDATE", "event": "exit", "to": "PROPOSE"},
        ),
        "unknown state": run({"run": "r", "state": "THINK", "event": "enter"}),
    }
    # A guard with inverted polarity must be accepted when it is on the passing side.
    inverted_ok = run(
        {"run": "r", "state": "CONSOLIDATE", "event": "enter"},
        {"run": "r", "state": "CONSOLIDATE", "event": "guard", "guard": "g", "source": "judge",
         "probability": 0.17, "threshold": 0.2, "comparison": "lt", "model": "m", "question": "q",
         "ok": True},
        {"run": "r", "state": "CONSOLIDATE", "event": "exit", "to": "PROPOSE"},
    )
    failures = 0
    if ok:
        print("selftest: a clean log was rejected:", ok)
        failures += 1
    if inverted_ok:
        print("selftest: an inverted passing guard was rejected:", inverted_ok)
        failures += 1
    for name, problems in cases.items():
        if not problems:
            print(f"selftest: rule {name!r} did not bite")
            failures += 1
        else:
            print(f"selftest: {name} -> {problems[0]}")
    print("selftest PASS" if failures == 0 else f"selftest FAIL ({failures})")
    return 0 if failures == 0 else 1


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", nargs="?", type=Path)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv[1:])
    if args.selftest:
        return selftest()
    if args.log is None:
        parser.error("give a run log or --selftest")
    try:
        lines = args.log.read_text().splitlines()
    except OSError as error:
        print(f"cannot read {args.log}: {error}", file=sys.stderr)
        return 2
    events, problems = parse(lines)
    problems.extend(check(events))
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1
    print(f"OK: {len(events)} events, {len({e.get('run') for e in events})} run(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
