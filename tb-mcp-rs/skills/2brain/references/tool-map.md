# Tool map — the 52 tools of the `2brain` adapter

Generated from the running binary rather than hand-maintained: the table below is what
`tools/list` actually advertises with every grant, and the capability split is read from
`src/capability.rs` (the one place a tool is classified; a test fails when a tool has no row).

Regenerate with:

```sh
/usr/local/bin/2brain mcp /root/vault --allow-mutations --allow-composition \
  --allow-workspace-shape                        # concise is the default; read tools/list
```

Counts: **33 read + 16 mutate + 2 discover + 1 watch = 52**. Without `--allow-mutations` a launch
advertises 36; with mutations alone 47; composition adds 2; workspace shape adds 3.

## Reads (33) — no grant needed

| tool | arguments | answers |
|---|---|---|
| `tb_anchors` | `path`, `repo` | List every heading in one file, in document order, as the anchors a `#fragment` can address: text, level, and span. |
| `tb_block_ids` | `path`, `repo` | List every addressable block id in one file, in document order, with kind (record, typed_block, marker), type claim, and span. |
| `tb_candidates` | `limit`, `offset`, `summary` | List the files that satisfy a type's required fields without claiming it, grouped by file, with the candidate type names and the fields that |
| `tb_children` | `path`, `repo` | List a directory's direct entries: files and subdirectories, hidden excluded, catalog-derived so only directories holding a catalogued file |
| `tb_content` | `max_chars`, `path`, `repo` | Read one member's file text, resolved from the member's declared root, with the content hash that guards a later write and the HEAD commit o |
| `tb_diagnostics` | `code`, `limit`, `offset`, `repo`, `scope`, `severity` | List a page of diagnostics. |
| `tb_files` | `limit`, `offset`, `repo`, `scope` | List a page of catalogued files, including assets and dependencies by default. |
| `tb_follow` | `origin_path`, `peek`, `repo`, `target` | Resolve one wikilink string, including one you are about to write: does this address resolve, and what is there? target carries the link (`n |
| `tb_frontmatter` | `path`, `repo` | Read one file's parsed frontmatter as JSON. |
| `tb_graph_health` | `limit`, `repo`, `scope` | Report what is wrong with the graph in one answer: directed cycles (strongly connected components, shortest first), dead ends (nothing links |
| `tb_grep` | `case_sensitive`, `limit`, `offset`, `query`, `repo`, `scope` | Search the catalogued files for literal text, one match per line. |
| `tb_hubs` | `limit`, `offset`, `repo`, `scope` | List a ranked page of hubs. |
| `tb_imports` | `scope` | List resolved cross-repo type imports. |
| `tb_instances` | `limit`, `offset` | List every resolved instance in the workspace, with its file, claim, closure, effective shape and values, and addressable inline records. |
| `tb_instances_of` | `body`, `instance`, `limit`, `offset`, `origins`, `type_name` | List every instance matching a type. |
| `tb_members` | `repo` | List workspace members, optionally selecting an exact repo. |
| `tb_neighborhood` | `depth`, `direction`, `kinds`, `max_nodes`, `path`, `repo`, `scope` | Walk the reference graph from one seed file and return the reachable subgraph (nodes plus edges). |
| `tb_overview` | — | Get the workspace overview. |
| `tb_preview_mutation` | `content`, `new_string`, `old_string`, `op`, `path`, `replace_all`, `repo` | Preview a write, edit, or delete without changing anything. |
| `tb_reconcile` | `expect_absent`, `expected_hash`, `path`, `repo` | Check what actually happened after an outcome_unknown mutation. |
| `tb_references` | `direction`, `path`, `repo` | List one file's wikilink edges, classified by kind and surface: outgoing (what it links to) and incoming (what links to it), each with span |
| `tb_resolve_member` | `path`, `repo` | Resolve which declared member owns a member-relative path, with its root, role, and whether it is editable and local. |
| `tb_resolved` | `path`, `repo` | Resolve one instance: field values with their contribution provenance, the type claim and closure, and the candidates it could have claimed. |
| `tb_rollback_preview` | `commit`, `repo` | Preview reverting one commit without touching anything: the paths it changed, whether a revert would restore the parent's bytes or delete th |
| `tb_search` | `exact`, `limit`, `min_should_match`, `offset`, `query`, `repo`, `scope` | Rank notes by relevance to a query instead of scanning them. |
| `tb_semantic_tokens` | `path`, `repo` | Read one file's semantic token stream: resolved and broken wikilinks, pinned commits, typed field values, type claims with the identity they |
| `tb_subtypes` | `base`, `scope` | List all direct and transitive subtypes of a base type. |
| `tb_top_level_dirs` | — | List top-level directories in own scope. |
| `tb_type` | `name`, `repo` | Get one type definition by name, across the workspace or in a named member. |
| `tb_type_tree` | — | Get the complete, unpaged type tree for the whole workspace, including dependencies. |
| `tb_typed` | `path`, `repo` | Classify a path: a typed instance (with its claim, whether it resolves, and its closure), a type-def, or a plain note. |
| `tb_types` | `limit`, `offset`, `repo`, `summary` | List a name-sorted page of types across the workspace or in a named member. |
| `tb_validate_value` | `repo`, `type_name`, `value` | Validate a value against a type without touching disk. |

## Mutations (16) — require `--allow-mutations`

| tool | arguments | answers |
|---|---|---|
| `tb_add_dep` | `dep`, `expected_hash`, `ref`, `remote`, `repo` | Declare a dependency in one member's .arsumbris/repo.yaml, as a byte splice that keeps comments and order. |
| `tb_add_workspace_member` | `expected_hash`, `name`, `repo`, `role` | Declare a member in one workspace's .arsumbris/workspace.yaml, in the edit or discover list. |
| `tb_assign_block_id` | `at`, `expected_hash`, `path`, `repo` | Mint a block id for the entity at a byte offset and return the id and its [[target^id]] ref. |
| `tb_batch` | `ops`, `repo` | Apply 1 to 50 file operations (write_file, edit_file, delete_file) as one all-or-nothing saga: every op passed the same gate it would pass a |
| `tb_create_file` | `content`, `path`, `repo` | Create a new file in a member. |
| `tb_delete_file` | `expected_hash`, `path`, `repo` | Delete one member's file after a successful tb_content read of the same path. |
| `tb_edit_file` | `expected_hash`, `new_string`, `old_string`, `path`, `replace_all`, `repo` | Replace one exact string in a member's file after a successful tb_content read of the same path. |
| `tb_inline` | `at`, `expected_hash`, `into`, `into_expected_hash`, `path`, `repo` | Fold one file into a referrer as an inline record and delete it, as one saga: the chosen [[path]] becomes the record, other whole-file links |
| `tb_promote` | `at`, `block_id`, `expected_hash`, `path`, `repo`, `to` | Lift an inline record into its own file and leave a reference where it sat, as one saga: every [[host^id]] referrer follows to the new file, |
| `tb_remove_dep` | `dep`, `expected_hash`, `repo` | Remove a dependency from one member's .arsumbris/repo.yaml, as a byte splice that keeps siblings and comments; a sole entry collapses deps t |
| `tb_remove_workspace_member` | `expected_hash`, `name`, `repo` | Remove a member from one workspace's edit and discover lists. |
| `tb_rename` | `expected_hash`, `path`, `repo`, `to` | Move a file inside its member and rewrite every inbound reference to the new name, preserving each referrer's spelling, fragments, and ::rep |
| `tb_rename_block_id` | `block_id`, `expected_hash`, `path`, `repo`, `to_block_id` | Rename an inline record's block id and carry every referrer with it: the ^: declaration and each [[host^id]] / [[^id]] rewrite as one mutati |
| `tb_rename_type` | `expected_hash`, `new_name`, `old_name` | Rename a type-def and carry every reference with it, as one saga: the def file moves and claims, slot shapes, meta, and wikilinks follow. |
| `tb_set_workspace_member_role` | `expected_hash`, `name`, `repo`, `role` | Move a declared member between the edit and discover lists in one commit. |
| `tb_write_file` | `content`, `expected_hash`, `path`, `repo` | Write one member's file text after a successful tb_content read of the same path. |

Five of those need one more grant: `tb_add_dep`, `tb_remove_dep` require `--allow-composition`;
`tb_add_workspace_member`, `tb_remove_workspace_member`, `tb_set_workspace_member_role` require
`--allow-workspace-shape`. Without the grant the tool is absent from `tools/list` and a call is
refused with `policy/member_grant`.

## Discovery (2) — answered from the catalogue, no engine contact

| tool | arguments | answers |
|---|---|---|
| `tb_describe_tool` | `name` | Get one tool's full definition: description, input and output schemas, and annotations. |
| `tb_search_tools` | `capability`, `detail`, `limit`, `query` | Find tools by words from their name, description, or parameter names. |

## Watch (1) — a change subscription

| tool | arguments | answers |
|---|---|---|
| `tb_watch` | `timeout_ms` | Wait for the next change to the workspace, up to timeout_ms (default 10000, max 30000). |

## Resources

`2brain://<repo>/<path>` is a catalogued file a client can hold and re-read without a tool call:
`resources/list` pages the catalogue, `resources/read` returns the text. There is no resource
subscription — `tb_watch` is the change signal. The scheme moved to `2brain://` on 2026-11-16 with the
legacy `arsumbris://` still accepted on parse for one release, so a URI a client cached keeps working
while everything newly emitted carries the current name.

## What is not here

The retired Node daemon's surface (`bash`, `glob`, `grep_files`, the four `au_host_*` relays,
`au_type_system`) is deliberately absent — the reasons are in `docs/internal/serving-parity.md` §2b. `tb_grep`
is literal text over catalogued files; it is not a regex or glob tool.
