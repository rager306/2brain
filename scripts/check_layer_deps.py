#!/usr/bin/env python3
"""Verify hexagonal/onion crate dependency boundaries via `cargo metadata`.

Rules:
1. Domain crates (`crates/domain/*`) may ONLY depend on other domain crates
   and pure std/data crates — NEVER on `tb-app`, `crates/adapters/*`, `two-brain`,
   `tokio`, `rmcp`, `gix`, `notify`, `fs4`, or `clap`.
2. Application crate (`tb-app`) may depend ONLY on `crates/domain/*` (never on
   concrete adapters or `two-brain`).
3. Driven/Driving adapters (`crates/adapters/*`) may depend on `crates/domain/*`
   and `tb-app`, never on `two-brain`.
4. Only `two-brain` (composition root) wires adapters into `tb-app`.
"""
from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys

DOMAIN_CRATES = {
    "tb-contracts",
    "tb-syntax",
    "tb-schema",
    "tb-graph",
    "tb-search",
    "tb-cognition",
}
APP_CRATES = {"tb-app"}
ADAPTER_CRATES = {"tb-fs-git", "tb-journal", "tb-skills", "tb-ipc", "tb-mcp"}
BIN_CRATES = {"two-brain"}

FORBIDDEN_IN_DOMAIN = (
    APP_CRATES
    | ADAPTER_CRATES
    | BIN_CRATES
    | {"tokio", "rmcp", "gix", "notify", "fs4", "clap"}
)
FORBIDDEN_IN_APP = ADAPTER_CRATES | BIN_CRATES | {"rmcp", "gix", "notify", "fs4", "clap"}
FORBIDDEN_IN_ADAPTERS = BIN_CRATES


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=root,
        capture_output=True,
        text=True,
        check=True,
    )
    meta = json.loads(proc.stdout)
    violations: list[str] = []
    seen = set()

    for pkg in meta["packages"]:
        name = pkg["name"]
        seen.add(name)
        normal_deps = {
            dep["name"]
            for dep in pkg["dependencies"]
            if dep.get("kind") in (None, "normal")
        }
        if name in DOMAIN_CRATES:
            bad = sorted(normal_deps & FORBIDDEN_IN_DOMAIN)
            if bad:
                violations.append(f"domain crate `{name}` illegally depends on: {bad}")
        elif name in APP_CRATES:
            bad = sorted(normal_deps & FORBIDDEN_IN_APP)
            if bad:
                violations.append(f"app crate `{name}` illegally depends on: {bad}")
        elif name in ADAPTER_CRATES:
            bad = sorted(normal_deps & FORBIDDEN_IN_ADAPTERS)
            if bad:
                violations.append(f"adapter crate `{name}` illegally depends on: {bad}")

    expected = DOMAIN_CRATES | APP_CRATES | ADAPTER_CRATES | BIN_CRATES
    missing = sorted(expected - seen)
    if missing:
        violations.append(f"missing expected workspace crates: {missing}")

    if violations:
        for line in violations:
            print(f"LAYER VIOLATION: {line}", file=sys.stderr)
        return 1
    print(f"layer boundaries ok: {len(expected)} crates verified (domain={len(DOMAIN_CRATES)}, app={len(APP_CRATES)}, adapters={len(ADAPTER_CRATES)}, bin={len(BIN_CRATES)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
