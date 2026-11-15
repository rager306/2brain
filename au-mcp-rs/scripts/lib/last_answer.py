#!/usr/bin/env python3
"""Print the last thing a client said to the user, from a transcript it streamed.

The verification scripts assert that the FINAL ANSWER names what the engine
returned. Grepping the whole transcript for that name is not the same claim: the
model's own tool arguments contain it, so a failed tool call would satisfy the check.

    last_answer.py --client claude /tmp/turn.jsonl
    last_answer.py --client codex  /tmp/codex-turn.jsonl
"""

from __future__ import annotations

import argparse
import json
import sys


def claude(path: str) -> str:
    text = ""
    for line in open(path, encoding="utf-8", errors="replace"):
        try:
            record = json.loads(line)
        except Exception:
            continue
        if record.get("type") != "assistant":
            continue
        for block in (record.get("message") or {}).get("content") or []:
            if isinstance(block, dict) and block.get("type") == "text":
                text = block.get("text") or text
    return text


def codex(path: str) -> str:
    text = ""
    for line in open(path, encoding="utf-8", errors="replace"):
        try:
            record = json.loads(line)
        except Exception:
            continue
        item = record.get("item") or {}
        if record.get("type") == "item.completed" and item.get("type") == "agent_message":
            text = item.get("text") or text
    return text


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client", choices=["claude", "codex"], default="claude")
    parser.add_argument("transcript")
    args = parser.parse_args()
    print({"claude": claude, "codex": codex}[args.client](args.transcript))
    return 0


if __name__ == "__main__":
    sys.exit(main())
