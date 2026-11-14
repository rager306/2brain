# Tool map

`arsumbris-rs` (`au_*`) against the legacy bridge (`mcp__*`). The adapter's
answers are the engine's own JSON under the read key; the bridge's are daemon
projections with summaries and paging.

## Adapter reads (16 + 2 discovery)

| tool | argument | answers |
|---|---|---|
| `au_overview` | — | `{overview}`: members, paths, counts |
| `au_top_level_dirs` | — | `{top_level_dirs}` |
| `au_members` | — | `{members}`: repo, role, editable, root |
| `au_types` | `repo?`, `limit`, `offset` | `{types}` |
| `au_type` | `name`, `repo?` | `{type}` or null |
| `au_type_tree` | — | `{type_tree: {roots, nodes}}` |
| `au_subtypes` | `base` | `{base, subtypes}` |
| `au_hubs` | `repo?`, `scope?`, `limit`, `offset` | `{hubs}` |
| `au_diagnostics` | `repo?`, `severity?`, `code?`, `limit`, `offset` | `{diagnostics}` |
| `au_files` | `repo?`, `scope?`, `limit`, `offset` | `{files}` |
| `au_grep` | `query`, `repo?`, `scope?`, `case_sensitive?`, `limit`, `offset` | `{grep: {matches, total, truncated, skipped_files}}` |
| `au_rollback_preview` | `repo`, `commit` | `{rollback_preview: {subject, parent, paths, safe}}` |
| `au_graph_health` | `repo?`, `scope?`, `limit` | `{graph_health: {counts, cycles, isolated, dead_ends, sources, broken_links, central, truncated}}` |
| `au_content` | `repo`, `path` | `{content: {text, hash, commit}}` |
| `au_imports` | `scope?` | `{imports}` |
| `au_instances_of` | `type_name`, `origins?`, `instance?`, `body?` | `{instances_of}` |
| `au_validate_value` | `type_name`, `value`, `repo?` | `{validate_value: [verdict]}` |
| `au_preview_mutation` | `repo`, `path`, `op`, ... | `{preview_mutation}` |
| `au_reconcile` | `repo`, `path`, `expected_hash?` / `expect_absent?` | `{outcome, current_hash, current_commit}` |
| `au_frontmatter` | `repo`, `path` | `{frontmatter}` or null |
| `au_anchors` | `repo`, `path` | `{anchors}` (headings) or null |
| `au_block_ids` | `repo`, `path` | `{block_ids}` (records, fences, markers) or null |
| `au_references` | `repo`, `path`, `direction?` | `{references_out}` and/or `{references_in}` |
| `au_neighborhood` | `repo`, `path`, `direction?`, `depth?`, `kinds?`, `scope?`, `max_nodes?` | `{neighborhood: {nodes, edges, truncated}}` |
| `au_instances` | `limit?`, `offset?` | `{instances, count, total, offset, next_offset}` |
| `au_candidates` | `summary?`, `limit?`, `offset?` | `{candidates, aborted_at_load}` |
| `au_semantic_tokens` | `repo`, `path` | `{semantic_tokens}` |
| `au_resolve_member` | `repo`, `path` | `{resolve_member}` |
| `au_search_tools` | `query`, `detail?`, `limit?` | `{tools}` |
| `au_describe_tool` | `name` | one full tool definition |

Mutations (only when the launch grants them): `au_write_file`,
`au_edit_file`, `au_delete_file` — each needs `expected_hash` from a prior
`au_content` of the same path and answers the engine's mutation result
(`path`, `hash`, `commit`, `diagnostics`, `reflected`, ...).

## Bridge equivalents

| bridge | adapter | difference |
|---|---|---|
| `mcp__au_overview` | `au_overview` | bridge returns the bare payload |
| `mcp__au_members`, `mcp__au_files`, `mcp__au_hubs`, `mcp__au_top_level_dirs`, `mcp__au_type_tree`, `mcp__au_subtypes` | same names under `au_` | payload identical once unwrapped |
| `mcp__au_types` | `au_types` | bridge adds `total`, `by_repo`, `detail`, paging |
| `mcp__au_diagnostics` | `au_diagnostics` | bridge adds `summary` and defaults to one severity |
| `mcp__au_validate` | `au_validate_value` | bridge adds `valid`, `unresolved` |
| `mcp__au_instances_of` | `au_instances_of` | bridge uses `ofType` and pages the records |
| `mcp__au_instances` | — | all instances at once; use `au_instances_of` |
| `mcp__au_list_imports` | `au_imports` | same list |

The bridge declares the workspace-member verbs too, so the adapter now matches it
there as well — behind a third grant, because reshaping a workspace is a bigger
trust decision than editing a note.

The adapter also has `au_create_file` (create a file that does not exist, refused
if it does) and `au_batch` (1-50 file ops as one all-or-nothing saga: the bridge
requires a per-op confirm instead, so a batch there is N calls)
 — the bridge's `write_file` covers both cases, so a create on the
bridge side needs no extra verb.

The adapter now also answers `grep_files`' job with `au_grep` (the engine's own
`grep` read, literal and catalogue-scoped), so the bridge's `grep_files` — a stub
there — is the last one standing on that side.

Bridge-only tools are host and workspace operations rather than engine reads:
`mcp__bash`, `mcp__glob`, `mcp__read_file_pinned`,
`mcp__write_file`, `mcp__edit_file`, `mcp__delete_file`, `mcp__rename`,
`mcp__rename_block_id`, `mcp__assign_block_id`, `mcp__inline`, `mcp__promote`,
`mcp__add_dep`, `mcp__remove_dep`, `mcp__add_workspace_member`,
`mcp__remove_workspace_member`, `mcp__set_workspace_member_role`,
`mcp__rename_type` (the adapter has every one of these behind its grants), the `mcp__au_host_*` family, and the graph reads
(`mcp__au_neighborhood`, `mcp__au_references`, `mcp__au_children`,
`mcp__au_candidates`, `mcp__au_anchors`, `mcp__au_block_ids`,
`mcp__au_frontmatter`, `mcp__au_follow`, `mcp__au_resolved`,
`mcp__au_semantic_tokens`, `mcp__au_typed`, `mcp__au_type_system`,
`mcp__au_resolve_member`).
