#!/usr/bin/env python3
"""Enumerate every surviving trace of the pre-2brain name, sorted by who reads it.

The rename is not one thing. A string is classified by its consumer, and each
class has a different rule:

  live        read by code, a client, or an operator today -> must be renamed.
  format      on disk or on the wire, owned by an already deployed reader -> the
              compatibility window; renamed only under a migration, then deleted.
  content     repository identity and wikilinks in the knowledge tree -> one
              atomic migration (names, deps, registry, links travel together).
  history     a frozen record (reviews, research studies, run logs, CHANGELOG)
              -> never edited; an unresolvable record is worse than a frozen one.
  lineage     the fork paragraph -> the one place the upstream name belongs.

Usage:
  python3 tools/kb/legacy_audit.py            # table
  python3 tools/kb/legacy_audit.py --json     # machine form
  python3 tools/kb/legacy_audit.py --check    # exit 1 if LIVE is non-empty
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# Extensions whose bytes are text worth scanning.
TEXT = {".rs", ".md", ".toml", ".yaml", ".yml", ".json", ".sh", ".py", ".mjs",
        ".js", ".ts", ".txt", ".nu", ".toml", ".svg", ".yml"}

# Directories that are frozen records or imported history: never "live".
FROZEN_PREFIX = ("docs/reviews/", "docs/research/", "docs/internal/",
                 "tb-engine/doc/", "upstream/", ".repowise/")
FROZEN_FILES = {"CHANGELOG.md", "docs/trajectory.md", "docs/naming.md",
                "docs/rebrand-plan.md",
                # Dated assessments and plans that measured the pre-rename layout:
                # rewriting them would falsify the record they exist to keep.
                "docs/architecture-assessment.md", "docs/graph-engine-assessment.md",
                "docs/phase-d.md", "docs/repo-inventory.md",
                # The documents that exist to map old names to new ones.
                "tb-mcp-rs/docs/live-comparison.md", "tb-mcp-rs/docs/runtime-map.md",
                "tb-mcp-rs/docs/verification.md",
                "tb-engine/README-fork.md", "AGENTS.md",
                "docs/agents-and-decisions-assessment.md", "docs/retrieval-spike.md",
                "tb-mcp-rs/docs/client-matrix.md",
                # This tool spells the patterns it looks for.
                "tools/kb/legacy_audit.py"}

# The patterns, each with the class its matches belong to and a short note.
PATTERNS = [
    ("env_au",      re.compile(r"\bAU_[A-Z_]+"),                  "live",    "operator env var still on the old prefix"),
    ("tool_au",     re.compile(r"\bau_[a-z_]+\b"),                "live",    "tool name on the old prefix"),
    ("type_au",     re.compile(r"\bau\.engine\.[a-z_]+\b|::au-engine\b"), "format", "type namespace / builtin repo"),
    ("scheme_au",   re.compile(r"arsumbris://"),                  "format",  "parse alias still accepted"),
    ("identity_dot",re.compile(r"\.arsumbris\b"),                 "format",  "on-disk identity directory spelling"),
    ("repo_name",   re.compile(r"\bname:\s*au-[a-z0-9-]+"),       "content", "repository identity name"),
    ("wikilink_au", re.compile(r"::au-[a-z0-9-]+"),               "content", "wikilink to an old repository name"),
    ("prose_au",    re.compile(r"(?<![.\w/])arsumbris(?![.\w])", re.I), "prose", "bare product name in prose"),
    ("binary_au",   re.compile(r"\bCARGO_BIN_EXE_au\b|/bin/au\b|\bexe\(\"au\"\)"), "live", "the engine binary by its old name"),
]

LINEAGE_FILES = {"README.md", "AGENTS.md", "LICENSE.md", "docs/README.md"}
LINEAGE_HINT = re.compile(r"fork|lineage|upstream|preserved|orig", re.I)


def is_text(path: Path) -> bool:
    return path.suffix.lower() in TEXT


def tracked() -> list[Path]:
    out = subprocess.run(["git", "-C", str(ROOT), "ls-files"],
                         capture_output=True, text=True, check=True).stdout
    return [ROOT / p for p in out.splitlines() if p]


def classify_file(rel: str) -> str:
    if rel.startswith(FROZEN_PREFIX) or rel in FROZEN_FILES:
        return "history"
    if rel in LINEAGE_FILES:
        return "lineage"
    return "live"


def scan() -> dict:
    hits = defaultdict(lambda: defaultdict(list))
    for path in tracked():
        if not is_text(path):
            continue
        rel = str(path.relative_to(ROOT))
        base = classify_file(rel)
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        for name, pat, cls, note in PATTERNS:
            for m in pat.finditer(text):
                line = text.count("\n", 0, m.start()) + 1
                # A bare product name in a lineage paragraph is where it belongs.
                if name == "prose_au" and base == "lineage":
                    line_text = text.splitlines()[line - 1]
                    if LINEAGE_HINT.search(line_text):
                        cls = "lineage"
                hits[name]["rows"].append({
                    "file": rel, "line": line, "match": m.group(0),
                    "class": base if base in ("history", "lineage") else cls,
                })
    return hits


def on_disk_dirs() -> list[str]:
    out = subprocess.run(
        ["find", str(ROOT), "-type", "d", "-name", ".arsumbris",
         "-not", "-path", "*/target/*", "-not", "-path", "*/.git/*"],
        capture_output=True, text=True).stdout
    return [str(Path(p).relative_to(ROOT)) for p in out.splitlines()]


def report(hits, dirs, as_json: bool) -> int:
    summary = Counter()
    for name, data in hits.items():
        for row in data["rows"]:
            summary[row["class"]] += 1
    summary["dirs_arsumbris"] = len(dirs)

    if as_json:
        print(json.dumps({
            "summary": dict(summary),
            "by_pattern": {k: Counter(r["class"] for r in v["rows"]) for k, v in hits.items()},
            "dirs_arsumbris": dirs,
        }, indent=2, default=dict))
        return 0

    print(f"tracked files: {len(tracked())}")
    print(f"on-disk .arsumbris directories: {len(dirs)}")
    print()
    print(f"{'pattern':14s} {'live':>5s} {'format':>6s} {'content':>7s} {'prose':>5s} {'history':>7s} {'lineage':>7s}  note")
    for name, pat, cls, note in PATTERNS:
        rows = hits.get(name, {}).get("rows", [])
        c = Counter(r["class"] for r in rows)
        print(f"{name:14s} {c.get('live',0):5d} {c.get('format',0):6d} {c.get('content',0):7d} "
              f"{c.get('prose',0):5d} {c.get('history',0):7d} {c.get('lineage',0):7d}  {note}")
    print()
    print("summary:", dict(sorted(summary.items())))
    live = summary.get("live", 0)
    print()
    if live:
        print(f"LIVE traces remain: {live}. These are read by code or an operator now.")
        for name, pat, cls, note in PATTERNS:
            rows = [r for r in hits.get(name, {}).get("rows", []) if r["class"] == "live"]
            if rows:
                print(f"  {name}: {len(rows)}")
                for r in rows[:8]:
                    print(f"    {r['file']}:{r['line']}  {r['match']}")
    else:
        print("LIVE traces: 0.")
    return 0


def main(argv: list[str]) -> int:
    hits = scan()
    dirs = on_disk_dirs()
    as_json = "--json" in argv
    rc = report(hits, dirs, as_json)
    if "--check" in argv:
        live = sum(1 for v in hits.values() for r in v["rows"] if r["class"] == "live")
        return 1 if live else 0
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
