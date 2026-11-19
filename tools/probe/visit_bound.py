#!/usr/bin/env python3
"""Measure how much of the corpus one read walks, at three corpus sizes.

Entry 1 of `docs/research/kutha-ideas-2026-11-16.md` asks a question a latency
number cannot answer: does a read's WORK stay bounded as the corpus grows, or
does it grow with the corpus while the answer stays one page? The engine reports
the answer per verb: `graph_health` with `verbs: true` carries, beside its
payload, how many documents each answered request visited and how many posting
blocks it decoded. This probe reads that counter around every query, on three
copies of the live vault, and fits the slope of visits against note count.

    python3 tools/probe/visit_bound.py                 # the table, on stdout
    python3 tools/probe/visit_bound.py --json out.json # the raw per-query rows

Safety, by construction:

- the live vault is only ever READ, by `cp -a`; every request goes to a daemon
  this probe started on a copy under `--work`, with its own `HOME`;
- the probe refuses an entry that resolves to the live vault, and it never sends
  a mutation verb;
- it stops only the daemons it started, through their own copy's socket.

The truncation is NESTED: the 256-note corpus is a strict subset of the 512-note
one, and both are subsets of the full copy, so a difference between sizes is the
corpus GROWING, never the corpus changing shape. The order the notes are kept in
is a deterministic spread over the sorted relative paths (a bit-reversal
permutation), so the kept set is reproducible from the corpus alone.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path

# One frame may not exceed this, matching the engine's own bound.
MAX_FRAME = 16 * 1024 * 1024

# How long a cold daemon may take to bind its socket on a 1 000-note corpus.
READY_TIMEOUT = 300.0

# The five read verbs this probe measures, and why these five: each has a served
# code path that examines more than one document (a posting walk, a byte scan, a
# backlink fold, a graph walk). Single-document reads (`content`, `frontmatter`,
# `type`) are excluded because their work cannot grow with the corpus, and
# `diagnostics` / `files` / `overview` are excluded because their ANSWERS are
# corpus-sized by definition, so a growing visit count would say nothing. The
# served binary on this host predates the per-verb `verbs` field, so traffic
# cannot rank the catalog; the selection is by code path, stated rather than
# measured.
VERBS = ("search", "grep", "references_in", "neighborhood", "hubs")

# Two more reads are measured, and reported apart: their ANSWER is corpus-sized
# (the whole graph), so a visit count that grows with the corpus is the contract,
# not a defect — they are the control that shows what "bounded by the answer"
# looks like at the other extreme. `graph_health` itself is deliberately NOT
# here: reading its counter to sample a delta runs the same closure twice, so its
# own row cannot be measured without measuring the measurement.
CLOSURE_VERBS = ("graph_shape", "overview")

# The fixed query set: 20 text queries spanning the document-frequency bands of
# the full corpus on 2026-11-16 (`.md` files holding the term), so the same set
# is meaningful at every size. `df` is recorded for the reader, not used here.
# Ten single terms, six two-term ORs, two three-term ORs, one conjunction past
# the punctuation split (`error->fix`), and one explicit AND.
TEXT_QUERIES: list[dict] = [
    {"name": "wikilink", "query": "wikilink"},          # df 0.3%
    {"name": "mutation", "query": "mutation"},          # df 1.7%
    {"name": "daemon", "query": "daemon"},              # df 4.1%
    {"name": "page", "query": "page"},                  # df 4.5%
    {"name": "note", "query": "note"},                  # df 5.3%
    {"name": "engine", "query": "engine"},              # df 11.8%
    {"name": "index", "query": "index"},                # df 15.0%
    {"name": "decision", "query": "decision"},          # df 23.7%
    {"name": "graph", "query": "graph"},                # df 30.4%
    {"name": "agent", "query": "agent"},                # df 61.2%
    {"name": "daemon status", "query": "daemon status"},
    {"name": "bm25 ranking", "query": "bm25 ranking"},
    {"name": "search index", "query": "search index"},
    {"name": "pattern decision", "query": "pattern decision"},
    {"name": "error fix", "query": "error fix"},
    {"name": "graph type", "query": "graph type"},
    {"name": "search index cache", "query": "search index cache"},
    {"name": "knowledge graph type", "query": "knowledge graph type"},
    {"name": "error->fix exact", "query": "error->fix", "exact": True},
    {"name": "search index AND", "query": "search index", "min_should_match": 2},
]


def exchange(socket_path: Path, request: dict) -> dict:
    """One frame out, one frame in. The same framing `engine_probe.py` uses."""
    body = json.dumps(request).encode()
    if len(body) > MAX_FRAME:
        raise SystemExit(f"request frame too large: {len(body)} bytes")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(120)
        sock.connect(str(socket_path))
        sock.sendall(struct.pack(">I", len(body)) + body)
        (length,) = struct.unpack(">I", read_exactly(sock, 4))
        if length > MAX_FRAME:
            raise SystemExit(f"response frame too large: {length} bytes")
        return json.loads(read_exactly(sock, length))


def read_exactly(sock: socket.socket, count: int) -> bytes:
    chunks = []
    remaining = count
    while remaining:
        chunk = sock.recv(remaining)
        if not chunk:
            raise SystemExit("engine closed the connection mid-frame")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def counters(socket_path: Path, verb: str) -> tuple[int, int]:
    """`(documents visited, posting blocks decoded)` for one verb, right now."""
    reply = exchange(
        socket_path, {"read": "graph_health", "id": 1, "limit": 1, "verbs": True}
    )
    if reply.get("type") != "response" or not reply.get("ready"):
        raise SystemExit(f"graph_health refused: {reply.get('error')}")
    row = reply["result"]["verbs"][verb]
    return int(row["documents_visited"]), int(row["posting_blocks_decoded"])


def answer_size(verb: str, result: dict) -> int:
    """How big the answer was, so a reader can see it does not track the corpus."""
    payload = result.get(verb)
    if verb in ("search", "grep"):
        return int(payload["total"])
    if verb == "neighborhood":
        return len(payload.get("nodes") or [])
    if verb == "graph_shape":
        return int(payload["node_count"])
    if verb == "overview":
        return int(payload["graph_shape"]["node_count"])
    return len(payload)


def measure(socket_path: Path, verb: str, request: dict) -> dict:
    """One request, with the verb's visit counters sampled either side of it."""
    docs_before, blocks_before = counters(socket_path, verb)
    reply = exchange(socket_path, {"id": 1, **request})
    if reply.get("type") != "response" or not reply.get("ready"):
        raise SystemExit(f"{verb} {request} refused: {reply.get('error')}")
    docs_after, blocks_after = counters(socket_path, verb)
    return {
        "verb": verb,
        "request": request,
        "documents": docs_after - docs_before,
        "posting_blocks": blocks_after - blocks_before,
        "answer": answer_size(verb, reply["result"]),
    }


def run(command: list[str], **kwargs) -> subprocess.CompletedProcess:
    return subprocess.run(command, check=False, **kwargs)


def notes_of(root: Path) -> list[str]:
    """Every markdown note, as a sorted relative path: the corpus this probe sizes."""
    return sorted(
        str(path.relative_to(root))
        for path in root.rglob("*.md")
        if ".git" not in path.parts
    )


def bit_reverse(value: int, bits: int) -> int:
    out = 0
    for _ in range(bits):
        out = (out << 1) | (value & 1)
        value >>= 1
    return out


def spread_order(notes: list[str]) -> list[str]:
    """A deterministic, evenly spread permutation: `order[:n]` is the n-note corpus.

    Sorting by the bit-reversed index walks the sorted list in successively finer
    strides, so a prefix covers every directory in proportion instead of taking a
    directory prefix. Prefixes NEST by construction: `order[:256]` is inside
    `order[:512]`, which is the property that makes a size series a size series.
    """
    bits = max(1, (len(notes) - 1).bit_length())
    order = sorted(range(len(notes)), key=lambda i: bit_reverse(i, bits))
    return [notes[i] for i in order]


def copy_vault(live: Path, dest: Path) -> None:
    """`cp -a` the live vault: the only access this probe makes to it, read-only."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    result = run(["cp", "-a", str(live), str(dest)])
    if result.returncode != 0:
        raise SystemExit(f"cp -a {live} {dest} failed")


def truncate(dest: Path, notes: list[str], keep: set[str]) -> int:
    """Delete every note outside `keep`, and report how many remain."""
    removed = 0
    for relative in notes:
        if relative in keep:
            continue
        path = dest / relative
        if path.exists():
            path.unlink()
            removed += 1
    return len(notes) - removed


def seed_path(root: Path, notes: list[str]) -> str:
    """The one path the `references_in` and `neighborhood` queries address.

    Chosen from the SMALLEST corpus, so the same relative path exists at every
    size: the note the other notes link to most, counted with the vault's own
    `[[wikilink]]` spelling. Deterministic: the highest count, then the path.
    """
    import re

    link = re.compile(r"\[\[([^\]|#^]+)")
    referenced: dict[str, int] = {}
    for relative in notes:
        try:
            text = (root / relative).read_text(errors="ignore")
        except OSError:
            continue
        for target in link.findall(text):
            referenced[target.strip()] = referenced.get(target.strip(), 0) + 1
    by_stem = {Path(relative).stem: relative for relative in notes}
    best = max(
        ((count, by_stem[stem]) for stem, count in referenced.items() if stem in by_stem),
        default=(0, notes[0]),
    )
    return best[1]


def start_engine(binary: Path, entry: Path, home: Path, log_path: Path) -> tuple:
    """Start a private daemon on `entry` and wait for its socket."""
    home.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, HOME=str(home))
    log = log_path.open("wb")
    process = subprocess.Popen(
        [str(binary), "daemon", "start", str(entry)],
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    deadline = time.time() + READY_TIMEOUT
    while time.time() < deadline:
        if process.poll() is not None:
            log.close()
            raise SystemExit(
                f"engine for {entry} exited before binding a socket: see {log_path}"
            )
        # Both spellings of the per-user state directory: `~/.2brain` is the
        # engine's historical name and `~/.2brain` the served one, and a fresh
        # HOME holds whichever the binary writes, so the probe follows the binary
        # rather than assuming one.
        for state in (".2brain", ".2brain"):
            for candidate in sorted((home / state / "tb-engine" / "run").glob("*.sock")):
                return process, candidate, log
        time.sleep(0.1)
    process.terminate()
    log.close()
    raise SystemExit(f"engine for {entry} bound no socket within {READY_TIMEOUT:.0f}s")


def stop_engine(binary: Path, entry: Path, home: Path, process, log) -> None:
    """Ask the daemon to stop over its own socket, then make sure the process is gone."""
    env = dict(os.environ, HOME=str(home))
    run([str(binary), "daemon", "stop", str(entry)], env=env, capture_output=True, timeout=120)
    try:
        process.wait(timeout=30)
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=15)
    log.close()


def fit(points: list[tuple[float, float]]) -> tuple[float, float]:
    """Least-squares slope and coefficient of determination for (x, y) points."""
    count = len(points)
    mean_x = sum(x for x, _ in points) / count
    mean_y = sum(y for _, y in points) / count
    sxx = sum((x - mean_x) ** 2 for x, _ in points)
    sxy = sum((x - mean_x) * (y - mean_y) for x, y in points)
    if sxx == 0:
        return 0.0, 0.0
    slope = sxy / sxx
    intercept = mean_y - slope * mean_x
    ss_tot = sum((y - mean_y) ** 2 for _, y in points)
    ss_res = sum((y - (intercept + slope * x)) ** 2 for x, y in points)
    r2 = 1.0 - ss_res / ss_tot if ss_tot else 1.0
    return slope, r2


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "tb-engine/target/release/tb-engine",
        help="the engine binary to start on each copy (default: the workspace release build)",
    )
    parser.add_argument(
        "--live", type=Path, default=Path("/root/vault"), help="the vault to copy"
    )
    parser.add_argument(
        "--work", type=Path, default=Path("/tmp/visit-bound"), help="where copies and HOMEs go"
    )
    parser.add_argument(
        "--sizes",
        default="256,512",
        help="truncation targets; the full copy is always the last size",
    )
    parser.add_argument("--json", type=Path, help="write the raw per-query rows here")
    parser.add_argument(
        "--keep", action="store_true", help="reuse copies already under --work"
    )
    args = parser.parse_args()

    binary = args.binary.resolve()
    live = args.live.resolve()
    work = args.work.resolve()
    if not binary.exists():
        raise SystemExit(f"no engine binary at {binary}")
    if not live.is_dir():
        raise SystemExit(f"no vault at {live}")
    if work == live or live in work.parents or work in live.parents:
        raise SystemExit("--work must not overlap the live vault")

    truncations = [int(size) for size in args.sizes.split(",") if size]

    print(f"engine   {binary}", file=sys.stderr)
    print(f"vault    {live} (read-only, copied)", file=sys.stderr)

    # The master copy: one read of the live vault, every other copy made from it.
    master = work / "corpus"
    if not master.exists() or not args.keep:
        if master.exists():
            shutil.rmtree(master)
        copy_vault(live, master)
        print(f"copied   {live} -> {master}", file=sys.stderr)
    notes = notes_of(master)
    order = spread_order(notes)

    sizes: list[tuple[int, Path]] = []
    for target in truncations:
        dest = work / f"corpus-{target}"
        if dest.exists():
            shutil.rmtree(dest)
        copy_vault(master, dest)
        kept = truncate(dest, notes, set(order[:target]))
        sizes.append((kept, dest))
        print(f"truncated {dest} to {kept} notes", file=sys.stderr)
    sizes.append((len(notes), master))

    # The seed path must exist in the smallest copy, hence in every copy.
    smallest = min(sizes, key=lambda pair: pair[0])
    seed = seed_path(smallest[1], notes_of(smallest[1]))
    print(f"seed     {seed}", file=sys.stderr)

    rows: list[dict] = []
    for count, root in sizes:
        home = work / f"home-{count}"
        if home.exists():
            shutil.rmtree(home)
        log_path = work / f"engine-{count}.log"
        process, socket_path, log = start_engine(binary, root, home, log_path)
        try:
            for query in TEXT_QUERIES:
                # `exact` and `min_should_match` belong to `search` alone; `grep`
                # rejects an unknown field rather than ignoring it.
                for verb in ("search", "grep"):
                    request = {"read": verb, "query": query["query"], "limit": 10, "offset": 0}
                    if verb == "search":
                        if query.get("exact"):
                            request["exact"] = True
                        if "min_should_match" in query:
                            request["min_should_match"] = query["min_should_match"]
                    row = measure(socket_path, verb, request)
                    row.update(size=count, name=query["name"])
                    rows.append(row)
            for name, verb, request in (
                ("references_in", "references_in", {"read": "references_in", "path": seed}),
                (
                    "neighborhood depth 1",
                    "neighborhood",
                    {"read": "neighborhood", "path": seed, "depth": 1},
                ),
                (
                    # Past depth 1 the walk requires a kind set; the walk is also
                    # capped by `max_nodes`, so this series shows both the fan-out
                    # growth and the cap.
                    "neighborhood depth 2",
                    "neighborhood",
                    {
                        "read": "neighborhood",
                        "path": seed,
                        "depth": 2,
                        "kinds": ["navigational", "contributing", "field"],
                    },
                ),
                ("hubs", "hubs", {"read": "hubs", "limit": 20, "offset": 0}),
                ("graph_shape", "graph_shape", {"read": "graph_shape"}),
                ("overview", "overview", {"read": "overview"}),
            ):
                row = measure(socket_path, verb, request)
                row.update(size=count, name=name)
                rows.append(row)
        finally:
            stop_engine(binary, root, home, process, log)
        print(f"measured {count} notes on {socket_path}", file=sys.stderr)

    report(rows, [count for count, _ in sizes], seed)
    if args.json:
        args.json.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
        print(f"wrote    {args.json}", file=sys.stderr)
    return 0


def report(rows: list[dict], sizes: list[int], seed: str) -> None:
    """Print the per-verb table and the per-query slopes behind it."""
    print(f"\n# documents visited per query, by corpus size (seed {seed})\n")
    print("| verb | " + " | ".join(f"{size} notes" for size in sizes) + " | slope | R^2 |")
    print("|---|" + "---:|" * len(sizes) + "---:|---:|")
    for verb in VERBS + CLOSURE_VERBS:
        means = []
        for size in sizes:
            matching = [
                row
                for row in rows
                if row["verb"] == verb and row["size"] == size and row["answer"] > 0
            ]
            # A query the smaller corpus cannot answer visits nothing, and its
            # zero would drag the mean down for a reason that is not the read.
            means.append(
                sum(row["documents"] for row in matching) / len(matching)
                if matching
                else 0.0
            )
        slope, r2 = fit(list(zip(sizes, means)))
        print(
            f"| `{verb}` | "
            + " | ".join(f"{mean:.1f}" for mean in means)
            + f" | {slope:.2f} | {r2:.2f} |"
        )
    print("\n# posting blocks decoded per query, min–max over each verb's query set\n")
    print("| verb | " + " | ".join(f"{size} notes" for size in sizes) + " |")
    print("|---|" + "---:|" * len(sizes))
    for verb in VERBS + CLOSURE_VERBS:
        cells = []
        for size in sizes:
            values = [
                row["posting_blocks"]
                for row in rows
                if row["verb"] == verb and row["size"] == size
            ]
            low, high = min(values), max(values)
            cells.append(f"{low}" if low == high else f"{low}–{high}")
        print(f"| `{verb}` | " + " | ".join(cells) + " |")
    print("\n# per-query slopes (documents visited against note count)\n")
    print("| verb | query | " + " | ".join(str(size) for size in sizes) + " | slope |")
    print("|---|---|" + "---:|" * len(sizes) + "---:|")
    for verb in VERBS + CLOSURE_VERBS:
        names = []
        for row in rows:
            if row["verb"] == verb and row["name"] not in names:
                names.append(row["name"])
        for name in names:
            series = []
            for size in sizes:
                found = [
                    row
                    for row in rows
                    if row["verb"] == verb and row["name"] == name and row["size"] == size
                ]
                series.append(found[0] if found else None)
            points = [(row["size"], row["documents"]) for row in series if row]
            slope, _ = fit(points)
            print(
                f"| `{verb}` | {name} | "
                + " | ".join(f"{row['documents']}" if row else "-" for row in series)
                + f" | {slope:.2f} |"
            )


if __name__ == "__main__":
    sys.exit(main())
