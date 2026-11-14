---
name: arsumbris
description: Read and change an arsumbris knowledge workspace through its MCP servers. Use when the task needs vault content, type definitions, diagnostics, a validated note, or an audit of what the workspace knows. Covers the tool ladder, the result envelope, the safe read-then-write loop, and the recovery path for an unclear mutation.
---

# Work an arsumbris workspace

The workspace is typed markdown: notes claim a `type:`, link each other with
`[[wiki-links]]`, and carry addressable `^block-ids`. The engine is the
authority; every MCP server in front of it is a reader or a gated writer.

## Pick the surface

| surface | shape | use it for |
|---|---|---|
| `arsumbris-rs` | tools named `au_*`, results are the engine's own JSON | reads, preview, reconcile, gated writes |
| `arsumbris` | tools named `mcp__*`, results are daemon projections | only when a tool is missing above (shell, glob, pinned read, host panes, `au_type_system`) |

Do not mix their answers in one comparison: the bridge wraps the same facts in
summaries and paging. See `references/tool-map.md` for the full mapping.

## Find the tool you need

1. `au_search_tools {"query": "<task words>", "detail": "summary"}` — names and
   one line each; `detail` may be `name`, `summary`, or `schema`.
2. `au_describe_tool {"name": "..."}` — the full definition with schemas.
3. Call it.

If the catalog was launched with `--tools concise`, the list you see is already
one line per tool; the ladder above still returns everything else.

## Read the result

Every result is the engine's object under the read's key:

```json
au_overview -> {"overview": {...}}
au_content  -> {"content": {"text": "...", "hash": "...", "commit": "..."}}
```

Errors arrive as tool errors with an actionable sentence. Two of them are
routing signals, not failures:

- `policy/<stage>: ...` — a mutation gate refused. The stage names the fix
  (`prior_read` = read the file first, `freshness` = re-read for a new hash,
  `workspace_shape` = this launch may not change which members are served).
- JSON-RPC `-32600 mutations are disabled by this server's launch policy` —
  this launch is read-only; do not look for another way to write.

## Sequences

**Orient in a workspace**

1. `au_overview` — members, paths, type counts, diagnostics counts.
2. `au_members` — which member is editable (`vault` is the entry).
3. `au_types` then `au_type {"name": ...}` — the vocabulary you must obey.

**Locate text**

1. `au_grep {"query": "<literal phrase>"}` — one match per line across the
   catalogue; literal, not regex, and paged.
2. `au_files {"repo": "vault", "limit": 50}` — catalogued paths, paged.
3. `au_content {"repo": "vault", "path": "inbox/note.md"}` — text plus the hash
   a later write must quote.

**Follow a link or inspect one file's surface**

`au_frontmatter` for the parsed claim and fields; `au_anchors` for the headings a
`#fragment` can address; `au_block_ids` for what `^id` can address; `au_references`
for the edges in and out; `au_neighborhood` for a bounded walk (`depth` past 1
needs `kinds`).

**Find everything of a kind**

`au_instances_of {"type_name": "note"}` — one record per instance, with the
file, the claim, and the field values. Filter with `origins: ["file"]` when
nested records are noise.

**Check a value before writing it**

`au_validate_value {"type_name": "insight", "value": {...}}` — one verdict per
identity the name denotes. An unknown type returns `identity: null` with
`unknown-type-claim`: that is a finding, never a valid verdict.

**Write, safely**

1. `au_content` the exact path; keep `hash`.
2. `au_preview_mutation` if the change is not obvious; it writes nothing.
3. `au_write_file` / `au_edit_file` / `au_delete_file` with `expected_hash`.
4. If the answer is `outcome_unknown`, **do not retry**. Call
   `au_reconcile` with the hash the mutation should have produced, or
   `expect_absent: true` for a delete; it reports `applied` or `not_applied`.

**Close the loop**

`au_diagnostics` after a write. A dangling `[[wiki-link]]` or a missing required
field shows up here; fix it before reporting success.

## Gotchas

- Paths are member-relative. `/root/vault/x.md` must be passed as
  `{"repo": "vault", "path": "x.md"}`; absolute paths and `..` are refused.
- Write into `inbox/`; `knowledge/` is the curated pass.
- Every `[[wiki-link]]` in body and `topics:` must resolve to an existing file
  name before you save.
- Some reads are unpaged (`au_type_tree`, `au_subtypes`, `au_members`,
  `au_imports`, `au_validate_value`): expect large answers there. `au_content`
  takes `max_chars`; `au_instances_of`, `au_instances`, `au_types`, `au_hubs`,
  `au_diagnostics`, `au_files`, `au_grep`, and `au_candidates` page with
  `limit`/`offset`.
- A mutation consumes the recorded read: a second write needs a fresh
  `au_content`, even with the same hash.
- `au_diagnostics` on the engine returns every severity; the bridge's tool
  defaults to one. Compare like with like.

## Done means

- the change is visible through a fresh `au_content`, with a new hash;
- `au_diagnostics` reports no new finding pointing at your file;
- for a delete, the file is absent and no inbound link remains.
