---
id: ADR-0002
title: Promote-with-checks refuses on ERROR diagnostics in the engine, as an optional rider
status: Proposed
lifecycle: "[proposed]"
date: 2026-11-18
supersedes: none
related: [ADR-0001]
---

# ADR-0002: Promote-with-checks is an engine-side rider, not an adapter workflow

## Status

**Proposed [proposed].** No code exists. The record fixes the shape and the acceptance gate so the
implementation has one place to land; it authorizes nothing until the gate below passes.

## Context

The engine's mutation contract is advisory: a write that produces validation diagnostics succeeds, and the
diagnostics ride the result (`docs/research/write-contracts.md` §4). That is right for a draft — a note being
written is allowed to be inconsistent for a moment — and wrong for the step that declares a note *accepted*,
where an unresolved error means the record should not be promoted at all.

Today "accepted" is a `status:` value a caller maintains, and nothing enforces it. The adapter cannot enforce it
on its own: it has no atomic rollback, and a check it performs holds for one client and not for the next process.
The same note records the rule this ADR follows — **a precondition that matters belongs in the engine's mutation
handling**, and the adapter's copy exists only to phrase the error.

## Decision

An optional `require_clean` rider on the existing file mutations (`write_file`, `edit_file`, `create_file`, and
`batch`). Default `false`, so every current caller keeps the advisory contract byte for byte.

When `true`, the engine:

1. applies the change to a **scratch copy of the held knowledge base** (no disk write, no commit);
2. computes the diagnostics that state would produce;
3. **refuses** with `policy/diagnostics` naming every ERROR-severity code and its paths, when any exist;
4. otherwise performs the ordinary mutation — one write, one commit, one rebuild.

The refusal is a precondition failure, not a new kind of answer: it carries no `hash`, nothing reached the
working tree, and a retry after the caller fixes the note is an ordinary retry.

## Alternatives considered

- **The adapter previews, then writes.** Rejected: two calls are not one precondition. A second writer between
  them changes the answer, and a client that speaks the wire directly bypasses the check entirely.
- **A new `promote` mutation verb.** Rejected: promotion is a status change to an existing file, not a different
  file operation. A new verb would duplicate the write path, the saga, and its audit records to gain one flag.
- **Write first, revert on error.** Rejected: a revert is a second write with its own failure mode, and a crash
  between them leaves the tree in the state the check exists to prevent.
- **Refuse on warnings too.** Rejected for now: warnings are the normal state of a growing base (`orphan`,
  `stale`), so refusing on them makes promotion unusable. A `max_severity` argument can grow later if a caller
  asks with a measured need.

## Consequences

- The check costs a scratch build **only when requested**; the default path is unchanged, and that is the claim
  the implementation must measure rather than assert.
- `WIRE.md` gains one optional argument on four mutations. Additive and optional, so `schema_version` stays 29
  and the captured fixtures do not change — the same rule the read side already follows.
- The adapter exposes the rider on the same four tools; its copy of the rule is the tool description, not a second
  check.
- Accepted knowledge is now enforceable at its edge, which is what the knowledge-base maintenance lanes need: a
  proposal can be checked before it is accepted instead of after.

## Implementation note (2026-11-18, no code written)

The "scratch copy" this record asks for already exists: `PreviewOutcome::Built { preview: &KnowledgeBase, .. }`
in `src/engine.rs` holds the **would-be** knowledge base for a single-file op, so
`preview.diagnostics()` answers the check without a second build mechanism. The seam is therefore
`serve.rs::handle_mutation`: for a flagged write, run the same `PreviewInput::File` the preview verb builds,
keep the ERROR-severity diagnostics, and refuse with `policy/diagnostics` before the saga starts. Two facts the
implementation must respect:

- the refusal must be the preview's reject when the preview itself rejects (an absent file, a stamp on a
  non-list field), so one failure is not reported twice with two different shapes;
- the check and the write must read the same held version. A preview taken, then a mutation run against a newer
  version, is exactly the two-call race this ADR refuses in the adapter.

Scope for the first implementation slice: `write_file` only. `edit_file`, `create_file` and `batch` follow in the
same change or are named as still missing — an argument that exists on a tool and does nothing is the stub this
decision exists to avoid.

**Progress 2026-11-18.** All four verbs are implemented and tested: each parses `require_clean`, and the check
runs before its saga — through one helper over the preview's single-file `PreviewOp` for `write_file`,
`edit_file` and `create_file`, and through a composed `PreviewInput::Set` for `batch`. The tests cover the refusal
for all four (bytes unchanged for a write and an edit, no file at all for a create, `policy/diagnostics: …
type-repo-unknown`), the clean cases, and an edit that lands once its target type exists.

`batch` needed the report this record's earlier paragraph said did not exist, and it turned out to be one
indirection away: the composed change set is built in request order through `mutate::compute_edit`, the same pure
transform the committing op uses, so a preview of it answers the question directly; a diagnostic's file maps to
the LAST op that touched it, so the refusal reads `batch op 2 (write_file <path>): <code> in <path>`, and a
diagnostic on a path no op touched is named `elsewhere in the tree` instead of being blamed on an innocent op.

Two facts the implementation respected, as this record required: a rejecting preview is reported once, through
the preview's own reject, not twice in two shapes; and the check and the write read the same held version,
because the composed set is previewed and then applied inside the same `handle_mutation` with the write lock
held across both. Gate part 3 (the default path did not slow down) is still a measurement nobody has taken, so
the record stays `Proposed`. The adapter sends the key only when a caller asks for it: the engine refuses unknown fields, so
an explicit `false` would break a caller paired with an engine that predates this record.

## Acceptance gate

1. A two-process test: one write with `require_clean: true` that produces an error is **refused**, the refusal
   names the code, and the file's bytes are unchanged; the same write with the error fixed **succeeds** and
   commits. **Done** for a single process (`require_clean_refuses_a_write_that_would_introduce_an_error`); the
   two-process shape needs the rider exposed through the adapter, which it is, so a client-level test is the
   remaining half of this item.
2. A default-path test: the same write without the rider still succeeds with the diagnostic in its result.
   **Done** — the same test asserts the advisory path lands and reports `type-repo-unknown`.
3. A measurement, not a claim: ordinary mutations are no slower within noise on the fixture corpus, with the
   number and the command recorded next to the change.
