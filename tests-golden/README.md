# Golden wire fixtures — arsumbris fork

Captured 2026-11-12 on the PRODUCTION serving stack (live daemon + bridge,
vault `/root/vault`), via `bridge_probe.py` (socket client), before any fork
change. Engine version at capture: bundle `0.0.5`-era WIP, commit `44b32f4`
(branch `feat/hotpath-profiling-and-memchr-wikilinks`).

| file | tool | notes |
|---|---|---|
| `au_overview.json` | `au_overview` | full graph shape + members + type counts |
| `au_hubs.json` | `au_hubs` | top-20 hubs by reference totals |
| `au_diagnostics.json` | `au_diagnostics` | clean state (total: 0) |

Contract: after engine changes these responses are byte-identical unless the
diff is intentional; intentional diffs regenerate the fixture in the SAME
commit with the reason in the message. Known gap (P0 infra): `au_files` and
other size-heavy tools are not captured yet — they need a socket-native
capture client (no LLM in the loop); the payloads here were captured through
`bridge_probe.py`, which is already deterministic.

Vault is a living corpus: the graph content itself drifts (new notes change
counts). The contract therefore applies to response STRUCTURE and, when
comparing content, against a frozen vault fixture (see AGENTS.md «Dev loop»:
fixture copy of the vault), not the live corpus.
