# Fifth engine swap: `au_search` into the served daemon

**The procedure is scripted: `au-engine/scripts/swap-engine.sh`.** This document is the reasoning and
the rehearsal record; the script is the execution, because the risky part is the ordering, not the
decisions.

```sh
cd /root/arsumbris/au-engine
./scripts/swap-engine.sh --check      # preconditions; changes nothing
./scripts/swap-engine.sh              # install + restart + compare + auto-rollback on any failure
./scripts/swap-engine.sh --restore    # put the backed-up binary back and restart
```

The script encodes the properties this document argues for:

- it **refuses** instead of guessing (no candidate, a candidate older than the sources it claims to
  build, uncommitted engine changes, a running cargo job, an engine that already answers `search`);
- it captures the four-read baseline and the rollback copy **before** touching the live binary;
- after `install` it never leaves the window open: a failed restart, a silent engine, a shape
  difference, or a `search` that is still refused all end in `restore`, and `restore` is a file put back
  plus a restart — not a diagnosis;
- a *content* difference (the vault moved between captures) is reported as such and does not roll back
  a working engine; a *shape* difference does.

## What changes

The served binary is `b7e48c82…` from the fourth swap (`015aeda`). Engine HEAD
`44c6967` is that build plus one additive read: `search` (verb landed in `d888abe`,
dead code removed in `44c6967`). `SCHEMA_VERSION` stays 29
(`crates/au-engine/src/serve.rs:132`), no key changes shape, and nothing in
`tests-golden` needs regenerating. `au-engine/tests-golden/engine/au_search.json`
is fixture-derived, not vault-derived, and is checked on every `--test it` run.

## Before (measured read-only, 2026-11-16)

| fact | value |
|---|---|
binary | `/root/.arsumbris/bin/au`, 13 859 712 bytes, 2026-11-14 14:45 |
sha256 | `b7e48c82c7ffe891272a9c32822354e065333ca48e21c0ff5344c94f491d5991` |
version | `au 0.0.5` (no commit marker — the checksum is the only identity) |
built from | monorepo `015aeda` (fourth swap, `rollback_preview`) |
daemon | `arsumbris-engine.service`, Main PID 3288310, started 2026-11-14 14:45:28 +07 |
socket | `/root/.arsumbris/au-engine/run/adcfe8680a485529.sock` |
status line | `daemon running for /root/vault (engine=up, ref=ready, ready=true, version=90)` |
vault | HEAD `4bcc4f2d432d96f923a55b89cb2a28b1651a872a`, 5 dirty entries (3 modified, 2 untracked) |
clients | `au-mcp-rs` on the same socket (pids 1194089 granted, 1194879 read-only) and the Python bridge |

`version=90` is the rebuild counter; a restart begins a new count and it is not a
health signal.

## Candidate

**REHEARSED** — built exactly as the swap would build it:

```sh
cd /root/arsumbris/au-engine
RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release -p au-cli
```

| fact | value |
|---|---|
path | `/root/arsumbris/au-engine/target/release/au` |
size | 13 926 360 bytes |
sha256 | `4601966524667503af41f7dbdb48281fc065638b185950b017c097ad2195654b` |
version | `au 0.0.5` |
built from | engine tree `44c6967`; the repo HEAD was `1046db7` at build time and moved to `e511507` during the rehearsal, with no engine change (`git diff --stat 44c6967..HEAD -- au-engine` empty, `git status --porcelain -- au-engine` empty) |
toolchain | `cargo 1.99.0 (5f94df478 2026-08-27)`, `rustc 1.99.0 (b940084d7 2026-09-28)` |
gate evidence at HEAD | `--test it golden_captures` 3/3 pass; `--test it search_read` 6/6 pass, including *a body rewrite changes the ranking on the next read* (the write→index patch path) |

The target path is a hardlinked artifact of the shared target directory, so the
window copies it once to `/root/backups/au-20261116-candidate` and installs from
there; a later `cargo build` cannot change what gets installed.

## Rehearsal on a copy — REHEARSED

Method: `/root/vault` copied to `/tmp/swap5/vault` (folder still named `vault`),
own `HOME=/tmp/swap5/home`, own socket, one daemon at a time. The copy and its HOME
were deleted after the rehearsal; nothing from it is reused below.

```sh
cp -a /root/vault /tmp/swap5/vault     # HEAD and dirty set identical before/after
```

Three lessons from the copy that the runbook must keep:

1. **A bare `HOME` makes the copy a one-member workspace.** Members resolve through
   `~/.arsumbris/au-engine/config/repos.yaml`; with an empty `HOME` the copy answers
   `members: 1` and one `peer-unmounted` warning for `mcp-bundle`. The rehearsal
   therefore copied the live registry with the `vault` entry re-pointed to the copy
   — **and only that entry**, because leaving it at `/root/vault` would have made
   the copy's daemon index the live vault. With the registry in place: 7 members,
   9 top-level dirs, 0 diagnostics, matching live.
2. **Same path, same socket name.** Two independent daemon starts on the copy both
   served `…/run/ddfb1f81599cf61d.sock`, so the restart is expected to bring back
   `adcfe8680a485529.sock` and the adapters' explicit socket paths stay valid.
3. **grep defaults to `scope: all`.** The default scans the member repositories too,
   so `wikilink` answers 107 matches, 96 of them outside the vault in member
   repositories (the first page is `au-mcp` TypeScript sources). Pin `scope: "own"`
   in the comparison to keep it inside the vault (11 matches); a member tree edited
   during the window would otherwise look like engine drift.

### Headline

**Five reads, six captures across two probe rounds, byte-identical between the
served binary and the candidate on one frozen copy.** `search` is the only difference: the served binary
refuses it with

```
malformed read: unknown variant `search`, expected one of `diagnostics`, …
```

and the candidate answers. That is the whole delta, which is what
`au-mcp-rs/docs/verification.md` already recorded (8/8 identical, `search` the only
verb the served build rejects).

### Rehearsal table

| read | request | served vs candidate | vs checked-in fixture | verdict |
|---|---|---|---|---|
| `overview` | `{"read":"overview"}` | byte-identical (11 308 B) | same 8 keys, same 7 members, same 9 dirs; counts drift with the corpus: nodes 1168→1185, edges 4576→4606, components 212→213, largest component 935→949 | **identical where it must be**; the drift is the living vault |
| `hubs` | `{"read":"hubs","limit":20,"offset":0}` | byte-identical (3 773 B) | same 20 paths in the same order; 2 entries' ref counts moved (refs_total 123→125, 56→59) | **identical**; content drift only |
| `diagnostics` | `{"read":"diagnostics","limit":50,"offset":0}` | byte-identical (23 B) | identical (`{"diagnostics": []}`) — 0 findings both sides | **identical** |
| `graph_health` | `{"read":"graph_health","limit":20}` | byte-identical (13 937 B) | same keys; counts drift: cycles 17→21, nodes_in_cycles 664→673, sources 185→197, components 212→213; dead ends 20, isolated 202, broken_links 0 unchanged; cycle *membership* differs accordingly | **identical**; content drift only |
| `grep` (text) | `{"read":"grep","query":"wikilink","scope":"own","limit":20}` | byte-identical, 11 vault matches (the default `all` scope also scans member sources: 107 matches, identical too) | no fixture | **identical** |
| `grep` (literal) | `{"read":"grep","query":"bm25 ranking","scope":"own"}` | byte-identical, 0 matches | — | **identical**; and it shows the dual — `search` finds 3 documents for the same words |
| `search` | `{"read":"search","query":"wikilink","limit":20}` | served **refuses** (`unknown variant`); candidate answers, 6 documents; a second independent daemon start returned the same bytes | `engine/au_search.json` is fixture-derived and not comparable to a vault capture; validated by `--test it search_read` 6/6 | **new verb works and is deterministic** |
| `search` paging | `{"read":"search","query":"pattern","limit":5}` | served refuses; candidate: `total 273`, 5 rows, `next_offset: 5`, `next_offset: null` on the last page | — | **paging and null-offset behave** |
| `search` exact | `{"read":"search","query":"error->fix","exact":true,"limit":5}` | served refuses; candidate: `exact: true`, `total 13`, first document `$VAULT/inbox/arsumbris-fork-backlog.md` | — | **matches the T1 acceptance record** |
| daemon-shaped `au_diagnostics` | projection of the above | — | **identical**: `summary` 0/0/0, `severity: error` list empty, same `offset/limit/truncated` | **identical** |
| daemon-shaped `au_overview` | same engine read | — | same 8 keys, 7 members (same roots), 9 dirs; counter drift, `diagnostic_counts` 1 warning → 0; `members[].git.root` is now `/root/arsumbris` because the bundle became one monorepo | **same shape; named drift** |
| daemon-shaped `au_hubs` | same engine read | — | 20 rows, 19 paths shared; `inbox/pattern-oracle-problem.md` left the top 20, `inbox/jevact-ibm-runtime-decision-gate.md` entered; 6 ref counts moved | **same shape; named drift** |

What the copy cannot prove: the live socket, the live index's BM25 statistics
(the copy is a snapshot plus a re-pointed registry, not the served state), client
reconnection, and quiescence. Those are the steps marked IN THE WINDOW.

## The window — IN THE WINDOW

### Preconditions

- **A health tick just happened.** `au-health.sh` runs every 15 minutes from root's
  crontab and restarts `arsumbris-engine` (then `arsumbris-mcp`) if a status probe
  fails. Start the window right after a tick and finish inside 15 minutes. Do not
  disable the watchdog: a failed swap with the watchdog off leaves the vault unserved.
- **No agent turn is in flight.** A restart drops in-flight reads (they see a closed
  socket) and a lost mutation reply becomes `outcome_unknown`, which only the caller's
  `au_reconcile` path repairs. A Hermes gateway is running on this host as normal
  background; the precondition is no *active turn*, and the operator, not a probe,
  confirms it.
- **The vault is quiet for the whole window.** The comparison is meaningful only if
  no note changes between the baseline capture and the post-install capture.
- Checks, all read-only:

```sh
tail -1 /root/.arsumbris/logs/health-checks.log        # engine=1 mcp=1 action=ok
systemctl status arsumbris-engine --no-pager | head -4 # active (running), Main PID
HOME=/root /root/.arsumbris/bin/au daemon status /root/vault
sha256sum /root/.arsumbris/bin/au                      # b7e48c82… (rollback source)
sha256sum /root/arsumbris/au-engine/target/release/au  # 46019665… (candidate)
pgrep -af "cargo test"                                 # must be empty: a stale test holds the target lock
cd /root/arsumbris && git diff --stat 44c6967..HEAD -- au-engine   # must be empty, else rebuild
cd /root/vault && git rev-parse HEAD && git status --porcelain | wc -l   # 4bcc4f2, 5
```

### 1. Candidate and rollback artifacts — **REHEARSED, already in place (2026-11-16 22:28)**

```
/root/backups/au-20261116-pre-search   sha256 b7e48c82c7ffe891272a9c32822354e065333ca48e21c0ff5344c94f491d5991
                                       (= /root/.arsumbris/bin/au, verified identical)
/root/backups/au-20261116-candidate    sha256 4601966524667503af41f7dbdb48281fc065638b185950b017c097ad2195654b
/root/backups/au-20261116-pre-live/    au_overview.json, au_hubs.json, au_diagnostics.json, au_graph_health.json
                                       captured from the LIVE socket through scripts/engine_probe.py
```

The probe reported `au_search: engine answered 'error', not a ready response` — that is the served
binary refusing the new verb, which is the expected state before the swap and the reason `search` is
captured after, not before. Creating these three artifacts touched nothing live: two copies and four
read requests.

The commands below are kept so the artifacts can be re-made if the window slips past a vault commit;
they are no longer the first thing the window does.

### 1a. Original recipe — IN THE WINDOW if the artifacts must be rebuilt

```sh
install -d /root/backups/au-20261116-pre-live
install -m 0755 /root/arsumbris/au-engine/target/release/au /root/backups/au-20261116-candidate
install -m 0755 /root/.arsumbris/bin/au /root/backups/au-20261116-pre-search
sha256sum /root/backups/au-20261116-candidate /root/backups/au-20261116-pre-search
# 46019665…  candidate      b7e48c82…  pre-search (the rollback binary)
```

### 2. Live baseline — IN THE WINDOW

```sh
SOCK=/root/.arsumbris/au-engine/run/adcfe8680a485529.sock
OUT=/root/backups/au-20261116-pre-live
/root/arsumbris/au-engine/scripts/engine_probe.py --socket "$SOCK" \
  --root /root/vault --out "$OUT" \
  --verb au_overview --verb au_hubs --verb au_diagnostics --verb au_graph_health
# `search` is excluded on purpose: the served binary refuses it and the probe exits.
python3 - "$SOCK" "$OUT" <<'PY'
import importlib.util, json, pathlib, sys
spec = importlib.util.spec_from_file_location("p", "/root/arsumbris/au-engine/scripts/engine_probe.py")
p = importlib.util.module_from_spec(spec); spec.loader.exec_module(p)
sock, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
for name, q in {"au_grep": "wikilink", "au_grep_bm25": "bm25 ranking"}.items():
    r = p.exchange(sock, {"read": "grep", "id": 1, "query": q, "scope": "own", "limit": 20, "offset": 0})
    assert r.get("type") == "response" and r.get("ready"), r
    (out / f"{name}.json").write_text(json.dumps(p.normalize(r["result"], "/root/vault"), indent=1, sort_keys=True) + "\n")
PY
```

Capture the baseline and make no copy in between: the second swap on 2026-11-14
compared against a twenty-minute-old baseline and four graph reads "differed"
because the digest loop had written three notes.

### 3. Install and restart — IN THE WINDOW

```sh
install -m 0755 /root/backups/au-20261116-candidate /root/.arsumbris/bin/au
sha256sum /root/.arsumbris/bin/au          # 46019665… before the restart, not after
systemctl restart arsumbris-engine
```

Installing while the daemon runs replaces the file but not the process: between
these two lines the path holds the new binary and the daemon is still the old one.
Never restart from the health cron in that state — run the restart here, and check
the PID changed.

### 4. Verify — IN THE WINDOW

```sh
systemctl status arsumbris-engine --no-pager | head -6     # active, NEW Main PID
HOME=/root /root/.arsumbris/bin/au daemon status /root/vault
ls -l /root/.arsumbris/au-engine/run/                      # adcfe8680a485529.sock back
```

The unit's `ExecStartPost` already polls `daemon status` 40 times at one second
(exit 1 if it never answers), so a failed start surfaces as a failed unit rather
than a silent hang.

### 5. Live read comparison — IN THE WINDOW

The same captures against the live socket after the restart. `search` is added here
and only here, because the old binary refuses it.

```sh
SOCK=/root/.arsumbris/au-engine/run/adcfe8680a485529.sock
OUT=/root/backups/au-20261116-post-live
install -d "$OUT"
/root/arsumbris/au-engine/scripts/engine_probe.py --socket "$SOCK" \
  --root /root/vault --out "$OUT" \
  --verb au_overview --verb au_hubs --verb au_diagnostics --verb au_graph_health
python3 - "$SOCK" "$OUT" <<'PY'
import importlib.util, json, pathlib, sys
spec = importlib.util.spec_from_file_location("p", "/root/arsumbris/au-engine/scripts/engine_probe.py")
p = importlib.util.module_from_spec(spec); spec.loader.exec_module(p)
sock, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
reqs = {
    "au_grep": {"read": "grep", "query": "wikilink", "scope": "own", "limit": 20},
    "au_grep_bm25": {"read": "grep", "query": "bm25 ranking", "scope": "own", "limit": 20},
    "au_search_wikilink": {"read": "search", "query": "wikilink", "limit": 20},
    "au_search_page": {"read": "search", "query": "pattern", "limit": 5},
    "au_search_exact": {"read": "search", "query": "error->fix", "exact": True, "limit": 5},
}
for name, req in reqs.items():
    r = p.exchange(sock, {"id": 1, "offset": 0, **req})
    assert r.get("type") == "response" and r.get("ready"), (name, r)
    (out / f"{name}.json").write_text(json.dumps(p.normalize(r["result"], "/root/vault"), indent=1, sort_keys=True) + "\n")
print("captured", len(reqs), "reads")
PY
diff -ru /root/backups/au-20261116-pre-live "$OUT"
# only the three au_search_*.json may be "Only in …-post-live"; everything else identical
cd /root/vault && git rev-parse HEAD && git status --porcelain
```

Then the searched-verb facts, against the live socket. The durable assertions are
properties, not counts: BM25 scores and totals belong to the corpus, and the live
corpus may have moved since the copy was frozen.

| check | expected |
|---|---|
`grep "wikilink"` | identical to the baseline (11 vault matches with `scope: own`) |
`search "wikilink"` | an answer frame, `total ≥ 1`, every match carrying `path`, a numeric `score` and `matched_terms`; the same query sent twice returns the same bytes |
`search "pattern" limit 5` | 5 rows, integer `next_offset`, and the page it points at ends with `next_offset: null` |
`search "error->fix" exact` | `exact: true`; expected first document `inbox/arsumbris-fork-backlog.md` (the copy's answer); `total` may differ from the copy's 13 if a note arrived since |

### 6. Client paths — IN THE WINDOW

```sh
cd /root/arsumbris/au-mcp-rs && python3 scripts/live_parity.py   # exit 0 = PASS, 2 = could not compare
/root/.arsumbris/bin/au-health.sh                                # engine=1 mcp=1 action=ok
```

The adapter and bridge reconnect per request, so no restart of `arsumbris-mcp` or of
the adapter processes is expected. `au-health.sh` is not on `PATH`; its cron entry
runs it by absolute path.

## Abort criteria — stop and restore on any of these

1. `diff -ru` finds a difference in the six shared captures **and** `git -C /root/vault
   rev-parse HEAD` is unchanged since the baseline. A commit landing in between
   explains counter drift; no commit explains nothing.
2. The daemon is not ready after the unit's own 40-second poll, `systemctl status`
   is failed, or the socket is absent.
3. Any shared read returns an error frame or loses a key it had.
4. `search` answers an error frame or `total: 0` for a term the corpus holds, the same
   query answers different bytes twice, no page ever reports `next_offset: null`, or
   `exact error->fix` ranks a document not holding both terms.
5. Vault HEAD or the dirty set changed without an agent action to explain it.
6. After `install` but before a successful restart, anything fails: restore first.
   The running daemon is then the old inode while the path holds the new binary.

## Rollback — named artifact `/root/backups/au-20261116-pre-search`

```sh
sha256sum /root/backups/au-20261116-pre-search     # b7e48c82c7ffe891272a9c32822354e065333ca48e21c0ff5344c94f491d5991
install -m 0755 /root/backups/au-20261116-pre-search /root/.arsumbris/bin/au
systemctl restart arsumbris-engine
HOME=/root /root/.arsumbris/bin/au daemon status /root/vault
```

Then re-run step 5 into `/root/backups/au-20261116-post-rollback` and diff it
against `-pre-live`: it must be empty. The Node layer does not need a restart
(verified in the first swap); schema 29 on both sides, so no client is reconfigured.
If the rollback itself fails, older builds are still on disk: `au-20261114-post-health`
(`98ea88e3…`, the third swap), `au-20261114-post-batch` (`95a55cae…`) and
`au-20261114-post-grep` (`97826aae…`), and the vault is protected by its own git.

## Not proven here

- The live socket comparison, the restart, and client reconnection: they need the
  window and were deliberately not run.
- Mutation behaviour beyond the index-patch test: the new verb is read-only, and the
  full engine suite's recorded baseline (678/1 lib, 710/3 `it`, the four root-only
  failures) was not re-run today; only `golden_captures` and `search_read`.
- BM25 scores against the live corpus: the copy's numbers are the copy's corpus.
- Daemon start cost at the live corpus size was not measured separately: on the
  copy the first read answered within about a second of the socket appearing, and
  the unit's own readiness poll allows 40 seconds.
