# What the ruvector ADR corpus decides that a git-backed note base can use

Read of `/root/vendor-source/ruvector/docs/adr` (read-only), 2026-11-16. Scope: the decision layer, not the
marketing. Every entry below names a mechanism, the exact `path:line` (or `path` for a document), the 2brain
subsystem it would change, a cost grade, the failure it risks, and the smallest experiment that decides whether it
is worth building.

## Method and counts

- The directory holds **317 top-level ADR markdown files**, one generated `INDEX.md`, and four namespaced
  directories (`coherence-engine` 22 `ADR-*` files, `delta-behavior` 10, `quantum-engine` 15,
  `temporal-tensor-store` 6 — 53 in all, of which the generated index counts 47 as namespaced ADRs). The index
  reports **370 ADR files indexed, 323 on the canonical counter, 27 frozen duplicate numbers spanning 61 files**
  (`docs/adr/INDEX.md`). The "322" in the brief is the directory's entry count, not its ADR count.
- Every title was listed and classified in one pass before reading (buckets and counts in the appendix).
- **40 ADRs were read in full** (end to end, including Evidence, Failure Modes, Alternatives and Open Questions);
  **277 were classified only**. A further group (about 20) had their Decision and Evidence sections read without
  the tail and are counted as classified only, not as full reads. Stub ADRs (under ~60 lines with no decision
  body) are named in the appendix, not padded into entries.
- One idea under two names is one entry. Where a subject recurs across many ADRs (filtered retrieval, witness
  logs, content addressing), the entry cites the two that decide it and names the rest by id.

**How to read the rank.** Entries are ordered by value per unit of risk, not by how interesting the mechanism is:
a compile-time change with no serving path ranks above a new index that would need a migration. Cost is S
(under ~300 lines, no new persisted state), M (300–1000 lines, or a new persisted/derived artifact), L (a new
crate, a new service, or a migration of stored data). Risk names what breaks or gets slower, not "complexity".

**Status versus code, checked with the code graph.** The status line describes integration, not existence. The graph
(100,930 nodes, 4,492 files over this tree) plus a direct source check show five mechanisms cited here already exist
as compiled crates with tests while their ADR still says *Proposed*: `crates/ruvector-agent-memory/src/arbitration.rs`
with `ten_memories_one_source_is_one_lineage` (`tests/arbitration.rs:63`), called only from the benchmark
(`src/main.rs:342`); `crates/ruvector-diskann/src/reuse.rs:254` `RecallTrigger`, re-exported at `src/lib.rs:27` and
referenced nowhere else; `crates/ruvector-retrieval-receipt/src/lib.rs:121`; `crates/ruvector-staged-workspace/tests/invariant.rs:23`
and `:153`; and `crates/ruvector-context/src/shard.rs:28`. In each case the ADR's own next step ("the live serving hook
is the remaining glue", "not wired into the default query path") is accurate and the status line is not: a reader who
stops at "Proposed" underestimates what exists, and one who sees a crate assumes it is serving. The disagreement is
systematic — this corpus builds reference implementations and names integration as the remaining gap — so where one
exists below I lower the *implementation* risk, not the data-model risk. I did not use degree as evidence of
importance: the graph's hotspot list is dominated by trivial impls with four-digit fan-in from generated code.

---

## 1. Restrict the candidate set before scoring, never after ranking

**Mechanism.** A selective predicate (repo, scope, type, status, or a caller's capability mask) is applied to the
candidate set *before* scoring, so a low-selectivity filter pays only for eligible documents; the measured
alternative — score everything, drop the ineligible afterwards — returns the same answers for 7.9× the latency at
12.5% selectivity, and loses recall entirely once a top-k cut sits above the filter.

**Source.** `docs/adr/ADR-160-acorn-filtered-hnsw.md:37` (post-filter starvation at low selectivity),
`:69` ("expand neighbors of ALL visited nodes regardless of predicate"), `:86` (pre-filter baseline);
`docs/adr/ADR-268-capability-gated-ann.md:52` (EagerMask: 57 µs vs 450 µs at 12.5% access, both recall@10 = 1.000,
`:184`). ADR-160, ADR-268.

**What it would change in 2brain.** `serve.rs:14592-14594`: `search_view` runs the full ranking and then
`.filter(|hit| in_read_scope(...))`. `repo` and `scope` therefore decide only what is *returned*, not what is
*scored*. Move the predicate into candidate selection in `search.rs` (an eligible-document bitmap per repo/member,
computed at index build and patched with the same mutation that moves a file), and keep `in_read_scope` as the
single shared predicate so `grep` and `search` cannot disagree. The same hook is what any future `type:`,
`status:` or `freshness:` facet must use; building those as post-rank filters would repeat this defect and, once a
top-k cut is added for latency, would become a recall bug rather than only a cost bug.

**Cost.** S–M. A per-repo document bitmap, a filter hook in `search_view`, and a patch path in `incremental.rs`;
~150–300 lines, no new persisted state (membership is already in `kb.repos` and the catalogue).

**Risk.** A stale bitmap after a rename or a move makes a note silently unsearchable, and no ranking metric will
catch a false negative. If the bitmap and `in_read_scope` are two implementations, they will drift; the ADR's own
lesson is that the filter must be the same predicate at both places.

**First experiment.** On the served vault, run 20 labelled queries at `scope: own` and `scope: all` and compare
`documents_visited` from `graph_health {verbs: true}` (the counters exist in `verb_metrics.rs`; the MCP adapter
does not yet declare `verbs`, so run against the engine socket). The ratio own/all is the wasted work. Then assert
the answers are byte-identical to today's. The candidate engine (`tools/ranking/candidate_engine.py`) is the
control.

---

## 2. Classify every write as add / ignore / revise / reject / defer, and persist the pending and rejected states

**Mechanism.** Every incoming statement is forced into exactly one of five ledger operations — add, ignore,
revise-outdated-belief, reject-unreliable, defer-for-verification — and accepted, pending and rejected states are
persisted, so a write is a claim about what it supersedes rather than an unconditional append.

**Source.** `docs/adr/ADR-307-three-level-persistent-memory-livemem-tarl.md:31` (the five operations),
`:78` ("No incoming observation writes directly to the transactional tier without passing through the
classification"). ADR-307 (TARL pattern, arXiv:2608.03699).

**What it would change in 2brain.** `mutate.rs` and the type system. The substrate is half-present:
`tb-base-types/type/supersede.type.yaml` gives an optional `superseded_by`, but only **3 of 1,032 notes** use it,
and there is no `reject` or `defer` distinction — `status: pending` exists (13 notes today) without a rule for
what may leave it. Add (a) a `tb_revise` mutation that writes the new note and its predecessor's `superseded_by`
in one saga (so the link cannot be half-written), (b) `reject` and `defer` states with a required reason and a
reviewer, and (c) a diagnostic that fails a `revise` which names no predecessor, or a supersession cycle. The
*enforcement* belongs in the verb, not in optional frontmatter: the 3/1,032 measurement is what happens when the
link is voluntary.

**Cost.** M. New frontmatter fields, one mutation plus its diagnostics, a state transition check in
`resolution_build.rs`, and a migration for notes that want the new states; ~600–900 lines across `mutate.rs`,
`resolution_build.rs`, `diagnostics.rs`.

**Risk.** Status semantics are load-bearing for the durable query class (built from `freshness` and descriptions)
and for the freshness prior that was already rejected twice. Adding states can invalidate the labelled
expectations. A one-directional `superseded_by` will make search prefer a deprecated note unless the read path
demotes it explicitly at query time rather than by a global score multiplier (the failure 2brain already
measured).

**First experiment.** Classify the last 200 mutations in the vault's git history as new-file / content-replacement /
correction-of-an-existing-claim / deletion, using the diff and the note's own text. The decision number is the
share that are *corrections* (a revise, not an add). If it is under ~5%, the ledger is premature; if it is high —
as a project memory written by several agents should be — the four states are the missing vocabulary, and the
3/1,032 figure says agents will not supply it voluntarily.

---

## 3. Count evidence lineages, not documents (and bound the count above)

**Mechanism.** Retrieved documents are resolved to their recorded sources and clustered into evidence lineages;
each lineage contributes one vote, so ten copies of one source count once, and the arbitrated score is provably
never larger than the naive count.

**Source.** `docs/adr/ADR-330-cama-pattern-correlation-aware-memory-arbitration.md:84` (`effective_evidence` per
lineage), `:90` (downgrade-only bound), and the security corollary at `:138` (only *declared* derivation is
detected). ADR-330.

**What it would change in 2brain.** `search.rs` ranking plus the type system. The vault already exhibits the
failure: the self-retrieval probe found four archived copies outranking their live originals, and the durable
class has 13 of 26 pairs where the transient note wins. Add a `derived_from`/`source` edge (or interpret
an existing `[[link]]` as derivation only when the type says so), group hits by lineage root after ranking, keep
one representative per lineage, and report `lineage_size` per hit. The durable prior's third attempt should be a
query-time facet, and lineage grouping is the facet that addresses the duplicate problem structurally rather than
by reweighting.

**Cost.** M. The ranking pass is ~100 lines; the work is the provenance edge and deciding which links mean
derivation. Without a recorded edge the mechanism degrades to duplicate detection by description shingles, which
is a heuristic, not the ADR's mechanism. A tested reference for the scoring half is already in-tree
(`crates/ruvector-agent-memory/src/arbitration.rs:181`, `:227`; `tests/arbitration.rs:63`), called only from the
bench (`src/main.rs:342`).

**Risk.** The ADR's own corollary is the risk: a copy that records no parent mints a fresh lineage, so this is a
correctness mechanism for honest-but-correlated writers, not an anti-poisoning control (their audit measured 20
rumour-copying agents yielding 21 lineages and zero downgrade). A wrong merge suppresses genuine independent
evidence, and aggregate hit rates will not show it.

**First experiment.** Build duplicate families two ways — identical description shingles, and the
`ops/skills-archive` path pattern — and measure how many of the 60 labelled queries have two or more members of one
family in the top 10, and whether collapsing to one changes hit@1. The four known archive inversions are the first
case; the 13-of-26 durable pairs are the second.

---

## 4. Make the write gate a type, not a call

**Mechanism.** The writer accepts only a value that can be produced by the validator, so an unvalidated note is
not merely discouraged from reaching disk — it cannot be passed to the write function at all.

**Source.** `docs/adr/ADR-047-proof-gated-mutation-protocol.md:21` (`ProofGate<T>`, inner value private),
`:48` (`unlock` / `into_inner` are the only accessors), and the "no `unsafe` escape hatch" note at `:32`.
ADR-047.

**What it would change in 2brain.** `tb-core`'s types and `mutate.rs`/`tb-mcp-rs`. Validation today is a set of
calls (`value_validate`, `ensure_mixin`, diagnostics) that each entry point must remember; there are 16 mutating
verbs and several internal writers. Introduce `Validated<Note>` (or `Note<Checked>`) with a private field,
constructed only by the validator module, and make `write_file`, `create_file`, `edit_file` and the rename/move
paths take it. `ProofRequirement` maps to the existing checks; there is no proof engine to port.

**Cost.** S. The validators exist; this is a newtype plus constructors and call-site churn, ~200 lines. Zero
runtime cost, no state, no migration.

**Risk.** Call-site churn in a 4.5k-line adapter. A `#[cfg(test)]` bypass constructor will be added for
convenience and will quietly become the production escape hatch; the type must be `!Copy` and `!Clone`-by-default,
with the test constructor in a module that production code cannot import.

**First experiment.** Not a benchmark: a compile test. Delete the validator call on a branch and confirm the build
fails. Then count how many functions today both construct a note and call a writer inside one body — that is the
bypass surface the type closes.

---

## 5. Reuse a derived structure while a probe says it is still good; rebuild on a measured ceiling

**Mechanism.** A derived structure is reused across mutations while a cheap probe over a fixed query sample says
its answers still agree with a fresh build; a periodic rebuild covers the tail, and the probe's cost must be a
small fraction of the rebuild it avoids.

**Source.** `docs/adr/ADR-202-reuse-under-drift-real-gnn-trajectory.md:28` (holding ceiling: reuse within 2% up to
40% churn), `:66` (`RebuildPolicy { AlwaysRebuild, ReweightOnly, Periodic { k } }`), and the addendum (`:209`:
probe trigger reaches 97.2% recall at 7 rebuilds vs 12, probe cost <2% of the ~73 s saved; stale control collapses
92% → 33%). ADR-202, with ADR-200 as the synthetic predecessor.

**What it would change in 2brain.** `incremental.rs`. The index is patched per mutation; the open question is when
an incrementally patched index, backlink graph or resolution graph should be trusted versus rebuilt. Add a
`RebuildPolicy` and a probe over ~20 fixed queries that compares the live ranking to a freshly built copy, expose
`last_probe_gap`, `stale_since_rebuild` and `rebuilds` in `graph_health`, and make the probe use the candidate
engine in tests. The existing golden captures are the anchor: a probe that disagrees with a golden is a defect
regardless of the policy.

**Cost.** M. Probe harness, policy, and metrics; ~400 lines. No new persisted state beyond a counter, unless the
probe set is stored in the workspace. `RecallTrigger` is already implemented at
`crates/ruvector-diskann/src/reuse.rs:254` with tests at `:471` and `:482`, and no caller outside the crate — exactly
the ADR's stated remaining glue.

**Risk.** Running the probe per mutation puts real work on the write path; running it too rarely lets a stale index
serve silently. A probe drawn from the notes themselves can be tautological — 2brain's own self-retrieval
experiment says so explicitly — so the probe set must be queries, not descriptions. The ADR's stale control is the
warning: a benchmark that cannot show degradation proves nothing about the rebuild policy.

**First experiment.** On a vault copy, apply 100 real mutations in order; at 10 checkpoints compare the
incremental ranking to a full rebuild on a 20-query probe and report agreement. If agreement is 1.0 through 100
mutations, the policy has nothing to buy and should be dropped; if it falls, the churn point is the threshold.

---

## 6. Bind every derived or adopted artifact to the hash of the bytes it was derived from

**Mechanism.** A derived artifact (a preview receipt, an excerpt, a cached package) records the content hash and
revision of the exact input it was computed from, and a mismatch at read time is a hard rejection rather than a
warning, so stale state is invalid by construction.

**Source.** `docs/adr/ADR-318-stagedworkspace-content-hash-state-binding.md:84` ("Stale state is automatically
invalid"; a hash mismatch is a hard rejection at read time, not a convention),
`docs/adr/ADR-329-recache-pattern-schema-resource-cache.md:94` (identity is SHA-256 over the *resolved* content,
never a self-declared name, plus downgrade-only hit accounting),
`docs/adr/ADR-334-scope-sharded-context-index.md:57` (never adopt a pre-existing file at a computed name).
ADR-318, ADR-329, ADR-334.

**What it would change in 2brain.** `pkgcache.rs` first. Its identity is a resolved commit sha — the code says so
plainly and adds that the name is "deterrence, not cryptographic integrity" (`pkgcache.rs:184-188`): a partial or
poisoned extraction under a known sha is adopted by name, and a concurrent populate is accepted without checking
what landed. Add a manifest hash over the extracted tree (or a signed tree hash verified once at adoption) and
refuse a snapshot whose content does not match. Second, `tb_preview_mutation`: the answer should carry the hash of
the state it ran against, so an apply can reject a preview taken against a different tree. Third, excerpt/backlink
answers should carry the note hash they were cut from when the excerpt crosses a mutation boundary.

**Cost.** S–M. One manifest per cache entry and a verify-on-adopt path (~150 lines); a hash field in preview
answers (~50); no migration. The tree hash is computed once per adoption, not per read. The hash-plus-revision
rejection is implemented as invariant tests in `crates/ruvector-staged-workspace/tests/invariant.rs:23` and `:153`,
a workspace member with no dependents.

**Risk.** Hashing a large pack on every adoption costs milliseconds and is fine; hashing on every read is not.
A hard reject can turn a benign race into a failed agent step, so the error must name the current hash and the
expected one. A tree hash that depends on directory enumeration order will flake; canonical ordering is the real
work.

**First experiment.** Corrupt one byte in a cached package snapshot under a known sha, then run the member that
depends on it: today the corruption is served. After the change, adoption must fail naming the mismatched file.
Time the hash of the largest cached pack; it must stay under the fetch it replaces.

---

## 7. Bound a read with a ladder that has an unconditional floor and a hard round cap

**Mechanism.** A more expensive pass (proximity, graph expansion, more candidates) is purchased only when its
expected gain exceeds its cost, while a set of mandatory checks runs regardless of the economics and a hard cap on
rounds bounds the worst case, because a rung whose evidence never resolves the question is bought forever.

**Source.** `docs/adr/ADR-331-pandora-pattern-voi-cost-aware-routing.md:97` (escalate-only invariant: the gate may
only turn a cheap route into an expensive one, never rescue a weak candidate into the cheap path),
`docs/adr/ADR-337-adaptive-runtime-monitoring-voi-escalation.md:215` ("`max_rounds` is the only hard bound on
wall-clock cost"; measured 2.6% when the first rung resolves, 108–205% when it does not, 1075% on an all-mandatory
stream, against a 5% target), `:90` (the unconditional floor is not subject to the economics).
ADR-331, ADR-337. ADR-272 (recall-bounded search: a quality threshold instead of a fixed k) is the API shape that
makes "how much is enough" askable.

**What it would change in 2brain.** `search.rs` and `wire.rs`. Today `tb_search` always runs BM25 with the proximity
pass walking every unordered query-term pair in each candidate before `min_should_match` filters; `min_should_match`,
`best_idf` and `weak` are the only uncertainty signals.
Add an `effort` (or `escalate`) request field with `max_passes`, a mandatory floor (grants, freshness, scope are
always checked — never purchasable), and an answer field reporting the passes actually run and why. The proximity
tail work already named in `docs/trajectory.md` is the first rung: run it only on documents that already satisfy
`min_should_match`.

**Cost.** M. Wiring effort through verb, ranking and answer, ~300–500 lines; no new state. The ladder itself is a
pure function and is testable without the engine.

**Risk.** The measured failure is precise: an escalation whose signal does not resolve costs `max_rounds` every
time, so the cap must be set against the workload's own latency, not against a target percentage. If `weak` is a
poor uncertainty estimate, the ladder either never escalates (no gain) or always escalates (a slowdown). The
`value_of_success` analogue is policy: a nominal 1.0 makes the gate inert while every metric looks healthy.

**First experiment.** On the 60 labelled and 30 absent queries, record `best_idf`, `matched_terms` and `weak`, then
compute offline whether a rule "escalate when `best_idf` is in [1.0, 2.0)" would have changed any top-5 ordering.
If no ordering changes, there is nothing for the ladder to buy. Also measure the proximity pass's share of p95
(689 µs today); a ladder that cannot move the tail is not worth its branch.

---

## 8. Score abstention on unanswerable queries instead of scoring only answers

**Mechanism.** A retrieval system is graded by a decision-theoretic utility that pays for a correct answer,
charges for a wrong one, and pays for abstaining on a query the corpus cannot answer, which makes "I don't know" a
measured action rather than an unmeasured failure.

**Source.** `docs/adr/ADR-270-self-reconstructing-graph-memory-beyond-sota.md:53` (`abstainThreshold`),
`:60` (the utility: correct +1, abstain 0, wrong −1; unanswerable abstain +1, any answer −1),
`:158` (hallucination 0.17 → 0.03, calibration 0.417 → 0.775). ADR-270.

**What it would change in 2brain.** `tools/ranking` and `search.rs`'s `weak` threshold. The pieces exist but are
not measured as a decision: `RARE_IDF = 2.0` (`search.rs:158-166`) flags a hit as weak, and `tasks_eval.py:101-103`
counts an absent question as unanswered when the top hit matched at most one term — its own header calls that "a
weak test, labelled weak rather than dressed up". Add ~30 absent queries (the current set has three), score with
the +1/0/−1 utility, and sweep `RARE_IDF` instead of fixing it.

**Cost.** S. A query file and ~100 lines of scoring; no engine change unless the sweep moves `RARE_IDF`, in which
case one constant plus its golden capture.

**Risk.** A threshold tuned on three absent examples overfits, and the ADR records exactly that failure (abstention
confidence derived from traversal depth looked as weak as an unanswerable query). A threshold set too high
suppresses real answers that legitimately use common words — the `weak` doc comment already argues why such a note
must remain retrievable.

**First experiment.** Sweep `RARE_IDF` in {1.5, 1.75, 2.0, 2.25, 2.5} over the 60 labelled plus the 30 absent
queries. Report the risk score, the false-abstain count on answerable queries, and hit@5. Decision rule, stated
before the run: adopt a value only if risk rises and answerable hit@5 does not fall.

---

## 9. Measure an accelerator against the tuned incumbent, and accept a structural NO-GO

**Mechanism.** An accelerator is compared head-to-head with the already-tuned cheap method at matched quality; if
both exploit the same locality, the smarter bound prunes exactly what the incumbent already skips, so the win is
structurally impossible rather than merely hard, and the honest outcome is not to build it.

**Source.** `docs/adr/ADR-205-region-pruned-ivf-vs-plain-ivf-nprobe.md:37` (pre-registered gate),
`:79` ("The win is not 'hard'; it is **structurally impossible** against a tuned incumbent"), `:99` (the low-dim
control that disproved the predicted sign, recorded rather than hidden). ADR-205.

**What it would change in 2brain.** `tools/ranking` first, `search.rs` only if the measurement survives. The live
candidate is the proximity tail: `docs/trajectory.md` proposes "run it only on documents that already satisfy the
minimum, or skip high-document-frequency pairs". ADR-205's test says measure the overlap between the documents the
proximity pass visits and those `min_should_match` already excludes. If the sets coincide, the optimization saves
nothing and the tail must be attacked differently.

**Cost.** S. An offline analysis over existing postings plus one candidate-engine run; no shipped code until
measured.

**Risk.** Optimizing the wrong axis: skipping high-df pairs can change rankings, not only cost, and the proximity
pass now carries a measured gain (literal hit@1 0.571 → 0.714). The measurement must include a ranking-equality
check on all three query classes, not a stopwatch alone.

**First experiment.** For 100 queries, compute |documents visited by the proximity pass \ documents surviving
`min_should_match`| / |visited|. If it is under 5%, drop the idea. Then apply a hard skip on the top-1% df terms
and compare hit@1/hit@5 on literal, topical and durable classes; a latency win with a ranking change is not the
same result.

---

## 10. Emit a witness for every privileged action, and prove the kinds and the verbs are the same set

**Mechanism.** Every mutating action has a witness kind and emits a hash-chained record as part of the action; if
the record cannot be written the mutation does not proceed, and a census test asserts that the kind enum and the
mutation surface are the same set in both directions.

**Source.** `docs/adr/ADR-134-witness-schema-log-format.md:17` ("No witness, no mutation"; emission is part of the
action, not a logging layer), `:103` ("If a privileged action exists without a corresponding kind, the system has a
gap in its audit trail"), `docs/adr/ADR-194-proof-gated-writes.md:105` (receipts are tamper-evidence, not
authorization — the honest scope). ADR-134, ADR-194.

**What it would change in 2brain.** `mutate.rs`, `serve.rs` and the audit log. The `mcp_policy` records are already
hash-chained and name the tool, and the principal comes from `--principal`. Add one `ActionKind` variant per
mutating verb, a test that every mutating verb maps to a kind and every kind to a verb (the same both-directions
census that `Request::verb()` already gets against `WIRE.md`), and a failure path: if the audit record cannot be
written, the saga compensates instead of committing.

**Cost.** S–M. An enum, a mapping, a census test, and a failure path; ~250 lines. No new state.

**Risk.** A failed audit write now fails mutations that used to succeed — correct, but it surfaces as availability
loss if stderr is a pipe that can block. The sink must be non-blocking and its failure explicit; the ADR's FNV
chain is also not cryptographic, which is fine for tamper-evidence and must not be described as more.

**First experiment.** The census test is the first measurement: it reports how many of the 16 mutating verbs lack a
distinct kind today. Then `kill -9` mid-mutation 50 times and verify every recovered state has either a matching
witness record or a recorded compensation (the saga recovery report already exposes `recovered` and `failures`).

---

## 11. Count tombstones, repair on a threshold, and say what deletion actually erases

**Mechanism.** A deletion strategy is chosen against a measured recall cost, and a repair pass runs when the
tombstone fraction crosses a threshold, because tombstones keep occupying the search structure and degrade results
even though the entry is gone.

**Source.** `docs/adr/ADR-258-hnsw-delete-repair.md:47` (TombstoneOnly −1.9 pp recall@10 at 20% deletion; batch and
eager repair recover 0.9 pp), `:100` (`tombstone_fraction` as the repair trigger; batch size as the rate limit),
`:135` (GDPR: a tombstone does not make the data unreachable; physical erasure is a separate act). ADR-258.

**What it would change in 2brain.** `incremental.rs` and the delete path in `mutate.rs`. If a delete or rename
leaves stale entries and the index is patched rather than rebuilt, add a `tombstone_fraction` counter, a threshold,
and a repair that rebuilds the affected posting lists, exposed in `graph_health`. Separately, decide whether
`delete_file` must physically remove text from the index: the note remains in git history, so the honest answer may
be "removal from the index and the working tree, not from history" — and the tool description should say that,
because the current reader may assume otherwise.

**Cost.** S–M. Counter, threshold, and repair path; ~200–350 lines. A hub-note deletion is the ADR's own unhandled
entry-point case and needs the same election logic.

**Risk.** A repair running inside the mutation slot stalls writes; a repair running concurrently with reads must be
atomic or the index serves a mix. Deleting a hub note can disconnect the derived graph more than the fraction
suggests, so the trigger cannot be only a count.

**First experiment.** On a vault copy, delete a contiguous subtree holding 20% of the notes in 10 batches (the
worst case) and measure hit@1/hit@5 on the labelled set after each batch, with and without repair. Repair is
justified only if the loss exceeds the golden capture's noise band.

---

## 12. Record each experiment as a machine-readable manifest with a pre-registered verdict

**Mechanism.** An experiment carries a manifest that fixes the primary metric, the minimum meaningful effect, the
paired comparison, the seeds and the exact pass/fail/inconclusive rule before it runs; repository settings may
narrow the built-in gate but never widen it, and "inconclusive" is a recorded outcome rather than a failure to be
retried until it passes.

**Source.** `docs/adr/ADR-282-nightly-research-quality-gate.md:126` (inconclusive is first-class; headline values
may not be manually transcribed; confirmation on held-out seeds),
`docs/adr/ADR-335-repo-native-optimization-manifest.md:109` ("may **never** widen"; a manifest is an untrusted
proposal) and `:70` (issue #920: a frozen absolute threshold rejected 30 candidates that were strictly better on
primary, recall, qps and cost), `docs/adr/ADR-325-d2acci-pattern-stage-level-memory-diagnostic-gate.md:72` (attribute
a regression to a pipeline stage, not only to the end result). ADR-282, ADR-335, ADR-325.

**What it would change in 2brain.** `docs/` plus `tools/ranking`. 2brain already pre-states decision rules in prose
("rejected by the rule stated before the run") and records every card number with its command in
`tools/cards/metrics.json`. The increment is a JSON record beside each research note, generated by the runner from a
repo-level manifest that names the three query classes and the gates, with an explicit `inconclusive` branch and a
stage field (parse, index, rank, excerpt) so a regression names the stage that caused it.

**Cost.** S. A schema, a runner change, a CI check; no engine change.

**Risk.** Ceremony nobody reads. The manifest must be emitted by the harness, never hand-written, or it drifts from
the run it describes. #920 is the warning: an absolute threshold frozen in code silently rejects genuine
improvements, so every gate value must be relative to the current baseline and versioned.

**First experiment.** Backfill the five most recent ranking experiments into the schema and check that each has a
reproducible command and a rule that predates its numbers. Then run one new experiment through it and confirm the
verdict is `inconclusive` when the interval overlaps zero and the minimum effect.

---

## 13. Publish a number only with its harness, and retire a metric when its noise exceeds the effect

**Mechanism.** A published result must disclose the corpus, configuration, comparison baseline and procedure, and a
metric is retired as a claim target when its noise band is wider than the effect being claimed — a claim carries
its inversion point, not only its best point.

**Source.** `docs/adr/ADR-277-rvagent-positioning-and-claims.md:161` (SWE-bench Verified retired because "the noise
band exceeds the effect sizes we would be claiming"), `:175` ("A result without harness disclosure is not a
result"), `:256` (publish the falsification: the point where the delta vanishes or inverts),
`docs/adr/ADR-267-sota-validation-protocol.md:271` (SOTA claim rules: beat top-3, signed manifest, config,
hardware; an invalid example is spelled out). ADR-277, ADR-267.

**What it would change in 2brain.** `docs/` and `tools/cards`. The cards already refuse to invent a value and
`render.py --check` runs in CI. Add to each card: corpus size, engine build, query-class size and a 95% interval;
add a `docs/claims.md` rule that a ranking number appears only with its class and interval; and keep a short list of
retired metrics with the reason. The shipped numbers (literal 0.714, topical 0.660, durable 0.538) are exactly the
kind of small-sample estimates this rule protects.

**Cost.** S. Docs plus a field in `metrics.json` and `render.py`; no engine change.

**Risk.** A 26-pair class may not be able to support any interval narrow enough to distinguish two variants; the
honest statement becomes "not distinguishable", which is less useful for a README card and more useful for the next
decision. A CI check that fails on a missing interval can block a release.

**First experiment.** Recompute the three shipped numbers with bootstrap intervals over queries. If the literal vs
topical gap's interval contains zero, the card must say so. This measures existing numbers; it adds no feature.

---

## 14. Run a structural preview as a shadow execution with receipts, and state what it does not prove

**Mechanism.** A structural change is executed against a shadow copy through the same gate and step recorder as a
real mutation, each step emitting a receipt; the preview surface can block but never authorize, and its answer binds
only to the state it was provisioned with.

**Source.** `docs/adr/ADR-319-truss-pattern-shadow-execution-generated-capabilities.md:110` (the broker inspects and
blocks but "never gains promotion authority"; separation of powers),
`docs/adr/ADR-339-ruv-context-javascript-binding.md:118` ("a faithful, deterministic policy simulator"; handles are
not portable; a decision binds only to the scope table the host provisioned), `:182` (the README states what the
package is *not*, in its first paragraph). ADR-319, ADR-339.

**What it would change in 2brain.** `mutate.rs`, `serve.rs`, `tb_preview_mutation`. Today the preview computes the
file set, the policy verdict and the refusals (`preview_riders`) without executing the write. The increment is to
run the real write path in a temporary worktree with the same gate, record per-file before/after hashes, and return a
receipt that says "ran against state H; this does not authorize apply". `tb_rollback_preview` should consume the
same receipt. The 2brain analogue of the wasm module's non-portable handle is the preview answer: it is evidence
about one tree, not a permission.

**Cost.** M. A shadow worktree and a step recorder; ~500 lines; temp directories only, no persistent state.

**Risk.** Preview/apply drift: a shadow run is evidence only if it uses the same binary and configuration, and only
if apply re-checks the state hash. Two code paths through the write surface is the maintenance cost, and a preview
that disagrees with apply is worse than no preview.

**First experiment.** Run the three largest structural operations (a `rename_type` cascade, a directory move, a
multi-file edit) in preview and then apply them to a candidate copy; compare the per-file hashes in the receipt to
the applied hashes. Any difference names a preview defect and the step that diverged.

---

## 15. One writer, and read-only roles whose tool surface contains no writer

**Mechanism.** Concurrent agents are given context-gathering and review roles with no mutating tools in their
surface at all, while exactly one writer serializes mutations, because at equal budget parallel writers on
interdependent text produce locally valid but jointly incoherent edits.

**Source.** `docs/adr/ADR-275-rvagent-subagent-topology.md:27` ("One writer. Auxiliary intelligence around it.
Never parallel writes."), `:67` (a reviewer performs better with no shared context), `:118` (read-only gatherer:
+2.1 pp, −17.9% cost, −34.5% input tokens; an expensive model in that slot gave zero lift at 5.4× cost). ADR-275.

**What it would change in 2brain.** The tool surface (`tb-mcp-rs/src/lib.rs`) and `serve.rs`. The gate already has
three grants and one mutation slot; add a `reader` principal whose `tools/list` omits the 16 mutating verbs (rather
than offering them and denying), and a `reviewer` mode that reads a diff without the writer's session state. The
denial path stays — hiding a tool is not authorization.

**Cost.** S. A per-principal tool-list filter and a test that the mutating verbs are absent; ~150 lines.

**Risk.** A client that caches `tools/list` across a principal change sees a stale surface, and a reader that cannot
see the verbs cannot discover why a write failed. Hiding must be presentation only; the fail-closed gate remains
the enforcement.

**First experiment.** Count write attempts per principal in the audit log; a reader-class principal with zero
successful writes and some denials is safe to narrow. Then run the same 10 diffs through a reviewer with and
without the writer's context and compare findings — the ADR's claim is that the reviewer does better without it.

---

## 16. Approve a capability expansion separately from a content mutation

**Mechanism.** A change that widens what an agent may do (a new grant, a member, a write scope) takes its own
approval record, distinct from ordinary content approval, and the actor that proposed it cannot be the actor that
approves it.

**Source.** `docs/adr/ADR-315-governance-constitution-capability-expansion.md:58` (a separately-authorized approval
record, distinct from ordinary promotion), `:60` (proposer ≠ approver; invariant 7), `:128` (the capability-table
diff is queryable over a window). ADR-315.

**What it would change in 2brain.** `members.rs`, `serve.rs`, and the audit chain. Content and authority share one
write path today; a grant change should be a different mutation class with its own approval, its own audit record
(`grant.before`/`grant.after`), and a read verb that diffs the grant table over a window. If the grant table is a
file an agent can write, the separation is a convention unless the content verbs refuse that key by type.

**Cost.** S/M. A second approval path, an audit schema addition, and a read verb; ~250 lines.

**Risk.** Two approval paths double ceremony for a small team. The real failure is a grant change that arrives as a
normal note edit, so the first job is to prove that is currently possible, not to build the second path.

**First experiment.** Diff the grant table between two commits and check whether any content mutation could have
produced the change. Then attempt a grant change through each of the 16 mutating verbs and confirm all are refused
with a named reason. If one succeeds, that is the defect; if none does, this entry is only the audit query.

---

## 17. Let the gate say "defer", and make every decision replayable

**Mechanism.** The gate returns permit, defer or deny, where defer escalates a risky but not forbidden action with a
reason and a time-bounded single-use token, and every decision is a receipt that can be replayed deterministically
from the recorded inputs and state hash.

**Source.** `docs/adr/ADR-067-mcp-gate-permit-system.md:53` (three decisions), `:52` (time-bounded `PermitToken`),
`:58` (`replay_decision` with the same inputs and state snapshot), `:82` (single-use nonce and rate limit).
ADR-067.

**What it would change in 2brain.** The write gate in `serve.rs` and `wire.rs`. Today the gate is binary and
fail-closed. Add `defer` for mutations whose blast radius crosses a threshold (a `rename_type` touching more than N
notes or more than M members), carrying the preview hash as the thing to confirm; the token binds (principal, tool,
argument hash, state hash) and expires. The same record makes `replay_decision` possible: given the audit record, the
gate must return the same outcome.

**Cost.** S. A third outcome, a token, and a replay test; ~200 lines. No new state if receipts go to the audit chain.

**Risk.** Defer is a new state clients must handle; one that retries instead of honoring the token can turn one
mutation into two. The token must be single-use and the defer path must not weaken deny. A replay that depends on
ambient state (clock, environment) will not reproduce.

**First experiment.** Replay 100 recorded gate decisions from the audit log and check determinism. Then compute how
many of the last 50 mutations would have been deferred at a proposed blast-radius threshold; 0% or more than 20%
means the threshold is wrong.

---

## 18. Negotiate versions against a published matrix, and make "unsupported" a result

**Mechanism.** Client and server exchange a contract naming the exact build, schema and policy versions; a
combination absent from a published matrix is refused with a machine-readable code and the nearest supported
combination, and "no compatible path" is reported rather than silently degraded.

**Source.** `docs/adr/ADR-291-runtime-compatibility-version-negotiation.md:81` (published matrix; reject before
build), `:97` (independent rejection at load), `:135` (`Unsupported` is first-class; never promote above the built
profile), `:223` (a semantic version alone does not identify a build). ADR-291.

**What it would change in 2brain.** `wire.rs`, the adapter (`tb-mcp-rs`) and `.2brain/repo.yaml`. The adapter and
engine are separate processes; skew today surfaces as a failed call or a misread field. Add `wireSchemaVersion`,
an optional-features list and the engine build to the handshake, a small `compat` table in the repo, and a refusal
`Unsupported { needed, closest }`.

**Cost.** S/M. A version field, a matrix, and refusal paths; ~200 lines.

**Risk.** Over-strictness blocks an upgrade that would have worked. The matrix must be data, not code, and the
refusal must name the closest supported combination so an operator can act. Selection must never promote above what
was validated.

**First experiment.** Run the installed adapter against the previous release's engine and count the RPCs before the
first failure (or whether it silently misreads). After the handshake, the count should be one. This is a
compatibility measurement, not a feature demo.

---

## 19. Make an exported pack carry its schema, its provenance, and one state commitment

**Mechanism.** A portable artifact carries its own schema dictionary, file-level provenance and generation; its
accelerators are marked derived and rebuildable with the generation they cover; and manifest, content, schema and
provenance are bound into one commitment so no part can be replayed against a different artifact.

**Source.** `docs/adr/ADR-280-rvf-durable-self-contained-metadata.md:47` (`META_SEG` is authoritative, the index is
derived), `:146` (the index records the generation it covers; a mismatch makes it unusable),
`:155` (a scan that exceeds its budget returns a typed error, never a partial result),
`:284` (`artifact_state_commitment` over generation, Merkle roots, schema hash and parent). ADR-280.

**What it would change in 2brain.** `wire.rs` and the context pack. Give the pack a manifest with the repo commit,
every note hash, the type-schema hash, the engine version and a `derived` section naming the index generation. A
reader that finds a mismatched derived section rebuilds rather than trusts. One SHA-256 over a canonical
concatenation is the commitment. This is also the honest form of a "portable knowledge base": the notes are the
authority, the index never is.

**Cost.** M. Format work, a reader path, and a migration for existing packs; ~500 lines.

**Risk.** Format churn for a feature with few consumers. A commitment over a directory is sensitive to file
ordering, so canonical encoding is the work. The budget lesson transfers directly: a fallback scan that exceeds its
budget must error, not return a partial pack that looks complete.

**First experiment.** Build a pack, flip one byte in a note, and confirm the reader refuses and names the changed
component. Then rebuild the derived section from the notes alone and confirm the commitment matches.

---

## 20. Shard by scope when one store serves several authorities, with a fanout ceiling

**Mechanism.** Each exact scope (authority, tenant, member, path) is a physically separate index with its own
manifest; search resolves the scope first and touches only its shards under a fanout ceiling, and a shard whose
manifest does not hash to its name is refused.

**Source.** `docs/adr/ADR-334-scope-sharded-context-index.md:46` (invariants 1–3: no cross-namespace index call, a
structured path prefix rather than a text prefix, fanout validated before the first ANN call),
`:107` (manifest binding, fail closed on a missing/malformed/renamed shard), `:119` (filtering a shared result set
is rejected because it traverses unauthorized candidates; the same-uid adversary is conceded).
ADR-334, with ADR-268's per-vector mask as the in-index alternative.

**What it would change in 2brain.** `search.rs`, `repo.rs`, `serve.rs`. Today one held IR serves every member and
scope is a post-ranking filter. The cheap intermediate is one index per member with a merged top-k and a fanout
ceiling; physical sharding is the extreme. It becomes necessary when members multiply or when grants differ per
member, not at 1,032 notes in one vault.

**Cost.** L. N indexes, N memory budgets, startup proportional to the whole corpus, and a migration.
`crates/ruvector-context/src/shard.rs:28` is a working reference (build, load, manifest checks) with no dependents in
the tree.

**Risk.** Memory and startup scale with total content, not with the queried scope (the ADR's own non-goal);
availability is not isolated (a global scope ceiling); and a same-uid process can author a valid shard, so this is
isolation against honest confusion, not a hostile host.

**First experiment.** Measure the status quo before building anything: run 50 queries with `scope: own` on the
served daemon and count the `documents_visited` attributable to other members (the counters exist). If out-of-scope
documents are under 10% of visits, sharding does not pay. Build the per-member index only above that.

---

## What ruvector decided that 2brain should explicitly NOT adopt

These are decisions, not untested ideas. Each is a place where the corpus's own evidence argues against importing
the mechanism.

1. **A vector index or an embedding model on the serving path.** ADR-210 makes MiniLM the default embedding engine,
   and ADR-154/264/300 supply quantized and cluster indexes for it. The corpus's own evidence points the other way
   for this query shape: ADR-274 §4 cites CoREB (arXiv:2605.04615) showing that short keyword queries — "auth
   flow", "user service", exactly what agents emit — collapse essentially every embedding model to near-zero
   nDCG@10, and ADR-160/268 both assume dense retrieval as the substrate. 2brain's literal class exists because
   BM25 plus proximity wins on identifiers. Importing the index would add a model, a build step and a staleness
   surface to the one path the product deliberately keeps model-free. Revisit only with a query class that
   *requires* semantic matching and a measured win on it.
2. **LLM summarization or compaction on the read path.** ADR-252 proposed coherence-weighted agent-memory
   compaction; ADR-274 then reversed it on evidence (JetBrains: masking +2.6% solve rate at 52% lower cost;
   summarization extended trajectories 13–15% by destroying stopping signals). What transfers is "mask, keep a
   dereferenceable id, re-inject invariants verbatim", not "summarize". 2brain already returns spans and excerpts;
   adding a model to shorten them would put a nondeterministic, fabricating step on the serving path.
3. **A four-tier memory hierarchy with demand paging semantics.** ADR-136's hot/warm/dormant/cold tiers and ADR-288's
   immutable base plus delta chain are designed for megabytes of RAM and a hypervisor. 2brain's whole IR is ~72.6
   MiB and the corpus is about a thousand notes. The tiers add a combinatorial transition surface for a problem that
   does not exist here. The transferable parts are the *explicit promotion rule* and "one module owns transitions",
   not the tiers themselves.
4. **Recency or freshness as a global score multiplier.** ADR-211 adds exponential temporal decay, and ADR-251 adds
   an agentic clock; ADR-211's own caveat is that the half-life must be tuned per domain, and ADR-251's
   baseline-dominance gate is explicitly **unmet** (0 win / 1 tie / 1 loss against a fair windowed baseline, and it
   refused to control production). 2brain measured the freshness prior twice and rejected it for a structural
   reason: the durable class is built from descriptions and the phrase class from bodies, so a global multiplier
   moves documents across the boundary where adjacency tie-breaks. Adopt the *gate* (beat a fair baseline or do not
   ship), not the decay.
5. **Parallel writers, CoW fork or CRDT merge of note state.** ADR-275 rejects these for coding on production-scale
   negative evidence, and ADR-026's vector-native COW is a storage mechanism for a different substrate. 2brain is a
   git-backed tree with one saga per mutation; a mergeable note type would reintroduce semantic conflicts that git
   already handles at the file level, and the ADR's own finding is that a CRDT can merge two edits without making
   the result coherent.
6. **A detached proof and signing infrastructure.** ADR-194 defers `SignedGate` and says its receipts are
   tamper-evidence, not authorization; ADR-304 leaves receipt signing and root trust out of scope and concedes the
   engine is trusted at issuance; ADR-142's TEE attestation needs hardware. The useful part for 2brain is the hash
   chain and the receipt shape, which it already has. Key management, publisher identities and a Merkle accumulator
   are a large operational surface for a single-team vault. Revisit when several organizations share one store, not
   before.

## What the ADR corpus itself taught about how to write ADRs

The practice worth copying is **stating what would falsify the decision, before the number exists**. The corpus does
it unevenly rather than by template: exactly one ADR has a literal `Rejection Criteria` section, six mention
pre-registration, and the rest carry the equivalent in prose — ADR-205 froze its gate in
`docs/plans/bet4-ivf-pruning/PRE-REGISTRATION.md` and then recorded that its own control disproved the predicted
sign; ADR-202's addendum reports a benchmark-degeneracy artifact that flatters reuse and says so; ADR-251 leaves its
baseline-dominance gate **unmet** and declines to control production; ADR-337 states flatly that its module "has no
call site" and that its 0.06% overhead figure must never appear without the caveat that the investigators cost
nothing. What separates those from the other 300 is not a section heading but the order of events: the falsifier is
written first, and the negative result is left in. Four habits are worth borrowing into 2brain's process docs: a
**status line that distinguishes Proposed / Accepted / Implemented / NO-GO / superseded** (and can say "the contract
exists, the implementation is in another repo and blocked"); a **verification block before the decision** that says
what exists in-repo "checked, not assumed", whether the upstream artifact exists at all, and which name collisions
need disambiguation; a **denominator on every number** (the 20% monitoring figure must carry "of the inference
compute being monitored"; the 15–77% figure must carry "n = 2 pairs"); and a **claim rule** that retires a metric
whose noise band exceeds the effect instead of arguing about it. The numbering discipline is the least interesting
part and the most copyable: a generated index is the counter (`docs/adr/INDEX.md`), duplicate numbers are frozen and
cited by slug (`ADR-134 (witness-schema-log-format)`), and CI fails on a *new* collision — never renumber what has
already been cited.

## Appendix: coverage

**Full reads (40).** 047, 067, 124, 160, 194, 202, 205, 211, 256, 258, 267, 268, 270, 272 (diverse-beam), 272
(recall-bounded), 274, 275, 277, 280, 282, 288, 291, 300, 301, 304, 305, 307, 315, 316, 318, 319, 320, 329, 330, 331,
334, 335, 336, 337, 339.

**Classified only (277).** Titles, filename grammar, status line and the generated index; for roughly 20 of these
the Decision and Evidence sections were also read but the tail was not, and they are counted here rather than as
full reads.

**One-pass buckets (rule-assisted, then adjusted after reading).** Retrieval and indexing (~51), agent/MCP surface
(~47), storage, formats and provenance (~44), memory tiers and lifecycle (~22), governance and safety (~18),
measurement, benchmarking and process (~8), and the remainder (~127) in runtime/model or product/domain areas that
do not touch a git-backed note base (LLM inference, sparse attention, Hailo/NPU, ESP32, hypervisors, photon
computing, trading, dermatology, astrophysics, publishing and deployment). The boundary between "storage" and
"retrieval" is judgment, not fact; the counts are an aid to coverage, not a taxonomy claim.

**Stub ADRs named and skipped:** `ADR-089-cnn-browser-demo.md` (45 lines), `ADR-123-brain-cognitive-enrichment.md`
(52), `ADR-143-implement-missing-capabilities.md` (55), `ADR-271-metaharness-darwin-sona-self-improvement.md` (68).
They carry a direction, not a mechanism with evidence.

**Families read through the canonical member rather than every duplicate.** The five `ADR-272` files are five
different mechanisms under one frozen number (adaptive recall, recall-bounded, diverse beam, speculative search,
bounded RAG); only the two relevant here were read in full and are cited as `ADR-272 (diverse-beam)` and
`ADR-272 (recall-bounded)`. Similarly `ADR-134 (witness-schema-log-format)` and `ADR-256 (hybrid-sparse-dense)`
are cited by slug, per the corpus's own rule (`ADR-316`).
