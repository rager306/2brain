#!/usr/bin/env python3
"""Measure the knowledge base itself, not the code that serves it.

`tools/cards/metrics.json` describes the engine. This describes the corpus: how much of it is typed, how much
carries a source, how much is reachable by following a link, and what the vault has written down and not answered.
Run it weekly and compare: a base that grows in notes but not in reachable, sourced notes is growing sideways.

    python3 tools/kb/census.py                 # human-readable
    python3 tools/kb/census.py --json          # the same numbers, for metrics.json

The engine is asked for two things only — the isolated set and the broken links — because those are graph facts
the filesystem cannot answer. Everything else is frontmatter, which is the actual contract.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import statistics
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tb-mcp-rs/scripts/lib"))
from mcp_stdio import StdioMcp, is_error, tool_payload  # noqa: E402

ADAPTER = os.environ.get("BRAIN_BIN", "/usr/local/bin/2brain")
VAULT = Path(os.environ.get("BRAIN_VAULT", "/root/vault"))
LINK = re.compile(r"\[\[([^\]|#]+)")
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
            fields[match.group(1)] = match.group(2).strip()
    return fields


def census() -> dict:
    notes = []
    for path in VAULT.rglob("*.md"):
        relative = path.relative_to(VAULT)
        head = relative.parts[0] if relative.parts else ""
        if head in SERVICE:
            kind = "service"
        elif head == "inbox":
            kind = "inbox"
        else:
            kind = "note"
        text = path.read_text(errors="ignore")
        fields = frontmatter(text)
        body = text.split("---", 2)[-1]
        body_links = [target.strip() for target in LINK.findall(body)]
        # The vault's convention puts "what this note is about" in `topics:` as wikilinks, and the engine counts
        # those as field references. A body-only count therefore calls a connected note isolated.
        topic_links = [target.strip() for target in TOPIC.findall(text[: len(text) - len(body)])]
        links = body_links + topic_links
        notes.append(
            {
                "path": str(relative),
                "kind": kind,
                "typed": "type" in fields,
                "sourced": bool(fields.get("source_url") or fields.get("provenance")),
                "described": bool(fields.get("description")),
                "freshness": fields.get("freshness", ""),
                "status": fields.get("status", ""),
                "outbound": len(links),
                "body_links": len(body_links),
                "topic_links": len(topic_links),
            }
        )

    content = [note for note in notes if note["kind"] in ("note", "inbox")]
    outbound = [note["outbound"] for note in content]
    return {
        "notes": len(content),
        "drafts": sum(1 for note in notes if note["kind"] == "inbox"),
        "service_files": sum(1 for note in notes if note["kind"] == "service"),
        "typed": sum(1 for note in content if note["typed"]),
        "sourced": sum(1 for note in content if note["sourced"]),
        "described": sum(1 for note in content if note["described"]),
        "with_status_current": sum(1 for note in content if note["status"] == "current"),
        "outbound_total": sum(outbound),
        "outbound_median": statistics.median(outbound) if outbound else 0,
        "no_outbound": sum(1 for count in outbound if count == 0),
        "no_body_links": sum(1 for note in content if note["body_links"] == 0),
        "no_topic_links": sum(1 for note in content if note["topic_links"] == 0),
        "freshness": {
            key: sum(1 for note in content if note["freshness"] == key)
            for key in ("evergreen", "stable", "slow", "volatile")
        },
        "source": "frontmatter census of the vault; graph facts from the engine",
    }


def graph_facts() -> dict:
    try:
        with StdioMcp([ADAPTER, "mcp", str(VAULT)], name="kb-census") as client:
            health = tool_payload(client.call("tb_graph_health", {"limit": 200}))
            payload = health["graph_health"]
            counts = payload["counts"]
            return {
                "nodes": counts["nodes"],
                "edges": counts["edges"],
                "isolated": counts["isolated"],
                "dead_ends": counts["dead_ends"],
                "broken_links": counts["broken_links"],
                "components": counts["components"],
            }
    except Exception as error:  # the engine is optional for the file-side census
        return {"error": str(error)[:200]}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    data = census()
    data["graph"] = graph_facts()
    if args.json:
        print(json.dumps(data, indent=2))
        return 0

    total = data["notes"]
    def share(key: str) -> str:
        return f"{data[key]}/{total} ({100 * data[key] // max(total, 1)}%)"

    print(f"vault  {VAULT}  — {total} notes ({data['drafts']} in inbox), {data['service_files']} service files")
    print(f"  typed      {share('typed')}")
    print(f"  described  {share('described')}")
    print(f"  sourced    {share('sourced')}")
    print(f"  status:current {share('with_status_current')}")
    print(f"  links      {data['outbound_total']} total, median {data['outbound_median']} per note, "
          f"{data['no_outbound']} with none")
    print(f"             of which body links leave {data['no_body_links']} bare, "
          f"`topics:` links leave {data['no_topic_links']} bare")
    print("  freshness  " + ", ".join(f"{key} {count}" for key, count in data["freshness"].items()))
    graph = data["graph"]
    if "error" in graph:
        print("  graph      unavailable: " + graph["error"])
    else:
        print(f"  graph      {graph['nodes']} nodes, {graph['edges']} edges, {graph['isolated']} isolated, "
              f"{graph['dead_ends']} dead ends, {graph['broken_links']} broken links")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
