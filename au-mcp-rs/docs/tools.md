# Tools

49 tools: 28 engine reads, 5 derived reads and discovery helpers, and 16
mutations that exist only when the launch grants them (13 with the mutation
grant, 2 more with composition, 3 more with workspace shape). Every result is the
engine's own object under the read's key, and carries
`_meta["io.arsumbris/engineVersion"]`.

## Discovery

`au_search_tools {query, detail?, limit?}` — rank tools by name, parameter name,
or description. `detail` is `name`, `summary` (default), or `schema`; `limit` is
1–20, default 5.

`au_describe_tool {name}` — one full definition with input and output schemas.
It refuses a name this launch does not expose, so a read-only server cannot leak
a mutation's schema.

## Workspace shape

| tool | arguments | notes |
|---|---|---|
| `au_overview` | — | members, dependency topology, absolute paths, type and diagnostic counts; not own-scoped |
| `au_members` | `repo?` | exact nonempty repo, else every member; unpaged; unknown repo returns `[]` |
| `au_top_level_dirs` | — | top-level directories in own scope |
| `au_imports` | `scope?` | resolved cross-repo type uses; `own` filters importers, not external owners; unpaged |

## Vocabulary

| tool | arguments | notes |
|---|---|---|
| `au_types` | `repo?`, `summary`, `limit`, `offset` | `{types: [...] \| null}`; default summary, limit 50, range 1–500 |
| `au_type` | `name`, `repo?` | `{type}` or null; a nonempty `repo` does not override a qualified name; an empty `repo` is rejected, not widened |
| `au_type_tree` | — | full tree with roots and nodes; adjacency matched by name; unpaged |
| `au_subtypes` | `base`, `scope?` | direct and transitive; unknown base returns `[]`; unpaged |

## Addressability and links

| tool | arguments | notes |
|---|---|---|
| `au_frontmatter` | `repo`, `path` | parsed frontmatter as JSON, or null when the file is neither a typed instance nor a note |
| `au_anchors` | `repo`, `path` | every heading with text, level, and span; null when the target does not resolve, `[]` when it has no headings |
| `au_block_ids` | `repo`, `path` | every addressable id with kind (`record`/`typed_block`/`marker`) and type claim; duplicates included |
| `au_references` | `repo`, `path`, `direction?` | `out`, `in`, or both (default); answers `references_out` and/or `references_in` |
| `au_neighborhood` | `repo`, `path`, `direction?`, `depth?`, `kinds?`, `scope?`, `max_nodes?` | bounded N-hop walk returning `{nodes, edges, truncated}`; past depth 1 the engine requires `kinds` |
| `au_instances` | `limit?`, `offset?` | every resolved instance with its shape and values; the engine sends all (942 live), so paging adds `total`/`offset`/`next_offset` beside its `count` |
| `au_candidates` | `summary?`, `limit?`, `offset?` | files that satisfy a type without claiming it |
| `au_semantic_tokens` | `repo`, `path` | the ordered token stream: links, pins, field values, claims, blocks, headings |
| `au_resolve_member` | `repo`, `path` | which member owns the path, with root, role, editable and local |

## Content and graph

| tool | arguments | notes |
|---|---|---|
| `au_files` | `repo?`, `scope?`, `limit`, `offset` | default scope `all`; ordered by path components; absolute paths possible |
| `au_graph_health` | `repo?`, `scope?`, `limit` | cycles, dead ends, sources, isolated files, broken links, and the most central nodes in one report; counts cover the whole graph, lists are capped (default 20, max 200) |
| `au_grep` | `query`, `repo?`, `scope?`, `case_sensitive?`, `limit`, `offset` | literal (not regex) search, one match per line with line and character column; case-insensitive unless asked; `total` covers the whole scanned set and `skipped_files` counts what the bounds left out (above 1 MiB, or binary); default limit 20, max 200 |
| `au_hubs` | `repo?`, `scope?`, `limit`, `offset` | default limit 20; counts include edges from outside the scope |
| `au_diagnostics` | `repo?`, `severity?`, `code?`, `scope?`, `limit`, `offset` | filters AND; every severity unless one is named |
| `au_content` | `repo`, `path`, `max_chars?` | `{content: {text, hash, commit} \| null}`; with `max_chars` the text is windowed and the answer says `truncated`, `total_chars`, `next_offset`; the hash always covers the whole file |
| `au_instances_of` | `type_name`, `origins?`, `instance?`, `body?`, `limit?`, `offset?` | bare name matches every identity; with paging the answer adds `total`, `offset`, `next_offset` |
| `au_validate_value` | `type_name`, `value`, `repo?` | one verdict per identity; unknown type gives `identity: null` with `unknown-type-claim`, never `[]` |

## Mutations

`au_preview_mutation {repo, path, op, content?, old_string?, new_string?, replace_all?}`
returns the would-be product and blast radius; it writes nothing and needs no
hash, but the member must be editable.

`au_create_file {repo, path, content}` makes a file that does not exist yet. It
has no text to compare against, so its gate replaces the read-then-hash pair with
the opposite promise: the server reads the path first and refuses with
`policy/absence: file already exists; use au_write_file` instead of overwriting,
then writes without `expected_hash`. The window between that read and the write
is a few milliseconds; a concurrent creator can still land first, and the git
history of the member is what makes that recoverable.

`au_write_file`, `au_edit_file`, `au_delete_file` share one gate: a prior
`au_content` of the same path in the same process, a matching `expected_hash`,
an editable member, one mutation at a time. An engine rejection wrote nothing; a
transport failure is `outcome_unknown` and must not be retried blindly.

Structural mutations ride the same gate (prior read, hash, editable member, one
at a time) and each is one engine saga:

| tool | arguments | effect |
|---|---|---|
| `au_assign_block_id` | `repo`, `path`, `at`, `expected_hash` | mints an id for the entity at a byte offset; returns `id` and its `[[target^id]]` ref; idempotent on an entity that already has one |
| `au_rename_block_id` | `repo`, `path`, `block_id`, `to_block_id`, `expected_hash` | renames an inline record's id and carries every referrer |
| `au_rename` | `repo`, `path`, `to`, `expected_hash` | moves a file inside its member and rewrites inbound references; refuses a type-def (use `au_rename_type`) and a cross-member target |
| `au_inline` | `repo`, `path`, `into`, `at?`, `expected_hash`, `into_expected_hash` | folds one file into a referrer as an inline record and deletes it; both files must be armed |
| `au_promote` | `repo`, `path`, `to`, `at` xor `block_id`, `expected_hash` | lifts an inline record into its own file and leaves a reference |
| `au_batch` | `repo`, `ops` (1–50 of write_file / edit_file / delete_file, each with its own `expected_hash`) | applies the ops in order as one engine saga: one commit per touched member, and a rejection on any op leaves every file as it was |
| `au_rename_type` | `old_name`, `new_name`, `expected_hash` | renames a type-def and carries every claim, slot, and wikilink; the freshness check is the type's hash |
| `au_add_dep` / `au_remove_dep` | `repo`, `dep`, `expected_hash?` | author `.arsumbris/repo.yaml`; requires the composition grant |

### Workspace shape

These three change *which* members a workspace serves, so they need
`allow_workspace_shape` on top of mutations. None of them touches a note, and the
engine rebuild is what makes the change observable.

| tool | arguments | effect |
|---|---|---|
| `au_add_workspace_member` | `repo`, `name`, `role` (`edit` \| `discover`), `expected_hash?` | declares a member in the named list; refuses a name already declared in either list |
| `au_remove_workspace_member` | `repo`, `name`, `expected_hash?` | drops the name from both lists, and from `disabled:` in the same splice; the containing repo cannot be removed |
| `au_set_workspace_member_role` | `repo`, `name`, `role`, `expected_hash?` | moves a declared member between the lists in one commit; refuses an absent member, an unchanged role, and moving the host into `discover` |

`au_reconcile {repo, path, expected_hash? | expect_absent?}` answers `applied` or
`not_applied` with the current hash and commit, and re-arms the guard when the
file differs, so a deliberate retry needs no extra read.

## Pagination and size

Paged reads: `au_types`, `au_hubs`, `au_diagnostics`, `au_files`, `au_grep`,
`au_instances`, `au_instances_of`, `au_candidates`. Unpaged reads can be large: `au_type_tree`,
`au_validate_value`, `au_members`, `au_imports`, `au_subtypes`,
`au_preview_mutation`, `au_reconcile`.
`au_content` and `au_instances_of` take explicit budgets; the rest are bounded by
the 16 MiB wire frame, and an oversized frame fails rather than being truncated.
