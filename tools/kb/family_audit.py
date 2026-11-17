#!/usr/bin/env python3
"""Audit `family:` against the rule the project adopted on 2026-11-17.

The rule (docs/research/family-field-2026-11-17.md): `family:` names the note's series or
source namespace, `topics:` names its subjects. A single-member family is legitimate when it
is provenance; a topic-shaped family with one member is a finding for the next intake pass,
not a schema error. This tool reports the split; it never writes.

    python3 tools/kb/family_audit.py [VAULT]        # human-readable
    python3 tools/kb/family_audit.py --json         # for a card or a census diff
    python3 tools/kb/family_audit.py --selftest
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from collections import Counter
from pathlib import Path

PROVENANCE = re.compile(r"^f/(arxiv-|gh-|docs-|www-|[a-z0-9-]+\.[a-z]{2,})")
TOPIC = re.compile(r'^\s*-\s*"?\[\[([^\]|#]+)\]\]"?\s*$', re.MULTILINE)
SERVICE = {"ops", "self", "templates"}


def frontmatter(text: str) -> dict[str, str]:
    if not text.startswith("---"):
        return {}
    end = text.find("\n---", 3)
    if end < 0:
        return {}
    fields = {}
    for line in text[3:end].splitlines():
        match = re.match(r"([a-z_]+):\s*(.*)", line.strip())
        if match:
            fields[match.group(1)] = match.group(2).strip().strip('"')
    return fields


def topics_of(text: str) -> list[str]:
    head = text[: text.find("\n---", 3)] if "\n---" in text else ""
    return [target.strip() for target in TOPIC.findall(head)]


def classify(family: str) -> str:
    return "provenance" if PROVENANCE.match(family) else "topic"


def audit(root: Path) -> dict:
    notes = []
    for path in sorted(root.rglob("*.md")):
        relative = path.relative_to(root)
        if relative.parts and relative.parts[0] in SERVICE:
            continue
        text = path.read_text(errors="ignore")
        fields = frontmatter(text)
        notes.append(
            {
                "path": str(relative),
                "family": fields.get("family", ""),
                "topics": topics_of(text),
            }
        )

    families: Counter[str] = Counter()
    class_of: dict[str, str] = {}
    for note in notes:
        family = note["family"]
        if family:
            families[family] += 1
            class_of[family] = classify(family)

    singles = [name for name, count in families.items() if count == 1]
    by_class = {
        "provenance": [name for name in families if class_of[name] == "provenance"],
        "topic": [name for name in families if class_of[name] == "topic"],
    }
    names_as_topics = sorted(
        name
        for name in families
        if any(topic == name or topic == name.removeprefix("f/") for note in notes for topic in note["topics"])
    )
    return {
        "notes": len(notes),
        "with_family": sum(1 for note in notes if note["family"]),
        "with_topics": sum(1 for note in notes if note["topics"]),
        "with_both": sum(1 for note in notes if note["family"] and note["topics"]),
        "families": len(families),
        "singles": len(singles),
        "by_class": {
            name: {"values": len(values), "notes": sum(families[v] for v in values)}
            for name, values in by_class.items()
        },
        "singles_by_class": {
            name: sum(1 for v in values if families[v] == 1) for name, values in by_class.items()
        },
        "topic_singles": sorted(
            (name for name in by_class["topic"] if families[name] == 1),
            key=lambda name: (-families[name], name),
        ),
        "families_also_topics": names_as_topics,
    }


def selftest() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "a.md").write_text(
            "---\nfamily: f/arxiv-2601.00001\ntopics:\n  - \"[[agents]]\"\n---\nbody\n"
        )
        (root / "b.md").write_text("---\nfamily: f/patterns\n---\nbody\n")
        (root / "c.md").write_text(
            "---\nfamily: f/agent-architectures\ntopics:\n  - \"[[f/patterns]]\"\n---\nbody\n"
        )
        (root / "d.md").write_text("---\nfamily: f/gh-owner-repo\n---\nbody\n")
        (root / "ops").mkdir()
        (root / "ops" / "e.md").write_text("---\nfamily: f/ignored\n---\nbody\n")
        result = audit(root)
        assert result["notes"] == 4, result
        assert result["families"] == 4, result
        assert result["singles"] == 4, result
        assert result["by_class"]["provenance"]["values"] == 2, result
        assert result["by_class"]["topic"]["values"] == 2, result
        assert result["singles_by_class"] == {"provenance": 2, "topic": 2}, result
        assert result["topic_singles"] == ["f/agent-architectures", "f/patterns"], result
        assert result["families_also_topics"] == ["f/patterns"], result
        assert classify("f/docs-google-com") == "provenance"
        assert classify("f/agent-architectures") == "topic"
    print("selftest passed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vault", nargs="?", default="/root/vault")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        selftest()
        return 0
    root = Path(args.vault)
    if not root.is_dir():
        print(f"not a directory: {root}", file=sys.stderr)
        return 2
    result = audit(root)
    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=1))
        return 0
    print(f"notes {result['notes']} | family {result['with_family']} | topics {result['with_topics']} | both {result['with_both']}")
    for name, stats in result["by_class"].items():
        print(f"  {name:10} {stats['values']:4} values, {stats['notes']:4} notes, "
              f"{result['singles_by_class'][name]:4} single-member")
    print(f"families also used as topics: {len(result['families_also_topics'])}")
    for name in result["topic_singles"][:10]:
        print(f"  topic-shaped single: {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
