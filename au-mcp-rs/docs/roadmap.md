# Roadmap

Status on 2026-11-13, HEAD `aba2b4f`. Verified against the code, not the notes:
45 tools registered (31 advertised read-only, 14 mutations), 26 captured fixtures,
144 tests green, ten documents.

## Done — with evidence

| item | evidence |
|---|---|
| 16 engine reads | `src/lib.rs`: overview, top_level_dirs, type, type_tree, types, hubs, diagnostics, subtypes, members, imports, files, content, instances_of, validate_value, preview_mutation, reconcile |
| 3 mutations behind a gate | `write_file`, `edit_file`, `delete_file`; `tests/policy.rs`, `tests/write_file*.rs` |
| 5 addressability/link reads | `frontmatter`, `anchors`, `block_ids`, `references`, `neighborhood`; `tests/links.rs`, fixtures |
| 4 scan/resolution reads | `instances`, `candidates`, `semantic_tokens`, `resolve_member`; `tests/scan.rs`, fixtures |
| 4 classification reads | `typed`, `resolved`, `children`, `follow`; `tests/scan.rs`, `tests/links.rs` |
| build identity | `--version` carries `+<commit>[.dirty]`; `build.rs`, `tests/cli.rs` |
| discovery and profiles | `au_search_tools`, `au_describe_tool`, `--tools full\|concise`, `--payload both\|text\|structured`; `tests/discovery.rs` |
| protocol | four legacy revisions and `2026-07-28`, pre-init `ping` rule; `tests/stdio.rs`, `tests/modern_stdio.rs` |
| faults | lost mutation reply, engine death mid-read, truncated frame; `tests/faults.rs` |
| budgets | `au_content {max_chars}`, `au_instances_of {limit, offset}`; `tests/budgets.rs` |
| contracts | synthetic corpus (21), production golden replay, client matrix, bridge parity, rollback rehearsal |
| one binary | `arsumbris` (delegates `daemon`/`open`/`members` to `au_cli`, `mcp`/`info` here); `tests/cli.rs` |
| agent skill | `skills/arsumbris`, installed by `scripts/install_skill.sh` |
| trajectory | `docs/roadmap.md`, `docs/runtime-map.md` |

## Phase A — remaining engine reads

- [x] `au_frontmatter` — `frontmatter`
- [x] `au_anchors` — `anchors` (local form: empty target + origin)
- [x] `au_block_ids` — `block_ids`
- [x] `au_references` — `references_out` + `references_in`
- [x] `au_neighborhood` — `neighborhood`
- [x] `au_instances` — `instances` (payload carries `count`, `aborted_at_load`)
- [x] `au_candidates` — `candidates` (its own `limit`/`offset`)
- [x] `au_semantic_tokens` — `semantic_tokens`
- [x] `au_resolve_member` — `resolve_member`
- [ ] `au_typed` — `instance`
- [x] `au_follow` — `resolve_target` / `resolve_block_id` / `resolve_anchor`, plus a line peek
- [ ] `au_children` — `references_out` filtered; pin the rule first
- [ ] `au_resolved` — `resolve_target` / `resolve_block_id` / `resolve_anchor`
- [-] `au_type_system` — **out of scope**: it reads typed `doc` atoms from the `au-mcp-type-knowledge` content package, not the engine

## Phase B — engine mutations

- [x] `assign_block_id`, `rename_block_id`, `rename` — behind the gate, with `tests/structural.rs`
- [x] `inline`, `promote` — two-file gate: both files armed, one slot for the pair
- [x] `rename_type` — type-hash gate (the type has no path; its `hash` from au_type is the freshness check)
- [x] `add_dep`, `remove_dep` — behind a **second grant** (`allow_composition`), because they author `.arsumbris/repo.yaml`
- [x] `add_workspace_member`, `remove_workspace_member`, `set_workspace_member_role`
      — behind a **third grant** (`allow_workspace_shape`): they change which
      members the workspace serves, so a mistake alters every later read rather
      than one note. `tests/composition.rs` pins that the grant is independent of
      the composition grant and that a launch without it neither advertises nor
      accepts the three verbs.

## Phase C — the host decision

`bash`, `glob`, `grep_files`, `read_file_pinned`, `au_host_*`. Either they stay a
host capability, or `arsumbris host` gets an explicit root and no shell. Not
decided, and not needed while the bridge stays available.

## Phase D — retire the Node stack

After A and B prove parity on a real client: `au-mcp`, `au-mcp-sdk`,
`au-type-codegen`, `au-engine-sdk`, `au-mcp-bridge`, both client adapters. The
open question is extensions: a workspace can declare `mcp.tool` plugins today, and
a compiled binary cannot. If that matters, the replacement is an explicit
process-per-plugin contract, not a return to Node.

## Standing checks

Every slice ends with `cargo fmt --check`, the full `cargo test --offline`,
`clippy -D warnings`, the ignored real-engine test, and a regenerated fixture
corpus when a wire answer changed.
