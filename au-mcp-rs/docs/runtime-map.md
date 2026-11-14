# Runtime map: what runs, and what is left outside Rust

Measured on the bundle, 2026-11-14. The goal is not "rewrite everything"; it is to
know which module owns which job, and which ones the unified Rust binary can
absorb without losing behaviour.

## Today's modules

| module | language | size | role | in the served path |
|---|---|---|---|---|
| `au-engine` | Rust | 1853 files, 714 MB (with `target/`) | the engine: parser, type graph, diagnostics, daemon, wire | **yes, the authority** |
| `au-mcp-rs` | Rust | 6 848 LOC `src/` + 8 204 LOC of tests | MCP server over the engine wire; one binary `arsumbris` | yes, as the new front-end |
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

Roughly 26 100 lines of TypeScript (counted over `src/` of the seven packages) and
293 lines of Python sit in the served path today. The Rust replacement is 6 848
lines, with 8 204 lines of tests — measured 2026-11-14 with `find <pkg>/src -name
'*.ts' | xargs cat | wc -l`, the same shape the table's per-package numbers use.

## Tool surface: 49 daemon tools, 51 Rust tools

Of the 49 `mcp.tool` definitions in `au-mcp-core`, 41 have a Rust counterpart
(three under a clearer name: `au_list_imports` → `au_imports`, `au_validate` →
`au_validate_value`, `grep_files` → `au_grep`). The 8 that remain are group C;
groups A and B were closed earlier.

The Rust surface is 51 because ten tools answer questions the daemon never asked:
`au_batch`, `au_create_file`, `au_grep` (the engine read, not the daemon's host
`grep_files`), `au_graph_health`, `au_rollback_preview`, `au_watch`, `au_reconcile`,
`au_preview_mutation`, `au_content`, and the two discovery tools `au_search_tools`
and `au_describe_tool`. Counted against the code on 2026-11-14, not against these
notes.

### A. Engine reads — closed

All 13 reads are in the Rust surface (`au_frontmatter`, `au_anchors`,
`au_block_ids`, `au_references`, `au_neighborhood`, `au_instances`,
`au_candidates`, `au_semantic_tokens`, `au_resolve_member`, `au_typed`,
`au_resolved`, `au_children`, `au_follow`), each with a test that pins the shape;
most also have a captured fixture in `tests/fixtures/synthetic-reads/`.

One daemon tool is deliberately out of scope: `au_type_system` reads `doc` atoms
from a content package (`au-mcp-type-knowledge`), not the engine, so a server over
the engine wire has nothing to answer it with.

### B. Engine mutations — closed

All 16 are in the Rust surface behind three grants: mutations (`write_file`,
`edit_file`, `delete_file`, `batch`, `assign_block_id`, `rename_block_id`,
`rename`, `rename_type`, `inline`, `promote`, `create_file`), composition
(`add_dep`, `remove_dep`), and workspace shape (`add_workspace_member`,
`remove_workspace_member`, `set_workspace_member_role`); the second and third
grants each require `allow_mutations`.

### C. Host tools that are not engine reads — decided

`bash`, `glob`, `read_file_pinned`, the four `au_host_*` tools (intents, pane
operations, snapshot), and `au_type_system` (a content-package read, not an
engine read). The engine serves none of them.

**Decision: host tools stay a client capability; the server does not carry a
host shell or scanner.** Evidence, not preference:

- the live bridge declares these tools and they are stubs: the workspace's own
  agent manual tells agents to search with an ordinary file search outside MCP
  (`manual/mcp-access.md`, "known traps"), and the four `au_host_*` tools are
  documented as always failing because the host process was removed;
- every client that mounts this server already has `bash`, `glob`, and `grep`,
  over the same filesystem, with the same user's permissions;
- a server that serves a vault and also exposes a shell turns one grant into
  arbitrary code execution as the socket's owner, which is exactly the boundary
  the policy module exists to keep.

The engine's `grep` read now serves literal content search (`au_grep`) for a
client with no local filesystem. Ranked or richer search remains an engine read
(`grep`/`search` verb with the index's own ordering) rather than a host shell,
because only the engine can answer with the workspace's semantics. That is a
wire change, not a policy change, and it is the first candidate when searching
is measured as a bottleneck.

## Phases

| phase | work | risk |
|---|---|---|
| A | 13 reads from group A, each with a test and most with a captured fixture | low: pure mapping, no new authority |
| B | 16 mutations from group B, behind the existing gate, one at a time | medium: rename/move semantics, workspace reshaping |
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
