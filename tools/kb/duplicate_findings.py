#!/usr/bin/env python3
"""Find notes that record the same finding twice.

The vault was built by repeated ingest passes, so one observation can land as two notes: an English
and a Russian rendering, a "g0" regression and a "g01" pair, two spellings of a measured number. Each
duplicate inflates the corpus (two notes where one is the evidence) and splits inbound links between
them.

The measure is deliberately dumb and checkable: token-set Jaccard over the note BODIES, with
identical-looking descriptions pulling a pair forward. A pair is a CANDIDATE; the numbers in the two
notes decide. Anything below `--threshold` is not reported.

A `claim` and the insight its `evidence:` names are EXCLUDED by construction: they restate one
finding on purpose, and a detector that flagged them would bury the real duplicates under the
vault's own claim structure. What is left is one finding recorded twice — an English and a Russian
rendering of the same source, or two ingest passes over one document.

    python3 tools/kb/duplicate_findings.py                     # report
    python3 tools/kb/duplicate_findings.py --json              # for a merge plan
    python3 tools/kb/duplicate_findings.py --selftest
"""

from __future__ import annotations

import argparse
import itertools
import json
import re
import sys
from pathlib import Path

LINK = re.compile(r"\[\[([^\]|#]+)")
FRONTMATTER_END = re.compile(r"\n---\s*\n")
WORD = re.compile(r"[A-Za-zА-Яа-яЁё0-9]+")
SKIP_DIRS = {"templates"}
DEFAULT = 0.30


def parts(text: str) -> tuple[str, str]:
    match = FRONTMATTER_END.search(text, 3)
    return (text[: match.start()], text[match.end() :]) if match else ("", text)


def tokens(text: str) -> frozenset[str]:
    return frozenset(word.lower() for word in WORD.findall(text))


def corpus(root: Path) -> dict[str, tuple[frozenset[str], frozenset[str], str, str]]:
    out = {}
    for path in sorted(root.rglob("*.md")):
        if any(part.startswith(".") or part in SKIP_DIRS for part in path.relative_to(root).parts):
            continue
        head, body = parts(path.read_text(errors="ignore"))
        description = ""
        match = re.search(r"^description:\s*\"?(.*?)\"?\s*$", head, re.MULTILINE)
        if match:
            description = match.group(1)
        out[path.stem] = (tokens(description), tokens(body), description, path.read_text(errors="ignore"))
    return out


def jaccard(left: frozenset[str], right: frozenset[str]) -> float:
    if not left or not right:
        return 0.0
    return len(left & right) / len(left | right)


def evidence_of(text: str) -> set[str]:
    """The note stems a `claim` names in its `evidence:` field."""
    head, _ = parts(text)
    match = re.search(r"^evidence:\s*\"?(.*?)\"?\s*$", head, re.MULTILINE)
    return set(LINK.findall(match.group(1))) if match else set()


def pairs(sources, threshold: float) -> list[dict]:
    notes = {stem: (value[0], value[1]) for stem, value in sources.items()}
    descs = {stem: value[2] for stem, value in sources.items()}
    cited = {stem: evidence_of(value[3]) for stem, value in sources.items()}
    rows = []
    for left, right in itertools.combinations(sorted(notes), 2):
        if right in cited[left] or left in cited[right]:
            continue
        head_l, body_l = notes[left]
        head_r, body_r = notes[right]
        desc_l, desc_r = descs[left][:90], descs[right][:90]
        body = jaccard(body_l, body_r)
        if body < threshold:
            continue
        rows.append(
            {
                "a": left,
                "b": right,
                "body": round(body, 3),
                "description": round(jaccard(head_l, head_r), 3),
                "desc_a": desc_l,
                "desc_b": desc_r,
            }
        )
    return sorted(rows, key=lambda row: -(row["body"] + row["description"]))


def selftest() -> int:
    import tempfile

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        shared = "regression g01 plural query returns zero hits the singular finds two"
        (root / "one.md").write_text(
            f"---\ndescription: \"plural query costs the query\"\n---\n\n{shared}\n", encoding="utf-8"
        )
        (root / "two.md").write_text(
            f"---\ndescription: \"plural query costs the query\"\n---\n\n{shared}\n", encoding="utf-8"
        )
        (root / "three.md").write_text(
            "---\ndescription: \"unrelated\"\n---\n\nhoney badgers do not care\n", encoding="utf-8"
        )
        rows = pairs(corpus(root), 0.3)
        assert len(rows) == 1 and {rows[0]["a"], rows[0]["b"]} == {"one", "two"}, rows
        assert rows[0]["body"] == 1.0, rows
    print("selftest ok")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("vault", nargs="?", default="/root/vault")
    parser.add_argument("--threshold", type=float, default=DEFAULT)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    rows = pairs(corpus(Path(args.vault)), args.threshold)
    if args.json:
        print(json.dumps(rows, ensure_ascii=False, indent=1))
        return 0
    print(f"{len(rows)} candidate pairs at body Jaccard >= {args.threshold}")
    for row in rows[:20]:
        print(f"  {row['body']:.2f} body / {row['description']:.2f} desc  {row['a']}")
        print(f"                                  {row['b']}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
