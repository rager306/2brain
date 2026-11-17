#!/usr/bin/env python3
"""Report topic maps whose body does not link the notes that name them.

The question is not "is the graph right" — it is. A note declares membership with `topics:
[[some-map]]`, and the engine resolves that edge both ways (`references_in` on the map returns
every note that names it, with `slot: topics` and `surface: frontmatter`). The question is whether a
READER of the map can reach those notes: a hub that 67 notes name and that links 15 of them is a
hub with 52 invisible members.

This tool answers that offline, over the same catalogued corpus the engine walks. It never writes:
the earlier draft of this file generated a "Named by" section into each map, which made the map's
body a second copy of the frontmatter index and added machinery to keep the two in step. The engine
already serves the index (`references_in`), so the map body is a human's navigation choice and the
gap is a report, not a file to generate.

    python3 tools/kb/moc_links.py                 # report
    python3 tools/kb/moc_links.py --json          # for a census diff
    python3 tools/kb/moc_links.py --check         # non-zero when any map has an unlinked namer
    python3 tools/kb/moc_links.py --selftest
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

TYPE_MOC = re.compile(r"^type:\s*moc\s*$", re.MULTILINE)
LINK = re.compile(r"\[\[([^\]|#]+)")
FRONTMATTER_END = re.compile(r"\n---\s*\n")
SKIP_DIRS = {"templates"}


def paths_of(root: Path) -> list[Path]:
    return [
        path
        for path in sorted(root.rglob("*.md"))
        if not any(part.startswith(".") or part in SKIP_DIRS for part in path.relative_to(root).parts)
    ]


def frontmatter(text: str) -> str:
    match = FRONTMATTER_END.search(text, 3)
    return text[: match.start()] if match else ""


def body(text: str) -> str:
    match = FRONTMATTER_END.search(text, 3)
    return text[match.end() :] if match else text


def corpus(root: Path) -> tuple[dict[str, str], set[str]]:
    """Stem to text, plus the stems that name more than one file."""
    notes: dict[str, str] = {}
    counts: dict[str, int] = {}
    for path in paths_of(root):
        counts[path.stem] = counts.get(path.stem, 0) + 1
        notes.setdefault(path.stem, path.read_text(errors="ignore"))
    return notes, {stem for stem, count in counts.items() if count > 1}


def report(root: Path) -> list[dict]:
    notes, ambiguous = corpus(root)
    rows = []
    for stem, text in notes.items():
        if not TYPE_MOC.search(frontmatter(text)):
            continue
        namers = sorted(
            other
            for other, other_text in notes.items()
            if other != stem and stem in LINK.findall(frontmatter(other_text))
        )
        linked = set(LINK.findall(body(text)))
        unlinked = [name for name in namers if name not in linked and name not in ambiguous]
        rows.append(
            {
                "map": stem,
                "namers": len(namers),
                "linked": len(namers) - len(unlinked),
                "unlinked": len(unlinked),
                "ambiguous": len([name for name in namers if name in ambiguous]),
                "examples": unlinked[:5],
            }
        )
    return sorted(rows, key=lambda row: -row["unlinked"])


def selftest() -> int:
    import tempfile

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "hub.md").write_text(
            "---\ndescription: \"A hub.\"\ntype: moc\n---\n\n# hub\n\n- [[listed]] — here\n",
            encoding="utf-8",
        )
        (root / "listed.md").write_text("---\ntopics: [\"[[hub]]\"]\n---\n", encoding="utf-8")
        (root / "orphan.md").write_text("---\ntopics: [\"[[hub]]\"]\n---\n", encoding="utf-8")
        (root / "plain.md").write_text("---\ndescription: \"No topics.\"\n---\n", encoding="utf-8")
        rows = report(root)
        assert len(rows) == 1 and rows[0]["map"] == "hub", rows
        assert rows[0]["namers"] == 2 and rows[0]["linked"] == 1 and rows[0]["unlinked"] == 1, rows
        assert rows[0]["examples"] == ["orphan"], rows
    print("selftest ok")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("vault", nargs="?", default="/root/vault")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    rows = report(Path(args.vault))
    if args.json:
        print(json.dumps(rows, indent=1))
        return 0
    total = sum(row["unlinked"] for row in rows)
    naming = sum(row["namers"] for row in rows)
    behind = [row for row in rows if row["unlinked"]]
    if args.check:
        for row in behind:
            print(f"gap  {row['map']}: {row['unlinked']} of {row['namers']} namers unlinked")
        print(f"moc_links: {len(rows) - len(behind)} complete, {len(behind)} with a gap")
        return 1 if behind else 0
    print(f"{len(rows)} topic maps, {total} of {naming} declarations not linked from the map body")
    print(f"{len(behind)} maps have a gap")
    for row in behind[:12]:
        print(f"  {row['unlinked']:4} of {row['namers']:4}  {row['map']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
