# AGENTS.md — 2brain runbook

Numbers in this file are summaries for orientation. The instrument is
`tools/cards/metrics.json`, which records each count with the command that produced it and the date; a count
here that disagrees with that file is a stale sentence, not a second source of truth.

You are working in the 2brain workspace (`/root/2brain`). Development happens HERE
(allspark coding sessions run with this directory as the workdir); product code is never vendored
into allspark/prime-agent.

## What this is

**2brain** is a Rust implementation of a typed knowledge graph (typed markdown + `[[wiki-links]]` +
`^block-ids`, frontmatter contracts, MCP serving) with a judgement layer kept outside the serving path.
It is a fork of the `arsumbris` project (<https://github.com/arsumbris>), which stalled after
`0.0.2-alpha`; the `0.0.5`-era upstream work is preserved as the tag `archive/upstream-0.0.5`
(commit `44b32f4`). The only place the upstream name belongs now is the lineage paragraph in
`README.md`; the engine's file format still spells `tb.engine.*` and `.arsumbris/`, which is a format
waiting for its migration (`docs/rebrand-plan.md`, tier C).

## Layout

| path | role |
|---|---|
| `tb-engine/` | Rust engine Cargo workspace — the authoritative writer, serving `/root/vault` through `2brain-engine.service` |
| `tb-mcp-rs/` | **The serving path**: the Rust MCP adapter behind `/usr/local/bin/2brain` (52 tools, fail-closed gate, `scripts/install.sh` builds and installs it) |
| ~~`au-mcp-bridge/`~~ | **retired 2026-11-16** — the Python stdio bridge Hermes used to speak; removed with the Node daemon when Hermes moved to `/usr/local/bin/2brain` (`docs/internal/hermes-2brain.md`) |
| `tb-type-system/` | the type-system contract. **Do not break**: vault format compatibility depends on it |
| `tb-mcp-sdk/` | the `mcp.*` vocabulary: 17 type definitions (`mcp.tool`, `mcp.skill`, `mcp.inject`, `agent-profile`) that content notes are written in. Its TypeScript was deleted 2026-11-16 |
| `tb-ingest/`, `tb-govern/` | ingest pipeline, workspace composition (`workspace.yaml` members) |
| `tb-skills/`, `tb-agent-guides/`, `tb-rules/`, `tb-writing-style/`, `tb-competency/`, `tb-defaults/` | content/knowledge repos |
| `docs/solutions/` | documented solutions from past work (created on first write) — short notes with YAML frontmatter (`module`, `tags`, `problem_type`); search there when implementing or debugging in an area it covers |

## Branch model

There is **one branch**: `main`, tracking `origin` (`github.com/rager306/2brain`). Development
happens on `main` or on short-lived feature branches that merge back into it.

**Upstream is detached (2026-11-16).** The upstream project stalled; its work lives here as
imported history, not as a branch to merge from. Merges from upstream are not planned. Where a
document needs to cite upstream state, it cites a hash or an archive tag:

| what upstream was | how it is cited |
|---|---|
| engine `0.0.5`-era work, including the hotpath profiling and memchr wikilink scan | commit `44b32f4`, tagged `archive/upstream-0.0.5` |
| release `0.0.2-alpha` | whichever commit the release pointed at, tagged `archive/upstream-0.0.2-alpha` when identified |
| the sibling repositories the bundle was assembled from | the `Add '<dir>/' from commit '<sha>'` merge commits in history; their content now lives in the `tb-*` directories, and their `remote:` anchors were removed when the tree became one repository |

Older revisions of this section named `remotes/origin/main`, `feat/hotpath-profiling-and-memchr-wikilinks`
and `fork/main`. None of those branches exist; the names are kept here only so an old document that
mentions them can be read.

## Code, tooling, and models: three layers, one rule each

Every file here belongs to exactly one of three layers, and the layer decides the language
and the review bar. Name the layer before adding a file.

| layer | where | language | rule |
|---|---|---|---|
| **Product code** | `tb-engine/`, `tb-mcp-rs/`, `tb-ingest/`, `tb-govern/` | **Rust only** | it runs in the serving path; nothing here may import, spawn, or require a script, a notebook, or a prototype |
| **Tooling** | `scripts/`, `tools/`, test support | shell, Python, or Rust — whatever fits | it may drive the product, never be driven by it; it exits honestly (non-zero on failure) and prints what it measured; it is never on the request path |
| **Models** | `tools/*`, spikes, a section of `docs/` | any language | allowed, and often the cheapest way to settle a design question — but it is labelled a model, it is never cited as shipped behaviour, and it names where the real implementation would live |

Rules that make the split hold:

- **A model that something other than a human reads has graduated.** Either rewrite it in
the product's language or move it into tooling with tests. Nothing sits in between.
- **Tooling may be replaced without touching the serving path.** If changing `scripts/`
forces a change in `tb-mcp-rs/`, the boundary was crossed in the wrong direction.
- **No Rust crate depends on a script or an interpreter.** A `python3` invocation inside
Rust is a design smell; a `python3` invocation inside a shell script is fine.
- ~~The one serving-path exception is `au-mcp-bridge/`~~ (Python). **Removed 2026-11-16**: the serving
  path is Rust end to end — `/usr/local/bin/2brain` (adapter) → engine socket. There is no exception left.
It is legacy scheduled for retirement by `docs/phase-d.md`, not a precedent to copy.
- **Every model states its evidence:** what it measured, on what corpus, and what it did not
measure. `docs/internal/w4.1-judgement-run-2026-11-16.md` is the shape to copy.

Worked examples, so the rule is not abstract:

| what | layer | why there |
|---|---|---|
| `tb-mcp-rs/src/gate.rs` — the mutation gate's state machine | product | the serving path walks it on every write |
| `tb-mcp-rs/scripts/lib/procedure.py` — two procedures as states, with `--selftest` | tooling | its only consumer is `mutation-turn.sh`, a verification script; no Rust file imports it |
| the Python BM25 prototype behind `docs/architecture-plan.md` T1 | model | it produced numbers and nothing else; the index that shipped is Rust (`tb-engine/crates/tb-engine/src/search.rs`) |

If the procedure runtime ever gets a **product** consumer — the server itself running an
agent procedure — it moves to `tb-mcp-rs/src/procedure.rs` and gains a tool; until then its
home is `scripts/`.

## Workspace and Rust environment

- This directory is ONE Git repository (`github.com/rager306/2brain`, public
  since 2026-11-16). Every `tb-*` directory inside it is a plain directory whose
  history was imported with `git subtree`; there are no nested repositories and
  no submodules. Run `git status` and `git log` once, at the monorepo root, and
  scope pathspecs when you mean one project (`git log -- tb-engine`).
- The pre-monorepo sibling checkouts (the old `/root/arsumbris` layout) are
  untouched and remain readable references. Do not edit them expecting changes
  to appear here; treat this repository as the working tree.
- The Rust toolchain, the crate map, and the build/test loop are in
  [`tb-engine/AGENTS.md`](tb-engine/AGENTS.md). A session whose working directory is inside `tb-engine/` reads
  that file automatically; a session at the repository root reads this one and opens it when it needs to compile.

## Live-system rules (read before touching anything)

1. This bundle SERVES the production vault at `/root/vault` (hundreds of
   live notes; every agent session reads/writes it through the daemon and the
   MCP server). The vault is a git repo — data is protected by history — but
   an engine bug can still degrade serving.
2. Never point tests at the live vault. Use a fixture copy:
   `git -C /root/vault worktree add /tmp/vault-fixture` or `cp -r`.
3. Never kill or restart the engine daemon or an MCP server process while agent
   sessions are live. Socket: `~/.2brain/engine/run/*.sock` (`~/.arsumbris` is a symlink).
4. Engine swaps to the live daemon require the golden wire contract to pass
   (below) — no exceptions. A swap is a read comparison on a frozen copy of the
   vault working tree first, then the install, then the same reads against the
   live socket; `docs/internal/engine-swap-2026-11-14.md` is a worked example, with the
   rollback artifact named in it. The served build is whatever the last swap
   record in `docs/internal/engine-swap-*.md` names; `2brain --version` and the
   daemon's own build id are authoritative, not this file.

## Dev loop

Engine build and test commands, the shared target directory and its disk budget, and the two traps that make a
build look slow are in [`tb-engine/AGENTS.md`](tb-engine/AGENTS.md). Run them from `tb-engine/` with
`RUSTUP_HOME=/root/.rustup`; the adapter has its own commands in the same file.

## Improvement backlog

`/root/vault/inbox/arsumbris-fork-backlog.md` — P0 (FTS5 `MATCH` date-filter
quirk, ingest validation of broken `[[wiki-links]]`, stale-description
detector), P1 (bm25() ranking with type-aware freshness — durable types do
not decay, transient types half-life; enrichment columns keywords/questions
from a cheap LLM; `error→fix` atoms), P2 deferred (scheduled connectors,
dashboard). Backlog notes cite their source studies.

## Language

- Communicate with the user in Russian, including progress updates, reports,
  installation results, and explanations of errors. Translate user-facing
  output from tools or instructions written in another language into Russian.
- Write code identifiers, comments, documentation, configuration prose, and
  commit messages in English, unless the user explicitly requests otherwise.
  When adapting existing non-English text for these artifacts, translate it to
  English.
- Preserve exact commands, paths, identifiers, protocol values, and error text
  when precision matters; provide a Russian explanation or translation alongside.

## Working approach

- Act proactively toward the current repository implementation goal. Identify
  dependencies, risks, and the next useful steps; resolve issues within the
  requested scope instead of stopping at the first superficial fix. Report
  assumptions, blockers, and meaningful progress. Ask before expanding scope
  or taking consequential or irreversible actions.
- Actively research current best practices, including relevant 2026 guidance,
  when they can inform a design or implementation. Prefer authoritative,
  verifiable sources; check publication dates and applicability. Adapt guidance
  to this codebase and its contracts rather than adopting trends uncritically.
- Implement solutions that are clear, efficient, and elegant for the actual
  requirements. Prefer the simplest design that preserves behavior and safety;
  avoid unnecessary abstractions and unmeasured optimization.
- Verify changes with tests proportionate to their risk. Cover gaps in logic and
  semantics, operation sequences and state transitions, cascading effects,
  failure paths, and regressions where relevant. Test observable behavior and
  contracts, not implementation details alone; state any coverage limits.
- Follow the live-system rules and existing compatibility contracts throughout.
  Proactivity does not authorize production changes, destructive actions, or
  changes outside the user's requested scope.
- After a solved, verified problem, automatically invoke the `ce-compound` skill with `mode:non-interactive` at the completion checkpoint when the work corrected a wrong belief about how something behaves: an approach that failed, a review finding that exposed a real defect, or an assumption, document, or first diagnosis that turned out wrong. A naming, style, or preference change does not count. Work that went as first planned does not need it. Do not judge whether the lesson deserves a document: `ce-compound` decides whether it qualifies and what holds it (a test, a lint rule, a coding standard, or a learning), and writes nothing when nothing qualifies. Capture at the checkpoint so what it writes can ship in the PR that produced it, and only where the repository treats captured learnings as tracked, committed knowledge.
- Write every report, summary, or handoff to the user through the `ce-noslop` skill. This applies when you are the top-level agent writing to the user, not when you are a subagent reporting to its caller. Do not apply it to code, config, verbatim quotes, or text the user asked to post as written.
- Before writing or reviewing code, read `CODING_STANDARDS.md` (the defect classes this repository's code follows) and `REVIEW.md` (how review behaves here). When a change establishes or retires a rule, update those files in the same change.

## Docs: public design vs \`docs/internal/\`

Two kinds of document live in \`docs/\`, and the split is a rule, not a habit:

- **design** — \`docs/*.md\` and \`docs/research/*.md\`. Claims about the system that survive a different
  machine: architecture, naming, the trajectory, measurements, the evidence index.
- **operations** — \`docs/internal/\`. Every document that names this host: \`/root/vault\`, a systemd unit, a
  socket path, the Hermes config, a backup directory, or the run log of one operator's evening.

If a document you are writing names a host path, a unit, or a client configuration, it goes to
\`docs/internal/\`. \`docs/internal/README.md\` holds the table and the reasoning. Internal documents link to the
design documents they serve; design documents must not need an internal one to be understood, because the
repository is public and the internal set is the part a second machine would rewrite first.

## Discipline

- No git identity in-repo; global config only.
- Docs and commits in English.
- `tb-type-system` is a contract, not a playground: type changes require a
  migration note and vault-rollback reasoning first.
- When a change alters wire shapes, bump/annotate per `tb-engine/crates/tb-engine/WIRE.md`.
