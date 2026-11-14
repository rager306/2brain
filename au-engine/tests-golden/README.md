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

The probe normalizes what would otherwise differ on every run: the workspace root
becomes `$VAULT`, the observed `version` is dropped (it counts rebuilds), and keys
are sorted. Two consecutive captures are byte-identical — verified.

**The vault is a living corpus.** New notes change `nodes`, `edges`, and every
count in these files, so the contract is STRUCTURE plus, when comparing content,
a frozen copy of the vault (`git -C /root/vault worktree add /tmp/vault-fixture`),
never the live corpus. When an engine change alters a shape intentionally, the
fixture is regenerated in the same commit and the message says why.
