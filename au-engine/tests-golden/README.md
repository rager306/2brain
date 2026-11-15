# Golden fixture sets — arsumbris fork

Two sets, captured at two moments, for two different reasons. They are not
interchangeable: the first is phrased in the retired bridge's projections, the
second in the engine's own answers.

## `*.json` (top level) — the pre-fork baseline, frozen

Captured 2026-11-12 on the production serving stack (`/root/vault`, live engine
plus the Node daemon and its Python bridge) before the fork's first engine change,
through `bridge_probe.py` — a socket client that talked to the daemon. That script
is **not in this repository**; the captures cannot be regenerated, because the
daemon is being retired and its projections (`summary`, a severity-filtered
`diagnostics`, a bare hub array) exist nowhere else.

| file | shape | note |
|---|---|---|
| `au_overview.json` | daemon projection | graph shape, members, hubs, type counts |
| `au_hubs.json` | daemon projection | a bare array of 20 hub objects |
| `au_diagnostics.json` | daemon projection | `summary` + one severity, clean at capture (total 0) |

They stay as the historical record of what the vault looked like before the fork
touched the engine. They are not a contract anything can be re-captured against.

## `engine/*.json` — the contract that outlives the bridge

Captured from a running engine with `scripts/engine_probe.py` (read-only: `read`
verbs only, a socket per request, no mutation verb ever sent). Re-capture:

```sh
au-engine/scripts/engine_probe.py \
    --socket ~/.arsumbris/au-engine/run/<hash>.sock \
    --out au-engine/tests-golden/engine
```

| file | verb | what it pins |
|---|---|---|
| `au_overview.json` | `overview` | members, dependency topology, type and diagnostic counts |
| `au_hubs.json` | `hubs {limit: 20}` | the navigational crossroads, ranked |
| `au_diagnostics.json` | `diagnostics {limit: 50}` | the workspace's problem list |
| `au_graph_health.json` | `graph_health {limit: 20}` | cycles, dead ends, sources, broken links, centrality |
| `au_search.json` | `search {query: "zarquon"}` | ranked notes, the paging totals, and the `next_offset` nullability — **fixture-derived**, see below |

The probe normalizes what would otherwise differ on every run: the workspace root
becomes `$VAULT`, the observed `version` is dropped (it counts rebuilds), and keys
are sorted. Two consecutive captures are byte-identical — verified.

**`au_search.json` is captured from a fixture, not the vault.** A BM25 score is a
property of corpus statistics (document frequency, average field length), so a
capture of it can only be compared against the corpus it came from; the living
vault cannot serve that. This capture therefore comes from the fixture in
`crates/au-engine/tests/it/search_read.rs`, taken in process by that test rather
than through the probe, and it is checked on every `cargo test --test it` run —
structure and values. Regenerate it with:

```sh
cd au-engine && AU_BLESS_SEARCH_CAPTURE=1 cargo +1.99.0 test -p au-engine --test it search_read
```

A probe run captures `search` against the vault under the same name, so it
replaces this file with a vault answer. That is a deliberate act: the equality
test then fails and names the re-bless command, and the capture is either
re-blessed back to the fixture answer or kept as a vault capture with its scores
recorded against a frozen vault.

**The vault is a living corpus.** New notes change `nodes`, `edges`, and every
count in these files, so the contract is STRUCTURE plus, when comparing content,
a frozen copy of the vault (`git -C /root/vault worktree add /tmp/vault-fixture`),
never the live corpus. When an engine change alters a shape intentionally, the
fixture is regenerated in the same commit and the message says why.
