---
name: 2brain
description: Read and change a 2brain knowledge workspace through its MCP server. Use when the task needs vault content, type definitions, diagnostics, a validated note, a ranked search, or an audit of what the workspace knows. Covers the surface, the tool ladder, the result envelope, the safe read-then-write loop, and the recovery path for an unclear mutation.
---

# Work a 2brain workspace

The workspace is typed markdown: notes claim a `type:`, link each other with
`[[wiki-links]]`, and carry addressable `^block-ids`. The engine is the authority;
the MCP server in front of it is a reader and a gated writer.

## The surface

One server: **`2brain`**, the Rust adapter over the engine socket.

| item | value |
|---|---|
| launch | `2brain mcp <workspace-or-socket>`; a workspace entry is resolved to its socket the way `au daemon start` resolves it |
| tools | `au_*`, 52 with every grant — 33 read, 16 mutate, 2 discover, 1 watch |
| grants | `--allow-mutations` (16), `--allow-composition` (+2), `--allow-workspace-shape` (+3); without them the tools are absent, not merely refused |
| results | the engine's own object under the read's key, plus `structuredContent` and `_meta.engineVersion` |
| resources | `arsumbris://<repo>/<path>` — catalogued files, no subscription (`au_watch` is the change signal) |

`references/tool-map.md` lists every tool with its arguments, generated from the running binary.

## Find the tool you need

1. `au_search_tools {"query": "<task words>", "detail": "summary"}` — names and one line each;
   `detail` may be `name`, `summary`, or `schema`. It answers from the catalogue, without the engine.
2. `au_describe_tool {"name": "..."}` — the full definition with schemas.
3. Call it.

If the catalog was launched with `--tools concise`, the list you see is already one line per tool.

## Read the result

Every result is the engine's object under the read's key:

```json
au_overview -> {"overview": {...}}
au_content  -> {"content": {"text": "...", "hash": "...", "commit": "..."}}
au_search   -> {"search": {"matches": [...], "total": n}}
```

Errors arrive as tool errors with an actionable sentence. Three of them are routing signals:

- `policy/<stage>: ...` — a mutation gate refused. The stage names the fix (`prior_read` = read the
  file first, `freshness` = re-read for a new hash, `member_grant` = this launch lacks the grant).
- JSON-RPC `-32600 mutations are disabled by this server's launch policy` — this launch is read-only;
  do not look for another way to write.
- `outcome_unknown` — the reply was lost, the mutation may or may not have landed. **Never retry it.**
  Call `au_reconcile` with the same `repo` + `path`; it reports the settled hash and re-arms the guard.

## Sequences

**Orient in a workspace**

1. `au_overview` — members, paths, type counts, diagnostics counts.
2. `au_members` — which member is editable (`vault` is usually the entry).
3. `au_types`, then `au_type {"name": ...}` — the vocabulary you must obey.

**Locate text**

1. `au_search {"query": "<words>", "limit": 10}` — BM25-ranked over the catalogue; the right first
   move for a theme or a question, because relevance ordering survives paraphrases.
2. `au_search {"query": "<phrase>", "exact": true}` — exact tokens only, for a literal identifier.
3. `au_grep {"query": "<literal phrase>"}` — one match per line, literal not regex, paged. Use it when
   you want every occurrence in order, not a ranking.
4. `au_files {"repo": "vault", "limit": 50}` — catalogued paths, paged.
5. `au_content {"repo": "vault", "path": "..."}` — text, `hash`, `commit`.

**Write one note**

1. `au_content` → keep `hash`.
2. `au_validate_value {"type_name": "<type>", "value": {...frontmatter...}}` — catches absent or
   mistyped required fields without touching disk.
3. `au_write_file {"repo", "path", "content", "expected_hash"}` — or `au_edit_file {"repo", "path",
   "old_string", "new_string", "expected_hash"}` for a targeted change.
4. `au_diagnostics {"severity": "warning"}` — the corpus must stay at zero.

Absolute paths and `..` are rejected: a write names a member and a path relative to that member's root.

**Answer a graph question**

`au_instances_of {"type_name"}` (what of this type), `au_references` (who links here),
`au_follow` (walk edges), `au_neighborhood` (around a node), `au_hubs` (most-linked), `au_type_tree`
(the vocabulary's shape), `au_graph_health` (cycles, dead ends, broken links, sources).

## Do not

- Do not compose a URL or a path from a `[[wiki-link]]` and read it directly: `au_references` and
  `au_neighborhood` already resolve the graph, and a hand-built path misses renames.
- Do not treat `au_grep` as a fallback for `au_search`: one is exhaustive and unranked, the other is
  ranked by design; picking the wrong one changes the answer, not just the speed.
- Do not retry an `outcome_unknown` mutation, and do not batch a write with a `au_diagnostics` gate in
  one step — the gate needs the write to settle first.
