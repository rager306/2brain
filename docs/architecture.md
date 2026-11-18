# 2brain — architecture, in one page

**What this page is.** The single entry point a new reader — human or agent — needs before touching anything
else. It states what the system is, what its parts are, which direction dependencies run, where the boundaries
are, and which documents carry the evidence. Everything here is checkable from the repository at the commit
below; nothing here is a plan — [`roadmap.md`](roadmap.md) is, and it is the only page that is.

**Measured at** `bdfc7b2`, 2026-11-18. Every claim names its file, and `repowise decision export` carries the
ones that govern change.

---

## 1. The system in three sentences

A knowledge base is **Markdown files in a Git repository**: notes claim a type, link each other with
`[[wiki-links]]`, and carry addressable `^block-ids`. The **engine** parses those files into a typed graph and
answers structural reads — this graph is derived and rebuildable, never the truth. The **adapter** exposes
that graph to agents over MCP, with every mutation behind a gate that names the stage that refused it.

The product is therefore a *serving path from files to agents*, and the two invariants that follow are: **files
are authoritative** (delete the graph, rebuild it, nothing is lost), and **a write is gated before it lands**
(an agent cannot write what the operator did not grant).

## 2. Layers, and the one rule that keeps them apart

Dependency direction is enforced by `Cargo.toml` alone; no crate imports upward.

| layer | crate / dir | owns | depends on |
|---|---|---|---|
| substrate | `tb-diagnostics` | diagnostic codes, spans, suggested fixes | — (leaf) |
| syntax | `tb-grammar`, `tb-parser` | shape expressions; frontmatter, Markdown, YAML, file kinds | `tb-diagnostics` |
| resolution | `tb-references` | wikilink parsing, file index, reference resolution | `tb-parser`, `tb-diagnostics` |
| domain | `tb-core` | type graph, closure, validation, candidate analysis — **no engine-schema names** | `tb-grammar`, `tb-parser`, `tb-references`, `tb-diagnostics` |
| engine | `tb-engine` | held state, build, wire protocol, mutations, serving | `tb-core` + all below |
| cli | `tb-cli` | the `au` binary, commands, daemon | `tb-engine` + all below |
| adapter | `tb-mcp-rs` | MCP surface, policy, mutation gate, audit | `tb-cli`, `tb-engine` |

Three rules that hold the split together:

1. **`tb-core` stays domain-pure.** It knows no engine-schema type names; the `tb-engine` parse wrapper
   canonicalizes claims (`tb-engine/src/engine_schema.rs`). This is why the schema rename in release 2 touched
   one crate's constants and not the domain.
2. **One writer.** The engine serialises mutations; the adapter adds a policy gate in front but never writes a
   file itself.
3. **The adapter cannot invent.** It sends wire requests and renders answers; every shape it serves is the
   engine's (`tb-mcp-rs/src/schemas.rs` mirrors `WIRE.md`, and the workspace README says so).

## 3. The request path, end to end

```
client (Claude Code, Codex, Hermes, any MCP client)
   │  stdio JSON-RPC 2.0
   ▼
2brain mcp <target>            tb-mcp-rs: 52 tools, policy, gate, audit, ledger
   │  Unix socket, length-prefixed frames
   ▼
au daemon start <root>         tb-engine: held IR, one writer, rebuilds on disk change
   │
   ▼
<root>/**/*.md  +  <root>/.2brain/repo.yaml   ← the authoritative files
```

Two things a reader should notice here. The **socket** is `0600` and names no user: the trust boundary is one
local OS user, and `--principal` is an audit label, not authentication. And the **ledger** is why a write needs
a prior read of the same path: the adapter remembers what it served and refuses a write over anything it did
not serve in this process.

## 4. The write contract, stated once

Four gates, in order, and the first that fails names itself (`policy/<stage>`):

| stage | question |
|---|---|
| `arguments` | is the request well-formed? |
| `path_scope` | does the path stay inside its member? |
| `member_grant` | does this run's `--allow-mutations` include the member? |
| `prior_read` | did this process serve the file's current hash? |
| `freshness` | does the caller's `expected_hash` equal it? |
| `editability` | does the engine author this member? (engine answer, not policy) |
| `capacity` | is the single mutation slot free? |

Beyond the gate, three facts that are contracts rather than accidents — each was measured, and
`docs/research/write-contracts.md` carries the code reading:

- **`tb_create_file` is atomic**: the absence check *is* the write (`O_CREAT|O_EXCL` in the engine), proven by
  two processes racing in `tb-mcp-rs/tests/create_race.rs`.
- **`tb_edit_file`'s `expected_hash` is a span precondition, not a whole-file compare-and-set.** The engine gets
  `old_string`, so a concurrent change elsewhere in the file does not cancel the edit. `tb_write_file` is the
  whole-file CAS. The difference is deliberate: a fragment edit that required a byte-identical file would refuse
  exactly the case of two agents editing different sections.
- **Diagnostics stay advisory.** A mutation does not reject because its result has validation errors. Drafting
  and *promoting into accepted knowledge* are different verbs, and the second does not exist yet — the one known
  gap on this path (`docs/research/write-contracts.md` §4).

## 5. What the engine serves, grouped

52 tools: **36 read-only** (33 read, 2 discover, 1 watch) and **16 mutate**, each behind a grant. The names
and one-line descriptions below are generated from the `#[tool(...)]` attributes in `tb-mcp-rs/src/lib.rs`,
so this page cannot drift from the surface it describes.

**orientation**

- `tb_overview` — Get the workspace overview
- `tb_top_level_dirs` — List top-level directories in own scope.
- `tb_members` — List workspace members, optionally selecting an exact repo
- `tb_files` — List a page of catalogued files, including assets and dependencies by default
- `tb_children` — List a directory's direct entries: files and subdirectories, hidden excluded, catalog-derived so onl
- `tb_resolve_member` — Resolve which declared member owns a member-relative path, with its root, role, and whether it is editable and local

**vocabulary**

- `tb_types` — List a name-sorted page of types across the workspace or in a named member
- `tb_type` — Get one type definition by name, across the workspace or in a named member
- `tb_type_tree` — Get the complete, unpaged type tree for the whole workspace, including dependencies.
- `tb_subtypes` — List all direct and transitive subtypes of a base type
- `tb_typed` — Classify a path: a typed instance (with its claim, whether it resolves, and its closure), a type-def, or a plain note

**instances**

- `tb_instances` — List every resolved instance in the workspace, with its file, claim, closure, effective shape and va
- `tb_instances_of` — List every instance matching a type
- `tb_resolved` — Resolve one instance: field values with their contribution provenance, the type claim and closure, a
- `tb_candidates` — List the files that satisfy a type's required fields without claiming it, grouped by file, with the
- `tb_validate_value` — Validate a value against a type without touching disk

**structure**

- `tb_neighborhood` — Walk the reference graph from one seed file and return the reachable subgraph (nodes plus edges)
- `tb_references` — List one file's wikilink edges, classified by kind and surface: outgoing (what it links to) and inco
- `tb_hubs` — List a ranked page of hubs
- `tb_graph_health` — Report what is wrong with the graph in one answer: directed cycles (strongly connected components, s
- `tb_follow` — Resolve one wikilink string, including one you are about to write: does this address resolve, and what is there

**text**

- `tb_search` — Rank notes by relevance to a query instead of scanning them
- `tb_grep` — Search the catalogued files for literal text, one match per line
- `tb_content` — Read one member's file text, resolved from the member's declared root, with the content hash that gu
- `tb_frontmatter` — Read one file's parsed frontmatter as JSON
- `tb_semantic_tokens` — Read one file's semantic token stream: resolved and broken wikilinks, pinned commits, typed field va

**addressability**

- `tb_anchors` — List every heading in one file, in document order, as the anchors a `#fragment` can address: text, level, and span
- `tb_block_ids` — List every addressable block id in one file, in document order, with kind (record, typed_block, mark

**provenance**

- `tb_rollback_preview` — Preview reverting one commit without touching anything: the paths it changed, whether a revert would

**operations**

- `tb_diagnostics` — List a page of diagnostics
- `tb_preview_mutation` — Preview a write, edit, or delete without changing anything
- `tb_reconcile` — Check what actually happened after an outcome_unknown mutation
- `tb_imports` — List resolved cross-repo type imports
- `tb_watch` — Wait for the next change to the workspace, up to timeout_ms (default 10000, max 30000)
- `tb_search_tools` — Find tools by words from their name, description, or parameter names
- `tb_describe_tool` — Get one tool's full definition: description, input and output schemas, and annotations

**mutate** (16, each behind a grant)

- `tb_write_file` — Write one member's file text after a successful tb_content read of the same path
- `tb_edit_file` — Replace one exact string in a member's file after a successful tb_content read of the same path
- `tb_delete_file` — Delete one member's file after a successful tb_content read of the same path
- `tb_batch` — Apply 1 to 50 file operations (write_file, edit_file, delete_file) as one all-or-nothing saga: every
- `tb_create_file` — Create a new file in a member
- `tb_add_workspace_member` — Declare a member in one workspace's .arsumbris/workspace.yaml, in the edit or discover list
- `tb_remove_workspace_member` — Remove a member from one workspace's edit and discover lists
- `tb_set_workspace_member_role` — Move a declared member between the edit and discover lists in one commit
- `tb_inline` — Fold one file into a referrer as an inline record and delete it, as one saga: the chosen [[path]] be
- `tb_promote` — Lift an inline record into its own file and leave a reference where it sat, as one saga: every [[hos
- `tb_rename_type` — Rename a type-def and carry every reference with it, as one saga: the def file moves and claims, slo
- `tb_add_dep` — Declare a dependency in one member's .arsumbris/repo.yaml, as a byte splice that keeps comments and order
- `tb_remove_dep` — Remove a dependency from one member's .arsumbris/repo.yaml, as a byte splice that keeps siblings and
- `tb_assign_block_id` — Mint a block id for the entity at a byte offset and return the id and its [[target^id]] ref
- `tb_rename_block_id` — Rename an inline record's block id and carry every referrer with it: the ^: declaration and each [[h
- `tb_rename` — Move a file inside its member and rewrite every inbound reference to the new name, preserving each r

`WIRE.md` is the contract; `tb-mcp-rs/docs/tools.md` is the caller's view; `tools/list` is the machine's.

## 6. Where the evidence lives

| question | document |
|---|---|
| what is true now | **this page** |
| what happens next | `docs/roadmap.md`, and `docs/ideas.md` for what was refused |
| what landed, dated | `docs/trajectory.md` |
| which decisions govern change | `.repowise/decisions.yaml` (`repowise decision list`) |
| the wire contract | `tb-engine/crates/tb-engine/WIRE.md` |
| the write contract, read from code | `docs/research/write-contracts.md` |
| what was measured, and refused | `docs/research/*-2026-11-1*.md`, one per question |
| how measurement is judged | `docs/research/measurement-doctrine.md` |
| what an agent should do, in order | `tb-mcp-rs/docs/agent-interface.md`, `progressive-disclosure.md` |
| this host, its units and sockets | `docs/internal/` — never design |

## 7. What this page deliberately does not cover

Diagnostics catalogue, ranking formula, graph algorithms, the intake pipeline, and the plan live in their own
documents, linked from `docs/README.md`. This page stops at "what the parts are and how a request flows",
because a page that grows to cover everything stops being the page a reader finishes.
