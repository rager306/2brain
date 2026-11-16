#!/usr/bin/env python3
"""Propose links for notes that have none, using the engine's own search.

The census shows the corpus grows by accumulation: 424 notes with no outbound link. A note nobody links to and
that links nowhere is a note the next agent will not reach by following the graph, and a note the search may rank
without any supporting context around it.

This does not write. It asks the engine for the note's nearest neighbours by the note's own description and
prints a proposal a person can accept, reject or ignore. Suggestions come from search, so they inherit the
ranking's current quality — which is the point: a suggestion list that looks wrong is evidence about ranking.

    python3 tools/kb/connect.py --limit 20          # proposal for 20 unlinked notes
    python3 tools/kb/connect.py --all --json        # every unlinked note, for a batch pass
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tb-mcp-rs/scripts/lib"))
from mcp_stdio import StdioMcp, tool_payload  # noqa: E402

ADAPTER = os.environ.get("BRAIN_BIN", "/usr/local/bin/2brain")
VAULT = Path(os.environ.get("BRAIN_VAULT", "/root/vault"))
LINK = re.compile(r"\[\[([^\]|#]+)")
SKIP_DIRS = {"ops", "self", "templates", ".git", ".2brain", ".arsumbris"}


def frontmatter_value(text: str, key: str) -> str:
    match = re.search(rf"^{key}:\s*(.*)$", text[:2000], re.MULTILINE)
    return match.group(1).strip() if match else ""


def unlinked(limit: int | None) -> list[dict]:
    found = []
    for path in VAULT.rglob("*.md"):
        relative = path.relative_to(VAULT)
        if any(part in SKIP_DIRS for part in relative.parts):
            continue
        text = path.read_text(errors="ignore")
        body = text.split("---", 2)[-1]
        if LINK.search(body):
            continue
        description = frontmatter_value(text, "description")
        if not description:
            continue
        found.append({"path": str(relative), "description": description})
        if limit and len(found) >= limit:
            break
    return found


def proposals(client: StdioMcp, note: dict, per_note: int) -> list[str]:
    query = " ".join(note["description"].split()[:14])
    if not query:
        return []
    try:
        reply = client.call("tb_search", {"query": query, "limit": per_note + 3})
    except Exception as error:
        return [f"search failed: {error}"]
    result = reply.get("result", reply)
    if result.get("isError"):
        text = result.get("content") or ""
        return [f"engine: {str(text)[:80]}"]
    block = (result.get("structuredContent") or {}).get("search") or {}
    root = str(VAULT) + "/"
    return [hit["path"].removeprefix(root) for hit in block.get("matches", [])
            if hit.get("path") and hit["path"].removeprefix(root) != note["path"]][:per_note]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, default=20)
    parser.add_argument("--per-note", type=int, default=3)
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    notes = unlinked(None if args.all else args.limit)
    out = []
    env = dict(os.environ, HOME=os.environ.get("BRAIN_HOME", "/root"))
    with StdioMcp([ADAPTER, "mcp", str(VAULT)], name="kb-connect", env=env) as client:
        for note in notes:
            out.append({**note, "candidates": proposals(client, note, args.per_note)})

    if args.json:
        print(json.dumps(out, indent=2))
        return 0

    empty = sum(1 for row in out if not row["candidates"])
    print(f"{len(out)} unlinked notes with a description, {empty} with no candidate\n")
    for row in out:
        print(row["path"])
        print("  " + row["description"][:150])
        for candidate in row["candidates"]:
            print("    -> " + candidate)
        print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
