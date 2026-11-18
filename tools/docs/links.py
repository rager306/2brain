#!/usr/bin/env python3
"""Check that every relative link in the repository's markdown resolves.

A relative link is `[text](target)` or `[text](target#anchor)` where target is
not a URL. The file must exist relative to the linking document, and when the
document links markdown, the anchor must match a heading slug.

Usage:
    tools/docs/links.py --check [path ...]   exit 1 on a broken link
    tools/docs/links.py --selftest           prove each rule bites
"""
from __future__ import annotations

import re
import sys
import tempfile
from pathlib import Path

LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
HEADING = re.compile(r"^(#{1,6})\s+(.*)$", re.M)
CODE_FENCE = re.compile(r"```.*?```", re.S)


def slug(text: str) -> str:
    """GFM heading anchor: lower, drop punctuation, one hyphen per space.

    A dash surrounded by spaces becomes two hyphens, which is why this does not
    collapse runs: `2026-11-17 — reconciliation:` must match the anchor GitHub
    generates for it.
    """
    text = re.sub(r"`([^`]*)`", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"[^\w\s-]", "", text.lower())
    return text.replace(" ", "-").strip("-")


def anchors(path: Path) -> set[str]:
    text = CODE_FENCE.sub("", path.read_text(encoding="utf-8", errors="replace"))
    return {slug(m.group(2)) for m in HEADING.finditer(text)}


def links_of(path: Path):
    text = CODE_FENCE.sub("", path.read_text(encoding="utf-8", errors="replace"))
    for m in LINK.finditer(text):
        target = m.group(1)
        if re.match(r"^[a-z][a-z0-9+.-]*:", target) or target.startswith("#"):
            continue
        yield target


def check(root: Path, docs) -> list[str]:
    broken = []
    for doc in docs:
        for target in links_of(doc):
            file_part, _, anchor = target.partition("#")
            dest = (doc.parent / file_part).resolve() if file_part else doc.resolve()
            if not dest.exists():
                broken.append(f"{doc}: {target} -> no such path")
                continue
            if anchor and dest.suffix == ".md":
                if anchor.lower() not in anchors(dest):
                    broken.append(f"{doc}: {target} -> no such heading")
    return broken


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "a.md").write_text("# Title Here\n\n[x](b.md) [y](b.md#section-two)\n")
            (root / "b.md").write_text("# Section Two\n")
            assert check(root, [root / "a.md"]) == [], "the passing case must pass"
            (root / "c.md").write_text("[x](gone.md)\n")
            assert any("no such path" in b for b in check(root, [root / "c.md"]))
            (root / "d.md").write_text("[x](b.md#nope)\n")
            assert any("no such heading" in b for b in check(root, [root / "d.md"]))
        print("selftest passed")
        return 0

    args = [a for a in argv if a != "--check"]
    root = Path(args[0]) if args else Path(__file__).resolve().parents[2]
    docs = sorted(root.rglob("*.md"))
    docs = [d for d in docs if ".git" not in d.parts and "target" not in d.parts]
    broken = check(root, docs)
    for line in broken:
        print(line)
    print(f"{len(docs)} documents, {len(broken)} broken links")
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
