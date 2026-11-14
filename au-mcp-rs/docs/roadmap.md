# Roadmap

Status on 2026-11-13, HEAD `aba2b4f`. Verified against the code, not the notes:
46 tools registered (31 advertised read-only, 15 mutations), 26 captured fixtures,
147 tests green, ten documents.

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
- [x] `create_file` — the gate's opposite promise: absence instead of
      read-then-hash, so a create cannot overwrite. Found from the vault backlog
      note P0-C, which recorded a live session falling back to the daemon because
      the Rust surface could not create a file
- [x] `add_workspace_member`, `remove_workspace_member`, `set_workspace_member_role`
      — behind a **third grant** (`allow_workspace_shape`): they change which
      members the workspace serves, so a mistake alters every later read rather
      than one note. `tests/composition.rs` pins that the grant is independent of
      the composition grant and that a launch without it neither advertises nor
      accepts the three verbs.

## Phase C — the host decision

**Decided: host tools stay a client capability.** The server carries engine reads
and gated mutations; it does not carry a shell, a file walker, or a content
scanner. The reasons are in [`runtime-map.md`](runtime-map.md): the live bridge's
host tools are stubs and its own agent manual routes content search outside MCP,
every client already has `bash`/`grep` over the same filesystem, and a server that
serves a vault plus a shell turns a note-editing grant into arbitrary code
execution as the socket's owner.

If a client with no local filesystem appears, the replacement is an engine
`grep`/`search` read — the index's semantics, not a host process — and that is a
wire change, recorded here as the first candidate when search is measured as a
bottleneck.

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
