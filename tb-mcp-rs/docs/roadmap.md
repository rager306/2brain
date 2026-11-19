# Roadmap (closed) — the adapter's completion record

**This page is a completion record, not a plan.** It says what the adapter set out to do and which item closed
with which evidence. What happens next is [`docs/roadmap.md`](../../docs/roadmap.md); a second forward-looking
page would be a defect. The gaps at the end of this file are carried in that roadmap's idea register
([`docs/ideas.md`](../../docs/ideas.md)) where they are still open.

Status on 2026-11-16, HEAD `3706515`. **The roadmap is closed**: the adapter is the only serving path
(`/usr/local/bin/2brain`), 52 tools are registered — 36 advertised without a grant, 47 with mutations,
52 with every grant — and the Node/Python stack it was competing with is deleted. The evidence for
each line below lives in [`verification.md`](verification.md); the items that stayed open, and why,
are at the end of the file.

The 2026-11-14 status line, kept for comparison: 51 tools, 35 without a grant, 165 adapter tests,
eleven documents.

## Done — with evidence

| item | evidence |
|---|---|
| first 16 read tools | `src/lib.rs`: overview, top_level_dirs, type, type_tree, types, hubs, diagnostics, subtypes, members, imports, files, content, instances_of, validate_value, preview_mutation, reconcile |
| first 3 mutations behind a gate | `write_file`, `edit_file`, `delete_file`; `tests/policy.rs`, `tests/write_file*.rs` |
| 5 addressability/link reads | `frontmatter`, `anchors`, `block_ids`, `references`, `neighborhood`; `tests/links.rs`, fixtures |
| 4 scan/resolution reads | `instances`, `candidates`, `semantic_tokens`, `resolve_member`; `tests/scan.rs`, fixtures |
| 4 classification reads | `typed`, `resolved`, `children`, `follow`; `tests/scan.rs`, `tests/links.rs` |
| build identity | `--version` carries `+<commit>[.dirty]`; `build.rs`, `tests/cli.rs` |
| discovery and profiles | `tb_search_tools`, `tb_describe_tool`, `--tools full\|concise`, `--payload both\|text\|structured`; `tests/discovery.rs` |
| protocol | four legacy revisions and `2026-07-28`, pre-init `ping` rule; `tests/stdio.rs`, `tests/modern_stdio.rs` |
| faults | lost mutation reply, engine death mid-read, truncated frame; `tests/faults.rs` |
| budgets | `tb_content {max_chars}`, `tb_instances_of {limit, offset}`; `tests/budgets.rs` |
| contracts | synthetic corpus (32), production golden replay, client matrix, bridge parity, rollback rehearsal |
| one binary | `2brain` (delegates `daemon`/`open`/`members` to `tb_cli`, `mcp`/`info` here); `tests/cli.rs` |
| agent skill | `skills/2brain`, installed by `scripts/install_skill.sh` |
| trajectory | `docs/roadmap.md`, `docs/runtime-map.md` |

## Phase A — remaining engine reads

- [x] `tb_frontmatter` — `frontmatter`
- [x] `tb_anchors` — `anchors` (local form: empty target + origin)
- [x] `tb_block_ids` — `block_ids`
- [x] `tb_references` — `references_out` + `references_in`
- [x] `tb_neighborhood` — `neighborhood`
- [x] `tb_instances` — `instances` (payload carries `count`, `aborted_at_load`)
- [x] `tb_candidates` — `candidates` (its own `limit`/`offset`)
- [x] `tb_semantic_tokens` — `semantic_tokens`
- [x] `tb_resolve_member` — `resolve_member`
- [x] `tb_typed` — `instance` + `resolve_target`; classifies a path as an instance, a type-def, or a note
- [x] `tb_follow` — `resolve_target` / `resolve_block_id` / `resolve_anchor`, plus a line peek
- [x] `tb_children` — `dir_entries`
- [x] `tb_resolved` — `instance`
- [-] `au_type_system` — **out of scope**: it reads typed `doc` atoms from the `au-mcp-type-knowledge` content package, not the engine

Phase A is closed. Three later engine reads also have adapter tools:
`tb_grep`, `tb_graph_health`, and `tb_rollback_preview` (see the backlog table
below).

## Phase B — engine mutations

- [x] `assign_block_id`, `rename_block_id`, `rename` — behind the gate, with `tests/structural.rs`
- [x] `batch` — 1–50 file operations as one engine saga; `tests/batch.rs`
- [x] `inline`, `promote` — two-file gate: both files armed, one slot for the pair
- [x] `rename_type` — type-hash gate (the type has no path; its `hash` from tb_type is the freshness check)
- [x] `add_dep`, `remove_dep` — behind a **second grant** (`allow_composition`, on top of `allow_mutations`), because they author `.arsumbris/repo.yaml`
- [x] `create_file` — the gate's opposite promise: absence instead of
      read-then-hash, so a create cannot overwrite. Found from the vault backlog
      note P0-C, which recorded a live session falling back to the daemon because
      the Rust surface could not create a file
- [x] MCP resources (`list`, `read`, `templates`) over the catalogue, with no
      resource subscription: the engine's own `changes` stream is exposed as
      `tb_watch` instead of a second notification path
- [x] `add_workspace_member`, `remove_workspace_member`, `set_workspace_member_role`
      — behind a **third grant** (`allow_workspace_shape`, on top of
      `allow_mutations`): they change which
      members the workspace serves, so a mistake alters every later read rather
      than one note. `tests/composition.rs` pins that the grant is independent of
      the composition grant and that a launch without it neither advertises nor
      accepts the three verbs.

## Phase C — the host decision

**Decided: host tools stay a client capability.** The server carries engine reads
and gated mutations; it does not carry a host shell, file walker, or content
scanner. The reasons are in [`runtime-map.md`](runtime-map.md): the live bridge's
host tools are stubs and its own agent manual routes content search outside MCP,
every client already has `bash`/`grep` over the same filesystem, and a server that
serves a vault plus a shell turns a note-editing grant into arbitrary code
execution as the socket's owner.

If a client with no local filesystem appears, the engine `grep` read already
serves literal content search (`tb_grep`) — the index's semantics, not a host
process. Ranked or richer search remains a wire change, recorded here as the
first candidate when search is measured as a bottleneck.

## Next items from the vault backlog, and where they actually live

The workspace backlog (`/root/vault/inbox/arsumbris-fork-backlog.md`) lists gaps
measured against turbovault. Three of the four are now closed in the engine; the
last is an **engine verb, not adapter work** — the adapter can only expose what
the wire serves, and a tool that composes N engine reads would turn one round
trip into hundreds:

| backlog item | what it needs | why it cannot be adapter-side |
|---|---|---|
| lexical search (P0-A) | **closed**: the engine's `grep` read, exposed as `tb_grep` | — |
| resources + subscribe (P1-A) | **half closed**: MCP resources serve the catalogue; the change signal is `tb_watch` over the engine's own `changes` stream, not a resource subscription | a subscription is a promise about future changes; only the process that owns the index can keep it, and `tb_watch` keeps the promise without a second notification lifecycle |
| batch execute (P1-C) | **closed**: the engine's `batch` mutation verb, exposed as `tb_batch` | — |
| graph hygiene report (P1-D) | **closed**: the engine's `graph_health` read, exposed as `tb_graph_health` | — |

Closed: `create_file` (backlog P0-C), the audit hash chain (P1-B first half) and
`rollback_preview` (P1-B second half),
`grep` (P0-A) as a new engine read rather than a host scanner, `batch` (P1-C) as a
saga over the engine's existing multi-file machinery, and `graph_health` (P1-D) as
one engine read that joins degrees, SCCs, and reference diagnostics. 

## Phase D — retire the Node stack

Not started. The checklist, the requirements table, the plugin decision and the
five-step removal order are in [`../../docs/phase-d.md`](../../docs/phase-d.md). Both
former blockers are answered: the mutation path is proven through a model (write,
refusal, recovery, batch, revert), and plugins are moot here — this vault declares
none, so freezing costs nothing and a process-per-plugin contract is the option if
extensions return later.

What goes: `au-mcp`, `au-mcp-core`, `au-mcp-sdk`'s **code**, `au-type-codegen`, `au-engine-sdk`,
`au-mcp-bridge`, and both client adapters — all deleted 2026-11-16; `au-mcp-sdk`'s `type/` stays as the
`mcp.*` vocabulary. What stays: the engine, and
successor role is covered by `WIRE.md` plus the adapter's `src/wire.rs`, and the
vault's content contract is untouched by any of this.

## Standing checks

Every slice ends with: `cargo fmt --check` (both halves), the full `cargo test
--offline` for `au-mcp-rs`, the engine suite with its recorded baseline diffed
against `tb-engine/tests-known-environment.txt`, `clippy --locked --all-targets
--all-features -- -D warnings` for `au-mcp-rs`, the ignored real-engine test, and a
regenerated fixture corpus when a wire answer changed.

Two of those are phrased narrowly on purpose, and the reasons are recorded rather
than implied:

- `clippy -D warnings` gates `au-mcp-rs` only. The engine workspace carries 85
  upstream style warnings (au-core 43, au-engine 38, au-testkit 4), so a red clippy
  there says nothing about a fork commit. `fmt --check` **is** a gate for both
  halves: the formatting pass landed on 2026-11-14 (`cargo fmt` over 131 files,
  formatting only; test counts unchanged), so the engine is clean and stays clean.
- The engine suite is compared, not required to be green: four tests fail as root
  because they inject failure through `chmod`. `tb-engine/scripts/run-known-root-tests.sh`
  re-runs exactly the recorded names as `nobody` and passes, which is what makes the
  baseline a statement about the runner rather than about the code.

## What is not done

Honest ledger, so a reader does not have to infer the gaps from the evidence files:

| gap | why it is open |
|---|---|
| nothing model-level remains on the mutation path | the last item — a model planning a revert from the commit `tb_content` reports — is `scripts/rollback-turn.sh`, 7/7. Reads, writes, the read-only refusal, lost-reply recovery, the transactional path and the planned revert are all proven through a model |
| `2026-07-28` on a third-party client | measured with `scripts/stdio_tee.py`: Codex asks for `2025-06-18` and is answered `2025-06-18`; no installed client requests the newer revision, so only our own tests reach it |
| a Cursor model turn | Cursor needs an API key this host does not have. Codex is no longer in this row: `codex exec` completes a read turn, recorded in `client-matrix.md` |
| resource subscriptions | not declared, by decision: `tb_watch` gives every client the change signal without a second lifecycle. A client that genuinely needs `notifications/resources/updated` would reopen the question |
| generated schemas | `tests/schema_contract.rs` catches drift against captured payloads, which is the safety half. Generating the schemas needs `WIRE.md` marked up machine-readably; today it is prose |
| an `tb_files` golden capture | the engine golden set covers four reads; the file listing is size-heavy and still uncaptured |
| a scripted live A/B | the eleven byte-identical reads of 2026-11-13 were a manual session, so they cannot be re-run by the next person |
| ~~Phase D (retire the Node stack)~~ | **done 2026-11-16**: the bridge, the Node daemon, both adapters, the SDK code and the codegen are deleted; the client spawns `/usr/local/bin/2brain`. Decisions and measurements in [`../../docs/internal/serving-parity.md`](../../docs/internal/serving-parity.md) |
