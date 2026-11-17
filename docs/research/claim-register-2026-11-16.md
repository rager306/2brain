# The claim register: decisions rest on claims, claims rest on measurements

Shipped into `/root/vault` on 2026-11-16 through the engine (12 notes, each its own commit, `tb_diagnostics` 0). The design
comes from the predecessor system's claim map (`docs/research/arscontexta-design-2026-11-16.md`), with the two fields it
lacked: **status and evidence**.

## The chain the register makes answerable

```
decision  --backed_by-->  claim  --evidence-->  the note that measured it
```

A decision note now carries `backed_by:` (claim notes) and `supersedes:`; a claim carries `evidence:` (the measurement it
rests on), `confidence:` and `source:`. Both are optional fields on existing types, so no note that predates the register
became invalid — the diagnostics over the whole vault are **0**.

The query this makes possible, and that the base could not answer before: **"what backs this decision, and what number is
behind that claim"** — by following two typed edges, not by reading prose that mentions both. `tb_references` answers it:
the decision note shows its claim as an outbound reference, the claim shows its evidence note.

## What is in the vault now

| kind | count | what it is |
|---|---:|---|
| claims (`type: claim`, six with evidence) | 6 | one line each, each naming the number it rests on |
| evidence notes (`type: insight`) | 5 | the measurement itself, already recorded before the register |
| decisions (`type: decision`, one with `backed_by`) | 1 | the concise-profile default, citing its claim |

The six claims, and the number each carries:

1. the concise profile saves **67 %** of a session's tool bytes, with no measured wrong-tool or wrong-argument cost;
2. a scoped read decides candidates before scoring — the post-ranking filter cost **9 % at 3 members, 28 % at 9**;
3. a plural costs the query without a word-form lookup — **0 hits against 2** for the singular control;
4. **36** provenance notes carry **17** observations (2.1×), and 13 of the 17 clusters hold no kinship signal;
5. abstention has no surface signal — leave-one-out catches **0 of 8** unanswerable queries;
6. graded nDCG@5 replaced a binary nDCG@10 that, as a monotone function of first-hit rank, could never disagree with MRR.

## The defect this exposed, which is the point of writing it down

**`type: claim` already meant something else.** Five notes in `ops/queue/` — the ingest queue's work items — carry
`type: claim` and a disjoint field set (`batch`, `file`, `current_phase`, `created`). So `tb_instances_of type_name=claim`
returns **11** entries: six propositions and five queue items. The register did not invent the collision; it made it visible.

That is the second time in one day that a field turned out to carry two meanings (`family:` was provenance and topic). The
fix is a decision about the type, not another pass: either the queue's work item becomes its own type, or the register's
propositions do. Until then, any query over `claim` must filter on `evidence` being present, which is exactly the kind of
work a type system exists to save.

## What the register does not do

- It does not verify anything: `evidence:` is a link, and a link can rot. The `source:` field names the document, not a
  hash of it, so a claim cannot detect that its evidence was rewritten.
- It does not rank: a claim has no weight anywhere in search, so "which claims does this base have" is a type query, not a
  relevance query.
- It is not a policy: `confidence: high` is a word the author chose, and nothing checks it against the evidence's strength.
