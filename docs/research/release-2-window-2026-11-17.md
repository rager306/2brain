# The migration window, made observable (release 2, step 7b)

Refusing the legacy spelling outright was the obvious way to close the window, and it was the wrong
one. Two things had to be true at once: a release-1 tree keeps READING correctly, and an operator can
see which files still hold stale bytes. Only the first was true before this change.

## What was added

`engine-schema-legacy-spelling` — a `hint` on any written claim that spells `au.engine.*` or
`::au-engine`. It never blocks, and its `fix` names the canonical spelling plus the command that
applies it for a whole tree (`au migrate identity`). The claim is already READ correctly by the
canonicalizer, so the note exists for the one thing a read cannot say: the bytes on disk are stale.

## What it caught immediately

The diagnostic is how the remaining writers were found, and three of them were the engine itself:

| writer | wrote | now writes |
|---|---|---|
| `serve.rs` `add_member`, seeding a workspace | `type: au.engine.workspace::au-engine` | `tb.engine.workspace::tb-engine` |
| `repo.rs` `serialize_user_registry` | `type: au.engine.repos` | `tb.engine.repos` |
| the test seed helpers (`seed_workspace`, `seed_repo`, `seed_readme`) | legacy | canonical |

The first two matter most: an engine that writes the spelling it calls legacy is a writer that keeps
the window open by itself, and no amount of test-weakening would have shown it as a defect.

## What was migrated

- 343 fixture files under `tb-engine/scenarios/` (241 scenarios): `au.engine.*` → `tb.engine.*`,
  `::au-engine` → `::tb-engine`. The 242 snapshots were already canonical after the flip, so they did
  not move — which is the check that the fixtures and the engine now agree.
- 17 `it` test files' inline fixtures. `engine_schema.rs` keeps both spellings on purpose: it is the
  file that TESTS the window.
- the vault's `.2brain/repo.yaml` comment.

## The test that pins the window

`an_engine_schema_claim_resolves_in_either_spelling` runs one fixture twice, changing only the
written claim. The canonical file is clean; the legacy file is clean of ERRORS and carries exactly
one `engine-schema-legacy-spelling` hint. A second unit test asserts a canonical file earns no note,
so the diagnostic cannot quietly start firing on current data.

## Measured

Engine lib **745 / 1** (the root-only unreadable fixture), `it` **734 / 3** (the three root-only
fixtures), fmt and clippy clean. On the live vault the new binary reports **0 diagnostics**; adding one
file with a legacy claim produces exactly the one hint, and `instances_of` on the legacy name still
answers its 4 rows.

## Still open

Nothing in the codebase writes the legacy spelling any more, but the acceptance stays: removing it is
what actually closes the window, and it can only be removed when no release-1 tree is expected to be
read. That is a release decision, not a code one, and the diagnostic is what will make it safe.
