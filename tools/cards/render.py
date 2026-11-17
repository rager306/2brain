#!/usr/bin/env python3
"""Render the repository's cards from `tools/cards/metrics.json`.

The numbers on these cards are measured, not decorative, so they live in one JSON file that records the command
each came from and the date it was taken. This script only draws them: it never invents a value, and a card that
would have to guess is not drawn at all.

    python3 tools/cards/render.py            # writes assets/cards/*.svg
    python3 tools/cards/render.py --check    # fails when the SVG on disk disagrees with the JSON

Each card is drawn twice, `-light.svg` and `-dark.svg`, so a reader's theme picks the palette the way GitHub's
own cards do.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
METRICS = ROOT / "tools/cards/metrics.json"
OUT = ROOT / "assets/cards"

THEMES = {
    "light": {"bg": "#ffffff", "border": "#d0d7de", "title": "#1f2328", "label": "#59636e", "value": "#1f2328", "accent": "#0969da"},
    "dark": {"bg": "#0d1117", "border": "#30363d", "title": "#e6edf3", "label": "#9198a1", "value": "#e6edf3", "accent": "#4493f8"},
}

WIDTH = 420


def escape(text: str) -> str:
    return (text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))


def card(title: str, rows: list[tuple[str, str]], footnote: str, theme: str) -> str:
    palette = THEMES[theme]
    height = 58 + 22 * len(rows) + (20 if footnote else 0)
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" viewBox="0 0 {WIDTH} {height}" role="img" aria-label="{escape(title)}">',
        f'<rect x="0.5" y="0.5" width="{WIDTH - 1}" height="{height - 1}" rx="10" fill="{palette["bg"]}" stroke="{palette["border"]}"/>',
        f'<text x="20" y="32" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, Noto Sans, Helvetica, Arial, sans-serif" font-size="14" font-weight="600" fill="{palette["title"]}">{escape(title)}</text>',
    ]
    y = 58
    for label, value in rows:
        parts.append(
            f'<text x="20" y="{y}" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, Noto Sans, Helvetica, Arial, sans-serif" font-size="12" fill="{palette["label"]}">{escape(label)}</text>'
            f'<text x="{WIDTH - 20}" y="{y}" text-anchor="end" font-family="ui-monospace, SFMono-Regular, SF Mono, Menlo, Consolas, monospace" font-size="12" font-weight="600" fill="{palette["value"]}">{escape(value)}</text>'
        )
        y += 22
    if footnote:
        parts.append(
            f'<text x="20" y="{y + 4}" font-family="-apple-system, BlinkMacSystemFont, Segoe UI, Noto Sans, Helvetica, Arial, sans-serif" font-size="10" fill="{palette["accent"]}">{escape(footnote)}</text>'
        )
    parts.append("</svg>")
    return "\n".join(parts) + "\n"


def cards(metrics: dict) -> dict[str, tuple[str, list[tuple[str, str]], str]]:
    gates = metrics["gates"]
    surface = metrics["surface"]
    ranking = metrics["ranking"]
    memory = metrics["memory"]
    kb = metrics["knowledge_base"]
    graph_metrics = metrics["graph"]
    known = sum(gate.get("known_environment_failures", 0) for gate in gates.values())
    return {
        "gates": (
            "Tests, counted",
            [
                ("adapter", f'{gates["adapter_tests"]["value"]}'),
                ("engine, unit", f'{gates["engine_lib_tests"]["value"]}'),
                ("engine, end to end", f'{gates["engine_it_tests"]["value"]}'),
                ("known environment failures", f"{known} (root-only)"),
            ],
            f'taken {gates["adapter_tests"]["date"]}, commands in tools/cards/metrics.json',
        ),
        "surface": (
            "The MCP surface",
            [
                ("tools", f'{surface["tools"]}'),
                ("read / mutate", f'{surface["read"]} / {surface["mutate"]}'),
                ("discovery / watch", f'{surface["discover"]} / {surface["watch"]}'),
                ("wire schema", f'{surface["wire_schema"]}'),
                ("binaries, languages on the request path", f'{surface["binaries"]}, {surface["languages_on_request_path"]}'),
            ],
            "one Rust binary serves reads, mutations and resources",
        ),
        "knowledge_base": (
            "The knowledge base itself",
            [
                ("notes / of them drafts", f'{kb["notes"]} / {kb["drafts"]}'),
                ("typed / sourced", f'{100 * kb["typed"] // kb["notes"]}% / {100 * kb["sourced"] // kb["notes"]}%'),
                ("links, median per note", f'{kb["edges"]} · {kb["outbound_median"]:.0f}'),
                ("no link at all / nothing points at it", f'{kb["notes_without_links"]} / {kb["zero_inbound"]}'),
            ],
            "measured by tools/kb/census.py, weekly",
        ),
        "graph": (
            "The graph's shape",
            [
                ("nodes / edges", f'{graph_metrics["nodes"]} / {graph_metrics["edges"]}'),
                ("bridges", f'{graph_metrics["bridges"]}'),
                ("articulation points", f'{graph_metrics["articulation_points"]}'),
                ("triangles, transitivity", f'{graph_metrics["triangles"]} · {graph_metrics["transitivity"]:.3f}'),
                ("isolated / sources", f'{graph_metrics["isolated"]} / {graph_metrics["sources"]}'),
            ],
            f'taken {graph_metrics["date"]}, after the .2brain migration',
        ),
        "memory": (
            "One vault, held in memory",
            [
                ("notes / megabytes", f'{memory["vault_notes"]} / {memory["vault_bytes"] / 1_000_000:.1f} MB'),
                ("engine resident", f'{memory["daemon_rss_mib"]:.1f} MiB'),
                ("index, bytes per document", f'{memory["index_bytes_per_document"]}'),
                ("rebuilt from files, not stored", "yes"),
            ],
            "the files are the source of truth; the engine holds a derived graph",
        ),
        "ranking": (
            "Ranking, measured",
            [
                ("literal queries (7), hit@1", f'{ranking["literal"]["hit_at_1"]:.3f}'),
                ("topical queries (53), hit@1", f'{ranking["topical"]["hit_at_1"]:.3f}'),
                ("durable pairs (26), hit@1", f'{ranking["durable"]["hit_at_1"]:.3f}'),
                ("index, bytes per document", f'{ranking["index_bytes_per_document"]} of {ranking["budget_bytes_per_document"]}'),
                ("search latency, p50 / p95", f'{ranking["search_p50_us"]} / {ranking["search_p95_us"]} µs'),
            ],
            "BM25 with coverage weighting and phrase adjacency",
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail when the SVGs on disk differ from the JSON")
    args = parser.parse_args()

    metrics = json.loads(METRICS.read_text())
    OUT.mkdir(parents=True, exist_ok=True)
    stale = []
    for name, (title, rows, footnote) in cards(metrics).items():
        for theme in THEMES:
            path = OUT / f"{name}-{theme}.svg"
            text = card(title, rows, footnote, theme)
            if args.check:
                if not path.exists() or path.read_text() != text:
                    stale.append(path.name)
            else:
                path.write_text(text)
    if args.check:
        if stale:
            print("cards are stale: " + ", ".join(sorted(stale)), file=sys.stderr)
            return 1
        print("cards match tools/cards/metrics.json")
    else:
        print(f"wrote {len(cards(metrics)) * 2} cards to {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
