# Tool map

`arsumbris-rs` (`au_*`) against the legacy bridge (`mcp__*`). The adapter's
answers are the engine's own JSON under the read key; the bridge's are daemon
projections with summaries and paging.

## Adapter reads (35: 29 engine reads, 6 derived and observability helpers)

| tool | argument | answers |
|---|---|---|
| `au_overview` | — | `{overview}`: members, paths, counts |
| `au_top_level_dirs` | — | `{top_level_dirs}` |
| `au_members` | `repo?` | `{members}`: repo, role, editable, root |
| `au_types` | `repo?`, `limit`, `offset` | `{types}` |
| `au_type` | `name`, `repo?` | `{type}` or null |
| `au_type_tree` | — | `{type_tree: {roots, nodes}}` |
| `au_subtypes` | `base` | `{base, subtypes}` |
| `au_hubs` | `repo?`, `scope?`, `limit`, `offset` | `{hubs}` |
| `au_diagnostics` | `repo?`, `severity?`, `code?`, `limit`, `offset` | `{diagnostics}` |
| `au_files` | `repo?`, `scope?`, `limit`, `offset` | `{files}` |
| `au_children` | `repo`, `path` | `{dir_entries}`: direct files and subdirectories |
| `au_grep` | `query`, `repo?`, `scope?`, `case_sensitive?`, `limit`, `offset` | `{grep: {matches, total, truncated, skipped_files}}` |
| `au_watch` | `timeout_ms` | `{watch: {added, removed, modified, version, timed_out}}` — the engine's `changes` subscription, one event or a timeout |
| `au_rollback_preview` | `repo`, `commit` | `{rollback_preview: {subject, parent, paths, safe}}` |
| `au_graph_health` | `repo?`, `scope?`, `limit` | `{graph_health: {counts, cycles, isolated, dead_ends, sources, broken_links, central, truncated}}` |
| `au_content` | `repo`, `path` | `{content: {text, hash, commit}}` |
| `au_imports` | `scope?` | `{imports}` |
| `au_instances_of` | `type_name`, `origins?`, `instance?`, `body?`, `limit?`, `offset?` | `{instances_of, total, offset, next_offset}` |
| `au_validate_value` | `type_name`, `value`, `repo?` | `{validate_value: [verdict]}` |
| `au_preview_mutation` | `repo`, `path`, `op`, ... | `{preview_mutation}` |
| `au_reconcile` | `repo`, `path`, `expected_hash?` / `expect_absent?` | `{outcome, current_hash, current_commit}` |
| `au_frontmatter` | `repo`, `path` | `{frontmatter}` or null |
| `au_anchors` | `repo`, `path` | `{anchors}` (headings) or null |
| `au_block_ids` | `repo`, `path` | `{block_ids}` (records, fences, markers) or null |
| `au_references` | `repo`, `path`, `direction?` | `{references_out}` and/or `{references_in}` |
| `au_follow` | `repo`, `origin_path`, `target`, `peek?` | `{follow}`: resolution verdict, referent, the `resolve_*` fields, and a line `snippet` |
| `au_neighborhood` | `repo`, `path`, `direction?`, `depth?`, `kinds?`, `scope?`, `max_nodes?` | `{neighborhood: {nodes, edges, truncated}}` |
| `au_instances` | `limit?`, `offset?` | `{instances, count, total, offset, next_offset}` |
| `au_candidates` | `summary?`, `limit?`, `offset?` | `{candidates, aborted_at_load}` |
| `au_semantic_tokens` | `repo`, `path` | `{semantic_tokens}` |
| `au_resolve_member` | `repo`, `path` | `{resolve_member}` |
| `au_typed` | `repo`, `path` | `{typed, kind, path, claim, resolves, closure}` |
| `au_resolved` | `repo`, `path` | `{instance}`: field values, provenance, claim, closure, candidates |
| `au_search_tools` | `query`, `detail?`, `limit?` | `{tools}` |
| `au_describe_tool` | `name` | one full tool definition |

Mutations (only when the launch grants them; three grants, each requiring
`allow_mutations`):

- mutation grant (11): `au_write_file`, `au_edit_file`, `au_delete_file`,
  `au_batch`, `au_assign_block_id`, `au_rename_block_id`, `au_rename`,
  `au_rename_type`, `au_inline`, `au_promote`, `au_create_file`;
- composition grant (2): `au_add_dep`, `au_remove_dep`;
- workspace-shape grant (3): `au_add_workspace_member`,
  `au_remove_workspace_member`, `au_set_workspace_member_role`.

A single-file write or edit needs `expected_hash` from a prior `au_content` of
the same path and answers the engine's mutation result (`path`, `hash`,
`commit`, `diagnostics`, `reflected`, ...); the structural tools carry the same
read-then-hash gate.

## Bridge equivalents

| bridge | adapter | difference |
|---|---|---|
| `mcp__au_overview` | `au_overview` | bridge returns the bare payload |
| `mcp__au_members`, `mcp__au_files`, `mcp__au_hubs`, `mcp__au_top_level_dirs`, `mcp__au_type_tree`, `mcp__au_subtypes` | same names under `au_` | payload identical once unwrapped |
| `mcp__au_types` | `au_types` | bridge adds `total`, `by_repo`, `detail`, paging |
| `mcp__au_diagnostics` | `au_diagnostics` | bridge adds `summary` and defaults to one severity |
| `mcp__au_validate` | `au_validate_value` | bridge adds `valid`, `unresolved` |
| `mcp__au_instances_of` | `au_instances_of` | bridge uses `ofType`; both page the records |
| `mcp__au_instances` | `au_instances` | same set; the adapter pages it with `limit`/`offset` |
| `mcp__au_list_imports` | `au_imports` | same list |
| `mcp__grep_files` | `au_grep` | the bridge tool is a stub; the adapter reads the engine's catalogue |

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

Tools with no adapter counterpart (8): `mcp__bash`, `mcp__glob`,
`mcp__read_file_pinned`, the four `mcp__au_host_*` tools, and
`mcp__au_type_system` (it reads typed `doc` atoms from a content package, not
the engine). Every other bridge tool has the counterpart named in the table
above, including the graph reads (`mcp__au_neighborhood`, `mcp__au_references`,
`mcp__au_children`, `mcp__au_candidates`, `mcp__au_anchors`,
`mcp__au_block_ids`, `mcp__au_frontmatter`, `mcp__au_follow`,
`mcp__au_resolved`, `mcp__au_semantic_tokens`, `mcp__au_typed`,
`mcp__au_resolve_member`).
