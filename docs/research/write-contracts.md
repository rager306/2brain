# The four write contracts, as they are

An external review of 2026-11-16 read `tb-mcp-rs/src/lib.rs` and `tb-engine/crates/tb-engine/src/mutate.rs` and
named four places where the write path's guarantee is weaker than a reader would assume. This note checks each one
against the code and records what it is, what it should be, and the test that decides it. Nothing here is a
description of intent: each claim was read in the source at commit `8288032`.

## 1. `tb_create_file` is a read-then-write; the absence check is not atomic

`tb-mcp-rs/src/lib.rs:2869` (`prepare_create`) reads the path through `tb_content`, and refuses when the payload
carries content. The write is a plain `write_file` sent afterwards (`lib.rs:1398`).

Within one server process the single mutation slot (`take_slot`, `lib.rs:2901`) serialises the pair, so two calls in
one session cannot race. **Two clients are two processes**, and nothing serialises them: both read absence, both
write, the second wins silently, and the first agent's note is gone with no record.

Status: **fixed 2026-11-16** (`1b601b0`). The engine has a `create_file` mutation whose absence check IS its write —
one `O_CREAT | O_EXCL` open inside the serialized mutation — and the adapter sends it, keeping its own read as the
cheap refusal. An existing path rejects with `detail.reason: "already_exists"` and the incumbent's hash.

Test: `tb-mcp-rs/tests/create_race.rs` runs two `2brain mcp` processes against one private engine, both creates in
flight before either reply is read, twenty rounds; one success, one `policy/absence` refusal, the winner's bytes
whole. The engine has its own two-connection proof that the reject carries `reason: already_exists`.

## 2. `expected_hash` on `tb_edit_file` is a read-ledger token, not a whole-file compare-and-set

The adapter checks `expected_hash` against the hash its own read ledger recorded for that path. The engine receives
`old_string` and nothing else; WIRE.md:1224 says so deliberately — *"the exact match is its own read-before-write
precondition, no hash arg exists."*

So the guarantee is: **the text at that span is what the caller saw.** A concurrent change elsewhere in the file
does not cancel the edit, and does not need to — a fragment edit that could only apply to a byte-identical file
would refuse exactly the case where two agents edit different sections of a long note.

Compare `tb_write_file` (`lib.rs:1200`), which does pass `expected_hash` and is a whole-file compare-and-set.

Status: **contract, now documented** (`1b601b0`). The behaviour was right; the description did not distinguish the
two, so a caller could read `expected_hash` as stronger than it is.

`tb_edit_file`'s description now states the span precondition and points at `tb_write_file` for whole-file freshness;
`tb-mcp-rs/docs/tools.md` carries the same paragraph. The adapter already pins the wire half
(`edit_file takes no expected_hash: the exact match is its precondition`, `tests/write_file.rs`).

## 3. `tb_rename_type` checks the grant on the defining member, and rewrites references elsewhere

`lib.rs:1688` reads the type, checks `expected_hash` against the type's hash, and then checks
`policy.permits_member(&owner)` — the member owning the **definition**. The engine's `rename_type` then rewrites the
def, the claims, the slot shapes and every wikilink, which can live in other members.

The engine has its own guard, and it is a different one: a rewrite that would touch a *consumed* member rejects with
`detail.blocking_consumed_referrers` (WIRE.md:1184). Consumed-versus-edit is the workspace's topology; the adapter's
`--allow-mutations` allowlist is this run's narrower grant. A member can be editable by the workspace and outside
this run's allowlist, and the cascade would still write it.

Status: **fixed 2026-11-16**. `rename_type` is now a previewable op on the engine's `preview_mutation` read,
addressed by type name rather than a path. Its product carries an `affected` list: every path the cascade rewrites,
with the member that owns it and what changes there — `definition_move`, `claim`, `slot_shape`, `qualifier`, `meta`,
`body_use`, `wikilink`. The preview is the mutation's own planning code (`plan_rename_type`, the written/exempt
split, the referrer rewrite), not a second model of it, so the two cannot disagree about the write set. The adapter
reads that list before it mutates: when any affected member is outside `policy.members` the call refuses as
`denied:member_grant` — recorded in the audit under the same value, against the member outside the grant — and the
message names the members and a few paths in each, saying plainly that the definition's own member is allowed and
the cascade is not. A preview that itself refuses answers with the mutation's own message. Every allowed case
proceeds exactly as before.

Test: `tb-mcp-rs/tests/real_engine.rs` builds the workspace this defect needs — one git tree, member `base`
defining `widget`, member `app` holding an instance that claims `widget::base` — and runs it against a private
engine. With `members: ["base"]` the call refuses, names `app` and the instance path, the mutation connection is
never made, and both files are byte-identical afterwards; with `members: ["base","app"]` the same call moves the def
and rewrites the claim to `gadget::base`. The engine side has its own pair in
`tb-engine/crates/tb-engine/tests/it/preview_mutation.rs`: a cross-member preview names the referrer's member and
kind, and a preview of an unreferenced type lists the definition move alone and writes nothing.

## 4. Diagnostics stay advisory on a normal mutation

`mutate.rs` keeps validation errors advisory: a write that produces them succeeds, and the diagnostics ride the
result. That is the right default for drafts, and it is stated in the code.

Status: **not a defect, an absent mode.** Promoting a draft into accepted knowledge is a different verb with a
different contract, and the schema has no such mode. Until it does, "accepted" is a `status:` value a caller
maintains, not something the engine enforces.

Fix, when the knowledge-base direction reaches it: a promote-with-checks path that refuses on ERROR-severity
diagnostics, separate from the draft path, and evidence that it does not make ordinary writing slower or noisier.

## What the four have in common

Three of the four are about **where the precondition lives**. The adapter is a policy layer in front of an engine
that already serialises its own writes; a check the adapter performs and the engine does not enforce is a check
that holds in one client and not in the next process. The rule this note leaves behind: *a precondition that
matters belongs in the engine's mutation handling, and the adapter's copy of it is for the error message.*
