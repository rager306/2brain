# Idea register — what is deferred, blocked, or refused

Every idea for this project that is **not** in the [roadmap](roadmap.md) queue lives here, with its status and
the thing that decided it. This page exists for one reason: a refused idea quietly returns every month, and the
number that refused it is the only thing that stops it.

| status | meaning |
|---|---|
| **shipped** | in the product; the evidence is the commit or the test |
| **ready** | decided, unblocked, not scheduled — the roadmap picks it up with an exit criterion |
| **deferred** | waiting on a named dependency, a measurement, or a user; the dependency is in the row |
| **blocked** | a named external thing must change first, and it is not in this repository |
| **refused** | measured or reasoned against; a number or a rule is quoted, and re-proposing it needs a new number |
| **unknown** | studied, not decided |

Three rules make the table usable. **A refusal quotes its number** (or names the rule it breaks). **A deferral
names its dependency** — "later" is not a status. **An idea nobody argued against is `unknown`, not `refused`**:
two rows below were once "refused" here with no document behind the refusal, and this pass demoted both rather
than keep a confident sentence that cited nothing.

Anything here that gets picked up moves to the roadmap and leaves a `shipped` row with a date.

---

## Serving path and the write contract

| idea | status | evidence | why / what decides it | next measurable step |
|---|---|---|---|---|
| **promote-with-checks** — a mutation rider that refuses on ERROR diagnostics instead of advising (deliberately **not** a new verb: `tb_promote` already lifts an inline record into its own file). Its acceptance profile needs more than structure: `decision.type.yaml` requires only `description` and `status`, and `evidence` is optional by design | **partly shipped**: `write_file` refuses and `WIRE.md` documents the rider; `edit_file`/`create_file`/`batch` and the default-path measurement are open (ADR-0002 stays `proposed`) | [`../tb-engine/doc/adr/0002-promote-with-checks.md`](../tb-engine/doc/adr/0002-promote-with-checks.md), [`research/write-contracts.md`](research/write-contracts.md) | diagnostics are advisory by design for drafts; promotion needed a mode, and ADR-0002 fixes its shape: an optional `require_clean` rider on four mutations, engine-side, refusing before the write | what is left of the ADR-0002 gate: a client-level refusal, the rider on the three remaining mutations, and a measured default path |
| a whole-file compare-and-set for `tb_edit_file` | refused | [`research/write-contracts.md`](research/write-contracts.md) | `expected_hash` is a **span precondition**: the caller proves it read the file, not that nobody else touched it. A fragment edit is the contract, and it is now stated as one | — |
| set up a `file` watcher so a write can see a concurrent clone | refused | `docs/research/write-contracts.md` §the four contracts | the engine is the single writer; a second writer would need a second authority | — |
| refuse the legacy `au.engine.*` spelling outright | ready (owner decision, roadmap #5) | [`research/release-2-window-2026-11-17.md`](research/release-2-window-2026-11-17.md) | no writer emits it any more, and the advisory proved the window is observable; closing it is a release decision | `instances_of` with the legacy name fails with a named error; fixtures re-blessed in the same change |
| an exact `grep` total instead of an early exit | ready (owner decision, roadmap #6) | [`trajectory.md`](trajectory.md) §decisions only the owner can take | the scan is file-bound, so an exact total was measured free; the question is whether it is part of the contract | one line in `WIRE.md` |
| generate the tool JSON schemas from `WIRE.md` | deferred — needs a machine-readable `WIRE.md` | [`../tb-mcp-rs/docs/roadmap.md`](../tb-mcp-rs/docs/roadmap.md) §what is not done | `tests/schema_contract.rs` already catches drift against captured payloads; generating needs the prose marked up | mark up one read's payload and generate its schema |
| resource subscriptions (`notifications/resources/updated`) | refused by decision | [`../tb-mcp-rs/docs/roadmap.md`](../tb-mcp-rs/docs/roadmap.md) | `tb_watch` gives every client the change signal without a second lifecycle to keep coherent | reopened only by a client that genuinely needs the notification |
| the `2026-07-28` protocol revision on a third-party client | deferred — no installed client asks for it | [`../tb-mcp-rs/docs/roadmap.md`](../tb-mcp-rs/docs/roadmap.md) | Codex asks for `2025-06-18` and is answered `2025-06-18`; only our own tests reach the newer revision | one recorded third-party turn on `2026-07-28` |
| a Cursor model turn in the client matrix | blocked — an API key this host does not have | [`../tb-mcp-rs/docs/roadmap.md`](../tb-mcp-rs/docs/roadmap.md) | the matrix is otherwise complete | one recorded Cursor read |
| a scripted live A/B | ready | [`../tb-mcp-rs/docs/verification.md`](../tb-mcp-rs/docs/verification.md) | the eleven byte-identical reads of 2026-11-13 were a manual session, so the next person cannot re-run them | a script that repeats exactly those eleven reads |
| a golden capture for the file listing | deferred — the payload is size-heavy | [`../tb-mcp-rs/docs/roadmap.md`](../tb-mcp-rs/docs/roadmap.md) | four engine reads are golden; `files` is not | a size-bounded slice with its own fixture |
| the `--payload text` default for every tool | refused | [`research/payload-modes.md`](research/payload-modes.md), [`research/hermes-log-findings.md`](research/hermes-log-findings.md) §1 | six calls failed in 46 s: a tool that declares `outputSchema` must return `structuredContent`, and the client's circuit breaker reported the server unreachable | — |
| the engine inside the adapter, connection pooling, a generic `tb_call` passthrough | refused | [`research/mcp-engine-path.md`](research/mcp-engine-path.md) §what not to do | 72.6 MiB of IR duplicated per session plus a second writer; pooling measured 1.14 vs 1.24 ms; a passthrough moves schema errors from build time to runtime | — |
| a `Prepared` mutation as a serializable value, and `allowed_next` in denials | refused, measured out | [`architecture-plan.md`](architecture-plan.md) P2.2–P2.3 | the refusal text already names the step that clears it; the value would be a struct nothing reads | — |
| the audit record carrying `repo:path@hash` | deferred — changes a hash-chained log format | [`architecture-plan.md`](architecture-plan.md) §2e, wave 3 P2.2 | it is the one consumer that would justify `Prepared`, so it needs its own decision | one record shape and a verifier over an existing chain |
| a full onion (domain/ports crates), a `serve.rs` split, TLA+ for the gate | deferred | [`architecture-plan.md`](architecture-plan.md) §6, [`architecture-assessment.md`](architecture-assessment.md) §5B | each is worth it only on a second adapter or transport, or past ~10 gate stages; `serve.rs` is 16 k lines and its split is a project, not a task | a second transport actually scheduled |
| one `schemas::for_tool(name)` table | refused | [`architecture-plan.md`](architecture-plan.md) P1.4 | the 15 functions carry genuinely different shapes; the invariant that matters is `tests/architecture.rs` | — |

## Retrieval and ranking

| idea | status | evidence | why / what decides it | next measurable step |
|---|---|---|---|---|
| an `aliases:` field in notes, and alias expansion at query time | refused | [`research/alias-field-2026-11-17.md`](research/alias-field-2026-11-17.md) | expansion took 15 candidates to **132 (8.27×)** and ranked the expected note **21st**; automatic pairing is 6/10 precise (285 pairs, 272 divergent) | the corpus-side fix that worked: the acronym in the note's own `description:`, which moved the row from absent to rank 6 |
| semantic retrieval | deferred — **only after candidate generation proves to be the bottleneck** | [`research/candidate-set-first-2026-11-16.md`](research/candidate-set-first-2026-11-16.md), [`research/task-set.md`](research/task-set.md) | ranking cannot fix a document that never entered the pool; a dense index changes cost and failure modes before the pool is measured | candidate-set recall per class, then selection measured separately |
| a freshness prior in ranking | refused, twice | [`research/durable-query-class.md`](research/durable-query-class.md), [`task-set.md`](research/task-set.md) | rejected the first time for cost, the second for a different reason — the query class that would justify it is not the one the task set measures | — |
| status-weighted decay (`current` 1.0, `aging` 0.7, `stale` 0.3, `superseded` 0.05) | deferred — blanket decay was refused; status multipliers are unmeasured here | `/root/vault/inbox/arsumbris-fork-backlog.md` P1⑤ | the enum exists; the weights are a guess until a task-set row moves | measure on the superseded-decision class before any weight |
| a confidence / abstention score | refused | [`research/abstention-2026-11-16.md`](research/abstention-2026-11-16.md) | the best two-feature rule was 6/8 in-sample and **0/8 under leave-one-out** | — |
| title as an indexed field | refused | [`research/task-set.md`](research/task-set.md) §4 | a third field changes `matched`, the `min_should_match` floor and the candidate set, so no weight can be gated away; six indexed variants were measured | shipped instead as post-ranking title coverage (≤2×, rare stems) |
| a blanket discount for `type: moc` | refused | [`research/task-set.md`](research/task-set.md) §4 | the exact answer to a named lookup *is* a map; the shipped rule applies only to queries of ≥3 distinct terms | — |
| gating on the wire `exact` flag | refused | [`research/task-set.md`](research/task-set.md) §4 | wrong boundary once: the task is *called* exact, the request is not | — |
| PageRank in the ranking path | deferred — kept offline | [`research/graph-centrality-2026-11-17.md`](research/graph-centrality-2026-11-17.md) | centrality overlapped degree 5/10, so it adds cost without a new ordering; betweenness would add O(V·E) to every read | run it in `tools/graph_centrality.py` against a failing class |
| betweenness centrality on the serving path | refused | [`research/graph-centrality-2026-11-17.md`](research/graph-centrality-2026-11-17.md) | O(V·E) per read for a signal degree already carries | — |
| a context-pack check that reads the fragments, not the file names | ready — the current check cannot fail when an excerpt loses the answer | verified in code: `tools/kb/context_pack.py:147-160` counts `evidence_found` by comparing returned paths with `task.accept`, and `:83` emits `"hash": None` for an excerpt, so a shortened fragment that drops the claim still scores as evidence found | a two-stage read: fast discovery (path, excerpt, why) then evidence (enough text, an address inside the document, version, status), scored by whether the pack supports the answer at a fixed budget |
| `tb_context` — the context pack as a tool | ready, **conditional** | [`research/context-pack.md`](research/context-pack.md) | only if it beats one call and 2.44 KB; the composition must be measured before a tool is added | the twelve-task pack measured as a composition |
| the task set at 40+ rows | ready (roadmap #3) | [`research/task-set.md`](research/task-set.md) | 1–3 rows per class is not a sample, and a percentage over it is arithmetic without a model | a pre-registered threshold per class |
| enrichment columns (`keywords`, `search_questions`) written by a cheap model | deferred — needs a model and a cache key | `/root/vault/inbox/arsumbris-fork-backlog.md` P1⑥ | recall on paraphrase is the claim; it is unmeasured on this task set, and a model on the write path is a new dependency | measure the paraphrase class before adding a column |
| the 160-question set as an instrument | shipped as a baseline, not as the task set | [`research/question-set-baseline-2026-11-16.md`](research/question-set-baseline-2026-11-16.md) | keyword strings beat questions and half the noise returns nothing — which is why the task set is task-level | — |
| the judge (`subq`) as an acceptance gate | refused; kept as a triage filter | [`research/judge-capabilities.md`](research/judge-capabilities.md) | staleness accuracy **0.884 at recall 0.333** — a filter, not an acceptance gate | — |
| a study with 8–12 external users | not executable here, dropped | [`trajectory.md`](trajectory.md) §what changes in this trajectory | it cannot be run from this host, and a soft promise is worse than a refusal | dogfooding records the same numbers a user would be told |
| a frozen 150-question set with machine grades and a 40-question kappa slice | deferred — needs hand labels and a measured paired σ | [`research/semantic-eval-practices-2026-11-16.md`](research/semantic-eval-practices-2026-11-16.md) §5–6 | every minimum detectable effect in this repository is arithmetic over an assumed σ, and the second opinion withdrew the first set's σ as the wrong type | one graded run on two builds replaces the assumption |
| a graded metric, or an MDE, as a shipping gate | refused as a gate, kept as description | [`trajectory.md`](trajectory.md) §measurement lane (MDE 0.23 at n=18, 0.077 at n=160; the shipped change was +0.033), [`research/second-opinion-2026-11-16.md`](research/second-opinion-2026-11-16.md) O1–O2 | a change below every effect the set can certify makes the metric a veto, not a gate | — |
| token accounting (`tokens_per_answer`) instead of bytes | ready | [`research/intelligence-axes-2026-11-16.md`](research/intelligence-axes-2026-11-16.md) §Axis 2 (baseline 610 tokens, 1.00 call) | we optimise the number we count and the client pays a different one | one probe extension reporting tokens beside calls |
| `hubs` ranked at build time rather than per read | ready | [`research/visit-bound-2026-11-16.md`](research/visit-bound-2026-11-16.md) (linear 0.92, log-log **1.53**) | the only read that grows faster than its candidate set; a cheaper scan would not fix it | one build-time ranking compared on the 20-query set |
| dropping `offset`/`limit` from the ten non-paging read schemas | ready | [`research/intelligence-axes-2026-11-16.md`](research/intelligence-axes-2026-11-16.md) §fixed-cost table (≈3 KB) | saves bytes without touching the validation contract | one schema diff, `tests/architecture.rs` green |
| condensing an over-long query engine-side | deferred — needs a measured BM25 description-dilution rate | [`research/arscontexta-methodology-2026-11-16.md`](research/arscontexta-methodology-2026-11-16.md) §4 | condensation is a fix for a dilution rate nobody has measured on our own 1 032 descriptions | the dilution measurement, then an A/B on the same queries |
| 1-hop link expansion as a second ranking mode | deferred — lexical-only nDCG@5 must first underperform expansion | [`research/arscontexta-methodology-2026-11-16.md`](research/arscontexta-methodology-2026-11-16.md) T8 | the comparison has not been run, so the mode has no gap to close | the comparison on the frozen copy |
| a source-URL identity lookup | shipped | [`research/source-url-lookup-2026-11-16.md`](research/source-url-lookup-2026-11-16.md) | a pasted URL finds the note that carries it; the absent-id row moved to rank 4 | — |

## The knowledge base

| idea | status | evidence | why / what decides it | next measurable step |
|---|---|---|---|---|
| `family:` as series/provenance, with topics as subject | decided and shipped as a hybrid rule | [`research/family-field-2026-11-17.md`](research/family-field-2026-11-17.md) | 429 values, 345 single-member — 159 provenance-shaped, **186 topic-shaped**; only 2 collisions | `tools/kb/family_audit.py` delta per week; the 186 are the intake queue |
| split the queue item out of `type: claim` into `ingest-task` | blocked — the external queue writer is not in this repository | [`research/claim-type-2026-11-17.md`](research/claim-type-2026-11-17.md) | 11 notes: 6 propositions, 5 queue items; the pipeline change must land in the same step | `instances_of claim` needs no `evidence` filter |
| a generated `## Named by` section in every map | refused, written and reverted | [`research/moc-coverage-2026-11-17.md`](research/moc-coverage-2026-11-17.md) | the map body becomes a second copy of the frontmatter index, and 372 edges move without a claim; `references_in` already serves the reverse index | — |
| the map residue — 102 declarations across 21 maps | ready (roadmap #8) | [`research/moc-coverage-2026-11-17.md`](research/moc-coverage-2026-11-17.md) | 73% of the original 372 is done; everything left is ≤4 notes per map except `index`, which is deliberate | `tools/kb/moc_links.py --check` passes per map touched |
| a second duplicate-merge pass | deferred — no evidence pairs are still appearing | [`research/duplicate-findings-2026-11-17.md`](research/duplicate-findings-2026-11-17.md) | 7 pairs merged after two ingest passes; a `superseded` field is the cheaper answer if they recur | re-run `tools/kb/duplicate_findings.py` after the next ingest |
| a note-level `superseded_by` field | deferred — only if pairs recur | [`research/duplicate-findings-2026-11-17.md`](research/duplicate-findings-2026-11-17.md) | the corpus already carries `status: superseded`; a second mechanism needs a case it cannot express | one case it cannot express |
| collapsing lineage echoes (36 notes are 17 observations) | deferred — needs a per-group decision | [`research/lineage-echoes-2026-11-16.md`](research/lineage-echoes-2026-11-16.md) | 13 of the 17 groups carry no kinship signal, so a rule would guess | decide one group by hand, then generalise only if the rule holds |
| `error→fix` atoms as a type | deferred — a repeat rate is measured, the record shape is not | `/root/vault/inbox/arsumbris-fork-backlog.md` P1⑦ | repeated errors were counted (terminal 15, execute_code 18 in 26 h); a type is only worth it if the fix is findable | one type definition and a probe over the log |
| a stale-`description` detector | ready | `/root/vault/inbox/arsumbris-fork-backlog.md` P0③ | a stale description is a future duplicate miss, because duplicate detection reads it | a detector with `--selftest` over the census |
| validating broken `[[wiki-links]]` at ingest | shipped — diagnostics catch them, and the vault has 0 broken links | `/root/vault/inbox/arsumbris-fork-backlog.md` P0②, `tools/cards/metrics.json` | the engine's diagnostics run before a note joins the graph | — |
| an FTS5 date filter | moot — the engine's own BM25 index replaced FTS5 | `/root/vault/inbox/arsumbris-fork-backlog.md` P0① | the quirk existed in the SQLite layer, which is gone | — |
| the last 10 intake refusals | deferred — each was refused for a stated reason | [`research/intake-targets-2026-11-17.md`](research/intake-targets-2026-11-17.md) | five share only a category; five have no candidate and no source that names them | re-run when a source that names the target arrives |
| scheduled connectors (GitHub events, mail) | deferred — only on an observed context loss | `/root/vault/inbox/arsumbris-fork-backlog.md` P2⑧ | GitHub already reaches the vault through the digest path | one recorded loss that the digest missed |
| a dashboard or graph visualisation | refused | `/root/vault/inbox/arsumbris-fork-backlog.md` P2⑨ | `overview`/`hubs`/`type_tree` answer the structural question; a picture solves no observed problem | — |
| a TUI | unknown (unargued) | a scan of this repository's documents finds no record that states the refusal; the only text was an earlier draft of this page | a TUI is not refused here, it is simply not proposed with a consumer; an idea whose refusal cannot be quoted is not a refusal | a consumer that names the interface it is missing |

## The graph

| idea | status | evidence | why / what decides it | next measurable step |
|---|---|---|---|---|
| community detection (label propagation) to propose merges | deferred — needs the consolidation pass as its consumer | [`research/graph-algorithms.md`](research/graph-algorithms.md) §clustering | "clusters without a consumer" are decoration; the pass that would act on them does not exist | the consolidation pass named as the consumer |
| a declarative traversal object behind one read verb | deferred | [`graph-engine-assessment.md`](graph-engine-assessment.md) §4, §7 | it is the one genuinely new product capability agents ask for in Cypher terms, but it needs a shape and a fail-closed typing story | one traversal that replaces two composed reads |
| migrating the graph to a store (HelixDB, Samyama) | refused | [`graph-engine-assessment.md`](graph-engine-assessment.md) §6; [`retrieval-spike.md`](retrieval-spike.md) (hub ranking 103.5 ms vs 9.8 ms) | the engine is text-search-bound, not storage-bound, and a store makes the graph a second authority | — |
| GPU or approximate centrality (HyperBall) | refused | [`research/sparx-2026-11-17.md`](research/sparx-2026-11-17.md) §do not adopt | runtime and approximation cost with no demonstrated bottleneck or relevance gain | — |

## The agent layer and the ontology

| idea | status | evidence | why / what decides it | next measurable step |
|---|---|---|---|---|
| an FSM in the agent layer (C2) | deferred — only where it is earned | [`research/meta-prompt-fsm.md`](research/meta-prompt-fsm.md), [`agents-and-decisions-assessment.md`](agents-and-decisions-assessment.md) | `tools/fsm/check_run.py` already enforces the rules that matter; a runtime state machine needs a second consumer | a workflow whose rules the checker cannot express |
| a typed decision layer over the subq API | deferred — the data-egress question is unanswered | [`agents-and-decisions-assessment.md`](agents-and-decisions-assessment.md) | the local `type: decision` + `backed_by` walk covers what is measured today | the local walk failing on a real question |
| ruVector as a dependency | refused as a dependency, kept as a pattern source | [`research/ruvector-crates-2026-11-16.md`](research/ruvector-crates-2026-11-16.md) | twelve mechanisms studied; a crate brings its cost and its maintenance with it | take one mechanism as a design, not a crate |
| four capability agents with fixed roles | deferred | [`agent-roles.md`](agent-roles.md) | a role without a consumer is a fiction; the same reason spawned subagents were used for this pass instead | one role with a measured job |
| tools generated from the ontology (P4) | deferred — needs a consumer of the ontology | [`architecture-plan.md`](architecture-plan.md) P4 | the ontology exists; nothing consumes it yet | one generated tool that replaces a hand-written one |
| the answer contract as MCP prompts and resources | shipped | [`research/answer-contract-2026-11-16.md`](research/answer-contract-2026-11-16.md) | both cost zero schema bytes and are what the protocol has a slot for | — |
| the arscontexta operational skills layer | deferred — the MCP successor lacks it, and demand is unmeasured | [`research/arscontexta-skills-2026-11-16.md`](research/arscontexta-skills-2026-11-16.md) | the three arscontexta studies are a pattern source, not a port list | port one skill and measure its use |
| kutha's twenty mechanisms | deferred — ranked, none in the queue | [`research/kutha-ideas-2026-11-16.md`](research/kutha-ideas-2026-11-16.md) | a mechanism enters the queue only against a measured gap | pick the one that answers a failing task-set class |
| pogocache's patterns | deferred | [`research/pogocache-patterns.md`](research/pogocache-patterns.md) | the cache is a derived structure here, and the engine already holds the graph in memory | a measured read-latency gap |
| per-claim staleness through a generative claim extractor | deferred — the unit must be the claim | [`research/judge-capabilities.md`](research/judge-capabilities.md) (0.884 accuracy at 0.333 recall per note) | a note with one stale claim of four reads as fresh, so note-level freshness cannot express the defect | a claim extractor plus a labelled per-claim sample |
| plugin loading, or jobs as MCP sampling hooks | refused | [`phase-d.md`](phase-d.md) §the plugin question (measured 2026-11-15: no workspace declares a plugin) | a compiled binary cannot load TypeScript modules, and nothing declares one; process-per-plugin is the shape if it ever matters | — |
| sparx's static graph analytics | shipped, as two diagnostics | [`research/sparx-2026-11-17.md`](research/sparx-2026-11-17.md) | bridges and articulation points (`G1`) and triangles/transitivity (`G2`) are additive `graph_health` fields; 37 bridges, 57 articulation points on the snapshot | — |

## Distribution and scope

| idea | status | evidence | why / what decides it | next measurable step |
|---|---|---|---|---|
| a published binary artifact, and `install.sh` that uses it | ready (roadmap #2) | [`internal/clean-host-gate-2026-11-17.md`](internal/clean-host-gate-2026-11-17.md) | the binaries are self-contained and the clean-host gate passed, but `install.sh` builds with `cargo +1.99.0` | install from the artifact on a host without cargo, then `doctor` |
| macOS binaries | ready, unproven | [`internal/clean-host-gate-2026-11-17.md`](internal/clean-host-gate-2026-11-17.md) | the clean-host gate ran on Linux x86_64 only | build, `doctor`, one read and one write |
| collapsing the surface into seven entries with a mode | refused — but the routing test was never run | [`research/intelligence-axes-2026-11-16.md`](research/intelligence-axes-2026-11-16.md) §Axis 1 ("seven entries against thirty-six"); [`research/tool-surface-tokens-2026-11-16.md`](research/tool-surface-tokens-2026-11-16.md) (concise profile 59 127 → 19 404 B, −67 %, `required` identical 36/36) | the surface costs bytes, not count, and the cheap part is already shipped; what the collapse would save is unmeasured | a 24-task routing test measuring the wrong-tool rate before any collapse |
| a Hopfield-style / learned selector before labelled data | unknown (unargued) | no document in this repository states the refusal; a scan of 1 874 markdown files finds zero mentions of the mechanism outside this page | a selector cannot be evaluated without labels, but that is an argument, not a recorded measurement — so it is not a refusal | a labelled set first, then a selector measured against it |
| a hosted, multi-tenant service | refused | [`../LICENSE.md`](../LICENSE.md), [`README.md`](../README.md) | Sustainable Use 1.0 forbids paid hosting of the server; the trust boundary is one operator | — |
| the Node/Python stack on the request path | removed | [`phase-d.md`](phase-d.md) | retired 2026-11-16; the serving path is one Rust binary | — |

## The documentation system

| idea | status | evidence | why / what decides it | next measurable step |
|---|---|---|---|---|
| keep maintaining `llms.txt` as a second map | deferred — kept as a 3 KB shim, not as a source of truth | Ahrefs study of 137 210 domains: ~28% publish a valid file and **97% of those got zero requests** (2026-06-15); Google states it does not use it; a repository has no URL for a host to fetch | the map that agents actually read is `docs/README.md`; `llms.txt` restates it and costs little, so it stays until one of the two is needed alone | delete it when a reader confirms `docs/README.md` is the entry point they use |
| gate counts generated from a saved test log instead of typed | shipped 2026-11-18 | `tools/cards/metrics.json` carried 211 for the adapter while `docs/trajectory.md` said 256 and a run summed to 260; `cargo test` prints a per-suite total and no grand total, so every hand-typed number is a guess with a date | `tools/cards/gates.py --from-log <file> --suite adapter` writes the value, the failure count and the method from the log; it computed 260 from the real 46-suite log and the cards still matched |
| a documentation job in CI | ready | CI runs `fmt`, `clippy`, tests and `render.py --check`; it has no doc check | `tools/docs/links.py --check` is dependency-free (802 documents, 0 broken) and would catch the class of defect the ADR-status case came from; `repowise doc-drift` needs an index, so it cannot run on a bare runner yet | add a job that runs `links.py --check`; add `doc-drift` only if the index can be built in CI |
| arc42 or C4 templates | refused | arc42's own canvas guidance; C4 "context and container diagrams are sufficient for most teams" | two crates, one binary, one socket, one vault — `docs/architecture.md` answers what runs and what talks to what, and states what it does not cover | — |
| a Mermaid container diagram in `architecture.md` | ready | the page is prose only; C4's context+container level is the one that pays | a reader asking "what talks to what" currently reads a code block | one diagram, no second architecture artefact |
| Vale or another prose linter | refused for now | no style contract exists in this repository | a linter with no contract enforces taste, not rules | write the style contract first, then decide |
| a required `Status:` line on any page that states future work | ready | `ADR-0001` read `Proposed` after its plan had been executed, so intent read as shipped | the cheapest fix for the most dangerous doc defect | one line in the frontmatter of the plan and design pages, checked by `links.py` or a sibling script |
| the `0001-` prefix shared by three files in `tb-engine/doc/adr/` | shipped 2026-11-18 | the two companions moved to `tb-engine/doc/plans/`; `adr/` now holds only records, and its README says so | a reader scanning the directory can tell the record from its annexes | — |

---

## How to use this register

- **Before proposing an idea**, search this page. A `refused` row is an answer; a `deferred` row names what would
  reopen it.
- **When a deferral's dependency arrives**, the row moves to the roadmap with an exit criterion, and this page
  keeps a `shipped` line with the date.
- **A new refusal needs a number.** "We tried it and it felt worse" is not a refusal; it is an `unknown`.
