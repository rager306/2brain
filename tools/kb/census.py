#!/usr/bin/env python3
"""Measure the knowledge base itself, not the code that serves it.

`tools/cards/metrics.json` describes the engine. This describes the corpus: how much of it is typed, how much
carries a source, how much is reachable by following a link, and what the vault has written down and not answered.
Run it weekly and compare: a base that grows in notes but not in reachable, sourced notes is growing sideways.

    python3 tools/kb/census.py                 # human-readable
    python3 tools/kb/census.py --json          # the same numbers, for metrics.json

The engine is asked for the graph facts a text scan cannot answer: `tb_graph_health` for the whole graph, and
`tb_references` for each note the regex calls unlinked. Frontmatter supplies the typing, source and status
fields.

The link counts in this file are SYNTACTIC: a regex over the bytes, so a link inside a fenced code block counts
and an inline `topics: ["[[x]]"]` array is missed. They carry a `syntactic_` name for that reason. The
graph-accurate link numbers come from the engine's `references` read (`tb_references` per note) and
`tb_graph_health`.
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
        "syntactic_outbound_total": sum(outbound),
        "syntactic_outbound_median": statistics.median(outbound) if outbound else 0,
        "syntactic_no_outbound": sum(1 for count in outbound if count == 0),
        "syntactic_unlinked_paths": [note["path"] for note in content if note["outbound"] == 0],
        "syntactic_no_body_links": sum(1 for note in content if note["body_links"] == 0),
        "syntactic_no_topic_links": sum(1 for note in content if note["topic_links"] == 0),
        "freshness": {
            key: sum(1 for note in content if note["freshness"] == key)
            for key in ("evergreen", "stable", "slow", "volatile")
        },
        "source": "frontmatter census plus regex link counts over note text; graph facts from the engine",
    }


def engine_edges(client: StdioMcp, paths: list[str]) -> dict[str, tuple[int, int]]:
    """Each note's (in, out) reference counts as the ENGINE sees them.

    A regex over the file cannot answer this: `topics:` has a block-list and an inline-array spelling, a body link can
    be qualified or pinned, and only the engine resolves any of them. The regex counts below stay as a file-level
    view; the numbers a reader should trust come from here.
    """
    edges: dict[str, tuple[int, int]] = {}
    for path in paths:
        try:
            reply = client.call("tb_references", {"repo": "vault", "path": path})
        except Exception:
            continue
        payload = (reply.get("result", reply).get("structuredContent") or {})
        edges[path] = (len(payload.get("references_in") or []), len(payload.get("references_out") or []))
    return edges


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
    # The regex says "no link"; only the engine can say whether that is true, and whether anything points AT a note.
    unlinked_by_regex = data.pop("syntactic_unlinked_paths", [])
    if unlinked_by_regex:
        env = dict(os.environ, HOME=os.environ.get("BRAIN_HOME", "/root"))
        with StdioMcp([ADAPTER, "mcp", str(VAULT)], name="kb-census-edges", env=env) as client:
            edges = engine_edges(client, unlinked_by_regex)
        data["engine"] = {
            "checked": len(edges),
            "no_edge_either_way": sorted(path for path, (i, o) in edges.items() if i == 0 and o == 0),
            "no_inbound": sorted(path for path, (i, _) in edges.items() if i == 0),
        }
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
    print(f"  file text  {data['syntactic_outbound_total']} [[wikilink]] matches (regex), "
          f"median {data['syntactic_outbound_median']} per note, {data['syntactic_no_outbound']} with none")
    print(f"             of which body text leaves {data['syntactic_no_body_links']} bare, "
          f"`topics:` block lists leave {data['syntactic_no_topic_links']} bare")
    print("  freshness  " + ", ".join(f"{key} {count}" for key, count in data["freshness"].items()))
    engine = data.get("engine")
    if engine:
        print(f"  engine     {engine['checked']} notes with no link by regex -> "
              f"{len(engine['no_edge_either_way'])} with no reference either way, "
              f"{len(engine['no_inbound'])} with nothing pointing at them")
    graph = data["graph"]
    if "error" in graph:
        print("  graph      unavailable: " + graph["error"])
    else:
        print(f"  graph      {graph['nodes']} nodes, {graph['edges']} edges, {graph['isolated']} isolated, "
              f"{graph['dead_ends']} dead ends, {graph['broken_links']} broken links")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
