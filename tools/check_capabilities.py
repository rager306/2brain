#!/usr/bin/env python3
"""Truthful capability registry validator (Issue #7 U1/R23/AE23, clean-room MIT).

Validates `tools/capabilities.json`:
1. Schema integrity and monotonicity (`enabled => rehearsed => tested => wired => implemented => specified`).
2. Every referenced implementation/doc path actually exists in the repository.
3. Every claimed Rust/Python test name actually exists in the repository test tree.
4. No unit may claim `implemented: true` without concrete `paths`, or `tested: true` without concrete `tests`.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

STAGES = ("specified", "implemented", "wired", "tested", "rehearsed", "enabled")


def collect_test_symbols(repo_root: Path) -> set[str]:
    symbols: set[str] = set()
    rust_fn_re = re.compile(r"\bfn\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(")
    py_fn_re = re.compile(r"\bdef\s+(test_[a-zA-Z0-9_]*)\s*\(")
    case_id_re = re.compile(r'"((?:SEM-\d+|AE\d+(?:-\d+)?|ADR-\d+))"')

    for rs_path in (repo_root / "crates").rglob("*.rs"):
        text = rs_path.read_text(encoding="utf-8", errors="ignore")
        for m in rust_fn_re.finditer(text):
            symbols.add(m.group(1))

    tools_dir = repo_root / "tools"
    if tools_dir.exists():
        for py_path in tools_dir.rglob("*.py"):
            text = py_path.read_text(encoding="utf-8", errors="ignore")
            for m in py_fn_re.finditer(text):
                symbols.add(m.group(1))
            for m in case_id_re.finditer(text):
                symbols.add(m.group(1))

    return symbols


def validate_capabilities(repo_root: Path, cap_file: Path) -> list[str]:
    errors: list[str] = []
    try:
        data = json.loads(cap_file.read_text(encoding="utf-8"))
    except Exception as exc:
        return [f"failed to parse {cap_file}: {exc}"]

    if data.get("schema_version") != "2brain.capabilities/v1":
        errors.append(
            f"unexpected schema_version: {data.get('schema_version')!r} (expected '2brain.capabilities/v1')"
        )

    units = data.get("units")
    if not isinstance(units, list) or not units:
        return errors + ["'units' must be a non-empty list"]

    known_symbols = collect_test_symbols(repo_root)
    seen_ids: set[str] = set()

    for idx, unit in enumerate(units):
        uid = unit.get("id", f"<index {idx}>")
        if not isinstance(uid, str) or not uid:
            errors.append(f"unit[{idx}] missing non-empty string 'id'")
            continue
        if uid in seen_ids:
            errors.append(f"duplicate unit id '{uid}'")
        seen_ids.add(uid)

        states = unit.get("states")
        if not isinstance(states, dict):
            errors.append(f"{uid}: missing 'states' object")
            continue

        for st in STAGES:
            if not isinstance(states.get(st), bool):
                errors.append(f"{uid}: state '{st}' must be boolean")

        # Monotonicity: a later stage cannot be true while an earlier stage is false.
        for i in range(len(STAGES) - 1):
            earlier, later = STAGES[i], STAGES[i + 1]
            if states.get(later) and not states.get(earlier):
                errors.append(
                    f"{uid}: monotonic violation — '{later}=true' requires '{earlier}=true'"
                )

        paths = unit.get("paths", [])
        if not isinstance(paths, list):
            errors.append(f"{uid}: 'paths' must be a list")
            paths = []
        if states.get("implemented") and not paths:
            errors.append(f"{uid}: 'implemented=true' requires at least one existing path")
        for rel in paths:
            full = repo_root / rel
            if not full.exists():
                errors.append(f"{uid}: missing referenced path '{rel}'")

        tests = unit.get("tests", [])
        if not isinstance(tests, list):
            errors.append(f"{uid}: 'tests' must be a list")
            tests = []
        if states.get("tested") and not tests:
            errors.append(f"{uid}: 'tested=true' requires at least one verified test reference")
        for sym in tests:
            if sym not in known_symbols:
                errors.append(f"{uid}: claimed test symbol '{sym}' not found in repository")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate 2brain capabilities.json evidence")
    parser.add_argument(
        "--file",
        default="tools/capabilities.json",
        help="Path to capabilities JSON relative to repo root",
    )
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    cap_file = repo_root / args.file
    errors = validate_capabilities(repo_root, cap_file)
    if errors:
        print(f"capabilities check FAILED ({len(errors)} error(s)):", file=sys.stderr)
        for err in errors:
            print(f"  - {err}", file=sys.stderr)
        return 1

    data = json.loads(cap_file.read_text(encoding="utf-8"))
    units = data["units"]
    impl_count = sum(1 for u in units if u["states"]["implemented"])
    tested_count = sum(1 for u in units if u["states"]["tested"])
    enabled_count = sum(1 for u in units if u["states"]["enabled"])
    print(
        f"capabilities ok: {len(units)} units validated "
        f"(implemented={impl_count}, tested={tested_count}, enabled={enabled_count})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
