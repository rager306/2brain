# Detach from upstream and converge on one language

Status: decision, 2026-11-16. The user's instruction: upstream is dead, so stop tracking it;
implement our architectural features and move everything into one repository, retiring the
language zoo along the way. This document is the trajectory; `docs/architecture-plan.md` keeps
the architecture work that is already closed.

## 1. What "detach" means here, measured

The repository is already one git repo with one remote (`origin` = `github.com/rager306/2brain`)
and **one branch** (`main`). Upstream is gone as a *remote*; what is left of it is:

| remnant | where | action |
|---|---|---|
| upstream history inside the subtree imports | reachable commits (`44b32f4` = `0.0.5`-era engine work; the 24 `Add '<dir>/'` merge commits) | keep; it is the provenance of every file |
| a **stale branch model** in the runbook | `AGENTS.md` still names `remotes/origin/main`, `feat/hotpath-profiling-and-memchr-wikilinks`, `fork/main` — none exist (`git branch -a` shows `main` only) | rewrite that section to describe the repository as it is, with the upstream commits named by hash as archive |
| the "upstream is frozen" framing in docs | `AGENTS.md`, `tb-engine/README-fork.md` | keep the fact, drop the implication that a merge may come |

**Decision:** no more merges from upstream. Upstream is *reading material*, referenced by commit hash,
never a branch to rebase onto. Archive it as an annotated tag so the state is citable after the branch
model is rewritten: `git tag -a archive/upstream-0.0.5 44b32f4` (and, if a release commit is
identified, `archive/upstream-0.0.2-alpha`).

## 2. The zoo, measured

| language | files | where | on the serving path? |
|---|---:|---|---|
| TypeScript | 368 | `au-mcp` (62), `au-mcp-core` (82), `au-mcp-adapter-codex` (86), `au-mcp-adapter-cc` (43), `au-engine-sdk` (36), `au-type-codegen` (30), `au-mcp-sdk` (17) | **yes**: `arsumbris-mcp.service` runs `/usr/bin/node au-mcp/src/cli.ts start /root/vault` |
| Rust | 238 | `au-engine` (8 crates), `au-mcp-rs` | **yes**: `arsumbris-engine.service`, and the `arsumbris-rs` MCP server Hermes already uses |
| Python | 39 | `au-mcp-bridge` (1, **serving**), `tb-mcp-rs/scripts` (10, tooling), `tools` (2), `au-ingest`/`au-govern` (1 each) | **yes, one file**: `bridge.py`, which Hermes' `mcp_servers.arsumbris` invokes |
| Shell | 19 | verification scripts | no |
| YAML/Markdown | 2 023 | type definitions, contracts, docs | contracts, not code |

So the zoo on the serving path is exactly **two artifacts**: the Node daemon and `bridge.py`. Everything
else non-Rust is either a client library (TS SDKs), a generator (`au-type-codegen`), or tooling
(`scripts/`, `tools/`), which the layers rule in `AGENTS.md` allows.

## 3. Trajectory

### X — detach (policy only, no code)
1. `AGENTS.md`: rewrite the branch model to the truth (one branch, upstream by hash, archive tags);
   state that merges from upstream are not planned.
2. Tag the upstream state as an archive.
3. Add one line to the runbook: "a file is judged by the layer it belongs to" already exists; add
   "a *component* is judged by whether it is on the serving path" — the retirement test.

### Y — one serving language (Rust), in the order that never loses a capability
The gate for every step is a parity inventory of what the retiring component provides versus what the
Rust adapter provides (`docs/serving-parity.md`, produced by that investigation). Order:

1. **Hermes drops the bridge.** `mcp_servers.arsumbris` (python bridge) is removed once
   `mcp_servers.arsumbris-rs` covers every tool a session actually calls. `arsumbris-rs` is already
   configured with `--allow-mutations` and was exercised live this month.
2. **Retire the Node daemon** (`arsumbris-mcp.service`) once its tool/hook/resource surface has a Rust
   counterpart or is shown unused. This is `docs/phase-d.md`, which already records that the vault has
   no `workspace.yaml`, no `plugins:` key and no plugin files, so "freeze plugins" costs nothing.
3. **Delete what the retirement orphans**: `au-mcp` (daemon), the bridge, their lockfiles and
   `node_modules`; keep `au-mcp-core`/`au-mcp-sdk`/`au-mcp-adapter-*`/`au-engine-sdk` only if an external
   consumer exists — otherwise they are the next candidates, and `au-type-codegen` stays as tooling.
4. **Prove it, don't assume it**: after each step, the same reads must answer (`docs/verification.md`
   has the parity scripts), one live Hermes session must work with only `arsumbris-rs` configured, and
   the rollback must be a config edit, not a rebuild.

### Z — one repository, one build
Already one repository. What remains is build residue: `package.json`/`pnpm-lock.yaml` per TS package.
Retiring the daemon removes the reason for a Node build in this repo; whatever TS stays becomes a
client library with its own published artifact, not a serving dependency.

## 4. What must not happen while detaching

- **Do not break the vault contract.** `au-type-system` is a contract; nothing in this trajectory may
  change a wire shape or a frontmatter rule.
- **Do not stop a service to see what breaks.** The parity inventory comes first, and it names the
  first check that would fail today.
- **Do not delete a TS package because it looks unused.** `au-mcp-core` carries the type definitions
  (`mcp.tool`, `mcp.hook`, `mcp.inject`) that the adapter's ontology work references; it may be a
  *contract* repository rather than a code one.
- **Do not rewrite history.** Upstream commits stay as they are; detaching is a policy change, not a
  rebase.

## 4b. Status and the parallel tracks (2026-11-16)

| track | state |
|---|---|
| X detach | **done** (`e511507`): one-branch branch model in `AGENTS.md`, `archive/upstream-0.0.5` pushed |
| inventory | **done** (`docs/repo-inventory.md`): every directory with its layer and fate, measured from `git ls-files` |
| parity gate for Y2 | **done** — `docs/serving-parity.md`: the Node daemon's 49 tools and 6 hooks measured, the 8 gaps decided, the checklist executed |
| Y1, Y2 | **done 2026-11-16** — the bridge is out of Hermes, `arsumbris-mcp.service` is stopped and disabled, the bridge and its generated state are deleted |
| Z (one build) | **open** — the Node workspace is no longer on the serving path, but the TS packages are still in the tree (`docs/repo-inventory.md`) |
| fifth engine swap runbook | **in flight** — independent of the retirement, but it is the same "prove it on a copy first" discipline |

Two facts from the inventory change the shape of Y:

1. **`au-mcp-core` and `au-mcp-sdk` are contracts with runtime inside them.** They carry 76 type
   definitions between them (`mcp.tool`, `mcp.inject`, `agent-profile`, …), which the ontology work
   cites. The retirement must delete *files*, not directories, and the commit must say which.
2. **`au-engine-sdk` (14 k LOC) and the two adapters are not on the serving path**, so they are a
   product decision rather than a retirement step: support a JS client, or let the Rust adapter be the
   only supported surface.

## 5. First actions

| action | owner | gate |
|---|---|---|
| parity inventory of the Node daemon and the bridge | investigation (running) | the checklist in §3 Y names the first failing check |
| archive tags + branch-model rewrite in `AGENTS.md` | this plan | `git branch -a` and `git tag` agree with the document |
| remove `mcp_servers.arsumbris` from Hermes | user's call (live system) | one session works with only `arsumbris-rs` |
| stop `arsumbris-mcp.service` | user's window | §3 Y2 checklist green, rollback = `systemctl start` |
