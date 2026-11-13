# Runtime map: what runs, and what is left outside Rust

Measured on the bundle, 2026-11-13. The goal is not "rewrite everything"; it is to
know which module owns which job, and which ones the unified Rust binary can
absorb without losing behaviour.

## Today's modules

| module | language | size | role | in the served path |
|---|---|---|---|---|
| `au-engine` | Rust | 1853 files, 714 MB (with `target/`) | the engine: parser, type graph, diagnostics, daemon, wire | **yes, the authority** |
| `au-mcp-rs` | Rust | 3 645 LOC | MCP server over the engine wire; one binary `arsumbris` | yes, as the new front-end |
| `au-mcp` | TypeScript | 5 586 LOC | Node daemon: sessions, plugin discovery, projection tools | yes, behind the bridge |
| `au-mcp-core` | TypeScript | 82 `.ts`, 49 tool defs, 6 hooks | the plugin package the daemon discovers | yes |
| `au-mcp-sdk` | TypeScript | 2 708 LOC | `mcp.*` base types, contracts, session vocabulary | yes |
| `au-type-codegen` | TypeScript | 1 764 LOC | type-def → JSON Schema / TS codegen | yes, drawn in by SDK and daemon |
| `au-engine-sdk` | TypeScript | 8 272 LOC | engine client for Node: wire, repo resolution, helpers | yes |
| `au-mcp-bridge` | Python | 293 LOC | stdio MCP → Node daemon socket | yes, the client's entry |
| `au-mcp-adapter-cc` | TypeScript | 1 516 LOC | Claude Code hooks ↔ daemon | client side |
| `au-mcp-adapter-codex` | TypeScript | 2 033 LOC | Codex hooks ↔ daemon | client side |
| `au-ingest` | Node scripts | 14 `.mjs` | source → markdown converters | offline toolchain |
| `au-type-system`, `au-base-types`, `au-rules`, `au-skills`, `au-agent-guides`, `au-writing-style`, `au-competency`, `au-defaults`, `au-govern` | YAML + Markdown | 12–145 files | vocabulary, rules, guides, templates, audits | content, mounted as members |

Roughly 22 000 lines of TypeScript and 293 lines of Python sit in the served path
today. The Rust replacement is 3 645 lines.

## Tool surface: 49 core tools, 21 Rust tools

Of the 49 `mcp.tool` definitions in `au-mcp-core`, 16 have a Rust counterpart
(two under a clearer name: `au_list_imports` → `au_imports`, `au_validate` →
`au_validate_value`). The remaining 33 split into three groups.

### A. Engine reads with no Rust tool yet — 14

Each is a wire verb the engine already serves, so the work is an argument struct,
a read call, a schema, and a fixture.

| daemon tool | engine read | note |
|---|---|---|
| `au_frontmatter` | `frontmatter` | direct |
| `au_anchors` | `anchors` | direct |
| `au_block_ids` | `block_ids` | direct |
| `au_references` | `references_out` + `references_in` | two reads, one answer |
| `au_neighborhood` | `neighborhood` | direct |
| `au_instances` | `instances` | direct, large answer |
| `au_candidates` | `candidates` (+ `candidate_counts`) | direct |
| `au_semantic_tokens` | `semantic_tokens` | direct |
| `au_resolve_member` | `resolve_member` | direct |
| `au_follow` | `resolve_target` | needs its argument semantics pinned |
| `au_children` | `references_out` filtered | needs the current rule, not a guess |
| `au_resolved` | `resolve_target` / `resolve_block_id` / `resolve_anchor` | one tool over three verbs |
| `au_typed` | `instance` | the resolved view of one instance |
| `au_type_system` | `type_batch` / `type_closure` | needs the daemon's projection rule |

### B. Engine mutations with no Rust tool yet — 11

The engine serves these verbs; each needs its own path scope and blast-radius
story before it can sit behind the mutation gate:

`assign_block_id`, `inline`, `promote`, `rename`, `rename_block_id`,
`rename_type`, `add_dep`, `remove_dep`, `add_workspace_member`,
`remove_workspace_member`, `set_workspace_member_role`.

Three of them are workspace-level (`add/remove member`, `set role`) and change
what the *served* workspace contains, so they need a policy decision beyond the
current member allowlist: who may reshape the workspace itself.

### C. Host tools that are not engine reads — 8

`bash`, `glob`, `grep_files`, `read_file_pinned`, and the four `au_host_*` tools
(intents, pane operations, snapshot). These need a host process; the engine
serves none of them. Options, in order of safety:

1. leave them to the host and keep the Node daemon for that slice;
2. add an explicit `arsumbris host` subcommand with a configured root and no
   shell, covering `glob` and `grep_files` only;
3. never expose `bash` from a server that also serves a vault.

## Phases

| phase | work | risk |
|---|---|---|
| A | 14 reads from group A, each with a captured fixture | low: pure mapping, no new authority |
| B | 11 mutations from group B, behind the existing gate, one at a time | medium: rename/move semantics, workspace reshaping |
| C | decide group C: keep in host, or a scoped `arsumbris host` | product decision |
| D | retire `au-mcp`, `au-mcp-sdk`, `au-type-codegen`, `au-engine-sdk`, `au-mcp-bridge`, both adapters once clients point at `arsumbris mcp` | after A and B prove parity |
| E | content repos unchanged; `au-ingest` converters stay a toolchain, not a server | none |

Phase A is the cheapest and the most useful: it closes the read gap that makes
an agent fall back to the bridge. Phase B is where the care is. Phase D is a
deletion, and it should happen last.

## What stays non-Rust on purpose

- **Content**: `au-type-system`, `au-base-types`, `au-rules`, `au-skills`,
  `au-agent-guides`, `au-writing-style`, `au-competency`, `au-defaults`,
  `au-govern`. These are the workspace's knowledge, not a runtime.
- **Ingest converters**: they turn PDFs and other sources into markdown offline.
  A server does not need them.
- **Client adapters**: they bridge a specific client's hook system. If a client
  can speak MCP to `arsumbris` directly, they have no job left.

## What retiring the Node stack would save

- ~22 000 lines of TypeScript and the `bun`/`node` runtime in the served path;
- the plugin discovery machinery: 49 type-defs and 66 entry modules loaded at
  daemon start;
- the two-hop path (client → bridge → daemon → engine) becomes one hop
  (client → `arsumbris` → engine).

It also costs: the plugin model itself. A workspace can define new `mcp.tool`
plugins in markdown and have them discovered; a compiled binary cannot. If that
extensibility matters, the answer is a narrow, explicit plugin surface in Rust
(process-per-plugin with a documented contract), not a return to Node.
