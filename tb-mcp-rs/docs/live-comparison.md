# Live A/B: `arsumbris` (bridge) vs `arsumbris-rs` (Rust adapter)

> **Historical, 2026-11-13.** An A/B against the Python bridge, which was deleted 2026-11-16.


Measured on 2026-11-13 in one allspark session, both servers connected to the
same production workspace (`/root/vault` + six dependency members). The bridge
reaches the Node `au-mcp` daemon; the adapter reads the engine socket directly.

## Surfaces

| | tools |
|---|---|
| `arsumbris` (bridge) | 49 |
| `arsumbris-rs` (adapter) | 16 |

Eleven names exist on both sides, two more are renamed pairs
(`au_list_imports` → `tb_imports`, `au_validate` → `tb_validate_value`). 38 tools
are bridge-only (host actions, workspace mutation, text search, graph traversal)
and 5 are adapter-only (`tb_content`, `tb_imports`, `tb_preview_mutation`,
`tb_reconcile`, `tb_validate_value`).

## Payloads that are identical

The adapter returns the engine's result object, so a read sits under its own key
(`{"members": [...]}`); the bridge returns the bare payload. Unwrapping that one
difference, these answers are **byte-identical**:

| read | size compared | result |
|---|---|---|
| `tb_top_level_dirs` | 9 directories | identical |
| `tb_members` | 7 members | identical |
| `tb_files` | 200 files | identical |
| `tb_hubs` | 100 hubs | identical |
| `tb_type_tree` | full tree | identical |
| `tb_types` | 95/95 types | identical |
| `tb_subtypes` (`mcp`) | 62 subtypes | identical |
| `tb_type` (`agent-profile`) | all 16 shared keys | identical |
| `tb_imports` | 27 imports | identical |
| `tb_instances_of` (`au.engine.repo::au-engine`) | 7 instance paths | identical |
| `tb_validate_value` / `au_validate` | diagnostics, identity, undeclared fields | identical |

## Payloads that differ by projection, not by fact

| read | bridge adds |
|---|---|
| `tb_types` | `total`, `by_repo`, `detail`, `offset`, `limit` around the same list |
| `tb_diagnostics` | `summary` (by code, by severity), the active `severity` filter, `offset`/`limit`/`truncated`; it **defaults to one severity**, while the engine read returns every finding |
| `au_validate` | `valid` and `unresolved`, derived from the diagnostics the engine returns |
| `tb_instances_of` | `ofType` argument, paged `instances`/`count`/`resolve`; the engine read is unpaged and returns each record's `fields`, `member`, `span` |

`tb_diagnostics` currently agrees on live data by being empty on both sides
(`total: 0`); the three-finding case is covered by `scripts/bridge_parity.py`
against a synthetic workspace.

## What this means for a cutover

- The engine-side facts are stable across both paths, so the adapter is not
  inventing or reshaping data.
- A consumer that wants the bridge's sugar (`valid`, `total`, `summary`,
  severity filtering) must either derive it or the adapter must add it as an
  explicit, documented projection. Right now it does not, on purpose: the
  payload stays byte-identical to what the engine served.
- The bridge's 38 extra tools are not read coverage the adapter lost; they are a
  different surface (host panes, shell, rename/promote, search), and a cutover
  has to keep or replace them consciously.
