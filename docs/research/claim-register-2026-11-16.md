# The claim register: typed claims bound to measured evidence (2026-11-16)

The predecessor system kept a claim map — one-line propositions "specific
enough to be wrong", each bound to the configuration default it justifies
(`arscontexta-design-2026-11-16.md`, §2). Its gap: the register carried no
status and no evidence, so a claim could not be checked, only quoted. The
vault already has the machinery to close that gap — typed frontmatter,
`[[wiki-links]]`, engine-validated edges — and was not using it. This pass
uses it: a `claim` type whose `evidence` field resolves to the note that
measured the number, and a `decision` extension whose `backed_by` field
resolves to the claims. Every write went through the engine as principal
`claims`; nothing was edited on disk.

## Relation to the earlier register document

The first version of this document (commit `ab23197`) described a **12-note** register. The vault has **13**: six
measurement notes, six claim notes, one decision. The miscount was in the prose, not in the vault — one claim
(`concise-surface-saves-67pct`) was left out of the list.

The notes are the same work, committed one per write, and this is the verifiable record:

```
69b7193  inbox/concise-tool-list-saves-39723-bytes-a-session.md            measurement
5727141  inbox/scoped-read-candidate-mask-decides-scope-before-scoring.md  measurement
8f8410e  inbox/plural-query-finds-0-hits-where-the-singular-finds-2.md     measurement
b9e6883  inbox/lineage-echoes-census-36-notes-carry-17-observations.md     measurement
f86a597  inbox/abstention-leave-one-out-no-rule-catches-more-than-0-of-8.md measurement
fa33131  inbox/graded-ndcg-at-5-replaced-a-binary-ndcg-at-10-that-mirrored-mrr.md measurement
6ce98e6  inbox/the-concise-surface-saves-67-percent-of-a-sessions-tool-bytes.md  claim
0679a4c  inbox/a-scoped-read-decides-candidates-before-scoring.md          claim
e6f50fc  inbox/a-plural-costs-a-query-without-the-word-form-lookup.md      claim
da39607  inbox/36-provenance-notes-collapse-to-17-observations.md          claim
473e72c  inbox/abstention-has-no-surface-signal-leave-one-out-catches-0-of-8.md claim
fca2d5b  inbox/ndcg-at-5-replaced-a-metric-that-could-not-disagree-with-mrr.md  claim
9f59787  inbox/the-concise-profile-is-the-default-tool-surface.md           decision
```

A previous draft of this section claimed the vault did not contain the register the earlier document described. That was
wrong, and it is the failure this project keeps correcting: **the claim was about the document, not about the vault**. The
vault held every note; the document had the count off by one. Verified by `git log` above and by
`tb_instances_of type_name=claim` returning six entries with `evidence` present.

## The types as written

`type/claim.type.yaml` (the type existed with an ingest-queue shape; the
edit is purely additive — nothing removed, nothing narrowed — so the five
live queue notes still validate):

```yaml
# A proposition specific enough to be wrong, bound to what backs it.
#
# Register reading: `evidence` names the measured number behind the claim (a
# [[wiki-link]] to the note that measured it, or a path into
# tools/cards/metrics.json with its key), `source` names the document the
# claim came from, and `confidence` is the base's high/medium/low. The queue
# fields (batch, file, current_phase, created) belong to the ingest-queue
# reading of the same type.
fields:
  description: String
  id: String
  status: [pending, acknowledged]
  target: String
  evidence?: String
  confidence?: String
  source?: String
  batch?: String
  file?: String
  current_phase?: String
  created?: String
```

`type/decision.type.yaml` (extension of the existing type; the house default
in `tb-defaults/project-memory` carries the same pair as `evidence?` /
`supersedes?`):

```yaml
# A choice that was made, with what it rests on: `backed_by` names the claim
# notes that carry the measured evidence, `supersedes` names the decisions
# this one replaces.
fields:
  description: String
  status: [current, aging, stale, superseded]
  topics: String[]
  source_url: String
  provenance: String
  backed_by?: String[]
  supersedes?: String[]
  confidence?: String
```

## The six claims

Each claim note is a prose-titled note in `inbox/`, `status: acknowledged`,
`confidence: high`, with `evidence` resolving to a measurement note written
in the same pass (none of the six numbers lives in
`tools/cards/metrics.json`; each measurement note names the source document
and the section that holds the number).

| claim (note title) | the number it carries | number's origin (file and key) | target it justifies |
|---|---|---|---|
| The concise surface saves 67 % of a session's tool bytes | 39 723 bytes ≈ 9 931 tokens, 67.3 % per session | `docs/research/tool-surface-tokens-2026-11-16.md` §2 byte table | the concise profile as the adapter's default listing |
| A scoped read decides candidates before scoring | post-ranking filter cost 9 % of a scoped read at 3 members, 28 % at 9; mask median 0.70× at 9 members (control 1.00×) | `docs/research/candidate-set-first-2026-11-16.md`, measurement table + verdict | the `DocMask` candidate mask in `Bm25Index::search`/`exact` |
| A plural costs a query without the word-form lookup | g01 plural: fail, 0 hits; g02 singular control: pass, 2 notes | `docs/research/question-set-2026-11-16.md`, regression class g01/g02 | a word-form (morphology) lookup in search |
| 36 provenance notes collapse to 17 observations | 36 notes → 17 observations, 2.1× over-count; 13 of 17 groups kinship-free | `docs/research/lineage-echoes-2026-11-16.md` | kinship-aware dedup before counting |
| Abstention has no surface signal (leave-one-out 0/8) | leave-one-out catch 0/8, single features miss the bar in 42/42 folds | `docs/research/abstention-2026-11-16.md`, leave-one-out table | the decision not to add a confidence field to search answers |
| nDCG@5 replaced a metric that could not disagree with MRR | old nDCG@10 = `1/log2(rank+1)`, monotone in MRR's `1/r`; replacement macro nDCG@5 0.655 vs MRR 0.599 | `docs/research/semantic-eval-practices-2026-11-16.md` §0.4/§1; `docs/research/task-set.md` macro table | nDCG@5 over graded evidence as the ranking eval's headline |

Each claim body states what would count against it (the falsifier) and the
boundary of the measurement, per the composability gate the predecessor
register lacked the fields to enforce.

## The decision-note example

`inbox/the-concise-profile-is-the-default-tool-surface.md` — the real
decision of 2026-11-16 (`tool-surface-tokens` §4), recorded as the worked
example of the shape:

```yaml
id: decision-concise-profile-default
type: decision
description: "The concise tool profile is the adapter's default; --tools full
  is the opt-in (decided 2026-11-16 on the 67% byte cut with no measured
  agent-task degradation)."
status: current
topics: ["[[tool-use]]"]
source_url: "file:///root/2brain/docs/research/tool-surface-tokens-2026-11-16.md"
provenance: "docs/research/tool-surface-tokens-2026-11-16.md section 4: byte
  census plus 26-scenario x 2-judge agent panel, 2026-11-16."
confidence: high
backed_by:
  - "[[the-concise-surface-saves-67-percent-of-a-sessions-tool-bytes]]"
supersedes: []
```

`backed_by` resolves: `tb_references` on the decision shows the outbound
`backed_by:field-string-wikilink` edge to the claim; `supersedes` is empty —
this decision replaces nothing yet.

## Verification (engine reads, not eyes)

Every number below is a tool answer captured after the writes.

**Diagnostics.** `tb_diagnostics` → `[]` (zero findings, whole workspace;
also zero per-write diagnostics on all 15 mutations). `tb_validate_value`
for a register-shaped claim and a `backed_by`-shaped decision: no
diagnostics, no undeclared fields.

**Instances.** `tb_instances_of claim` → **11**: the 5 pre-existing
ingest-queue claims (`ops/queue/codeactinstruct-00x`, shape unchanged) plus
**6 register claims** in `inbox/`, each with `status: acknowledged`,
`confidence: high`, and a resolved `evidence` link.

**Edges.** Outbound from the 13 new notes, per `tb_references`:
6 × `evidence` (claim → measurement note), 1 × `backed_by`
(decision → claim), 13 × `topics` (claim/measurement/decision → topic MOC)
= **20 edges, exactly the links written**. Inbound on the measurement notes
is exactly one per note, from its claim; claim 1 carries the decision's
inbound edge.

**Graph delta** (`tb_graph_health.counts`, before → after):

| count | before | after | Δ |
|---|---|---|---|
| edges | 4710 | 4730 | **+20** |
| nodes | 1215 | 1228 | +13 |
| broken_links | 0 | 0 | 0 |
| cycles | 29 | 29 | 0 (no cycle closed) |
| nodes_in_cycles | 708 | 708 | 0 |
| components | 208 | 208 | 0 |
| isolated | 198 | 198 | 0 |
| dead_ends | 20 | 20 | 0 |
| sources | 176 | 182 | +6 — exactly the six notes with zero inbound (claims 2–6, the decision) |

**Writes.** 15 engine commits as principal `claims` (audit ledger `outcome:
applied`, seq 1–13 for the notes): two type edits
(`6786347775`, `75ad0ecdcd`) and thirteen creations
(`69b7193808` … `9f597870b3`).

## Deviations, stated

1. **`tb_instances_of claim` returns 11, not 6.** Five ingest-queue notes
   share the type; the six register claims are the `inbox/` ones. Why the
   collision is a type decision rather than something this pass fixed, and
   why renaming was not an option (a `rename_type` cascade would rewrite
   live queue notes the pipeline still writes), is in "The collision this
   register exposed" below.
2. **Evidence resolves to a measurement note, not a `metrics.json` key.**
   None of the six numbers is in `tools/cards/metrics.json`, and this pass
   may not modify that file; each measurement note is a typed `insight`
   naming the source document and section. A metrics path stays a legal
   `evidence` value for future claims whose number lands there.
3. **The `status` enum was left at `[pending, acknowledged]`.** Register
   claims use `acknowledged`. Widening it (for example with a lifecycle
   value for claims that later lose their evidence) changes an enum shared
   with the queue reading — the owner should decide that.
4. **Notes live in `inbox/`,** per the vault's own pipeline rule
   (`CLAUDE.md`: new knowledge enters through inbox; promoting to
   `knowledge/` is the separate curated pass).

## What a claim register is for

The base could already answer *which note mentions a proposition* — search
was always able to find prose. What it could not answer is the question the
register exists for:

> **What backs this choice, how strong is each leg, and where is the
> number?**

Concretely: `tb_references` on any decision note returns its `backed_by`
claims; each claim's frontmatter carries `status` and `confidence`, and its
own references return the `evidence` measurement with the measured number
and the command that produced it. That is a two-hop typed walk
(decision → claims → measurements) over engine-validated edges — before
this pass the same question meant re-reading prose and hoping the numbers
were still in it. The inverse read is new too: `tb_instances_of claim` is
the whole register with status and confidence in one answer, so "which
claims currently hold, and what would count against each" is one query, not
an archaeology pass. A decision that loses its backing is now visible
mechanically: demote the claim and the decision's `backed_by` edge points
at a claim whose status says so.

## The collision this register exposed, and it is a type decision

**`type: claim` already meant something else.** Five notes in `ops/queue/` — the ingest pipeline's work items — carry
`type: claim` with a disjoint field set (`batch`, `file`, `current_phase`, `completed_phases`, `created`). The register
reused the type additively, so nothing broke, but the meaning is now shared:

```
tb_instances_of type_name=claim   ->  11 entries
                                      6 propositions (evidence present)
                                      5 queue work items (evidence absent)
```

The register did not create this; it made it visible. It is the second time in one day that one field carried two
meanings (`family:` was both provenance and topic), and the shape of the fix is the same: a decision about the type, not
another pass over the notes. Either the queue's work item gets its own type or the proposition does. Until then every
query over `claim` must filter on `evidence` being present — which is exactly the work a type system exists to save.

Widening `status` is the same knot from the other side: the register's claims use `acknowledged`, the queue's items use
`pending`, and one enum serves both.

## What the register does not do

- **It verifies nothing.** `evidence:` is a link and `source:` is a path, not a hash, so a claim cannot notice that its
  evidence was rewritten after the claim was written.
- **It ranks nothing.** No claim carries a weight in search, so "which claims does this base hold" is a type query, not a
  relevance query.
- **It enforces nothing.** `confidence: high` is a word the author chose; nothing compares it to the strength of the
  measurement it points at.
