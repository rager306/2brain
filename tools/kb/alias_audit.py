#!/usr/bin/env python3
"""Size the acronym/expansion alias class in a vault.

An alias here is one concept written two ways, so that a query on one surface cannot reach
the notes that state the other. This tool measures the cheapest such class: an all-caps
acronym and a multi-word expansion whose initials spell it, found in the SAME note.

What it measures, and what it does not:

- it does NOT decide that a pair is real. Initials collide by accident (`CI` -> `check
  interface`), so a human reads the ranking; `tools/kb/alias_audit.py --json` names the
  candidates and their note sets.
- it reports DIVERGENCE: the share of notes the acronym and its expansion do not share
  (`1 - Jaccard`). A real pair whose surfaces reach the same notes costs nothing; a pair
  that reaches different notes is the class a search can miss.

    python3 tools/kb/alias_audit.py [VAULT]        # human-readable
    python3 tools/kb/alias_audit.py --json         # for a census diff
    python3 tools/kb/alias_audit.py --selftest
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path

TOKEN = re.compile(r"[^\W_]+", re.UNICODE)
ACRONYM = re.compile(r"^[A-Za-z]{2,8}$")
WINDOW = 8


def is_acronym(term: str) -> bool:
    if not ACRONYM.match(term):
        return False
    uppercase = sum(1 for char in term if char.isupper())
    return uppercase == len(term) or uppercase >= 2


def tokens(text: str) -> list[str]:
    return TOKEN.findall(text)


def notes_of(root: Path) -> dict[str, list[str]]:
    notes = {}
    for path in sorted(root.rglob("*.md")):
        relative = path.relative_to(root)
        if relative.parts and relative.parts[0].startswith("."):
            continue
        notes[relative.as_posix()] = tokens(path.read_text(errors="ignore"))
    return notes


def acronyms(notes: dict[str, list[str]]) -> Counter:
    seen = Counter()
    for terms in notes.values():
        for term in set(terms):
            if is_acronym(term):
                seen[term] += 1
    return seen


def initials(notes: dict[str, list[str]]) -> dict[str, list[tuple[str, str]]]:
    index = defaultdict(list)
    for note, terms in notes.items():
        for start in range(len(terms)):
            letters, words = [], []
            for term in terms[start : start + WINDOW]:
                letters.append(term[0].lower())
                words.append(term)
                if 2 <= len(letters) <= WINDOW:
                    index["".join(letters)].append((note, " ".join(words)))
    return index


def pairs(notes: dict[str, list[str]], seen: Counter, index) -> list[dict]:
    where = defaultdict(set)
    for note, terms in notes.items():
        for term in set(terms):
            if is_acronym(term):
                where[term].add(note)
    rows = []
    for acronym, count in seen.items():
        if count < 2:
            continue
        phrases = Counter(
            phrase.lower() for note, phrase in index.get(acronym.lower(), []) if note in where[acronym]
        )
        if not phrases:
            continue
        expansion = phrases.most_common(1)[0][0]
        words = expansion.split()
        expansion_notes = set()
        for note, terms in notes.items():
            lowered = [term.lower() for term in terms]
            for start in range(len(lowered) - len(words) + 1):
                if lowered[start : start + len(words)] == words:
                    expansion_notes.add(note)
                    break
        shared = len(where[acronym] & expansion_notes)
        union = len(where[acronym] | expansion_notes)
        rows.append(
            {
                "acronym": acronym,
                "expansion": expansion,
                "acronym_notes": count,
                "expansion_notes": len(expansion_notes),
                "shared": shared,
                "divergence": round(1.0 - shared / union, 2) if union else 0.0,
            }
        )
    return sorted(rows, key=lambda row: (-row["expansion_notes"], -row["divergence"]))


def audit(root: Path) -> dict:
    notes = notes_of(root)
    seen = acronyms(notes)
    rows = pairs(notes, seen, initials(notes))
    return {
        "notes": len(notes),
        "acronyms": {
            "distinct": len(seen),
            "in_two_notes": sum(1 for count in seen.values() if count >= 2),
        },
        "pairs": rows,
        "divergent_pairs": sum(1 for row in rows if row["divergence"] >= 0.5),
    }


def selftest() -> int:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "one.md").write_text(
            "The SoT is the source of truth for the graph.\n", encoding="utf-8"
        )
        (root / "two.md").write_text("The source of truth wins.\n", encoding="utf-8")
        (root / "three.md").write_text("SoT again.\n", encoding="utf-8")
        report = audit(root)
        assert report["notes"] == 3, report
        assert report["acronyms"]["distinct"] == 1, report
        assert report["acronyms"]["in_two_notes"] == 1, report
        assert report["pairs"][0]["acronym"] == "SoT", report
        assert report["pairs"][0]["expansion"] == "source of truth", report
        assert report["pairs"][0]["shared"] == 1, report
    print("selftest ok")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("vault", nargs="?", default="/root/vault")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--top", type=int, default=20)
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    report = audit(Path(args.vault))
    if args.json:
        print(json.dumps(report, ensure_ascii=False, indent=1))
        return 0
    print(f"notes {report['notes']}")
    print(
        f"acronyms distinct {report['acronyms']['distinct']}, "
        f"in two notes {report['acronyms']['in_two_notes']}"
    )
    print(f"candidate pairs {len(report['pairs'])}, divergent {report['divergent_pairs']}")
    print(f"{'acr':6} {'acr#':>5} {'exp#':>5} {'shared':>6} {'div':>4}  expansion")
    for row in report["pairs"][: args.top]:
        print(
            f"{row['acronym']:6} {row['acronym_notes']:5} {row['expansion_notes']:5} "
            f"{row['shared']:6} {row['divergence']:4}  {row['expansion']}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
