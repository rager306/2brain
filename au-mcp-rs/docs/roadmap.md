# Roadmap

Status on 2026-11-13, after the unified binary (`97ae821`) and the runtime map
(`ac48835`). Phases come from [`runtime-map.md`](runtime-map.md).

## Done

- [x] Engine reads: 16 (`overview`, `top_level_dirs`, `type`, `type_tree`, `types`,
      `hubs`, `diagnostics`, `subtypes`, `members`, `imports`, `files`, `content`,
      `instances_of`, `validate_value`, `preview_mutation`, `reconcile`)
- [x] Mutations behind a policy gate: `write_file`, `edit_file`, `delete_file`
- [x] Discovery: `search_tools`, `describe_tool`; profiles `--tools`, `--payload`
- [x] Protocol: four legacy revisions and `2026-07-28`, including the pre-init
      `ping` rule
- [x] Faults: lost mutation reply, engine death mid-read, truncated frame
- [x] Budgets: `au_content {max_chars}`, `au_instances_of {limit, offset}`
- [x] Contract evidence: synthetic read corpus, production golden replay,
      client matrix, bridge parity, rollback rehearsal
- [x] One binary: `arsumbris` (`daemon`, `open`, `members` delegated to `au_cli`;
      `mcp`, `info` delegated to this crate)
- [x] Agent skill: `skills/arsumbris`, installed by `scripts/install_skill.sh`

## Phase A — remaining engine reads

Each is one argument struct, one wire read, one schema, one captured fixture.

- [x] `au_frontmatter` — `frontmatter`
- [x] `au_anchors` — `anchors`
- [x] `au_block_ids` — `block_ids`
- [x] `au_references` — `references_out` and `references_in`
- [x] `au_neighborhood` — `neighborhood`
- [ ] `au_instances` — `instances` (unpaged upstream; needs a page)
- [ ] `au_candidates` — `candidates` (+ `candidate_counts`)
- [ ] `au_semantic_tokens` — `semantic_tokens`
- [ ] `au_resolve_member` — `resolve_member`
- [ ] `au_typed` — `instance`
- [ ] `au_follow` — `resolve_target`; pin the argument semantics first
- [ ] `au_children` — `references_out` filtered; pin the rule first
- [ ] `au_resolved` — `resolve_target` / `resolve_block_id` / `resolve_anchor`
- [ ] `au_type_system` — `type_batch` / `type_closure`; pin the projection first

## Phase B — engine mutations

Behind the existing gate, one at a time, each with a blast-radius story:

- [ ] `assign_block_id`, `inline`, `promote`
- [ ] `rename`, `rename_block_id`, `rename_type`
- [ ] `add_dep`, `remove_dep`
- [ ] `add_workspace_member`, `remove_workspace_member`, `set_workspace_member_role`
      — these change what the workspace *is*; they need a policy beyond the
      member allowlist before they exist at all

## Phase C — the host decision

`bash`, `glob`, `grep_files`, `read_file_pinned`, `au_host_*`. Either they stay a
host capability, or `arsumbris host` gets an explicit root and no shell. Not
decided.

## Phase D — retire the Node stack

After A and B prove parity on a real client: `au-mcp`, `au-mcp-sdk`,
`au-type-codegen`, `au-engine-sdk`, `au-mcp-bridge`, `au-mcp-adapter-cc`,
`au-mcp-adapter-codex`. The open question is extensions: a workspace can declare
`mcp.tool` plugins today, and a compiled binary cannot; if that matters, the
replacement is an explicit process-per-plugin contract.

## Standing checks

Every slice ends with `cargo fmt --check`, the full `cargo test --offline`,
`clippy -D warnings`, the ignored real-engine test, and a regenerated fixture
corpus when a wire answer changed.
