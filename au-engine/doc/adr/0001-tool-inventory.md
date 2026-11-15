# ADR 0001: Source inventory of MCP tools and hooks

> **Note 2026-11-16.** The inventory below predates the removal of the TypeScript MCP layer; the tools
> it lists as daemon-side no longer exist. The Rust surface is the one in
> `au-mcp-rs/skills/arsumbris/references/tool-map.md`.

Status: proposed. This is a source-only inventory and a proposal for a future
port, not an assertion that any item is mounted, enabled, called, or used at
runtime. No compatibility or migration decision is implied by a `port` label.

## Scope and method

The baseline is the 49 `mcp.tool.*` and six `mcp.hook.*` type definitions in
`au-mcp-core/type/`. Names below are the short names in those filenames; they
are not a claim about an adapter's exposed name or prefix. Definitions point to
their implementation entries in `au-mcp-core/src/`. The plugin discovery and
loader are in `au-mcp/src/daemon/discovery.ts` and `au-mcp/src/plugins/`;
`au-mcp-core/src/index.ts` explains that definitions, not a barrel export, are
the load-path source. External contributed plugins are outside the 49/6 count.

Proposed statuses mean:

- **port**: preserve the capability and its observable contract, subject to
  explicit parity checks and the target's authorization model.
- **redesign**: preserve the need, but first design the boundary, safety,
  session semantics, dependencies, or target-specific integration.
- **defer**: do not include in the initial port; revisit when its dependency or
  a concrete requirement exists. This does not assert lack of use or value.

Categories and statuses are migration judgments based on source, not measured
usage. `broker: read` in metadata scopes the engine broker available to a
plugin; it is **not a guarantee that the plugin is read-only overall**. For
example, `au_host_intent_fire` and `au_host_pane_op` declare `broker: read`,
but `au-mcp-core/src/intent-tools.ts` relays commands to a host socket and can
change UI state. Plugins can have effects outside the broker; review their full
implementation and execution boundary before granting authority.

## Tools (49)

### Engine reads and validation (27)

Proposed **port**: these definitions describe the core graph, type, navigation,
and validation surface. Preserve argument, paging, scope, error and response
semantics, rather than assuming a same-named engine operation is equivalent.
`au_type_system` depends on a separately mounted knowledge package; preserve
that dependency explicitly. `au_semantic_tokens` is editor-oriented, so it is
proposed for later delivery instead of the initial port.

| Name | Category | Proposed status |
| --- | --- | --- |
| `au_anchors` | Link navigation | port |
| `au_block_ids` | Link navigation | port |
| `au_candidates` | Type induction | port |
| `au_children` | File catalog | port |
| `au_diagnostics` | Diagnostics | port |
| `au_files` | File catalog | port |
| `au_follow` | Link navigation | port |
| `au_frontmatter` | File metadata | port |
| `au_hubs` | Graph orientation | port |
| `au_instances` | Instance discovery | port |
| `au_instances_of` | Instance discovery | port |
| `au_list_imports` | Workspace discovery | port |
| `au_members` | Workspace discovery | port |
| `au_neighborhood` | Graph navigation | port |
| `au_overview` | Graph orientation | port |
| `au_references` | Graph navigation | port |
| `au_resolve_member` | Workspace discovery | port |
| `au_resolved` | Instance inspection | port |
| `au_semantic_tokens` | Editor integration | defer |
| `au_subtypes` | Type discovery | port |
| `au_top_level_dirs` | Graph orientation | port |
| `au_type` | Type inspection | port |
| `au_type_system` | Type-system reference | port |
| `au_type_tree` | Type discovery | port |
| `au_typed` | Instance inspection | port |
| `au_types` | Type discovery | port |
| `au_validate` | Candidate validation | port |

Source: corresponding `au-mcp-core/type/mcp.tool.<name>.type.yaml` and
`au-mcp-core/src/tool-au-*.ts`, with shared dispatch in
`au-mcp-core/src/engine-reads.ts` and `au-mcp-core/src/instances.ts`.

### Host integration (4)

Proposed **defer**: all four depend on the separate host composition and its
relay protocol. The two command tools have effects beyond the engine broker;
an eventual integration needs explicit authority and host-protocol review.

| Name | Category | Proposed status |
| --- | --- | --- |
| `au_host_intent_fire` | Host command | defer |
| `au_host_intent_list` | Host discovery | defer |
| `au_host_pane_op` | Host command | defer |
| `au_host_snapshot` | Host inspection | defer |

Source: the four corresponding `au-mcp-core/type/mcp.tool.au_host_*.type.yaml`
definitions, `au-mcp-core/src/intent-tools.ts`, `host-client.ts`, and
`host-protocol.ts`.

### Files, shell and search (7)

Proposed **port** for pinned reads and search, retaining source-specific scope,
matching, pagination and result behavior. Proposed **redesign** for writes and
deletion: the mutation channel, editable-member boundary, stale-read behavior,
authorization and failure semantics must be preserved or deliberately changed.
The `bash` definition and implementation describe an unavailable shell and
record attempted demand; defer it rather than silently introducing execution.

| Name | Category | Proposed status |
| --- | --- | --- |
| `bash` | Shell placeholder | defer |
| `delete_file` | File mutation | redesign |
| `edit_file` | File mutation | redesign |
| `glob` | File search | port |
| `grep_files` | Content search | port |
| `read_file_pinned` | Governed file read | port |
| `write_file` | File mutation | redesign |

Source: their definitions in
`au-mcp-core/type/`, individual entries in `au-mcp-core/src/tool-*.ts`, and
`au-mcp-core/src/file-tools.ts`.

### Graph-aware mutations (6)

Proposed **redesign**: these are more than ordinary file operations. They can
change references, multiple files, identities or record structure; preserve
engine transaction, precondition, collision, diagnostics and recovery behavior
before exposing them in another host.

| Name | Category | Proposed status |
| --- | --- | --- |
| `assign_block_id` | Block identity mutation | redesign |
| `inline` | Record transformation | redesign |
| `promote` | Record transformation | redesign |
| `rename` | Graph-aware rename | redesign |
| `rename_block_id` | Block identity mutation | redesign |
| `rename_type` | Type identity mutation | redesign |

Source: corresponding `au-mcp-core/type/mcp.tool.<name>.type.yaml` and
`au-mcp-core/src/tool-*.ts` entries; engine mutation semantics live in
`au-engine/crates/au-engine/src/`.

### Workspace composition (5)

Proposed **redesign**: these author dependency and workspace manifests and may
change the mounted graph and write authority. Specify validation, editable-root
selection, rebuild behavior and failure handling before porting.

| Name | Category | Proposed status |
| --- | --- | --- |
| `add_dep` | Dependency composition | redesign |
| `add_workspace_member` | Workspace composition | redesign |
| `remove_dep` | Dependency composition | redesign |
| `remove_workspace_member` | Workspace composition | redesign |
| `set_workspace_member_role` | Workspace composition | redesign |

Source: corresponding `au-mcp-core/type/mcp.tool.<name>.type.yaml`,
`au-mcp-core/src/tool-*.ts`, and `au-mcp-core/src/composition-tools.ts`.

## Hooks (6)

These are hooks, not tools. Mediation requires trusted session state and a
fail-closed design where a floor is required; porting a function alone is not
equivalent to porting its enforcement. Session-start notices are policy and may
be revisited after a target session model exists.

| Name | Category | Proposed status | Basis |
| --- | --- | --- | --- |
| `read-guard` | Mediator / safety floor | redesign | Guards overwrites using the session read hash and current content hash; creates and explicit hash preconditions follow different paths. |
| `tool-precondition` | Mediator / safety floor | redesign | Gates calls on declared read prerequisites and observed read/served state; requires trustworthy session tracking. |
| `nativeToolRedirect` | Mediator / native-tool gate | redesign | Enforces a configured native-tool allowlist only when a gate prefix exists; requires adapter-specific coverage. |
| `instance-count-notice` | Session-start policy | defer | Injects a configurable type-count notice; inert without configuration. |
| `repo-overview` | Session-start policy | defer | Builds orientation from workspace members and README metadata. |
| `requiredScope-check` | Session-start policy | defer | Warns about required tools/hooks that are unmounted or inactive, based on session scope. |

Source: six `au-mcp-core/type/mcp.hook.*.type.yaml` definitions and their
`plugin-runtime-meta.entry` implementations in `au-mcp-core/src/`. In
particular, critical mediator floors and noncritical session-start policies are
different failure domains, not interchangeable extension callbacks.

## External contributions and adapters

These locally present definitions are separate contributions, not members of
the 49-tool baseline. Inclusion in a future deployment depends on mounting,
discovery and policy; this list does not claim they are active. Each needs its
own contract, dependency and effect review.

| Definition | Repository | Category | Proposed status | Basis |
| --- | --- | --- | --- | --- |
| `mcp.tool.au_agent_guide` | `au-agent-guides` | Guide discovery | defer | Depends on mounted guide content and cross-package discovery. |
| `mcp.tool.ingest` | `au-ingest` | Ingestion and note mutation | defer | Has converter, filesystem and write effects beyond the baseline. |
| `mcp.tool.refresh_research_map` | `au-tree-research` | Research map mutation | defer | Domain-specific generated map and editable-member requirements. |
| `mcp.tool.weave_map_coverage` | `au-weave` | Coverage analysis | defer | Domain-specific graph and indexing requirements. |

`au-mcp-adapter-cc/type/mcp.adapter.cc.type.yaml` and
`au-mcp-adapter-codex/type/mcp.adapter.codex.type.yaml` describe adapters, not
tools or hooks in the baseline. Their launcher and agent integration are also
deferred pending a target adapter design.

## Consequences and verification

This inventory does not authorize a daemon replacement, change vault formats,
or establish wire compatibility. An implementation must independently compare
schemas and effect boundaries, test on fixtures rather than the production
vault, and pass the engine golden wire contract before any live engine swap.
Any intentional wire-shape change must follow `crates/au-engine/WIRE.md`.
