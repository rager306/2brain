---
id: ADR-0003
title: A cascade mutation carries the grant it must respect, and the engine checks it under the write lock
status: Proposed
lifecycle: "[proposed]"
date: 2026-11-18
supersedes: none
related: [ADR-0002]
---

# ADR-0003: the cascade grant travels with the mutation

## Context

`rename_type` rewrites the type definition and every file that references it. The adapter protects that
cascade by previewing it (`preview_mutation { op: rename_type }`), refusing when any rewritten member is outside
its policy, and then sending the mutation with `old_name` and `new_name` only.

The engine recomputes the cascade from the state it holds at mutation time. Both requests run the same planner,
so they agree **only while the state does not change between them**. When it does, the second plan can include a
referrer the preview never saw:

1. agent A previews a rename; the plan touches members `v` and `sub`, both allowed.
2. agent B adds `type: note` to a file in `sub2`, outside A's allowlist.
3. A sends the mutation; the engine plans again, now includes `sub2`, and rewrites it.

That is a grant check on one state and a write on another. It is a static finding, not a reproduced race: the
scenario needs two clients, and the existing real-engine test is `#[ignore]` and does not interleave.

## Decision

The mutation carries the grant. `rename_type` accepts an optional `allowed_members: [name]`.

- **Empty or absent** means unrestricted, which is the pre-existing contract for a direct caller with no policy.
- **Non-empty**: after planning, under the write pipeline's lock, the engine refuses with
  `policy/member_grant: rename_type <old> -> <new> rewrites N path(s) outside the grant: …` when any path the
  plan writes belongs to a member outside the list. Nothing is written, moved or committed.
- A path with no owning member counts as outside: it cannot be shown to be inside, so it is not assumed to be.
- The adapter sends its policy's allowlist when the policy has one, and sends nothing when it permits every
  member.

The check runs in `handle_mutation`, whose body holds the write lock, so the plan it checks is the plan it
executes — the state cannot move between them.

## Alternatives considered

- **Bind to a plan id or a preview version.** Correct and more general, but it needs the engine to keep preview
  state across requests, and the preview is currently a pure read. Rejected as a first step; it remains the
  option if a second cascade mutation needs the same guarantee.
- **Re-preview immediately before the mutation.** Still two requests: the window only narrows.
- **Recompute the allowed set in the adapter from a fresh preview and send nothing.** The engine would still be
  the writer, and the check would still live outside its lock.

## Consequences

- The guarantee moves to the process that writes, which is the rule the write-contract note already states.
- Additive optional argument, no wire bump, no fixture change. An engine that predates it refuses the unknown
  field — so a policy with an allowlist paired with an old engine refuses renames instead of running unguarded.
  That is the fail-closed direction, and it is why the adapter sends the list only when it has one.
- Only `rename_type` carries it today. A second cascade mutation (`rename`, `inline`, `promote`) gains it the same
  way when a consumer needs the guarantee.

## Acceptance gate

1. **Done:** an engine test with two members — the cascade reaches `sub`, the allowlist names only `v`, the
   rename is refused with `policy/member_grant`, the def does not move and the referrer keeps its old claim; the
   same rename with both members allowed lands and rewrites the referrer.
2. **Open:** an adapter-level test that forces a referrer into the workspace between the preview and the mutation
   and asserts the second plan is refused. It needs a client-driven two-connection run, which the ignored
   real-engine test is the place for.
3. **Open:** a deploy, which is what flips this record from `proposed`.

**Progress 2026-11-18.** The engine check, the argument, `WIRE.md` and the adapter's send-side are implemented;
gate 1 passes. Gates 2 and 3 are open, so the record stays `proposed`.
