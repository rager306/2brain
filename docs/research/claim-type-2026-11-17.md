# `type: claim` — one name, two records, and the split that survives review

Measured 2026-11-17 on `/root/vault` at `a8aba4b`: 11 notes claim `type: claim` — **6** propositions
carrying `evidence:` (the register) and **5** ingest-queue work items carrying `batch`/`file`/
`current_phase`. The collision was already visible on 2026-11-16 (`docs/research/claim-register-2026-11-16.md`).

## What the best practice says

- **One type, one record.** A type is a promise about required fields; if two records share it, every
  query must filter, which is exactly the work a type system exists to remove.
- **The name should say which record it is.** `claim` is the knowledge object (a proposition specific
  enough to be wrong); a queue item is an operational record about work in progress.
- **Rename the smaller, newer, less cited record.** Here that is the queue item: 5 notes against 6
  register claims plus a decision note that cites them (`backed_by`), and the register is the one the
  product now reads.

## Options

| option | shape | verdict |
|---|---|---|
| **A. queue item becomes `ingest-task`** | keep `claim` for propositions; rename the 5 queue notes and their type definition | **recommended** — 5 notes, one cascade, the knowledge meaning stays the default |
| B. propositions become `claim-note` | reverse split | worse: the register is the meaning the trajectory builds on, and its name would then repeat `-note` |
| C. discriminator (`claim_kind: proposition\|queue-item`) | one type, one enum | every query branches; the type stops promising anything; rejected |
| D. leave it, filter on `evidence` | no change | the overload is documented as a required filter, which is the defect |

**Decision (2026-11-17): A accepted, and it is blocked on the writer.** The queue item becomes
`ingest-task`; `claim` stays the proposition. The blocker is external: no writer for `ops/queue` lives in
this repository (the queue is filled by an outside pipeline), so renaming the type first would let that
pipeline recreate `type: claim` on its next write. The rename and the pipeline edit must land together.

## How A is executed safely

`tb_rename_type` (engine) carries claims, slot shapes, meta and wikilinks through one saga, and the
adapter refuses a cascade that would write outside the member grant. Before it: a dry-run preview
(`tb_preview_mutation` has no rename op for type; `rename_type` previews through its own grant path),
a vault commit immediately before (the rollback), and `tb_diagnostics == 0` afterwards. The queue's
own pipeline writes the old name until it is updated, so the rename and the pipeline change must land
together, or the pipeline recreates the old type the next time it writes a work item.

**Gate:** `tb_instances_of claim` returns only propositions, with no `evidence` filter; the queue still
answers `tb_instances_of ingest-task`. **Rollback:** `git revert` the vault commit and the pipeline
change together.

## What A does not decide

It does not widen `status` (the register uses `acknowledged`, the queue uses `pending`); that shared
enum is a second, smaller collision and is named but not solved here. It also does not verify claims:
`evidence:` is a link, not a hash, so a claim cannot notice its evidence changed.
