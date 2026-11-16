# RuVector crates: twelve mechanisms a notes knowledge base can use

Measured 2026-11-16 against `/root/vendor-source/ruvector` (178 directories under `crates/`, 175 with a
`Cargo.toml`, 837 575 lines of `src/*.rs`) and against this repository as it stands today (1 036 notes,
1 185 graph nodes, ~4 606 edges, per `docs/research/graph-algorithms.md`).

Method: every `Cargo.toml` description was read to triage 175 crates by subject, then the code behind the
mechanism was read in the crates whose subject was search and ranking, index build and patch, caching,
snapshots, quantisation, graph traversal and centrality, MCP serving, memory tiers, provenance, and
measurement. READMEs were not used as evidence; where a crate's own docs state a measured outcome, that
outcome is quoted and attributed.

**What 2brain already has, so that no entry below re-proposes it.** Seven mechanisms that look like ports
already exist here, in two cases in a weaker form:

| Crate says | 2brain already does | where |
|---|---|---|
| Lucene-style positive IDF, `k1=1.2`, `b=0.75` | same formula, per field, `description` weighted 3.0 | `tb-engine/crates/tb-engine/src/search.rs:52-56`, `:1180-1184` |
| a `weak`-match flag below a rarity line | `best_idf` + `weak()`, `RARE_IDF = 2.0` | `search.rs:155-171` |
| a minimum-should-match floor with a rare-term escape | `min_should_match` + `RARE_IDF` escape, tested | `search.rs:58-63`, `serve.rs:2351-2359` |
| filter before ranking, not after truncation | `query()` returns every hit, caller filters, then pages | `search.rs:1274-1300`, `serve.rs:14590-14601` |
| SHA-256 audit hash chain with recomputable preimage | present, but fail-open and field-poor | `tb-mcp-rs/src/policy.rs:281-297`, `:325-385` |
| staged deletion/repair strategies | incremental patch + a parity oracle against a fresh build | `search.rs:1264-1272`, `:1309+`, `:2231` |
| cycle detection / hubs | SCC and degree-ranked hubs already served | `serve.rs:15545`, `:15927-15976` |

Each entry below is therefore a mechanism that is absent, or present in a weaker form, in 2brain. They are
ordered by value per unit of risk: the cheapest change that alters a decision comes first, the most work with
the most over-claim exposure last — not by how interesting the mechanism is.

| # | entry | cost | lands in |
|---|---|---|---|
| 1 | vault state hash | S | `serve.rs`, `ir.rs` |
| 2 | paired bootstrap + graded query set | S | `tools/ranking`, `docs/research` |
| 3 | RRF for the signals with no common unit | S | `search.rs` |
| 4 | closed filter expression before paging | S/M | `search.rs`, `SearchArgs` |
| 5 | fail-closed witnessing + versioned record + verifier | S/M | `tb-mcp-rs/src/policy.rs` |
| 6 | Beta quality + evidence-gated promotion | M | type system, tool surface |
| 7 | dependent demotion on revision | M | type system, `mutate.rs` |
| 8 | lineage arbitration | M | `search.rs`, tool surface |
| 9 | note-level importance counters | M | `serve.rs`, `search.rs` |
| 10 | personalised PageRank from the best hit | M | `search.rs` |
| 11 | Merkle retrieval receipt | M | tool surface |
| 12 | witnessed non-action | S | type system, `mutate.rs` |

---

## 1. A vault state hash: one value that answers "are we looking at the same tree?"

**Mechanism.** Every artifact carries a content hash and a revision id at write time, and the workspace
computes one SHA-256 over all *current* heads in sorted `artifact_id` order — each pair encoded as a
u64-LE id length, the id bytes, then the 32 hash bytes. One hash summarises the whole bound state, so two
readers can compare a single value instead of comparing 1 036 files.

**Source.** `crates/ruvector-staged-workspace/src/workspace.rs:273-284` (the fold),
`crates/ruvector-staged-workspace/src/artifact.rs:59-77` (`ArtifactRef`: identity alone is explicitly *not* a
version reference). ADR-318, status Proposed.

**What it would change in 2brain.** `serve.rs`: a `state` field on the `overview` read and on every
response's `_meta` beside `io.2brain/engineVersion` (`tb-mcp-rs/src/lib.rs:2785`). It reuses
`ContentHash` (`tb-engine/src/ir.rs:64-79`, FNV-1a-64 with pinned vectors) for the per-file hash, so no new
hash function is introduced. The fold is over `kb.catalog`'s ordered map, computed on rebuild and cached on
the version counter.

**Cost.** S — about 20 lines, one cached `ContentHash` on the held `KnowledgeBase`, no new state, no
migration.

**Risk.** FNV-1a-64 is not collision-resistant: fine for "same vault?" between cooperating agents, wrong as
a receipt root against an untrusted party. The ruvector ledger is explicit about the same limit
(`crates/ruvector-agent-memory/src/ops.rs:12-24`: keyless FNV-1a is "tamper-evident against accidental
corruption only"). If this value ever anchors a receipt, it must move to SHA-256. A second risk is cost per
request: fold on rebuild, never per read, or a 1 036-file pass lands in every call.

**First experiment.** Compute it twice on the live vault with no edits — equal. Edit one note — different.
Shuffle the walk order — equal. Then run two daemon instances over the same tree and compare the values;
a mismatch is a bug in the fold, not in the vault.

---

## 2. A paired bootstrap with three outcomes, over a graded query set

**Mechanism.** For two rankers evaluated on the same queries, take the per-query metric deltas, resample
them 2 000 times, and report the mean delta with a 95% interval. The decision is three-valued: `pass` when
the lower bound clears a minimum effect (default 0.005), `fail` when the upper bound is at or below zero,
otherwise `inconclusive`. The RNG is seeded by hashing the data itself, so the same inputs produce the same
decision — a promotion record can cite a reproducible verdict.

**Source.** `crates/ruvector-sota-bench/harness/src/statistics.ts:20-47` (48 lines, complete). The report
shape it feeds — one composite score, a hard claim rule, git sha and params recorded with the run — is
`crates/ruvector-sota-bench/src/lib.rs:142-170` and `crates/ruvector-sota-bench/src/report.rs:17-70`.
ADR-265/266/267.

**What it would change in 2brain.** A new `tools/ranking/eval` binary plus a judged query file, and
`docs/research` gains a results note per ranking change. It extends the existing binary tripwire
(`docs/research/self-retrieval-2026-11-16.md`: description → note at rank 1, 1 033 of 1 037) into a graded
metric (nDCG@10 or MRR over 2–4 relevance grades).

**Cost.** S — 48 lines of statistic, plus the labelling work, which is the real cost: 50–100 queries with
graded judgements.

**Risk.** Low power: at 50 queries most changes will be `inconclusive`, which is honest but will feel like
a non-answer. Label before seeing results, or the judgements encode the hypothesis. Do not import the
crate's own benchmark numbers: its hybrid runner fabricates tokens from vector components
(`crates/ruvector-sota-bench/src/runners/hybrid.rs:15-44`) and reports `recall_at_1`, `recall_at_10` and
`recall_at_100` as the same number (`:73-77`), so its "80.8% vs 13.9%" header cannot come from that code.
Take the *shape*, not the figures.

**First experiment.** Validate the harness before trusting it: baseline vs itself must return
`inconclusive` with mean delta 0; baseline vs a deliberately degraded ranker (drop the description field)
must return `fail`. Then run the real comparison.

---

## 3. Reciprocal rank fusion, for the signals that have no common unit

**Mechanism.** `RRF(d) = Σ_i 1/(60 + rank_i(d))` over several ranked lists. It uses rank only, so lists
whose scores are in different units or different shapes can be combined without calibration.

**Source.** `crates/ruvector-hybrid/src/fusion.rs:126-175`, constant at `:22-23`; the documented failure of
min-max score fusion when distributions differ in shape at `:29-35`; relative-score fusion as the third
alternative at `:181-244`.

**What it would change in 2brain.** `search.rs` only — and only for *new* lists. The existing score is
multiplicative and well-behaved: `bm25_sum × (1 + 0.6·matched/total) × (1 + 0.3·closeness)`
(`search.rs:950-990`), bounded at 2.08×. Those three factors share a unit (BM25 evidence), so fusing them by
rank would discard magnitude and likely regress. The signals that *lack* a shared unit are the ones the
crate exists for: link-graph rank (entry 10), importance rank (entry 9). Fuse those with the BM25 list by
RRF, leaving the BM25 core additive.

**Cost.** S — ~40 lines plus a way to carry a second ranked list through `SearchIndex::query`.

**Risk.** RRF ignores how much better rank 1 is than rank 2, so a query with a decisive top hit and a query
with a flat tail get the same treatment. The counterexample in the same repository shows the naive
alternative plainly: `crates/mcp-brain-server/src/graph.rs:286-293` blends `cosine × 0.6 + ppr × 0.4`, a
linear mix of two incomparable scores. Neither is safe without the measurement in entry 2.

**First experiment.** For 30 queries, build the BM25 list and a backlink-degree list; measure top-10
overlap. Above ~80% overlap, fusion cannot help and the entry is dead. Below that, run RRF against the
BM25-only baseline through entry 2's harness.

---

## 4. A closed filter expression, applied before the page is cut

**Mechanism.** A tagged-JSON predicate AST (`eq/ne/gt/gte/lt/lte/range/in/match/exists/and/or/not`) with a
per-type payload index: `BTreeMap<i64, Set<id>>` for range predicates, `HashMap<keyword, Set<id>>` for
equality. The AST is the closed contract; the index is an optimisation behind it.

**Source.** `crates/ruvector-filter/src/expression.rs:7-78` (the grammar), `crates/ruvector-filter/src/index.rs:10-28`
(index kinds and their structures). The reason a selective predicate must be evaluated *inside* the walk,
not after the top-k is cut, is stated at `crates/ruvector-acorn/src/search.rs:7-15`: post-filtering starves
the candidate set and returns fewer than k while matching items exist.

**What it would change in 2brain.** `search.rs` + `SearchArgs` (`serve.rs:2335-2365`, whose
`deny_unknown_fields` already sets the discipline). The correct insertion point is the one scope already
uses: `search_view` filters the full ranked list at `serve.rs:14594` and only then applies `skip/take` at
`:14600-14601`. A typed predicate belongs on the same line, so `total` stays truthful and paging stays
correct. `matched_terms`, `best_idf` and `weak` are then computed on the filtered hit, not on the
pre-filter set.

**Cost.** S/M — the grammar is ~80 lines; the frontmatter fields already exist as typed values, so the
predicate reads the held instance rather than parsing again. No new index is needed at 1 036 notes.

**Risk.** Document-frequency semantics: if the filter narrows the statistics a term's IDF is computed from,
scores silently shift per filter and are no longer comparable. Keep df corpus-wide and say so. A filter
matching nothing must be distinguishable from a query with no hits — two different facts, and an agent will
read them as one.

**First experiment.** Query `decision` with and without `type: decision`; assert every returned path is in
`tb_type_counts`'s decision set, and that a filter matching 40 notes returns `total: 40` with a full page
rather than a short one.

---

## 5. Fail-closed witnessing, a versioned transition record, and a verifier

**Mechanism.** Three separable pieces. (a) A hash chain whose every entry commits the previous commitment,
the payload hash and the sequence number, at 64 bytes per write, with verification that *re-derives* the
chain from genesis rather than scanning it. (b) A transition record that binds not just who did what but
which artifact version changed: prior hash, prior revision, new hash, new revision, actor, and an evidence
grade of `Recomputed | SignatureVerified | TrustedAssertion`. (c) A tool trio — act, fetch receipt, replay
the decision — so a past decision can be re-derived and its chain re-verified.

**Source.** `crates/ruvector-proof-gate/src/gate.rs:93-116` (chain shape and 64 B/write), `:128-161`
(re-derivation), `crates/ruvector-staged-workspace/src/anchor.rs:41-64` (evidence grade), `:98-139`
(canonical byte encoding and the domain-separated signing preimage `domain || 0x00 || bytes`),
`crates/mcp-gate/src/lib.rs:9-19` (the three MCP tools). ADR-134, ADR-312.

**What it would change in 2brain.** `tb-mcp-rs/src/policy.rs`. The chain exists (`:281-297`, `:325-385`,
SHA-256, `prev` + `seq` + a recomputable preimage) but the record carries only principal, tool, member and
outcome — so an audit line cannot be tied to a version of a file — and a failed append is logged to stderr
while the action proceeds (`:323-324`: "the decision itself has already been made"). Add `prior_hash`,
`new_hash`, `mutation_id`, and `evidence_grade`; make the append failure abort a mutation; add a `verify_audit`
read that walks the file and re-derives every link.

**Cost.** S/M — four fields, one read, one error path. No new state: the file is the state.

**Risk.** Fail-closed witnessing converts "disk full" into "vault unwritable". Bound it: fail closed only
when the mutation carries a member grant, or keep a bounded ring buffer and refuse mutations when it cannot
be drained. Sequence gaps need an explicit rule — the crate's own ledger documents that gate-then-witness
leaves benign gaps and that the diagnostic is the *direction* of the gap
(`crates/ruvector-agent-memory/src/ledger.rs:461-472`).

**First experiment.** Take an existing audit file. Verify clean → pass. Flip one hex digit → must fail at
that line. Delete a middle line → must fail at the next `prev`. Measure the verifier's runtime over 10 000
lines; if it is slower than the mutation itself, the check belongs offline.

---

## 6. A Beta quality posterior and an evidence-gated promotion

**Mechanism.** A page's quality is `Beta(α, β)`; `mean = α/(α+β)`, `observations = α+β−2`, an upvote adds
to α and a downvote to β. Promotion from Draft to Canonical requires all of: `mean ≥ 0.7`, `observations ≥ 5`,
at least 3 evidence links, and at least 2 distinct contributors among them. A delta of type
correction/extension/deprecation is refused without at least one evidence link; a new contributor cannot
create a page until their reputation and contribution count clear a floor.

**Source.** `crates/mcp-brain-server/src/types.rs:181-211` (`BetaParams`), `:750-828` (page status, delta
type, evidence link with `test_pass / build_success / metric_improval / peer_review`),
`crates/mcp-brain-server/src/store.rs:1285-1303` (the four-part promotion check),
`crates/mcp-brain-server/src/routes.rs:4609-4615` and `:4756-4760` (the reputation and evidence gates).
ADR-062.

**What it would change in 2brain.** The type system and the tool surface: a `status: draft|canonical|contested`
claim (2brain already splices mixins into frontmatter — `mutate.rs` / `ensure_mixin.rs`, folded in
`serve.rs:5682+`), two mutations (`tb_evidence`, `tb_vote`), a `tb_promote` that refuses with the *named*
failing criterion, and `search.rs` marking draft claims the way it marks `weak`.

**Cost.** M — new claims, three mutations, counters in frontmatter, one check in `mutate.rs`. No daemon
state.

**Risk.** Votes become a political surface between agents, and a Beta posterior counts *votes*, not
independent support: twenty cheap upvotes from one principal promote a wrong note. Mitigate by gating
promotion on evidence links (with distinct contributors), not on `observations`, and by capping votes per
principal. The crate itself uses votes for ranking and evidence for promotion — keep that split.

**First experiment.** Add the two claims with defaults to 50 notes. Assert `check_promotion` is false for
all 50 today, and that each refusal names the missing criterion. Then promote one note by hand with three
evidence links from two principals and confirm the state transition is the only change.

---

## 7. Dependent demotion: a revision invalidates what was built on it

**Mechanism.** Edges are `depends_on`; the ledger keeps reverse edges. Revising or rejecting an accepted
entry moves it to Rejected and then walks the reverse edges, demoting every transitively accepted dependent
to Pending, so no downstream conclusion stays accepted on a rejected premise. All of those state changes
are staged and applied as one batch, witnessed before any state is written; `replay_history` over the
transition log must reproduce the current state map exactly.

**Source.** `crates/ruvector-agent-memory/src/ledger.rs:548-585` (BFS over reverse edges, dedup, sorted for
determinism), `:428-431` (accept requires every dependency already accepted), `:651-685` (witness the whole
batch first, then apply), `:691-699` (`replay_history`, the invariant). ADR-307.

**What it would change in 2brain.** The type system plus `mutate.rs`: an explicit `derived_from` edge (not
`[[wiki-links]]` — a link is a reference, not a premise), a `state: accepted|pending|rejected` claim on the
notes that carry it, a `tb_revise` mutation, and `preview_mutation` reporting the cascade before it runs.

**Cost.** M — edge kind, state field, cascade with a cap, one new mutation, one preview product.

**Risk.** A hub note (the served `hubs` read ranks exactly these) can demote hundreds of notes in one edit.
Cap the cascade, require a dry run above the cap, and make the cap visible in the refusal. Reusing wiki-links
as the premise edge would make every reference a dependency and the cascade uncontrollable.

**First experiment.** Hand-add `derived_from` to 10 notes under one root. Revise the root. Assert the
cascade set equals the set `preview_mutation` reported, and that a second revise of an already-rejected root
is refused rather than re-cascading.

---

## 8. Lineage arbitration: N echoes of one source are one observation

**Mechanism.** Group retrieved memories by provenance roots (union-find over declared parents); each lineage
contributes only its single strongest member's `confidence × reliability × freshness`; the arbitrated score
is therefore always ≤ the naive sum, and the crate re-checks that downgrade-only invariant at runtime rather
than clamping. The reported diagnostic is the number of independent lineages, not the score delta. When the
lineage count is below the sufficiency threshold, the outcome names the over-represented roots so the caller
looks *away* from them.

**Source.** `crates/ruvector-agent-memory/src/arbitration.rs:8-50` (including the limitation),
`:72-81` (downgrade-only), `:378-455` (union-find and assembly), `:202-207` (`independent_lineages`).
ADR-330.

**What it would change in 2brain.** `search.rs` result annotation and the context-pack contract
(`docs/research/context-pack.md` already defines the fields): each fragment gains a lineage id, and the pack
gains "N fragments, M independent lineages". A `tb_arbitrate` read answers "is this claim supported by
independent sources?" over a set of paths.

**Cost.** M — a parent field, union-find, one annotation pass, one read.

**Risk.** The crate is blunt about its own scope (`:19-50`): correlation comes from *declared* parents only,
so an agent that copies content without citing mints a fresh lineage, and a score reduction under a
reliability factor reflects that factor, not detected correlation. It is a corroboration signal under
cooperative provenance, not an anti-collusion control. With no parent field populated at write time it
degenerates to a no-op that looks like a measurement.

**First experiment.** Hand-label 20 searches: how many *independent* notes support the top claim? Compare to
the lineage count. Also count how many notes carry the parent field at all; below ~30% the mechanism has
nothing to work on and the first deliverable is the field.

---

## 9. Note-level importance from the data 2brain already counts

**Mechanism.** `importance = α·recency + β·frequency + γ·coherence`, each term normalised to [0,1] across the
candidate set: recency from the last access, frequency from the access count, coherence from agreement with
the active context. Recency as an exponential half-life is `0.5^(age/half_life)`; the graph variant
substitutes normalised in-degree for coherence.

**Source.** `crates/ruvector-agent-memory/src/compaction.rs:7-9`, `:96-115` (defaults 0.25/0.35/0.40),
`:165-188` (normalise then combine), `crates/ruvector-agent-memory/src/memory.rs:40-43` (touch), the decay
factor at `crates/ruvector-temporal-coherence/src/decay.rs:40-59`, and the in-degree gate at
`crates/ruvector-temporal-coherence/src/graph.rs:5-7`, `:48-54`.

**What it would change in 2brain.** `serve.rs` and `search.rs`. 2brain counts *verbs*, not notes:
`verb_metrics.rs:20-31` records per-verb requests, documents visited and posting blocks decoded, and
`visit_scope` is deliberately scoped to a request. A per-path hit counter (a `DashMap<PathBuf, u64>` in the
daemon, or a counter in the on-disk state dir) would supply both terms; the third term has a natural
non-vector form here, the backlink in-degree `hub_ranking` already folds (`serve.rs:15949-15963`).

**Cost.** M — new persisted counters, a normalisation pass over candidates, one flag.

**Risk.** Popularity bias: often-read is not correct, and the vault already has better priors in `status`
and note type (`docs/research/graph-algorithms.md` proposes exactly those). Use importance as a tie-break
and a review-queue signal, never as a multiplier on canonical or decision notes.

**First experiment.** Instrument hits for 200 searches, then compare the top 20 by importance against the top
20 by `hubs`. Overlap above ~90% means the signal adds nothing and should not be built; below ~50% it is
independent enough to fuse (entry 3).

---

## 10. Personalised PageRank from the best-matching note

**Mechanism.** Anderson–Chung–Lang forward push: one seed vertex, an `estimate` vector and a `residual`
vector; a vertex whose residual exceeds `ε·degree` is pushed, absorbing α into the estimate and spreading
`(1−α)/degree` to neighbours. Work is `O(1/ε)`, independent of graph size. After convergence the code checks
`Σestimate + Σresidual ≈ Σseed`; a violation is an error, not a clamp. The result is used as a second ranked
list, not as a score to add.

**Source.** `crates/ruvector-solver/src/forward_push.rs:3-15`, `:45`, `:144-243`, with the mass invariant at
`:227-230` and the push rule at `:317-323`. The blend counterexample — cosine × 0.6 + PPR × 0.4, with the
seed chosen as the nearest vector — is `crates/mcp-brain-server/src/graph.rs:277-298`, `:326-357`.

**What it would change in 2brain.** `search.rs`: seed = the top BM25 hit's path (already computed), graph =
the resolved link graph the engine already holds (`link_graph` / backlinks), output = "related, not
matching" paths. `docs/research/graph-algorithms.md` already scopes PPR; this supplies the implementation
shape, the ε/α contract and the invariant to test.

**Cost.** M — ~120 lines for the push loop plus a CSR build over 4 606 edges, rebuilt on a version bump.

**Risk.** The graph has holes: `docs/retrieval-spike.md` records 140 unresolved slugs and 263 mentions, so a
walk inherits every resolution failure. PPR also prefers hubs, which is the same bias as the existing degree
ranking — if it only re-finds hubs it is not worth carrying. At 4 606 edges a full PageRank per rebuild is
also cheap enough that the sublinear claim buys nothing at this scale; the reason to take this algorithm is
its single-seed locality, not its asymptotics.

**First experiment.** For 20 queries, compare the top 10 by PPR-from-top-hit against the top 10 by backlink
degree. Then count how many PPR-promoted notes contain *no* query term in body or description — that count is
the mechanism's whole value, and if it is ~0 the entry is dead.

---

## 11. A retrieval receipt over the result set

**Mechanism.** For one query, build leaves that commit to the query hash, the index state root, and each
result's id, rank and score, then a binary Merkle tree over them. A verifier holding a receipt and one
result can check inclusion in `O(log k)` bytes. The variant table states the cost directly: per-result chain
verification is `O(idx)` bytes and `O(idx)` work, Merkle is `O(log k)`.

**Source.** `crates/ruvector-retrieval-receipt/src/receipt.rs:58-72` (leaf), `:187-291` (tree, proof,
verify), `crates/ruvector-retrieval-receipt/src/index.rs:90-96` (the index root), and the threat model at
`src/lib.rs:9-34` — quoted here because it is the point: the receipt detects post-issuance mutation of a
receipt/result pair; it does **not** protect against a dishonest query engine, and it does **not** prove
write-chain membership.

**What it would change in 2brain.** The tool surface: `tb_search` gains `receipt: true` and a new `tb_verify`
takes (receipt, hit) and answers yes/no. The index root is entry 1's vault state hash or a per-read scope
hash. `docs/` gains the guarantee statement verbatim — the description of the tool must say what it proves.

**Cost.** M — canonical encoding of a hit, the tree, a verifier, and the tool descriptions. ~150–200 lines.

**Risk.** Over-claiming is the main risk: an unsigned, engine-produced receipt proves only "this is what I was
handed", which is still useful between agents but must never be described as proof of correctness. The
crate's own Merkle pads odd levels by duplicating the last node and documents the known malleability
weakness (`receipt.rs:177-185`); it is safe here because the server owns the leaf set, but it would need
RFC 6962 bit-length prefixing if leaves ever became caller-supplied. Cost per query is `O(k)` hashes.

**First experiment.** Issue a receipt for one search, verify it in a second process using only the vault state
hash, then flip one score digit and confirm refusal. Record receipt bytes and build time at k=10; if the
receipt is larger than the result set it describes, it is not yet worth a field.

---

## 12. Witnessed non-action: record what was seen and deliberately not kept

**Mechanism.** The ledger has an `ignore` operation: it records that a statement was seen and deliberately not
retained, as a witnessed transition carrying actor and reason, and creates no entry. The decision becomes
auditable rather than invisible, and it costs one row.

**Source.** `crates/ruvector-agent-memory/src/ledger.rs:221-242` (the operation),
`crates/ruvector-agent-memory/src/ops.rs:36-49` (the five-op vocabulary, of which this is the only one that
produces no object). ADR-307.

**What it would change in 2brain.** `mutate.rs` and the type system: an `ignored` record type (or a
`proposal` note with `decision: declined`) storing the content hash of what was declined, the principal, the
reason and the date; a read that answers "has this been proposed before?" by hash. This is the one mechanism
here aimed squarely at several agents sharing one memory: the same idea arriving twice from two agents is the
normal case, not the exception.

**Cost.** S — one record type, one mutation, one lookup.

**Risk.** An ignore list that outlives its reason: a declined idea can become right. Store the reason and the
date, make it advisory in the answer ("declined 2026-11-03: no test"), and never a hard block. The crate's
`ignore` is a ledger transition with no index, so it is only queryable by replaying history — that is the
part to improve, not copy.

**First experiment.** Record three real non-actions from the current intake session, then ask the same
question twice and confirm the second answer names the first decision and its reason. If the reason is not
enough for a reader to re-judge, the field is wrong.

---

## What is in these crates that 2brain should not port, and why

**1. The LSM tiered index and its compaction (`ruvector-lsm-ann`).** The FullLsm variant flushes a memtable
into L1 segments and merges L1 into L2, and every query merges candidates from every tier and re-ranks
(`crates/ruvector-lsm-ann/src/lsm.rs:154-172`, `:246-263`). It buys write throughput against a large
corpus. 2brain rebuilds a 1 036-note index in one pass and patches a content edit in place at the id the path
already holds (`search.rs:1238-1252`, `:1264-1272`), with a parity oracle against a fresh build. Tiers would
add a second consistency surface to a component whose current guarantee is exactly "patched == rebuilt".
Port the *measurement* discipline (merge-then-re-rank is testable), not the tiers.

**2. The quantisation family (`ruvector-rabitq`, `ruvector-turboquant`, `ruvector-pq-search`,
`ruvector-temporal-tensor`).** These are vector-only, and every one of them exists to make an embedding index
smaller or faster. Taking any of them puts an embedding model and a vector index on the serving path, which
this product rejects deliberately. Say the cost plainly: a new crate, a model dependency, an index that must
be rebuilt when the model changes, and the end of the "no LLM, no embeddings" property that the write gate's
whole trust story rests on. If semantic search is ever wanted, it belongs as an *offline* derived artifact
with its own provenance, never as the serving path.

**3. The snapshot manager (`ruvector-snapshot`).** A snapshot is bincode plus a SHA-256 checksum plus
keep-N retention (`crates/ruvector-snapshot/src/snapshot.rs:7-39`, `manager.rs:23-46`). Git in this vault
already *is* the snapshot, content-addressed, with history and a restore path that every tool understands. A
second copy is a second source of truth and a second thing to keep consistent. The only portable idea is
"verify the checksum on restore", and git already refuses a corrupt object.

**4. The semantic query cache (`ruvector-query-cache`).** The interesting variant caches on cosine similarity
above a chosen threshold; the exact variant hashes the query and re-checks equality
(`crates/ruvector-query-cache/src/exact_cache.rs:12-22`, `:48-56`). There is nothing expensive here to cache —
a search over 1 036 notes measures 0.16–1.03 ms per query (`docs/research/graph-algorithms.md`) — and a
cache keyed on the query alone would serve a result set from before a mutation, which contradicts the
freshness gate that the mutation path is built
around. If a cache is ever added, the key must be (canonical query, engine version), and that is a correctness
requirement, not a tuning knob. `docs/research/pogocache-patterns.md` already assesses the eviction side.

**5. Entropy-adaptive beam width (`ruvector-entropy-ann`).** The crate is a documented negative result:
ADR-303 is "Closed — negative result", and the code header says the softmin entropy of retrieved distances
"tracks local density, not routing ambiguity, and has the wrong sign (hard queries show *lower* entropy than
easy ones)", so the scaled variant saturates to the same beam width for every query
(`crates/ruvector-entropy-ann/src/lib.rs:17-22`, `src/search.rs:193-196`, `:314-318`). Copying the mechanism
would import a signal whose own authors measured it as inert. It is worth citing in `docs/` as an example of
a principled-looking signal that measured nothing.

---

## What their code organisation taught

175 crates for one project is not a model to copy, but four habits are (counts below are from
`crates/*/Cargo.toml` and `crates/*/src/**/*.rs`, not from any README). **They separate a mechanism from the
thing that measures it, and make the measurement crate independent of the thing measured.** Every research
slice — `ruvector-hybrid`, `ruvector-entropy-ann`, `ruvector-hnsw-repair`, `ruvector-lsm-ann`,
`ruvector-acorn`, `ruvector-diverse-beam`, `ruvector-recall-bounded`, `ruvector-temporal-coherence` — depends
on at most a few utility crates (`rand`, `rand_distr`, `serde`, `thiserror`, sometimes `rayon`) and on **no
sibling ruvector crate**: `ruvector-hybrid` reimplements BM25 and a flat dense index rather than depending on
`ruvector-core`, because a slice that shared the index could not isolate the index as the variable. Each has
a 600–900 line `lib.rs` holding the mechanism, a ~300 line `src/bin/benchmark.rs`, and a 100–120 line
synthetic dataset generator. The aggregator (`ruvector-sota-bench`) is the only crate with the real dependencies and
the feature flags (`synthetic-only`, `real-datasets`, `all-indexes`, `full-sota`). That is why the negative
results are trustworthy: the slice cannot be contaminated by the thing it measures, and the measurement is
one crate with the flags to run a subset. **They keep the heavy path behind a feature.** `ruvector-agent-memory`
depends on `ruvector-proof-gate` only under a `proof-gate` feature, so the ledger can be built and tested
without the cryptographic dependency. **They put the ADR next to the slice, and they do not delete a
negative.** 322 ADRs sit beside 175 crates; ADR-303 is explicitly "Closed — negative result" and the crate
header repeats the numbers. **They state the scope of a guarantee before the guarantee.** The receipt crate's
threat model and the arbitration crate's limitation section are the two best examples; both say what the
mechanism does *not* prove, in the module a reader hits first.

The cost of that shape is visible too: three independent SHA-256 chain implementations
(`ruvector-proof-gate`, `ruvector-retrieval-receipt`, and 2brain's own `policy.rs`) and two `EvidenceGrade`
enums (`staged-workspace/src/anchor.rs:41`, `agent-memory/src/ops.rs:221`). At 175 crates, a cross-cutting
concept gets re-implemented per crate. The lesson for 2brain is the opposite of "more crates": keep
`tb-engine` whole, put mechanisms behind one module each, and let `tools/` hold the measurement binaries —
which is close to the shape the repository already has (`tools/probe`, `tools/ranking`, `tools/judgement`
beside `tb-engine`), with the missing piece being the CRATE convention that a mechanism's first test is a
number, not an assertion.
