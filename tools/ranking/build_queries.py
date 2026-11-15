#!/usr/bin/env python3
"""Build a labelled query set for ranking evaluation, from the vault itself.

Track 3 of `docs/trajectory.md` changes how results are ordered, and "it looks better" is not a result.
This builds two classes of queries with a known expected note:

  `literal`  — a phrase that occurs in exactly one note (verified with `au_grep`), so any correct
               ranker must put that note first; a ranker that fails here is broken, not different.
  `topical`  — a few content words from the note's own title, which many notes may match; this is where
               ranking actually decides something.

Both are synthetic by construction: they are derived from the notes, not from questions a person asked.
That is stated in every row (`source`) so a report never presents them as real usage.

    python3 tools/ranking/build_queries.py --notes 40 --out tools/ranking/labelled-queries.json
"""

from __future__ import annotations

import argparse
import json
import os
import random
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tb-mcp-rs/scripts/lib"))
from mcp_stdio import StdioMcp, is_error, tool_payload, tool_text  # noqa: E402

ADAPTER = os.environ.get("BRAIN_BIN", "/usr/local/bin/2brain")
VAULT = os.environ.get("BRAIN_VAULT", "/root/vault")
PREFIX = VAULT.rstrip("/") + "/"

# A candidate phrase must be long enough to be specific and short enough to read like a query.
WORD = re.compile(r"[A-Za-z][A-Za-z'-]{2,}")
STOP = {
    "the", "and", "for", "that", "this", "with", "from", "into", "over", "than", "then", "they",
    "their", "there", "these", "those", "which", "while", "when", "where", "what", "have", "has",
    "had", "was", "were", "are", "not", "but", "you", "your", "our", "its", "can", "will", "would",
    "should", "could", "may", "might", "must", "one", "two", "all", "any", "each", "more", "most",
}


def body_of(text: str) -> str:
    """The note's body: everything after the closing frontmatter fence."""
    parts = text.split("---", 2)
    return parts[2] if len(parts) == 3 else text


def phrases(body: str, size: int = 4) -> list[str]:
    words = [w for w in WORD.findall(body)]
    out = []
    for start in range(0, max(0, len(words) - size), size):
        window = words[start : start + size]
        if sum(1 for w in window if w.lower() not in STOP) < 2:
            continue
        out.append(" ".join(window))
    return out


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--notes", type=int, default=40)
    parser.add_argument("--out", type=Path, default=REPO / "tools/ranking/labelled-queries.json")
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args(argv[1:])

    rng = random.Random(args.seed)
    with StdioMcp([ADAPTER, "mcp", VAULT], name="build") as client:
        paths: list[str] = []
        offset = 0
        while True:
            reply = client.call("au_files", {"repo": "vault", "limit": 500, "offset": offset})
            if is_error(reply):
                print(f"au_files refused: {tool_text(reply)}", file=sys.stderr)
                return 1
            payload = tool_payload(reply)
            page = payload.get("files", [])
            # au_files reports absolute paths; every file tool takes a path relative to the member.
            paths.extend(
                row["path"][len(PREFIX):]
                for row in page
                if row.get("path", "").startswith(PREFIX)
            )
            offset = payload.get("next_offset")
            if not page or offset is None:
                break
        notes = [p for p in paths if p.endswith(".md") and not p.endswith("SKILL.md")]
        rng.shuffle(notes)

        rows: list[dict] = []
        checked = 0
        for relative in notes:
            if len(rows) >= args.notes * 2 or checked >= args.notes * 6:
                break
            checked += 1
            front = client.call("au_frontmatter", {"repo": "vault", "path": relative})
            if is_error(front):
                continue
            meta = tool_payload(front).get("frontmatter") or {}
            if not isinstance(meta, dict) or not meta.get("type"):
                continue
            content = client.call("au_content", {"repo": "vault", "path": relative})
            if is_error(content):
                continue
            text = tool_payload(content).get("content", {}).get("text", "")
            body = body_of(text)

            # literal class: a phrase that occurs in exactly one note
            for phrase in phrases(body)[:6]:
                count = client.call("au_grep", {"query": phrase, "limit": 5})
                if is_error(count):
                    continue
                grep = tool_payload(count).get("grep", {})
                if grep.get("total") == 1:
                    rows.append({
                        "query": phrase, "expected": relative, "class": "literal",
                        "type": meta["type"], "source": "synthetic-unique-phrase",
                    })
                    break

            # topical class: content words from the file name
            stem = Path(relative).stem.replace("-", " ").replace("_", " ")
            words = [w for w in stem.split() if w.lower() not in STOP and len(w) > 2]
            if len(words) >= 2:
                rows.append({
                    "query": " ".join(words[:4]), "expected": relative, "class": "topical",
                    "type": meta["type"], "source": "synthetic-from-filename",
                })

    by_class = {}
    for row in rows:
        by_class.setdefault(row["class"], 0)
        by_class[row["class"]] += 1
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({"vault": VAULT, "rows": rows, "counts": by_class}, indent=1))
    print(f"wrote {args.out} ({len(rows)} rows, {by_class})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
