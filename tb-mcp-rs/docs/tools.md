# Tools

Two surfaces: **tools** (51) for actions and reads, and **resources** for
addressable files. A resource URI is `2brain://<repo>/<path>` (the legacy `arsumbris://` is accepted on parse), where the path is
member-relative and percent-encoded; `resources/list` pages the catalogue 100 at a
time and `resources/read` returns the engine's own text with `text/markdown`.
`resources/templates/list` publishes the one shape, so a client can construct a URI
without listing first. Resource subscriptions are not advertised: a client that
needs change notification calls `tb_watch`.

52 tools: 29 engine reads, 4 derived reads and observability helpers, 2 discovery, 1 watch, and 16
mutations that exist only when the launch grants them (11 with the mutation
grant, 2 more with composition, 3 more with workspace shape). The three grants
are independent, and composition and workspace shape each require
`allow_mutations`. Every result is the engine's own object under the read's key,
and carries `_meta["io.2brain/engineVersion"]`.

## Discovery

`tb_search_tools {query, detail?, limit?}` — rank tools by name, parameter name,
or description. `detail` is `name`, `summary` (default), or `schema`; `limit` is
1–20, default 5.

`tb_describe_tool {name}` — one full definition with input and output schemas,
returned as `{"tool": {name, description, inputSchema, outputSchema, annotations}}`.
It refuses a name this launch does not expose, so a read-only server cannot leak
a mutation's schema. The definition sits under a key because the declared
`outputSchema` is closed: returning it bare made a strict client reject the answer
with "additional properties are not allowed".

## Workspace shape

| tool | arguments | notes |
|---|---|---|
| `tb_overview` | — | members, dependency topology, absolute paths, type and diagnostic counts; not own-scoped |
| `tb_members` | `repo?` | exact nonempty repo, else every member; unpaged; unknown repo returns `[]` |
| `tb_top_level_dirs` | — | top-level directories in own scope |
| `tb_imports` | `scope?` | resolved cross-repo type uses; `own` filters importers, not external owners; unpaged |

## Vocabulary

| tool | arguments | notes |
|---|---|---|
| `tb_types` | `repo?`, `summary`, `limit`, `offset` | `{types: [...] \| null}`; default summary, limit 50, range 1–500 |
| `tb_type` | `name`, `repo?` | `{type}` or null; a nonempty `repo` does not override a qualified name; an empty `repo` is rejected, not widened |
| `tb_type_tree` | — | full tree with roots and nodes; adjacency matched by name; unpaged |
| `tb_subtypes` | `base`, `scope?` | direct and transitive; unknown base returns `[]`; unpaged |

## Addressability and links

| tool | arguments | notes |
|---|---|---|
| `tb_frontmatter` | `repo`, `path` | parsed frontmatter as JSON, or null when the file is neither a typed instance nor a note |
| `tb_anchors` | `repo`, `path` | every heading with text, level, and span; null when the target does not resolve, `[]` when it has no headings |
| `tb_block_ids` | `repo`, `path` | every addressable id with kind (`record`/`typed_block`/`marker`) and type claim; duplicates included |
| `tb_references` | `repo`, `path`, `direction?` | `out`, `in`, or both (default); answers `references_out` and/or `references_in` |
| `tb_follow` | `repo`, `origin_path`, `target`, `peek?` | resolves one wikilink string as if it appeared in `origin_path`: `follow.target`, `follow.resolved`, `follow.referent`, the chosen `resolve_*` fields, and with `peek` (default true) the addressed line as `snippet` |
| `tb_neighborhood` | `repo`, `path`, `direction?`, `depth?`, `kinds?`, `scope?`, `max_nodes?` | bounded N-hop walk returning `{nodes, edges, truncated}`; past depth 1 the engine requires `kinds` |
| `tb_instances` | `limit?`, `offset?` | every resolved instance with its shape and values; the engine sends all (942 live), so paging adds `total`/`offset`/`next_offset` beside its `count` |
| `tb_candidates` | `summary?`, `limit?`, `offset?` | files that satisfy a type without claiming it |
| `tb_semantic_tokens` | `repo`, `path` | the ordered token stream: links, pins, field values, claims, blocks, headings |
| `tb_resolve_member` | `repo`, `path` | which member owns the path, with root, role, editable and local |

## Content and graph

| tool | arguments | notes |
|---|---|---|
| `tb_files` | `repo?`, `scope?`, `limit`, `offset` | default scope `all`; ordered by path components; absolute paths possible |
| `tb_children` | `repo`, `path` | a directory's direct files and subdirectories (`dir_entries`), hidden excluded, catalogue-derived; null when the directory does not resolve |
| `tb_rollback_preview` | `repo`, `commit` | what reverting that commit would touch: per path `action` (`restore` or `delete`) and `state` (`clean`, `dirty`, `changed_since`), plus `safe` — false when any path is dirty or superseded. Read-only: a plan, not an action |
| `tb_graph_health` | `repo?`, `scope?`, `limit` | cycles, dead ends, sources, isolated files, broken links, and the most central nodes in one report; counts cover the whole graph, lists are capped (default 20, max 200) |
| `tb_watch` | `timeout_ms` | waits up to `timeout_ms` (default 10000, max 30000) for the next rebuild and returns its net file delta: `added`, `removed`, `modified`, `version`. A timeout is `timed_out: true`, an ordinary answer. It holds the call, so keep the timeout short |
| `tb_grep` | `query`, `repo?`, `scope?`, `case_sensitive?`, `limit`, `offset` | literal (not regex) search, one match per line with line and character column; case-insensitive unless asked; `total` covers the whole scanned set and `skipped_files` counts what the bounds left out (above 1 MiB, or binary); default limit 20, max 200 |
| `tb_hubs` | `repo?`, `scope?`, `limit`, `offset` | default limit 20; counts include edges from outside the scope |
| `tb_diagnostics` | `repo?`, `severity?`, `code?`, `scope?`, `limit`, `offset` | filters AND; every severity unless one is named |
| `tb_content` | `repo`, `path`, `max_chars?` | `{content: {text, hash, commit} \| null}`; with `max_chars` the text is windowed and the answer says `truncated`, `total_chars`, `next_offset`; the hash always covers the whole file |
| `tb_instances_of` | `type_name`, `origins?`, `instance?`, `body?`, `limit?`, `offset?` | bare name matches every identity; with paging the answer adds `total`, `offset`, `next_offset` |
| `tb_typed` | `repo`, `path` | classify a path as a typed instance (claim, resolution, closure), a type-def, or a plain note; answers `typed`, `kind`, and the instance fields when present |
| `tb_resolved` | `repo`, `path` | one instance's field values with provenance, claim, closure, and the candidates it could have claimed (`instance`); null when the path is not a parsed instance |
| `tb_validate_value` | `type_name`, `value`, `repo?` | one verdict per identity; unknown type gives `identity: null` with `unknown-type-claim`, never `[]` |

## Mutations

`tb_preview_mutation {repo, path, op, content?, old_string?, new_string?, replace_all?}`
returns the would-be product and blast radius; it writes nothing, needs no
grant and no hash, but the member must be editable. It covers the path-addressed
ops (`write_file`, `edit_file`, `delete_file`). The engine also previews
`rename_type`, which no path addresses: that op is what `tb_rename_type` calls
itself, and it answers an `affected` list besides the product.

`tb_rename_type` is the one structural mutation whose reach is not visible from
its arguments. It names a type; the engine decides which files the rename
rewrites, and a claim, a slot shape or a wikilink to the def can live in any
mounted member. So the server previews the cascade first, through the same
planning code the mutation runs, and reads `affected`: every file the rename
writes, with the member that owns it. When any of those members is outside the
launch's `members` allowlist the call is refused with `policy/member_grant`,
recorded as `denied:member_grant` against the member outside the grant, and the
message names the members and a few paths in each. The defining member being
allowed is not enough, and nothing is written on a refusal. The engine's own
consumed-member guard (`detail.blocking_consumed_referrers`) answers a different
question — the workspace's edit/consumed topology — so neither check replaces
the other.

`tb_create_file {repo, path, content}` makes a file that does not exist yet. It
has no text to compare against, so its gate replaces the read-then-hash pair with
the opposite promise: the path must be free. The server reads it first and
refuses with `policy/absence: file already exists; use tb_write_file` instead of
overwriting, and the engine re-checks absence where the write happens: the
mutation is `create_file`, one exclusive-create inside the serialized write, so a
second client creating the same path at the same instant is refused rather than
silently overwriting the first. Both refusals are the same `policy/absence`
message — the local read is the cheap one, the engine's is the guarantee.

`tb_write_file`, `tb_edit_file`, `tb_delete_file` share one gate: a prior
`tb_content` of the same path in the same process, a matching `expected_hash`,
an editable member, one mutation at a time. An engine rejection wrote nothing; a
transport failure is `outcome_unknown` and must not be retried blindly.

`expected_hash` means one thing to this server and another to the engine, and the
difference matters when two clients share the base. The server holds the hash the
`tb_content` read returned and refuses a mismatch; the engine, for `tb_edit_file`,
receives only `old_string`, so **its** precondition is that the text at that span
is what the caller saw. A concurrent change elsewhere in the file does not cancel
an edit, which is what two agents editing different sections of one note need.
`tb_write_file` is the whole-file compare-and-set: it passes `expected_hash` to
the engine, so any change at all since the read cancels it.

Structural mutations ride the same gate (prior read, hash, editable member, one
at a time) and each is one engine saga:

| tool | arguments | effect |
|---|---|---|
| `tb_assign_block_id` | `repo`, `path`, `at`, `expected_hash` | mints an id for the entity at a byte offset; returns `id` and its `[[target^id]]` ref; idempotent on an entity that already has one |
| `tb_rename_block_id` | `repo`, `path`, `block_id`, `to_block_id`, `expected_hash` | renames an inline record's id and carries every referrer |
| `tb_rename` | `repo`, `path`, `to`, `expected_hash` | moves a file inside its member and rewrites inbound references; refuses a type-def (use `tb_rename_type`) and a cross-member target |
| `tb_inline` | `repo`, `path`, `into`, `at?`, `expected_hash`, `into_expected_hash` | folds one file into a referrer as an inline record and deletes it; both files must be armed |
| `tb_promote` | `repo`, `path`, `to`, `at` xor `block_id`, `expected_hash` | lifts an inline record into its own file and leaves a reference |
| `tb_batch` | `repo`, `ops` (1–50 of write_file / edit_file / delete_file, each with its own `expected_hash`) | applies the ops in order as one engine saga: one commit per touched member, and a rejection on any op leaves every file as it was |
| `tb_rename_type` | `old_name`, `new_name`, `expected_hash` | renames a type-def and carries every claim, slot, and wikilink; the freshness check is the type's hash, and the grant must cover every member the cascade rewrites, not only the defining one |
| `tb_add_dep` / `tb_remove_dep` | `repo`, `dep`, `expected_hash?` | author `.arsumbris/repo.yaml`; requires the composition grant |

### Workspace shape

These three change *which* members a workspace serves, so they need
`allow_workspace_shape` on top of mutations. None of them touches a note, and the
engine rebuild is what makes the change observable.

| tool | arguments | effect |
|---|---|---|
| `tb_add_workspace_member` | `repo`, `name`, `role` (`edit` \| `discover`), `expected_hash?` | declares a member in the named list; refuses a name already declared in either list |
| `tb_remove_workspace_member` | `repo`, `name`, `expected_hash?` | drops the name from both lists, and from `disabled:` in the same splice; the containing repo cannot be removed |
| `tb_set_workspace_member_role` | `repo`, `name`, `role`, `expected_hash?` | moves a declared member between the lists in one commit; refuses an absent member, an unchanged role, and moving the host into `discover` |

`tb_reconcile {repo, path, expected_hash? | expect_absent?}` answers `applied` or
`not_applied` with the current hash and commit, and re-arms the guard when the
file differs, so a deliberate retry needs no extra read.

## Pagination and size

Paged reads: `tb_types`, `tb_hubs`, `tb_diagnostics`, `tb_files`, `tb_grep`,
`tb_instances`, `tb_instances_of`, `tb_candidates`. Unpaged reads can be large: `tb_type_tree`,
`tb_validate_value`, `tb_members`, `tb_imports`, `tb_subtypes`,
`tb_preview_mutation`, `tb_reconcile`.
`tb_content` and `tb_instances_of` take explicit budgets; the rest are bounded by
the 16 MiB wire frame, and an oversized frame fails rather than being truncated.
