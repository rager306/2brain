# What kutha-graph has that applies to a notes knowledge base — twenty mechanisms, ranked

Read on 2026-11-16: the 33 ADRs in `/root/kutha-graph/docs/ADR/`, the ten notes in `docs/architecture/`,
`README.md`, `STRATEGY.md`, and `AGENTS.md`. Everything below is a paraphrase of a stated decision, a
measured number, or a named test; nothing is invented. `Source` paths are relative to `/root/kutha-graph`.
Each `Source` line gives the path of its primary cell; a secondary cell named by id alone is in the same
directory under its own filename.

Kutha is research-stage. All 33 ADRs are `Proposed`; only a P0 spike is implemented. That matters for how to
read this note: what transfers is the *discipline* — named invariants, typed failures, receipts, cuts — and
occasionally a measured cost. It is not a shipped capability being recommended.

Ranking is by value per unit of risk: the defect a mechanism prevents, divided by the blast radius if it is
wrong. Measurement-only entries rank first because their blast radius is zero. Cost: **S** is under roughly
200 lines with no new state, **M** adds one verb or field set plus its tests, **L** means a new store, format,
or migration. Nothing here puts an LLM or a vector index on the serving path. Entry 14 is the only
LLM-adjacent mechanism, and its read-side check is a hash comparison, not a model call.

## What the corpus looks like, measured for this note

The live vault, frontmatter only, 2026-11-16:

| field | notes with it | what it holds |
|---|---:|---|
| `.md` files | 1,049 | 1,032 carry YAML frontmatter |
| `status` | 1,008 | 1,000 `current`, 5 `pending`, 3 one-offs (`active`, `acknowledged`, `aging`) |
| `freshness` | 784 | 777 in the four declared classes; 7 outside them (`aging` 3, `fast` 2, `durable` 2) |
| `confidence` | 605 | 582 `high`/`medium`/`low`; 23 free-text values |
| `provenance` | 867 | prose, e.g. "Extracted from … on 2026-02-22" |
| `source_url` | 880 | a source identity, not a source revision |
| `created` | 964 | one timestamp, role unstated |
| `generated_from` | 10 | a generator id (`arscontexta-1.6`), not a source hash |

Two facts follow from that table and drive several entries: the enum-valued fields are unvalidated and have
already drifted (7 of 784, 23 of 605), and no derived note records the *revision* of the source it consumed.

## The twenty, at a glance

| # | mechanism | kind | cost |
|---|---|---|---:|
| 1 | work per read is bounded, and tested at three sizes | methodology | S |
| 2 | a rebuilt index and a patched index answer identically | methodology | M |
| 3 | every read states its outcome and its completeness | pattern | M |
| 4 | two cuts, named: as-of-a-date and as-known-at-a-commit | idea | M |
| 5 | a derived structure carries the generation it was built from | pattern | S |
| 6 | one test and one kill mutation per invariant | methodology | M |
| 7 | diff two cuts as fact sets, not as text | algorithm | M |
| 8 | a historical read does not restore historical permission | pattern | S |
| 9 | one receipt per mutation, binding its interpretation | pattern | S |
| 10 | one mediated write path, checked as a census | pattern | M |
| 11 | governance lives in the tree and is retractable | idea | M |
| 12 | retire a note, do not overwrite it | pattern | M |
| 13 | a retry is not a second write | pattern | M |
| 14 | a derived note records the revision it consumed | idea | M |
| 15 | a cascade is sized before it runs | pattern | S |
| 16 | a handoff is a sealed capsule, and import re-validates | pattern | L |
| 17 | each clock has its own type | pattern | M |
| 18 | compacted history fails loudly | pattern | S |
| 19 | a read can carry a receipt of the evidence it returned | pattern | S |
| 20 | domain clocks are annotations, not new core axes | methodology | S |

Adjacent pairs that are deliberately separate, not duplicates: 3 and 18 (a result envelope versus one specific
failure it must express); 4 and 8 (reading a past cut versus being allowed to read it); 9 and 19 (a write
receipt versus a read receipt, a separation ADR-014 draws explicitly); 12 and 14 (an authored retirement
versus derived staleness); 17 and 20 (typed clocks versus the rule that a domain clock stays an annotation).

---

## 1. Work per read is bounded, and the bound is tested at three sizes — *methodology*

**Idea** — assert a visit bound (documents inspected, posting blocks decoded, nodes walked) on every read
primitive and test it at three corpus sizes, because latency on a thousand notes hides a linear scan that
stops hiding at a hundred thousand.

**Source** — ADR-041 §Evidence, oracle `extra_relatedto_horn_visits_constant_in_n` — `docs/ADR/ADR-041-csr-graphblas-hot-path.md`;
ADR-011 D011-2 (dense interned ids are what keep hop work constant) — `docs/ADR/ADR-011-lean-event-schema-lineage.md`;
ADR-040 Clarification 2026-09-30 (in-fold lookup indexes exist "to restore constant-time ingest") —
`docs/ADR/ADR-040-materialization-plugin-protocol.md`.

**What it would change in 2brain** — `search.rs` (posting decode, adjacency pass) and `backlinks.rs`,
`graph.rs` closure; `verb_metrics.rs` gains a per-verb count of documents and posting blocks visited; a probe
under `tools/` runs the same query against 256, 512 and 1,032 notes.

**Cost** — S. Counters and a probe; no schema, no new state, no migration. If `verb_metrics.rs` already
counts per verb, this is one more field per call site.

**Risk** — one increment per visited document in a path measured in microseconds, and a bound asserted so
tightly that a legitimately broad query fails its own test. Keep the counters behind the existing metrics
switch. A wrong bound that nobody re-reads becomes a test that pins an accident.

**First experiment** — for the five most-used read verbs, count documents visited for the same 20 queries
against three catalogue sizes (the live vault and copies truncated to 256 and 512 notes). A slope above about
1.2 in visited-documents-per-query is a defect; a flat slope makes the counter cheap insurance.

## 2. A rebuilt index and a patched index answer identically — *methodology*

**Idea** — for every derived structure, assert `answer(rebuild(corpus, cut)) == answer(apply_deltas(index,
tail), cut)` after edits, so the incrementally patched index cannot quietly disagree with one built from
scratch.

**Source** — ADR-040 Clarification 2026-09-13, which states that equality for exact query contracts —
`docs/ADR/ADR-040-materialization-plugin-protocol.md`; ADR-060 §Evidence 2026-10-05 (`SEM-replay-idempotence`,
`SEM-persist-roundtrip-random`, `SEM-replay-tamper-random`) — `docs/ADR/ADR-060-strict-replay.md`; ADR-012
D012-1 (snapshots are leases, not the source of truth) — `docs/ADR/ADR-012-snapshots-tiers-vacuum.md`.

**What it would change in 2brain** — `search.rs` (`SearchIndex` patch path), `incremental.rs`, `build.rs`;
plus a harness that snapshots a vault, applies K edits through `tb_edit_file`, and compares the served
index's top ten against a from-scratch build on the same tree.

**Cost** — M. A harness and a scratch-vault procedure; no production change unless the equality fails. A
failure is then a real fix in `incremental.rs`, which is the point.

**Risk** — a slow CI job (a full 1,032-note rebuild per case is seconds, so cap K), and a false pass if the
comparison uses paths instead of content hashes, since two notes can swap bodies. Compare (path, content
hash, rank).

**First experiment** — K = 50 random single-field edits on a copied vault; for the 60-query set and the 26
durable pairs, compare top ten before and after each edit against a rebuild. Report divergences per 1,000
query-edits. Zero is the target; each divergence is a defect with a reproducing edit attached.

## 3. Every read states its outcome and its completeness — *pattern*

**Idea** — give every read a result envelope that names the outcome (`Found` / `NoMatch` / `Unknown` /
`Unavailable`) and the completeness (exact or ranked; complete or truncated by `limit`), so an empty or short
list is never read as proof of absence.

**Source** — ADR-090 TR-06, typed non-success (`Unknown`, `Conflict`, `MissingAnchor`, `Incomplete`,
`Abstain`) — `docs/ADR/ADR-090-legal-reference-pack.md`; ADR-070 Clarification 2026-09-13, "an empty partial
result is not proof of absence" — `docs/ADR/ADR-070-cypher-temporal-as-of.md`; ADR-031 D031-1, a rejection is
a logged why-not — `docs/ADR/ADR-031-pack-scheduling-under-v.md`.

**What it would change in 2brain** — `serve.rs` read responses and `wire.rs`; `search.rs` supplies the
evidence for `NoMatch` (no query term occurs in the index after tokenization) and for truncation;
`tb_search`, `tb_grep`, `tb_references` first, then the remaining read verbs.

**Cost** — M. Additive fields on responses, one shared envelope type, and a per-verb decision about what
"complete" means. No migration; callers that ignore the field keep working.

**Risk** — an envelope that lies is worse than none. If `NoMatch` is inferred from a low score rather than
from term absence, it abstains on a rare-but-present term. If `complete: false` is never set, the field is
decoration. Abstention also changes agent behaviour: a query whose terms are merely misspelled must not become
indistinguishable from one the vault cannot answer.

**First experiment** — `tools/ranking/tasks.json` already holds three `absent` tasks and they score 0/3
(`docs/research/task-set.md`). Grow that to 20 absent tasks, measure current non-empty hit@1 (expected 20/20),
then measure the two candidate gates from that note (query-time stopword drop; per-term IDF floor) for false
abstention on the `exact` and `ru-cross` classes. Build the envelope only if the gate separates at least 18/20
absent while costing at most one rank on the answerable classes.

## 4. Two cuts, named: as-of-a-date and as-known-at-a-commit — *idea*

**Idea** — distinguish a valid-time read (what a note asserted about a date) from a knowledge-time read (what
the vault contained at a commit), make both explicit arguments, forbid one from silently standing in for the
other, and map knowledge time to a wall date through an anchor (a commit date) rather than a third axis.

**Source** — ADR-013 D013-1 and Clarifications 2026-10-01 / 2026-10-08 (`as_of(vt)` is `live_at(tt=MAX, vt)`;
`as_known_on(wall)` is transaction-time only and may show a fact not yet in force) —
`docs/ADR/ADR-013-bitemporal-facts-invalidation.md`; ADR-090 TR-02, no silent clock substitution —
`docs/ADR/ADR-090-legal-reference-pack.md`; ADR-012 Clarification 2026-09-30 (snapshot identity is (offset,
fingerprint), verified on open).

**What it would change in 2brain** — `tb_content`, `tb_references`, `tb_graph_health` gain
`as_known_on: <commit>`; `repo.rs` and `gitwriter.rs` resolve a path at a commit; `serve.rs` argument
plumbing. The real work is link resolution at that commit, not the argument.

**Cost** — M. One argument on three verbs, a git-backed resolver, a per-cut resolution mode. No migration and
no new store: git is the store.

**Risk** — resolving links at an old commit wants the old catalogue, so a naive implementation either builds
a second index per cut (slow, memory spike) or resolves against HEAD and returns links that did not exist
then — the exact lie the distinction exists to prevent. It also must not bypass present authorization
(entry 8).

**First experiment** — for 50 notes, compare `git show <commit>:<path>` with a prototype resolver at three
commits each. Two numbers: byte equality (should be 50/50) and how many historical link targets resolve only
at HEAD. If the second number is zero on this corpus, the distinction buys nothing here and the entry drops.

## 5. A derived structure carries the generation it was built from — *pattern*

**Idea** — give every lease an identity (corpus generation, type-schema digest, projection version) and make
a read that finds a stale identity rebuild or refuse, instead of answering from a structure built under a
different interpretation.

**Source** — ADR-040 Clarification 2026-09-13: lease identity binds branch/prefix, offset, cut, schema and
term interpretation, projection version; "query execution must reject an incompatible/stale lease or rebuild
it" — `docs/ADR/ADR-040-materialization-plugin-protocol.md`; ADR-021 D021-1 (lifecycle is records, not config;
which leases must rebuild on a dictionary change is an open question there) —
`docs/ADR/ADR-021-pack-plugin-lifecycle.md`; ADR-014 D014-4 (versions bound into receipts).

**What it would change in 2brain** — `search.rs` (`SearchIndex` gains `built_from`), `incremental.rs`
(generation bump on edit, rename, type change), `build.rs`; `engine.rs` owns the generation; the `serve.rs`
search path checks it before answering.

**Cost** — S. One digest and one comparison per read; no new state beyond a value already computable from the
catalogue.

**Risk** — hashing the whole catalogue on every read makes search slow, and a missing bump after a
`rename_type` cascade leaves the index stale in exactly the case the check exists for. Bump inside the
transaction that changes the catalogue; never poll.

**First experiment** — run 200 `tb_edit_file` mutations and read the search generation and the catalogue hash
immediately after each. Count windows where they disagree, and for each, whether the top ten for the 60-query
set differs. If the window is zero, this is cheap insurance; if it is nonzero, the milliseconds are the
measurement that justifies the check.

## 6. One test and one kill mutation per invariant — *methodology*

**Idea** — keep a machine-readable table mapping each invariant to a test name, a kill mutation and a status,
where `killed` is illegal without a recorded mutation and anything pinned-but-undecided is labelled
`characterization` in the test's own name.

**Source** — ADR-023 D023-2/D023-3/D023-5 — `docs/ADR/ADR-023-test-strategy-and-oracle-matrix.md`. The cell
states the rule ("Do not mark `killed` unless the test exists and a mutation is recorded") and records its own
first failure: Phase 86 relabelled rows that had been marked killed on the honour system as
`characterization`.

**What it would change in 2brain** — a new `docs/` note plus a YAML table and a small check script;
`tb-testkit` grows the mutation harness. No production code. It changes how existing tests are named and
claimed, not what they assert. Entries 1 and 2 are two rows this table would hold, which is why they rank
above it.

**Cost** — M. Twenty to forty rows is a day; the honest part is running one mutation per row.

**Risk** — theatre. Kutha's own record is the warning: a first pass marked rows killed that a later pass
demoted, because the mutation never actually failed. A stale row is worse than an absent one, so keep the table
small enough that someone re-reads it.

**First experiment** — pick 20 invariants already asserted in `tb-engine`/`tb-core` (hash guard, absence
precondition, saga rollback, link resolution, ignore scope, `expected_hash`). Apply one deliberate mutation to
each and record caught/not-caught. A 20/20 result makes the table a regression guard; anything below that is
the backlog the table would expose.

## 7. Diff two cuts as fact sets, not as text — *algorithm*

**Idea** — compute the difference between two resolved states as added, removed and changed edges, instances
and type claims, including rows that no longer exist, so a handoff or review sees what the graph lost rather
than only which files changed.

**Source** — ADR-061 §Evidence 2026-10-23: `diff_folds` / `GraphFold::diff` returns added, removed and changed
facts, keeps dead rows, and is explicitly not graph-edit distance and not cosine —
`docs/ADR/ADR-061-fork-and-diff.md`; ADR-060 requires both sides to be cuts of the same history.

**What it would change in 2brain** — a new read verb (`tb_graph_diff(from, to)`) over `graph.rs`,
`references`, `ir.rs`; `serve.rs` and `wire.rs`. It is not `git diff`: both sides need resolved links.

**Cost** — M. Two catalogue resolutions and a set diff; no new store and no migration. GED-style scoring is
out of scope by the ADR's own restriction.

**Risk** — identity by name. A renamed note becomes one removal plus one addition and a moved block two link
changes; read as "knowledge was lost", the diff will be wrong about renames. Resolution at two cuts also
carries entry 4's cost.

**First experiment** — for 30 consecutive commits, compute the edge-set diff and compare with
`git diff --stat`. Report edges changed per commit and, among them, the count that a text diff cannot
distinguish from a no-op (a rename rewriting twenty referrers is one move and twenty lines). If that count is
usually zero on this vault, text diff is sufficient and the verb is premature.

## 8. A historical read does not restore historical permission — *pattern*

**Idea** — check current authority to receive a result independently of the historical cut it is read at;
asking for the vault as it was on date D must not re-grant access revoked since.

**Source** — ADR-080 Clarification 2026-09-13: "Choosing AS OF before a revocation must not restore the
caller's present access" — `docs/ADR/ADR-080-abac-multi-tenant-slices.md`; ADR-051 Clarification 2026-10-01
(an action record binds resolved arguments, the admission decision and the policy hash) —
`docs/ADR/ADR-051-capability-security.md`.

**What it would change in 2brain** — `serve.rs` read path and `members.rs` grants; the order becomes: resolve
the caller's present grant, then the cut, then serve. Applies to `tb_content`/`tb_search` the moment entry 4
lands, and to any historical `tb_preview_mutation`.

**Cost** — S. One check ordering and no new state, as long as grants stay where they are. It becomes M if
entry 11 moves them into the tree.

**Risk** — the reverse error is also a failure: an authorized auditor asking what a member held then must be
allowed, under present authority. Collapsing the two questions yields either a leak or an unusable audit; the
distinction has to be in the verb, not in a comment.

**First experiment** — count read verbs that consult a member grant today. If the answer is zero, every read
is workspace-wide and there is nothing yet to protect. If it is nonzero, diff the grant set at the last 50
commits against today and count the notes that a historical cut would expose but the present grant hides.
That count is the size of the leak entry 4 would open.

## 9. One receipt per mutation, binding its interpretation — *pattern*

**Idea** — return and record a constant-size receipt per mutation: a digest over the pre-image, the
arguments, the result content hash, and the interpretation it was admitted under (type-schema digest, grant
and policy digest, engine version), so a later reader can tell not only what changed but under which rules.

**Source** — ADR-014 D014-1 (constant-size commitment; `digest` is a fixed `[u8; 32]`, independent of
cascade width) and D014-4 (a receipt may bind meta-prompt and dictionary snapshot ids) —
`docs/ADR/ADR-014-cascade-budgets-quantum-receipts.md`; ADR-011 clarifications (event, claim, support and
delivery identities are distinct); ADR-014 Clarification 2026-09-13, "call success is not quantum completion".

**What it would change in 2brain** — `serve.rs` mutation result and `gitwriter.rs` trailers (a `Mutation-Id`
trailer already exists); add `schema_digest`, `grant_digest`, `receipt`; a `tb_receipt(mutation_id)` read that
recomputes from git.

**Cost** — S. Three hashes and a trailer; the read side is recomputation, no new store. The inputs must be
durable, and git is.

**Risk** — overclaiming. A digest over the working tree proves nothing about tampering unless it covers a
durable pre-image; Kutha's note says its receipt certifies "that the engine quantum existed", not
how-provenance, and its `event_ids` vector still leaks width. Write down in the field's own doc comment what
it does not prove: no external anchor, and anyone who can rewrite the tree can rewrite its hashes.

**First experiment** — take 100 recent commits and recompute the receipt from the parent tree, the arguments
recovered from the audit row, and the committed tree; count mismatches. A mismatch means the audit row does
not carry enough to recompute and fields must be added before the digest. Also measure bytes added per call;
anything above about 200 is too much on the write path.

## 10. One mediated write path, checked as a census — *pattern*

**Idea** — every mutating verb enters through the same gate and leaves the same record shape (precondition,
saga, commit, receipt), and a conformance test enumerates the catalog and fails any verb whose record differs,
so a new verb cannot quietly acquire a bypass.

**Source** — ADR-010 Clarification 2026-10-01: "every record kind … enters the log through one path: propose,
admit, commit, receipt. A kind-specific early return or a side append without admission is a spike shortcut,
not the contract" — `docs/ADR/ADR-010-event-log-runtime-quantum.md`; ADR-021 D021-1 (lifecycle events are
records, not config).

**What it would change in 2brain** — `serve.rs` (24 request variants in the engine enum against the 16 tools
the adapter advertises), `mutate.rs`, `gitwriter.rs`; a test under `tb-mcp-rs/tests/` that drives each verb
once and asserts one commit, one `Mutation-Id` trailer, and one audit row naming the tool.

**Cost** — M. The test is the work; production changes only where a verb diverges. `set_ignores` is a
CONFIG-sort write with a deliberately relaxed dangling contract and must be a *declared* exception, not an
unlisted one.

**Risk** — a census that pins today's payloads freezes a mistake. Assert the contract (one commit, one
receipt, tool named), never the exact row shape, or every legitimate payload change edits the test.

**First experiment** — the census itself. For each of the 24 engine variants and the 16 advertised tools,
record: audit row present, tool named, `Mutation-Id` trailer, git commit, rollback path. The disagreements
(including the 24-versus-16 count) are the first output; if all 16 pass, the entry is a regression guard only.

## 11. Governance lives in the tree and is retractable — *idea*

**Idea** — the rules that decide what may be written (member grants, scope, roles) are records in the served
tree with a prior-read hash, admitted through the same gate as content, so a rule change is a diff, an audit
row, and a thing that can be pointed at a date instead of a process flag on the adapter.

**Source** — ADR-050 §Evidence 2026-10-03 and Clarification 2026-10-01: a YAML file only seeds a *new* log,
`admit` consults live `kutha:allows` facts, and "a relation that arrived by genesis can be retracted like any
other fact" — `docs/ADR/ADR-050-meta-prompt-dictionaries.md`; `docs/architecture/semantic-gap-review.md` F10
(a file as root of trust for admission was a defect); ADR-080 D080-5 (absent policy must not fail open for
regulated packs).

**What it would change in 2brain** — `members.rs` plus a `_meta` note or frontmatter block, the adapter's
`policy.members`, the `serve.rs` gate; a `tb_describe_policy` read that reports the effective rule and the
path and hash it came from; changes go through `preview_mutation` then `tb_edit_file`.

**Cost** — M. A new record shape, a bootstrap rule (who may create the first grant), and a migration from
adapter config. No new store.

**Risk** — recursion and lockout: a bad edit can remove the only authority that could repair it, so keep one
engine-level, non-retractable escape (local filesystem access) and state it. Reading the rule on every
mutation also costs a parse; cache by content hash.

**First experiment** — run the existing `real_engine` workspace tests with grants read from a note instead of
adapter config and count divergences (expected zero). Then time 200 mutations under each source; above about
1 ms per mutation the rule needs the same cached-parse treatment as the catalogue.

## 12. Retire a note, do not overwrite it — *pattern*

**Idea** — invalidation is a typed record that names what is retired and why; the retired version stays
readable at its cut; the default read excludes it without deleting it; no edit may silently stand in for
"this was superseded".

**Source** — ADR-013 D013-2 and D013-3: "Invalidation is typed; losers remain", "Silent overwrite is
forbidden", supersession is deterministic and equal endpoints alone do not establish it, and embedding
similarity cannot own stale-fact — `docs/ADR/ADR-013-bitemporal-facts-invalidation.md`; ADR-013 Clarification
2026-10-08 (Phase 79: retracting an already-dead target fails closed rather than no-oping).

**What it would change in 2brain** — frontmatter (`supersedes` / `invalidated_by` plus a reason), the
`search.rs` default filter, the `graph.rs`/`references` default view, and a `retire` verb or guarded field
write in `mutate.rs`; `tb_graph_health` reports retired notes that are still linked.

**Cost** — M. The field and verb are small; the cost is the read-default migration, because every ranking
measurement must be re-baselined under the filter and the 1,000 `current` notes must not change rank.

**Risk** — hiding a note still cited by a live note creates dangling evidence; hiding too little leaves the
vault noisier than today. The corpus also has almost no supersession signal (5 `pending`, 1 `aging`), so the
mechanism must be useful while empty — which is an argument for the field and the verb, not for a filter
applied by default on day one.

**First experiment** — from git history, count notes whose body was replaced wholesale in one commit (say
over 80% of lines) or deleted while referrers existed; for each, check whether a predecessor is still
reachable from search. Unreachable-predecessors per 100 such edits is the defect size. Near zero means git
already preserves enough and the entry drops to documentation.

## 13. A retry is not a second write — *pattern*

**Idea** — give each mutation an idempotency key that survives the client, and make the engine return the
original result for a repeated key instead of applying the write again; the durable record of the key is the
commit itself (a trailer), so the guarantee does not depend on an in-memory cache.

**Source** — ADR-011 Clarification 2026-09-13: distinguish event id, claim/version id, support id and
delivery/idempotency key, because "a retry of one delivery is not a second independent support" —
`docs/ADR/ADR-011-lean-event-schema-lineage.md`; ADR-050 Clarification 2026-10-10 (delivery keys are
namespaced by relation, so a policy pin and an admission pin with the same string do not collide); ADR-014
Clarification 2026-09-13 (resume "must prevent duplicate delivery/effects").

**What it would change in 2brain** — `serve.rs` mutation args, `mutate.rs`, a `gitwriter.rs` trailer, and a
rebuildable lookup over `git log --grep`; the repeated key returns the first result.

**Cost** — M. The lookup is a scan unless indexed; a cache in `.2brain` is a lease, not the truth, and the
design must survive its loss by rebuilding from trailers.

**Risk** — a key that collides across operations suppresses a legitimate second write (the namespacing fix
exists because of exactly that); a key that is not durable makes the guarantee a lie after a restart. Keep it
optional and per-call, never inferred from content.

**First experiment** — across the last 1,000 audit rows, count mutations that repeat an earlier row's (verb,
path set, content hash) within ten minutes: retry-shaped duplicates. Then kill the adapter mid-write 20 times
and count duplicate effects on retry. If both are zero on this corpus, the mechanism is insurance rather than
a fix, and it should wait.

## 14. A derived note records the revision it consumed — *idea*

**Idea** — an extract or summary stores a content hash of the source revision and the generator identity that
produced it; a deterministic read then reports the derivation as current, stale or unavailable, and a stale
derivation stays readable as history but is not presented as current.

**Source** — ADR-052 Clarification 2026-09-13: an observed source, an extracted proposition and an admitted
claim are distinct; "Source correction, retraction, or supersession makes affected summaries, embeddings,
cached answers, and action justifications ineligible for current use until re-evaluated", and "replaying
cached output does not renew its admission" — `docs/ADR/ADR-052-genai-enrichment-pack.md`; ADR-013
ASOF-ELIG-01 (a derived row can be live and still ineligible).

**What it would change in 2brain** — frontmatter on derived notes (`source_path`, `source_hash`, `generator`,
`generated_at`), a read (`tb_derived_status`) or a diagnostics pass, and the `inbox/` ingest. `search.rs` is
untouched except for an optional eligibility filter. **No LLM is on the read path**: the check is hash
equality plus presence.

**Cost** — M. A schema addition and a backfill story for 867 notes that carry `provenance` as prose and 10
that carry `generated_from` as a generator id. No content migration.

**Risk** — a hash that cannot be verified because the source note moved gives false confidence; eligibility
that flips on every source edit buries derived notes under "stale" until re-extraction catches up. The read
must distinguish "stale but readable" from "unavailable".

**First experiment** — classify the 867 `provenance` values: those naming a vault path, those naming an
external source (URL or paper), those naming a generator. For the in-vault ones, count how many sources have a
later commit than the note that cites them (`git log -1 --format=%ct`). That count over 867 is the backlog the
mechanism would make measurable.

## 15. A cascade is sized before it runs — *pattern*

**Idea** — a structural refactor declares its write set (which `preview_mutation` already returns) and refuses
above a caller-supplied ceiling, with the count visible in the preview, so a rename that would touch 800 files
cannot begin by accident.

**Source** — ADR-014 D014-2 and its hard separations ("Budget abort ≠ Silent truncate of truth") and
Clarification 2026-09-13 (an outcome must distinguish completed, budget-stopped, failed, and bind consumed
budget) — `docs/ADR/ADR-014-cascade-budgets-quantum-receipts.md`; ADR-031 D031-1 (run/queue/reject, with
reject a logged why-not).

**What it would change in 2brain** — `serve.rs` preview and mutation args, `mutate.rs`, and the `affected`
list the preview already builds from the mutation's own planning code; a refusal carries `refused:budget` in
the audit row.

**Cost** — S. The count exists; the ceiling is one comparison. The only design work is where the ceiling comes
from (caller argument, per-member policy note, or default).

**Risk** — a ceiling chosen from today's distribution blocks a legitimate mass migration, and a ceiling one
verb ignores (say `delete_dir`) makes the guard theatre. Because the saga rolls back, refusing before the
write is strictly better than a partial stop mid-write; copy the *rule*, not Kutha's partial-quantum
bookkeeping.

**First experiment** — the distribution of `affected` sizes. Over the last 100 refactor mutations in the audit
ledger, report p50/p90/p99/max. Below a p99 of about 50 a ceiling adds nothing; a max in the hundreds sets the
ceiling at p99 and the suite re-runs.

## 16. A handoff is a sealed capsule, and import re-validates — *pattern*

**Idea** — export a subtree (a family, a member) with a manifest listing every file's hash and its outbound
links, and import it as ordinary mutations through the gate, re-resolving links and re-validating types rather
than trusting the capsule.

**Source** — ADR-091 D091-1 and D091-2: a capsule exports a fold slice, "Import is ingest-as-events (or attach
as a read-only lease), never 'the capsule is now SoT'", and its witness may embed the receipt, not replace it
— `docs/ADR/ADR-091-rvf-portable-capsules.md`; ADR-090 D090-8 (import "must re-validate clocks and evidence
classes; never trust capsule as silent promotion").

**What it would change in 2brain** — an export/import verb pair (or one verb with a direction), manifest types
in `ir.rs`, `gitwriter.rs`; import routes through `tb_batch` semantics so the gate, hashes and commits apply.

**Cost** — L. Two verbs, a collision policy (same path, different content), and link rewriting, plus a
cross-workspace path mapping if a capsule must carry different member roots.

**Risk** — a capsule that cites notes it did not carry produces dangling links, so the export must close over
its link frontier or declare it open; importing over existing paths silently overwrites unless the absence
precondition is used. A sealed capsule also invites the belief that its hashes prove provenance, when they
prove internal consistency only.

**First experiment** — export the 36-note `f/patterns` family with a manifest, import into a scratch member,
and count path collisions (expect zero), links left dangling (the frontier size), and files written. A
frontier larger than the family means the capsule needs a declared-open flag, and the number is the evidence.

## 17. Each clock has its own type — *pattern*

**Idea** — valid time, commit (knowledge) time and wall time are distinct types that cannot be compared or
passed without an explicit conversion, so "as of a date" cannot accidentally be answered with the wrong one.

**Source** — ADR-013 Clarification 2026-10-01: "each clock is a distinct type, not an alias of one integer …
a stamp carries its clock in its type" — `docs/ADR/ADR-013-bitemporal-facts-invalidation.md`; ADR-013
Clarification 2026-10-08 (G-C09: the public `as_of` takes `ValidTime`, `as_known_on` takes `WallTime`, and
wrong-role clocks fail at compile time via the `ui_clock_facade` trybuild test); ADR-013 Verdict 2026-10-06
(the transaction-time-to-wall mapping is an anchor stream, not a third axis).

**What it would change in 2brain** — `tb-core` (`typedef.rs`, `instance.rs`, value kinds), `tb-parser`
frontmatter (`yaml.rs`), `tb-engine` date handling: newtypes with conversions at exactly the boundaries that
need them.

**Cost** — M. The types are small; the churn is at every parse and render site and in the wire shape (an
integer on the wire, typed after deserialization).

**Risk** — serde changes are wire-compatibility changes, and a clock that renders differently breaks golden
captures. Do the newtypes internally first with the wire unchanged, and add the compile-fail test in the same
commit as the first public `as_of`.

**First experiment** — a census. Count the places where a date crosses a layer as a bare integer or string
(`tb-core` values, parser, engine, wire) and classify each by the clock it means. Under about 20 sites the
change is a day; over about 60 the churn outweighs the protection and the entry should wait for entry 4.

## 18. Compacted history fails loudly — *pattern*

**Idea** — declare what history is retained, and when a historical read asks for a cut that compaction
removed, answer `Unavailable` explicitly instead of falling back to current content or an empty result.

**Source** — ADR-012 D012-4: vacuum is policy GC of the source of truth, a legal hold pins ranges, and "after
vacuum, AS-OF of a removed state is a defined failure, not a silent hole" —
`docs/ADR/ADR-012-snapshots-tiers-vacuum.md`; ADR-061 §Evidence 2026-10-23 (`fork_at` fails closed with
`ForkCutUnavailable` on a compacted cut); ADR-060 (replay divergence is a test failure, not best effort).

**What it would change in 2brain** — `repo.rs` and `gitwriter.rs` historical resolution, the entry 3 result
envelope, and a retention note in the workspace; `tb_graph_health` reports the oldest readable commit.

**Cost** — S once entry 4 exists: one failure branch and one documented policy, no new state.

**Risk** — users meet an error where they expected history, so the failure must name the cut and the policy.
Force-push or rebase on a served member is the real threat; a read that silently returns HEAD after a rewrite
is worse than an error, because it is indistinguishable from a successful historical read.

**First experiment** — on a copy of the vault, record the oldest commit any `provenance` string names, run
`git gc --prune=now`, then ask for a pre-GC cut through the entry 4 prototype and record what happens today
(current content, empty, or error). "Current content, silently" makes this a defect fix rather than a feature.

## 19. A read can carry a receipt of the evidence it returned — *pattern*

**Idea** — return, beside a read, a digest over (query, cut, result paths, content hashes) so an agent's later
claim can cite exactly what it was shown, and a reviewer can recompute it from git without trusting a log.

**Source** — ADR-014 D014-5: a read provenance receipt is "a tamper-evident read commitment: the evidence
subset handed to an agent for one query", and "read receipts must not become a second write SoT" —
`docs/ADR/ADR-014-cascade-budgets-quantum-receipts.md`. ADR-014's hard separations keep the write receipt and
the read receipt as different nouns, which is why entry 9 and this one do not collide.

**What it would change in 2brain** — `serve.rs` read result (one field) and `wire.rs`, computed from data the
read already has; no storage.

**Cost** — S. Hashing paths plus hashes already known; the cost is bytes in the payload and a few
microseconds.

**Risk** — it certifies only what the engine says it returned: an agent that truncates text client-side still
cites a receipt for the full content, and a receipt without the result paths cannot be recomputed. On a
mutable tree a receipt over uncommitted bytes is stale immediately — compute over content hashes and say
which.

**First experiment** — compute the digest for 1,000 searches on the live vault; measure bytes added (target
under 5% of payload) and recomputability after 20 new commits (should be 100%, since the hashes are
content-addressed). If it cannot be recomputed without the result list, the field must carry the paths or it
is not evidence.

## 20. Domain clocks are annotations, not new core axes — *methodology*

**Idea** — when a domain needs a fourth or fifth time (application end, publication, decision date), record it
as a typed stamp on a domain object or note field, and refuse both to widen the core's two axes and to
reinterpret an existing one as the new clock.

**Source** — ADR-013 Verdict 2026-10-06: C01 (a sunset clause) is a pack stamp, and "Forbidden: fifth axis
(D013-1), Horn body, treating `valid_to` as application-end" — `docs/ADR/ADR-013-bitemporal-facts-invalidation.md`;
ADR-013 §Evidence 2026-10-02 (the two axes plus annotations are *not* sufficient for transitional rules: the
gap is stated, not papered over); ADR-090 TR-01 (five legal clocks map as typed stamps, not peer axes).

**What it would change in 2brain** — no code. A schema rule and a review check on `typedef.rs` and
frontmatter: every new date field names its clock, its scale, and whether it is a projection or a source
field. The value kinds stay as they are.

**Cost** — S. One review rule and a note in the schema docs; the cost is refusing requests, not writing code.

**Risk** — under-specification is the failure this prevents: a `valid_from`-shaped field appears in
frontmatter, is later read as authoritative, and nobody notices it is a projection of an edit. Applied too
strictly, the rule defers a real need until someone invents a worse field outside the type system, which is
what `provenance` (867 notes, prose) already is.

**First experiment** — inventory the date and date-adjacent fields in the vault: `created` (964),
`source_url` (880), `provenance` (867, prose containing dates), `freshness` (784), `confidence` (605,
including 23 free-text values), `applies_to` (103, and it is a project scope, not a time). Classify each as
knowledge time, valid time, or not a clock, and count the ambiguous ones. Every ambiguous field is a clock
being smuggled in; the count decides whether the schema needs a third named axis or only a review rule.

---

## What 2brain should not adopt, and why

**The event log as source of truth (ADR-010 D010-1).** Git history is already an ordered, auditable,
replayable log of every note, and 2brain's own rule is that any index which is not derived-and-discardable
becomes a second source of truth (`docs/research/graph-algorithms.md`). A second append-only log beside git
creates two histories to reconcile and a new place for the truth to differ. Take the *guarantees* — cuts,
receipts, replay equivalence — and build them on git, as entries 4, 9 and 18 do. The mechanism that does not
transfer is the log itself.

**HNSW or any vector index on the serving path (ADR-042).** 2brain keeps embeddings out of serving on
purpose, and the ADR agrees with the reason: vectors are an access method with a declared quality contract,
never truth (D042-1), and ADR-013 states that cosine cannot own stale-fact. On 1,032 notes BM25 over body and
description answers in microseconds; a vector index would add a build/release lease whose correctness
argument is numeric and whose failure mode is a plausible wrong neighbour. What transfers is the fence, not
the index.

**Cypher or a GPML calculus as a query surface (ADR-070).** The calculus assumes typed nodes and edges with
uniqueness modes and path objectives. 2brain's graph is files, blocks and typed instances whose edges are
wikilinks with field provenance. The three questions agents ask — find, what cites this, what is nearby — are
already single verbs. A pattern language would need a schema the vault does not have, and the ADR's own first
stage shipped a deliberately narrow subset (fixed `*N` with `1 <= N <= 8`, no variable ranges, no intermediate
predicates), which is evidence about how much of the surface is actually needed.

**Multi-tenant ABAC with path rewrite and tenant slices (ADR-080 D080-1, D080-3).** One vault per workspace;
members are already trust boundaries and grants are checked at invocation; there is no IR to rewrite a policy
into, because there is no query compiler. A path-level enforcement point would be a second query engine with
no second tenant to justify it. What transfers is the ordering rule in entry 8 — present authority, then the
cut — which costs one check, not a policy compiler.

**A reactive cascade that runs behaviors until idle (ADR-010 D010-2, ADR-014).** 2brain's diagnostics are
advisory by contract and one mutation is one saga with a rollback. A cascade makes the end state of a write
depend on rule order and on other notes' reactions, which is precisely the nondeterminism the write gate
exists to exclude, and it would need budgets, receipts and a cascade-depth bound before it was safe. Backlinks
and closure are already folds; nothing in the vault needs a behavior.

## What was read, and what was not

All 33 ADR files were read in full, plus the ten notes in `docs/architecture/`, `README.md`, `STRATEGY.md` and
`AGENTS.md`. None was skipped before reading and none is an empty stub; three are thin and contributed no
entry:

- **ADR-062** (`docs/ADR/ADR-062-regimes-gated-loop.md`) — an optional, default-off cell whose stated purpose
  is to close a title ("Title is closed; agents will not reopen it as a missing paper"). It decides nothing
  transferable.
- **ADR-092** (`docs/ADR/ADR-092-naming-and-license.md`) — naming and licence. No mechanism.
- **ADR-022** (`docs/ADR/ADR-022-vertical-slice-module-conventions.md`) — slice-as-crate conventions with no
  implementing slice; ADR-094 records that the workspace is actually two crates. A crate layout does not map
  onto 2brain's workspace members, so nothing transfers.

Two further notes on honesty. First, every ADR is `Proposed`, and the ones cited as evidence are cited for a
named test or a measured number, not for a product claim: typed clocks and `as_known_on`, `replay_check` with
Horn execution replay, `commit::committed_prefix`, fact-set `diff_folds`, checkpoint receipts, the CSR lease
and fail-closed `fork_at` are in the P0 spike; Cypher, HNSW, ABAC, a legal pack and the six dictionaries are
not. Second, where an ADR describes an intended contract rather than a measurement, the entry says so in
`Risk` — the cascade-budget family (ADR-014, 030, 031) is the clearest case: it is an algebra of an intention,
and entry 15 deliberately takes only the cheap half (refuse above a declared size) and leaves the allocator
alone.
