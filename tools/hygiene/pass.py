#!/usr/bin/env python3
"""Nightly hygiene pass: deterministic candidates, one judged batch, one note.

Track 2 of `docs/trajectory.md`. The pass is deliberately read-mostly: it never edits an existing
note. It computes what the graph already knows, asks the judge one batched question about the notes the
graph cannot judge, and writes a single `moc`-typed report under `inbox/`.

    python3 tools/hygiene/pass.py                 # dry run: prints the note, writes nothing
    python3 tools/hygiene/pass.py --apply         # writes the note through the engine gate
    python3 tools/hygiene/pass.py --judge-limit 0 # skip the judge entirely (deterministic only)

Cost when applied: one judge call per 100 candidates (measured 0.76 s for 100) and one note write.
Exit codes: 0 done, 1 the pass could not complete, 2 the note would not validate.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tb-mcp-rs/scripts/lib"))
sys.path.insert(0, str(REPO / "tools/judgement"))
from mcp_stdio import StdioMcp, is_error, tool_payload, tool_text  # noqa: E402

ADAPTER = os.environ.get("BRAIN_BIN", "/usr/local/bin/2brain")
VAULT = os.environ.get("BRAIN_VAULT", "/root/vault")
JUDGE_LIMIT = 200  # the engine caps graph_health lists at 200

QUESTION = {
    "type": "noul",
    "instructions": (
        "Is this note self-contained reference material that needs no links from or to other notes? "
        "A note that belongs in a topic map — a pattern, an insight, a decision that other notes should "
        "cite — is NOT self-contained. An excerpt, a manual page or a standalone definition is."
    ),
    "criteria": {
        "true": "self-contained: no links needed",
        "false": "belongs in a topic map and should be linked",
    },
}


def engine_reads(socket_hint: str | None) -> dict:
    target = socket_hint or VAULT
    with StdioMcp([ADAPTER, "mcp", target], name="hygiene") as client:
        reply = client.call("au_graph_health", {"limit": JUDGE_LIMIT})
        if is_error(reply):
            raise SystemExit(
                f"graph_health refused: {tool_text(reply)}\n"
                "run with HOME=<the daemon's home> (root: HOME=/root) or pass --socket"
            )
        health = tool_payload(reply)
        candidates = health.get("graph_health", health).get("isolated", [])
        # `isolated` is computed over every catalogued file, and most of them are not notes:
        # SKILL.md, settings.json, shell scripts and tooling configs are legitimately unlinked.
        # A typed note needs a `type:` in frontmatter, so that is the filter that makes the
        # candidate set mean "a note nobody links", not "a file nobody links".
        typed = []
        prefix = VAULT.rstrip("/") + "/"
        for note in candidates:
            # graph_health reports absolute paths; the file tools take a path relative to the member.
            relative = note["path"][len(prefix):] if note["path"].startswith(prefix) else note["path"]
            front = client.call("au_frontmatter", {"repo": "vault", "path": relative})
            if is_error(front):
                continue
            payload = tool_payload(front)
            value = payload.get("frontmatter") or {}
            if isinstance(value, dict) and value.get("type"):
                excerpt = ""
                body = client.call("au_content", {"repo": "vault", "path": relative})
                if not is_error(body):
                    text = tool_payload(body).get("content", {}).get("text", "")
                    excerpt = text[:600]
                typed.append(dict(note, path=relative, type=value["type"],
                                  description=str(value.get("description", ""))[:200],
                                  excerpt=excerpt))
        merged = health.get("graph_health", health)
        merged["isolated_typed"] = typed
        return merged


def judge_isolated(notes: list[dict], model: str) -> tuple[dict[str, float], str]:
    """One question per note, batched by 100 (the measured safe batch)."""
    from audit import (
        DEFAULT_ENDPOINT,
        DEFAULT_MODEL,
        load_project_env,
        noul_probability,
        pick_answer,
        post_systemone,
    )

    key = os.environ.get("SUBQ_API_KEY")
    if not key:
        load_project_env(__file__)
        key = os.environ.get("SUBQ_API_KEY")
    if not key:
        raise SystemExit("no SUBQ_API_KEY: run with --judge-limit 0 for the deterministic pass")

    results: dict[str, float] = {}
    batch_id = ""
    for start in range(0, len(notes), 100):
        chunk = notes[start : start + 100]
        # An excerpt, not just the description: the question is about the note's nature, and a
        # description alone reads as "this is a note" for anything. Measured 2026-11-16: without an
        # excerpt every isolated note scored p≈0.98 (all "self-contained"), which is the same
        # collapse the staleness experiment hit when the material was too thin to judge.
        state = "\n\n".join(
            f"Note {i}\npath: {note['path']}\ntype: {note.get('type', '?')}\n"
            f"description: {note.get('description', '')}\n---\n{note.get('excerpt', '')}"
            for i, note in enumerate(chunk)
        )
        questions = {f"note_{i}": dict(QUESTION) for i in range(len(chunk))}
        body = {"model": model or DEFAULT_MODEL, "state": state, "questions": questions}
        reply = post_systemone(DEFAULT_ENDPOINT, body, key, timeout=120.0)
        batch_id = str(reply.get("id") or reply.get("batch_id") or "")
        for i, note in enumerate(chunk):
            # The wire helpers live in audit.py: one place that knows the answer envelope.
            probability = noul_probability(pick_answer(reply, f"note_{i}"))
            if probability is not None:
                results[note["path"]] = probability
    return results, batch_id


def render(health: dict, judged: dict[str, float], batch_id: str) -> str:
    counts = health.get("counts", {})
    today = dt.date.today().isoformat()
    # The judged set is the typed one: a config file being unlinked is not a hygiene finding.
    isolated = health.get("isolated_typed", [])
    candidates = [
        note for note in isolated
        if judged.get(note["path"], 1.0) <= 0.4  # judge says "not self-contained"
    ]
    lines = [
        "---",
        f"id: hygiene-{today}",
        f"description: \"hygiene pass {today}: {counts.get('nodes')} nodes, {counts.get('edges')} edges, "
        f"{counts.get('isolated')} unlinked files ({len(isolated)} typed notes), "
        f"{len(candidates)} worth linking\"",
        "type: moc",
        f"created: {today}",
        "status: current",
        "topics: []",
        f"generated_from: \"tools/hygiene/pass.py; judge batch {batch_id or 'none'}\"",
        "---",
        "",
        f"# Hygiene pass — {today}",
        "",
        "Deterministic, from `au_graph_health`:",
        "",
        "| metric | value |",
        "|---|---:|",
    ]
    for key in ("nodes", "edges", "components", "isolated", "sources", "dead_ends", "cycles", "broken_links"):
        lines.append(f"| {key} | {counts.get(key, '—')} |")
    lines += [
        "",
        f"Judged: {len(judged)} isolated notes, one batched question "
        "(self-contained vs belongs in a topic map). Threshold 0.4 on \"self-contained\".",
        "",
    ]
    if not candidates:
        lines.append("No isolated note needs a link this pass.")
    else:
        lines += ["Notes the judge says belong in a topic map:", ""]
        for note in candidates:
            lines.append(f"- `{note['path']}` — self-contained p={judged.get(note['path'], 0):.2f}")
    lines += [
        "",
        "This report is a proposal. Nothing was edited: linking a note is a human (or FSM PROPOSE) step.",
        "",
    ]
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="write the note (default: dry run)")
    parser.add_argument("--socket", help="engine socket or workspace (default: /root/vault)")
    parser.add_argument("--judge-limit", type=int, default=JUDGE_LIMIT)
    parser.add_argument("--model", default="")
    args = parser.parse_args(argv[1:])

    health = engine_reads(args.socket)
    print(f"graph: {json.dumps(health.get('counts', {}))}", file=sys.stderr)

    judged: dict[str, float] = {}
    batch_id = ""
    isolated = (health.get("isolated_typed") or [])[: args.judge_limit] if args.judge_limit else []
    if isolated:
        judged, batch_id = judge_isolated(isolated, args.model)
        print(f"judged {len(judged)} isolated notes in batch {batch_id!r}", file=sys.stderr)

    note = render(health, judged, batch_id)
    if not args.apply:
        print(note)
        return 0

    path = f"inbox/hygiene-{dt.date.today().isoformat()}.md"
    with StdioMcp([ADAPTER, "mcp", VAULT, "--allow-mutations"], name="hygiene-write") as client:
        check = client.call("au_validate_value", {"type_name": "moc", "value": {
            "description": "hygiene pass", "topics": []}})
        if is_error(check):
            print(f"the moc type rejected a minimal value: {tool_text(check)}", file=sys.stderr)
            return 2
        try:
            current = client.call("au_content", {"repo": "vault", "path": path})
            expected = tool_payload(current).get("content", {}).get("hash") if not is_error(current) else None
        except Exception:  # noqa: BLE001 - absent file is the normal first run
            expected = None
        # A new file is `au_create_file` (no hash exists); an existing one is `au_write_file` and
        # must carry the hash from its own read — the gate refuses a write without it.
        if expected:
            reply = client.call("au_write_file", {
                "repo": "vault", "path": path, "content": note, "expected_hash": expected,
            })
        else:
            reply = client.call("au_create_file", {"repo": "vault", "path": path, "content": note})
        if is_error(reply):
            print(f"write refused: {tool_text(reply)}", file=sys.stderr)
            return 1
        payload = tool_payload(reply)
        print(f"wrote {path} commit={payload.get('commit')} reflected={payload.get('reflected')}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
