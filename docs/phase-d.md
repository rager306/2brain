# Phase D: retiring the Node stack

Status: **done 2026-11-16.** Every package this file planned to delete is deleted, and the argument
below is kept as the record of why it was safe. What actually happened, in order, with measurements:
`docs/internal/serving-parity.md`; the resulting tree: `docs/repo-inventory.md`.

The text below is the pre-execution state (it says "not started", and describes two stacks side by
side). It is the justification that was written before the change, not a description of today.

## What "retire" means

Today the production vault is served by two stacks side by side: the engine daemon
plus the Node `au-mcp` package, reached through the Python `au-mcp-bridge` (this is what
`~/.hermes/config.yaml` points at), and the Rust adapter `au-mcp-rs` over the same engine
socket. Phase D removes the Node side: `au-mcp`, `au-mcp-core`, `au-mcp-sdk`,
`au-type-codegen`, `au-engine-sdk`, `au-mcp-bridge`, and both client adapters.

The engine is untouched by any of this. `au-engine` is the writer and the authority in
both worlds; what changes is the door in front of it.

## What must be true before the first package is deleted

| requirement | state | evidence |
|---|---|---|
| every daemon tool the vault's agents actually use has a Rust counterpart, or a deliberate decision not to | **done for reads and mutations**; 8 daemon tools have no counterpart and 6 of them are host tools by decision | `docs/runtime-map.md` sections A, B, C |
| a real client completes a model turn on reads | **done**: Claude Code and Codex | `scripts/model-turn.sh`, `scripts/codex-turn.sh` |
| a real client completes a model turn on the mutation path, including the read-only refusal | **done**: Claude Code | `scripts/mutation-turn.sh` |
| a client recovers from a lost mutation reply | **done**: model calls `tb_reconcile` and does not retry | `scripts/recovery-turn.sh` |
| a client chooses the transactional path when a change spans files | **done**: one `tb_batch`, not two writes | `scripts/batch-turn.sh` |
| a client plans a revert before doing it | **done** | `scripts/rollback-turn.sh` |
| the shared facts agree between the two stacks | **done on one comparable read** | `scripts/bridge_parity.py` |
| the vault's own tool surface (49 daemon tools) is what agents were trained on | **open question**: the daemon advertises 49, the adapter 51, and the extra 10 are new verbs. A client asking for a daemon tool by name would get "unknown tool" | `docs/bridge-parity.md` |

## The plugin question, which is the real blocker

A workspace can declare `mcp.tool` plugins today: TypeScript modules the daemon loads
at runtime. A compiled binary cannot load them. So retiring Node means deciding what
happens to a workspace that ships one.

Three options, and the choice belongs to whoever owns the vault's extensions, not to
this document:

1. **Freeze**: no new plugins; existing ones are ported to engine reads or dropped.
   Cheapest, and **measured on 2026-11-15: nothing declares one**. `/root/vault` has no
   `workspace.yaml`, no `plugins:` key in any YAML, and no `*mcp.tool*` or `*plugin*`
   file outside prose in `knowledge/`; `.arsumbris/repo.yaml` composes one dependency
   (`mcp-bundle`) and no extensions. The `mcp.tool.*` types in `au-mcp-core/src`
   are the daemon's own tool definitions, not workspace plugins.
2. **Process-per-plugin**: a plugin is an external MCP server the client launches, and
   the workspace declares its command instead of its module. Keeps extensibility,
   moves the trust boundary from "a module inside the daemon" to "a process the
   operator configured", and needs no loader in Rust.
3. **Keep a thin Node sidecar for plugins only**: the narrowest version of "do not
   retire yet", and the only one that keeps two runtimes in the served path.

The decision this repository argues for is (2) if plugins matter, (1) if they do not.

## Order of removal, when it is time

Each step is independently revertible, and each is a commit that can be reverted alone:

1. Point `~/.hermes/config.yaml` at the adapter (or at both), and run one Hermes
   session through it. Nothing is deleted yet.
2. Run the full verification set against the live vault: the eleven-read A/B, client
   matrix, one model turn per client. This is where "parity" stops being a fixture.
3. Remove `au-mcp-bridge` from the served path; keep the directory until step 4.
4. Delete `au-mcp`, `au-mcp-core`, `au-mcp-sdk`, `au-type-codegen`, `au-engine-sdk`,
   `au-mcp-adapter-*` from the bundle in one commit, with the plugin decision recorded.
5. Regenerate `docs/runtime-map.md` for the two-module world.

Rollback for every step: the engine and the vault are untouched, so restoring the
previous config re-creates the old path; `docs/internal/engine-swap-2026-11-14.md` is the model
for how a swap is rehearsed and reversed.
