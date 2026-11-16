#!/usr/bin/env python3
"""Measure what a scoped `search` pays for the documents it will not return.

Entry 1 of `docs/research/ruvector-adr-2026-11-16.md`: a selective predicate
(`repo` / `scope`) decides only what a read RETURNS, not what it SCORES. This
probe sizes that hole in the engine's own numbers on our corpus, before any code
moves, and provides the same measurement afterwards:

    python3 tools/probe/candidate_set.py                 # the table, on stdout
    python3 tools/probe/candidate_set.py --json out.json # the raw per-query rows
    python3 tools/probe/candidate_set.py --capture /tmp/before

Per query and scope it records what the engine reports it walked
(`graph_health { verbs: true }`, sampled either side of the request), the answer's
`total`, and the client-side wall time. The 20 queries are `visit_bound.py`'s own
fixed set, so the two probes answer their questions over one query set.

Safety, by construction: the live vault is only ever READ, by `cp -a`; every
request goes to a daemon this probe started on a copy under `--work`, with its own
`HOME` and registry (the copy's entry re-pointed at the copy); no mutation verb is
ever sent; the probe stops only the daemon it started, through that copy's socket.

`--capture DIR` writes every answered page under DIR, normalized (`$VAULT` for the
workspace root, sorted keys) and paged to the end, so the same capture taken before
and after a change is a byte-for-byte comparison of the answer sets rather than of
the first page.

The scope census (`files` per member) is what the visits are measured against: the
ratio own/all in visits is the wasted work, and the in-scope share of the catalogue
is what says how much of it a candidate mask could remove.

`--against SECOND-BINARY` is the paired mode, and the one a before/after claim
should rest on. Two binaries run at the same time on two identical copies of the
same frozen corpus; each query is asked on both, alternating which engine answers
first inside each round, and every answered page is captured from both and
diffed. Two runs in sequence compare two different machines — this host carries
other agents' work, and a drift between the blocks would be attributed to the
change.
"""

from __future__ import annotations

import argparse
import json
import shutil
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from visit_bound import (  # noqa: E402
    TEXT_QUERIES,
    copy_vault,
    counters,
    exchange,
    run,
    start_engine,
    stop_engine,
)

# The two scopes every query is asked at. `own` is `search`'s own default (the
# scope table in `serve.rs`), so this is not a contrived comparison: it is the
# request an agent actually sends against the request that widens it to every
# mounted member.
SCOPES = ("own", "all")

# The device registry the engine resolves members through. A copy needs its own
# `HOME` and a registry with the one entry it owns re-pointed at the copy;
# leaving it at `/root/vault` would make the private daemon index the live vault.
STATE_REGISTRY = Path("/root/.2brain/au-engine/config/repos.yaml")

# The page size the capture pages with: `SEARCH_MAX_LIMIT`, so the fewest requests
# cover the whole answer.
CAPTURE_PAGE = 100

# The page size the TIMED runs ask for. The ranking work a scoped read does is
# independent of the page size (the answer is ranked whole, then paged), so one
# match keeps the frame and its excerpts out of a measurement that is meant to be
# about the ranking.
TIMED_PAGE = 1

# The `repo`-pinned variants the capture adds: a `repo` engages the same scope
# predicate as `scope: own`, and a dependency pinned under `own` is the one case
# where the two halves of `in_read_scope` disagree (the name matches, the role
# does not).
PINNED = [("entry repo", "vault", "own"), ("dependency repo", "au-mcp-sdk", "own")]

# Two warm-up requests per (query, scope) before any measurement: the first frame
# pays a page-fault and a cold socket.
WARMUP = 2


def seed_registry(live: Path, copy: Path, home: Path) -> int:
    """Write the copy's own registry, and report how many entries moved.

    Two rewrites, because the entry a copy owns is named `vault` in every
    registry this host has even when the `--live` source is a pre-built copy of
    it: the entry named `vault` is re-pointed at the copy, and any other entry
    whose path IS the live source is re-pointed too. Leaving either at the served
    path would let the private daemon index the live vault.
    """
    config = home / ".2brain/au-engine/config"
    config.mkdir(parents=True, exist_ok=True)
    if not STATE_REGISTRY.exists():
        # An empty registry mounts the entry alone, so `scope: all` would have
        # nothing to widen to and the measurement would be a tautology.
        raise SystemExit(f"no registry at {STATE_REGISTRY}: every scope would answer the same")
    lines = []
    moved = 0
    owner = False
    for line in STATE_REGISTRY.read_text().splitlines():
        if line.strip() == "- name: vault":
            owner = True
        elif owner and line.strip().startswith("path:"):
            line = f"    path: {copy}"
            owner = False
            moved += 1
        elif line.strip().startswith("path:") and line.split("path:", 1)[1].strip() == str(live):
            line = line.replace(str(live), str(copy))
            moved += 1
        lines.append(line)
    (config / "repos.yaml").write_text("\n".join(lines) + "\n")
    return moved


def search_request(query: dict, scope: str) -> dict:
    """One `search` request: the query's own fields, at one scope."""
    request = {
        "read": "search",
        "id": 1,
        "query": query["query"],
        "scope": scope,
        "limit": CAPTURE_PAGE,
        "offset": 0,
    }
    if query.get("exact"):
        request["exact"] = True
    if "min_should_match" in query:
        request["min_should_match"] = query["min_should_match"]
    return request


def answer(socket_path: Path, request: dict) -> tuple[dict, float]:
    """One request and the client-side wall time it took, in microseconds."""
    started = time.perf_counter()
    reply = exchange(socket_path, request)
    elapsed = (time.perf_counter() - started) * 1e6
    if reply.get("type") != "response" or not reply.get("ready"):
        raise SystemExit(f"{request} refused: {reply.get('error')}")
    return reply["result"], elapsed


def measure_pair(socket_path: Path, query: dict, rounds: int) -> dict[str, dict]:
    """Both scopes of one query: visits, answer total, and an interleaved timing.

    The wall times alternate scope inside each round. Measuring one scope's whole
    block and then the other's would compare two different machines: this host
    carries other work, and a drift between the blocks would land on whichever
    scope ran second. Alternating pairs each sample against the same background,
    and the MINIMUM of the samples is reported beside the median because
    interference can only add time, never remove it.
    """
    rows: dict[str, dict] = {}
    for scope in SCOPES:
        request = search_request(query, scope)
        before = counters(socket_path, "search")
        result, _ = answer(socket_path, request)
        after = counters(socket_path, "search")
        rows[scope] = {
            "documents": after[0] - before[0],
            "posting_blocks": after[1] - before[1],
            "total": int(result["search"]["total"]),
            "samples": [],
        }
    for at in range(rounds):
        order = SCOPES if at % 2 == 0 else tuple(reversed(SCOPES))
        for scope in order:
            timed = {**search_request(query, scope), "limit": TIMED_PAGE}
            _, elapsed = answer(socket_path, timed)
            rows[scope]["samples"].append(elapsed)
    for row in rows.values():
        samples = sorted(row.pop("samples"))
        row["us"] = statistics.median(samples)
        row["us_min"] = samples[0]
        row["us_p25"] = samples[len(samples) // 4]
        row["us_p75"] = samples[(3 * len(samples)) // 4]
    return rows


def census(socket_path: Path) -> dict:
    """Catalogued files per member, and how many the `own` scope keeps."""
    rows: dict[str, int] = {}
    offset = 0
    while True:
        reply = exchange(
            socket_path,
            {"read": "files", "id": 1, "scope": "all", "limit": 200, "offset": offset},
        )
        if reply.get("type") != "response" or not reply.get("ready"):
            raise SystemExit(f"files refused: {reply.get('error')}")
        page = reply["result"]["files"]
        for row in page:
            name = row.get("repo") or "(unowned)"
            rows[name] = rows.get(name, 0) + 1
        if len(page) < 200:
            break
        offset += len(page)
    own = 0
    offset = 0
    while True:
        reply = exchange(
            socket_path,
            {"read": "files", "id": 1, "scope": "own", "limit": 200, "offset": offset},
        )
        page = reply["result"]["files"]
        own += len(page)
        if len(page) < 200:
            break
        offset += len(page)
    return {"files_by_repo": rows, "files_own": own, "files_all": sum(rows.values())}


def capture(socket_path: Path, queries: list[dict], out: Path, root: str) -> int:
    """Every answered page of every request, normalized, for a byte comparison."""
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    written = 0
    requests: list[tuple[str, dict]] = []
    for query in queries:
        for scope in SCOPES:
            requests.append((f"{query['name']}@{scope}", search_request(query, scope)))
    for name, repo, scope in PINNED:
        request = search_request({"name": "engine", "query": "engine"}, scope)
        request["repo"] = repo
        requests.append((f"engine@{name}", request))

    for name, request in requests:
        pages = []
        while True:
            result, _ = answer(socket_path, request)
            pages.append(normalized(result, root))
            next_offset = result.get("next_offset")
            if next_offset is None:
                break
            request = {**request, "offset": next_offset}
        path = out / f"{name.replace('/', '_')}.json"
        path.write_text(json.dumps(pages, indent=1, sort_keys=True) + "\n")
        written += 1
    return written


def normalized(value, root: str):
    """The workspace root as `$VAULT`, keys sorted: the `engine_probe.py` rule."""
    if isinstance(value, dict):
        return {key: normalized(item, root) for key, item in sorted(value.items())}
    if isinstance(value, list):
        return [normalized(item, root) for item in value]
    if isinstance(value, str) and root and root in value:
        return value.replace(root, "$VAULT")
    return value


def report_pair(rows: list[dict], tags: tuple[str, str], census_row: dict) -> None:
    """Two binaries side by side, per query and per scope, then the totals."""
    left, right = tags
    print(f"\n# two binaries, one frozen corpus: {left} then {right}\n")
    print(
        f"| query | scope | visits {left} | visits {right} | "
        f"µs {left} (p25–p75) | µs {right} (p25–p75) | µs {left} min | µs {right} min "
        "| total |"
    )
    print("|---|---|---:|---:|---:|---:|---:|---:|---:|")
    names = list(dict.fromkeys(row["name"] for row in rows if row["tag"] == left))
    for name in names:
        for scope in SCOPES:
            a = next(r for r in rows if r["name"] == name and r["scope"] == scope and r["tag"] == left)
            b = next(r for r in rows if r["name"] == name and r["scope"] == scope and r["tag"] == right)
            print(
                f"| `{name}` | {scope} | {a['documents']} | {b['documents']} | "
                f"{a['us']:.0f} ({a['us_p25']:.0f}–{a['us_p75']:.0f}) | "
                f"{b['us']:.0f} ({b['us_p25']:.0f}–{b['us_p75']:.0f}) | "
                f"{a['us_min']:.0f} | {b['us_min']:.0f} | {a['total']} |"
            )

    def sums(tag: str, scope: str, field: str) -> float:
        return sum(row[field] for row in rows if row["tag"] == tag and row["scope"] == scope)

    for scope in SCOPES:
        print(f"\n## totals at `{scope}`\n")
        print(f"| | {left} | {right} | {right} / {left} |")
        print("|---|---:|---:|---:|")
        for field, label in (
            ("documents", "documents visited"),
            ("posting_blocks", "posting blocks decoded"),
            ("us", "wall time, µs (median of the queries)"),
            ("us_min", "wall time, µs (minimum of the queries)"),
            ("total", "answer totals"),
        ):
            a = sums(left, scope, field)
            b = sums(right, scope, field)
            ratio = f"{b / a:.2f}x" if a else "n/a"
            print(f"| {label} | {a:.0f} | {b:.0f} | {ratio} |")
    if census_row:
        print(
            f"\ncatalogue: {census_row['files_all']} files, {census_row['files_own']} in "
            f"scope at `own` ({100.0 * census_row['files_own'] / census_row['files_all']:.1f}%)"
        )


def report(rows: list[dict], census_row: dict) -> None:
    """Per query, the two scopes side by side; then the totals and their ratio."""
    print("\n# one page per read, 20 queries at two scopes\n")
    print(
        "| query | visits own | visits all | µs own (p25–p75) | µs all (p25–p75) "
        "| µs own min | µs all min | total own | total all |"
    )
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for name in [row["name"] for row in rows if row["scope"] == "own"]:
        own = next(row for row in rows if row["name"] == name and row["scope"] == "own")
        every = next(row for row in rows if row["name"] == name and row["scope"] == "all")
        print(
            f"| `{name}` | {own['documents']} | {every['documents']} | "
            f"{own['us']:.0f} ({own['us_p25']:.0f}–{own['us_p75']:.0f}) | "
            f"{every['us']:.0f} ({every['us_p25']:.0f}–{every['us_p75']:.0f}) | "
            f"{own['us_min']:.0f} | {every['us_min']:.0f} | {own['total']} | {every['total']} |"
        )

    def sums(scope: str, field: str) -> float:
        return sum(row[field] for row in rows if row["scope"] == scope)

    def ratio(field: str) -> str:
        own = sums("own", field)
        every = sums("all", field)
        return f"{every / own:.2f}x" if own else "n/a"

    print("\n# totals over the query set\n")
    print("| | own | all | all / own |")
    print("|---|---:|---:|---:|")
    for field, label in (
        ("documents", "documents visited"),
        ("posting_blocks", "posting blocks decoded"),
        ("us", "wall time, µs (median of the queries)"),
        ("us_min", "wall time, µs (minimum of the queries)"),
        ("total", "answer totals"),
    ):
        print(
            f"| {label} | {sums('own', field):.0f} | {sums('all', field):.0f} "
            f"| {ratio(field)} |"
        )
    if census_row:
        files = census_row["files_all"]
        share = 100.0 * census_row["files_own"] / files if files else 0.0
        print(
            f"\ncatalogue: {files} files, {census_row['files_own']} in scope at `own` "
            f"({share:.1f}%)"
        )
        for name, count in sorted(census_row["files_by_repo"].items()):
            print(f"  {name}: {count}")


def paired(args, binaries: tuple[Path, Path], live: Path, work: Path) -> int:
    """Two binaries, two identical copies, an alternating measurement.

    Each engine runs on its own copy of `live` (same content, own `HOME`, own
    socket), so the corpus is frozen for both. Within each round the two engines
    answer in alternating order, which pairs every sample against the same
    background, and every answered page is captured from both for a byte diff.
    """
    tags = ("before", "after")
    sites = []
    for tag, binary in zip(tags, binaries):
        root = work / f"vault-{tag}"
        home = work / f"home-{tag}"
        for stale in (root, home):
            if stale.exists():
                shutil.rmtree(stale)
        copy_vault(live, root)
        home.mkdir(parents=True)
        seed_registry(live, root, home)
        sites.append((tag, binary, root, home))

    daemons = []
    sockets = {}
    try:
        for tag, binary, root, home in sites:
            process, socket_path, log = start_engine(binary, root, home, work / f"engine-{tag}.log")
            daemons.append((binary, root, home, process, log))
            sockets[tag] = socket_path
            print(f"{tag:7}  {binary} on {root}, socket {socket_path}", file=sys.stderr)

        census_row = census(sockets[tags[0]])
        members = exchange(sockets[tags[0]], {"read": "members", "id": 1})["result"]["members"]
        print(f"members  {len(members)}: " + ", ".join(m["repo"] for m in members), file=sys.stderr)

        for query in TEXT_QUERIES:
            for tag in tags:
                for scope in SCOPES:
                    for _ in range(WARMUP):
                        answer(sockets[tag], search_request(query, scope))

        rows: list[dict] = []
        for query in TEXT_QUERIES:
            measured = {tag: measure_visits(sockets[tag], query) for tag in tags}
            samples = {(tag, scope): [] for tag in tags for scope in SCOPES}
            for at in range(args.rounds):
                order = tags if at % 2 == 0 else tuple(reversed(tags))
                for tag in order:
                    for scope in SCOPES:
                        timed = {**search_request(query, scope), "limit": TIMED_PAGE}
                        _, elapsed = answer(sockets[tag], timed)
                        samples[(tag, scope)].append(elapsed)
            for tag in tags:
                for scope in SCOPES:
                    values = sorted(samples[(tag, scope)])
                    row = measured[tag][scope]
                    row.update(
                        tag=tag,
                        name=query["name"],
                        query=query["query"],
                        scope=scope,
                        us=statistics.median(values),
                        us_min=values[0],
                        us_p25=values[len(values) // 4],
                        us_p75=values[(3 * len(values)) // 4],
                    )
                    rows.append(row)

        reports = []
        for tag, (_tag, _binary, root, _home) in zip(tags, sites):
            out = work / f"capture-{tag}"
            written = capture(sockets[tag], TEXT_QUERIES, out, str(root))
            reports.append((tag, out, written))
    finally:
        for binary, root, home, process, log in daemons:
            stop_engine(binary, root, home, process, log)

    report_pair(rows, tags, census_row)
    print("\n## answer equivalence\n")
    for tag, out, written in reports:
        print(f"- `{tag}`: {written} answered pages under `{out}`")
    first, second = reports[0][1], reports[1][1]
    names = sorted(path.name for path in first.iterdir())
    same = names == sorted(path.name for path in second.iterdir()) and all(
        (first / name).read_bytes() == (second / name).read_bytes() for name in names
    )
    print(
        "\n"
        + (
            "The two capture directories are byte-identical: the answers are the same.\n"
            if same
            else "THE CAPTURES DIFFER: the change is not answer-preserving.\n"
        )
    )
    if args.json:
        args.json.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
        print(f"wrote    {args.json}", file=sys.stderr)
    return 0 if same else 1


def measure_visits(socket_path: Path, query: dict) -> dict[str, dict]:
    """The engine's own visit count and the answer's total, per scope."""
    rows = {}
    for scope in SCOPES:
        before = counters(socket_path, "search")
        result, _ = answer(socket_path, search_request(query, scope))
        after = counters(socket_path, "search")
        rows[scope] = {
            "documents": after[0] - before[0],
            "posting_blocks": after[1] - before[1],
            "total": int(result["search"]["total"]),
        }
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--against",
        type=Path,
        help="a second engine binary to run at the same time on an identical copy",
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "tb-engine/target/release/au",
        help="the engine binary to start on the copy (default: the workspace release build)",
    )
    parser.add_argument(
        "--live", type=Path, default=Path("/root/vault"), help="the vault to copy"
    )
    parser.add_argument(
        "--work", type=Path, default=Path("/tmp/candidate-set"), help="where the copy and HOME go"
    )
    parser.add_argument(
        "--rounds", type=int, default=25, help="timed runs per request, alternating scope"
    )
    parser.add_argument("--json", type=Path, help="write the raw per-query rows here")
    parser.add_argument("--capture", type=Path, help="write every answered page here")
    parser.add_argument("--keep", action="store_true", help="reuse the copy under --work")
    args = parser.parse_args()

    binary = args.binary.resolve()
    live = args.live.resolve()
    work = args.work.resolve()
    if not binary.exists():
        raise SystemExit(f"no engine binary at {binary}")
    if not live.is_dir():
        raise SystemExit(f"no vault at {live}")
    if work == live or live in work.parents or work in work.parents:
        raise SystemExit("--work must not overlap the live vault")
    if args.against is not None:
        against = args.against.resolve()
        if not against.exists():
            raise SystemExit(f"no engine binary at {against}")
        return paired(args, (against, binary), live, work)

    copy = work / "vault"
    home = work / "home"
    if not copy.exists() or not args.keep:
        if copy.exists():
            shutil.rmtree(copy)
        if home.exists():
            shutil.rmtree(home)
        copy_vault(live, copy)
        home.mkdir(parents=True)
        moved = seed_registry(live, copy, home)
        print(f"copied   {live} -> {copy} ({moved} registry entries re-pointed)", file=sys.stderr)

    log_path = work / "engine.log"
    process, socket_path, log = start_engine(binary, copy, home, log_path)
    try:
        members = exchange(socket_path, {"read": "members", "id": 1})["result"]["members"]
        print(f"members  {len(members)}: " + ", ".join(m["repo"] for m in members), file=sys.stderr)
        census_row = census(socket_path)
        print(f"engine   {binary}", file=sys.stderr)
        print(f"socket   {socket_path}", file=sys.stderr)

        for query in TEXT_QUERIES:
            for scope in SCOPES:
                for _ in range(WARMUP):
                    answer(socket_path, search_request(query, scope))

        rows: list[dict] = []
        for query in TEXT_QUERIES:
            pair = measure_pair(socket_path, query, args.rounds)
            for scope, row in pair.items():
                row.update(name=query["name"], query=query["query"], scope=scope)
                rows.append(row)

        if args.capture:
            written = capture(socket_path, TEXT_QUERIES, args.capture.resolve(), str(copy))
            print(f"captured {written} answers into {args.capture}", file=sys.stderr)
    finally:
        stop_engine(binary, copy, home, process, log)

    rows.sort(key=lambda row: ([query["name"] for query in TEXT_QUERIES].index(row["name"]),
                               row["scope"]))
    report(rows, census_row)
    if args.json:
        args.json.write_text(json.dumps(rows, indent=1, sort_keys=True) + "\n")
        print(f"wrote    {args.json}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
