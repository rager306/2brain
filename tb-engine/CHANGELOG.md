# Changelog

Notable engine and wire-contract changes, newest first.

The wire contract is `crates/tb-engine/WIRE.md`. Every frame carries a
`schema_version`; bumps are called out here.

Release markers (`## 0.0.2-alpha`) group the dated entries that shipped in that
2brain release. Dated entries above the newest marker are not released yet.

## 0.0.2-alpha

Wire schema stays **29**: every change below is additive to the wire. Behaviour
changes that alter what existing calls return are listed in 2brain
`MIGRATION.md` under 0.0.2-alpha.

## 2026-09-25

- Engine: a read or write that names no repo now scopes to the served entry.
  - before, the default was the repo rooted at the shallowest path, so a
    sibling member at the entry's own depth could stand in for the entry.
  - a composition verb with no `root` authored that sibling's
    `workspace.yaml`, or was refused as not editable when it was a `discover`
    member.
  - the same default fed the repo-scope `config` / `set_config` channel and
    a `validate_value` that names no repo.
  - WIRE.md already stated "absent = the served entry"; this makes the engine
    honor it. No wire change.
- Wire: a `root` argument matches a member root by canonical path, on the
  composition verbs, `config` / `set_config`, and `set_ignores`.
  - a symlinked spelling (`/var/...` for `/private/var/...`) resolved to no
    member and was refused; it now resolves. Additive, no `schema_version` bump.
- Wire: `remove_workspace_member` rejects the containing repo, nothing written.
  - it removed it and left `workspace-omits-containing-repo`, an `error` the
    write itself authored. The role and add verbs already refused the same
    move into `discover`.
  - a removed member also listed in `disabled:` leaves that list in the same
    commit, so no stranded `disabled-member-not-declared`.
- Wire: new mutation verb `set_workspace_member_disabled { name, disabled,
  root?, expected_hash? }`, switching a declared member off or on through the
  workspace's `disabled:` overlay, one commit.
  - disabling the containing repo, or a name not in `edit:` / `discover:`,
    rejects. Additive, no `schema_version` bump.

## 2026-09-24

- Wire: `preview_mutation` accepts `ensure_mixins` / `ensure_mixins_strict`
  on `write_file`, `edit_file`, and `rename`, folded after the stamps exactly
  as the real write folds them.
  - the product carries `ensure_mixins`, the per-mixin outcomes the write's
    response reports. A strict un-appliable mixin is a `reject`.
  - a stamp or mixin on a type-def target now rejects the preview, as it
    rejects the write.
  - Additive, no `schema_version` bump.
- CLI: `au members <entry> [--json]` resolves an entry's declared members
  without a daemon, for a launcher's pre-start check.
  - per member: `role`, the `tier` that located it (`entry` / `sibling` /
    `registry` / `cache` / `unmounted` / `disabled`), `path`, and the
    member-outcome `code`.
  - `complete: false` with `incomplete` diagnostics when a locked dependency
    is not yet in the package cache.
  - runs the build's own composition, so it answers what `au daemon start`
    mounts. Reads only.
  - refuses with exit `2` when `$HOME` is unset, as `au daemon start` does.
- Wire: each `members` row (and `overview.members`) carries `tier`, the
  resolution tier that located it (`entry` / `sibling` / `registry` / `cache`
  / `disabled`), the same value `au members` reports. Refines `local`.
  Additive, no `schema_version` bump.
  - JSON contract in WIRE.md §Daemon-less member resolution. Additive, no
    `schema_version` bump.
- Type reads: every full entry of `types`, `type`, `type_batch`, and
  `subtypes` carries `effective_fields` and `effective_meta`, the inherited
  view beside the def's own `fields` / `meta_blocks`. `summary` entries don't.
  - `effective_fields`: the closure's fields, each with `origin` (the declaring
    def) and `divergent`. A divergent field is one entry per origin.
  - `effective_meta`: per meta type identity (`meta_type: { name, hash,
    type_owners }`), the surviving blocks, each with `from` (the declaring
    def). More than one block is a conflict.
  - `type_closure` `fields` gains `divergent` and reports a divergent field
    once per origin, where it reported only the lex-min origin before.
  - Additive, no `schema_version` bump.
- New diagnostic `meta-inheritance-conflict` (warning). A type-def extending
  two unrelated parents that carry different blocks of the same meta type now
  gets this warning, where before one block silently won by `extends:` order.
  Meta inheritance uses the same rule as `location:`: a block is overridden
  only by one on a more specific type, and `meta: []` hides every ancestor's
  meta on every path. Blocks group by the meta type's identity: an in-sync
  own and peer copy of `myMeta` are one type and can conflict, a diverged
  pair are two types.
- `duplicate-meta-block` fires on one def carrying two written forms of one
  meta type: an own `myMeta` beside an in-sync `myMeta::peer`, or `myMeta`
  beside the self-qualified `myMeta::this-repo`. Before, only the same
  written `type:` twice fired. A diverged same-named pair still coexists.
- Placement checks (`location-mismatch`, `location-strict-violation`,
  `location-partial-unmet`) read each `location:` from the def the claim
  names. Two same-named types in different repos with identical fields but
  different `location:` blocks are one identity, and a claim could previously
  be checked against the other repo's block. A bare claim now uses its own
  repo's def, a `name::repo` claim that repo's. Inheritance is unchanged: a
  block is overridden by a block on a more specific type.

## 2026-09-23

- `semantic_tokens`: a `type-claim` token carries `resolved`, `{ name, repo,
  hash }`, the identity the claim resolves to (the same one validation uses),
  on every claim position: an instance, inline-record, or typed-fence `type:`,
  a type-def `extends:` parent, and a `meta:` `type:`. A consumer joins it onto
  an identity-keyed read such as an `instance_counts` row by `(name, hash)`.
  Absent when the claim does not resolve. Additive, no `schema_version` bump.
- `move_dir` / `delete_dir` refuse a folder the repo's scope excludes even when
  it holds no files (an empty `node_modules/x`), `<dir> (scoped out)`. A
  scoped-out folder holding files already refused.
- A `rename` onto a dangling symlink refuses as `destination already exists`,
  as `move_dir` does, not as a case collision.
- A folder op on a folder nested near the walk's depth cap refuses as
  `cannot walk`, not as a stale engine view that no rebuild would settle.
- A `move_dir` of about a thousand files or more no longer fails its commit:
  the commit message, one `Moved:` trailer per moved file, is passed to git as
  a file rather than a command-line argument. Linux caps one argument at
  128 KiB. The rollback's compensating commit is covered the same way.
- A name-keeping move (`rename`, `move_dir`) no longer rejects over a
  bare-name referrer it leaves unchanged in two more cases: one beyond a
  symlinked directory ("git cannot classify"), and one in a repo no git
  working tree covers. The git checks now run only over the files a mutation
  writes.
- A tracked file moved into a git-ignored folder (`rename`, `move_dir`) no
  longer records a `Moved:` trailer. The destination never enters history, so
  the pinned-reference forward trace would chain to a path git never recorded.
  The same rule as a move from a never-tracked source.
- Fixed: git read every path a mutation passed as a GLOB pattern, so a file
  named `a[1].md` also matched `a1.md`. A mutation touching `a[1].md` committed
  a human's uncommitted edit to `a1.md` under the engine's `Mutation-Id`, and a
  rollback could discard it. Every git call now reads paths literally.
- A mutation over a very large path set no longer fails to start git: staging,
  committing, and restoring read their paths from stdin, and the clean check
  runs in bounded batches. Before, tens of thousands of files in one folder op
  exceeded the OS argument limit.
  - A rollback asks git which paths exist at HEAD in one query, not one git
    process per path. Measured on a 6000-file commit, check, and restore:
    about 115s before, about 11s after, nearly all of it the per-path queries.
- `move_dir` and `delete_dir` re-check their files with one `git status` per
  working tree, immediately before the files move or are removed, not one per
  file. A large folder no longer holds the write lock across a git process per
  file. A file edited during the move phase itself is no longer caught; the
  upfront clean sweep and the pre-phase re-check still are.
- Fixed a data-loss path: a write under a symlinked directory now rejects.
  - Git reports a path beyond a symlink as clean, then refuses to stage it. A
    `move_dir` of a symlinked folder lost the moved files on rollback, and a
    `delete_dir` removed the link target's files without committing.
  - Every mutation rejects when git cannot classify a path it writes, naming
    git's reason; its preview rejects alike. This covers `write_file`,
    `edit_file`, `delete_file`, and `rename` under a symlinked directory too.
  - `move_dir` / `delete_dir` reject a folder that is a symlink or sits under
    one, `<dir> (symlink)`.
- `move_dir` and `rename` reject a destination whose parent directories the
  graph cannot hold: a symlink, a nested repo marker (declared or not, the
  moved files would drop out of the graph), or a directory differing from an
  existing sibling only by case.
- `preview_mutation` previews the folder verbs: `op: "move_dir"` `{ path, to }`
  and `op: "delete_dir"` `{ path }`, without writing or committing.
  - `move_dir`'s product carries `rewrites`, `delete_dir`'s `stranded`, both
    `untracked_files` and a new `untracked_dirs`, each equal to what the
    committing verb then reports.
  - The `target` is the folder (at `to` for a move): `hash` null,
    `identities` empty, `diagnostics` every file's under it, the verb
    result's shape. `blast_radius` lists every other file whose diagnostics
    change, e.g. the referrers a delete leaves dangling.
  - A preview refuses exactly where the verb would, the git-working-tree
    requirement included.
  - Additive, no `schema_version` bump.
- A `rename` preview now refuses a repo no git working tree covers, as the
  `rename` itself does. It used to return a product for a rename that would
  then refuse.
- The engine's in-repo runtime output now ignores itself in git: the saga's
  recovery state under `.arsumbris/au-engine/run/` (the intent marker, the
  parked copies of git-ignored files, the failure sentinels) and the
  `TB_TRACE` output under `.arsumbris/au-engine/logs/trace/`.
  - Each directory gets a `.gitignore` holding `*` when the engine creates it;
    an existing one is never overwritten.
  - Before, nothing ignored them, so a human's `git add -A` could commit them,
    including a parked copy of a git-ignored, possibly private, file. The
    engine's own commits were never affected, they are pathspec-scoped.
- New mutation `delete_dir` `{ path }`: deletes a folder as ONE saga, every
  catalogued file under it plus its directory skeleton.
  - Raw like `delete_file`: no referrer is rewritten. The result's `stranded`
    lists every reference from a surviving file into the folder,
    `[{ path, links: [{ target, to_file }] }]`; references between contained
    files are absent.
  - One deletion commit staging exactly the enumerated files, never the
    directory: a file created in the folder meanwhile is never deleted or
    committed. `last_live_commit` (its parent) resolves every deleted file.
  - `untracked_dirs` lists the removed directories no commit records;
    `untracked_files` the git-ignored files removed outside git.
  - Refuses what `move_dir` refuses about the folder, and a folder holding
    files that no git working tree covers (a partial delete could not roll
    back).
  - Additive, no `schema_version` bump.
- New mutation `move_dir` `{ path, to }`: moves a folder as ONE saga, every
  catalogued file under it plus its directory skeleton, empty directories
  included.
  - Every path-addressed reference into the folder is re-pointed, from inside
    and outside it; a bare-name reference is left byte-identical and
    uncommitted. One commit per working tree, one `Moved:` trailer per file.
  - Refuses a folder holding anything the engine does not catalogue (a
    scoped-out path, a nested `.git` or repo, a symlink), a catalog behind the
    disk, a taken, self-nested, cross-repo, or out-of-scope destination.
  - Result adds `rewrites` (the rename-preview shape) and `untracked_dirs` (the
    created directories no commit records); diagnostics are the folder's.
  - Additive, no `schema_version` bump.
- A mutation's rollback now puts the directory skeleton back, live and at crash
  recovery.
  - Fixes a failed `rename` into an existing EMPTY folder: the rollback pruned
    the folder along with the created file. It now stays.

- A file git ignores and has never tracked (a `.DS_Store` under a global
  ignore, an ignored scratch note) is written OUTSIDE git by every mutation:
  never staged, never committed, and rolled back from a copy parked before the
  write.
  - Fixes a data loss: when such a file shared a commit with a tracked change
    (a `rename` of it whose tracked referrer was rewritten), `git add` failed on
    the ignored path, and the rollback DELETED the file. The rollback now
    restores it byte-identical, live and at crash recovery.
  - Fixes a `rename` of a tracked file into an ignored folder, which failed its
    commit: it now lands, committing the source's deletion.
  - A mutation touching only ignored files was already safe (it made no commit);
    it is unchanged apart from the new field.
  - New `untracked_files` on every mutation result and every `preview_mutation`
    product: the absolute paths written, moved, or removed outside git,
    path-sorted, `[]` when none. The preview lists what the write will report.
  - A `delete_file` of such a file carries no `last_live_commit`; it was never
    in a commit.
  - Additive, no `schema_version` bump.

## 2026-09-22

- A mutation that moves or removes a file re-checks it right before touching
  it, not only in the upfront clean sweep: `rename`, `rename_type` (the def
  file), `delete_file`, `inline` (the folded-away file), and `set_ignores`
  removing `.auignore`.
  - Behaviour change: a file edited after the mutation was planned, but before
    it moved or was removed, now rejects the whole mutation ("changed since the
    mutation was planned"), which rolls back. Before, a move carried the edit to
    the new path and committed it under the mutation's `Mutation-Id`, and a
    delete dropped it.
  - The baseline is clean-at-HEAD, so it covers assets too. A moved file the
    mutation itself rewrote (a self-reference) is checked against those bytes.
  - An edit landing after the write but before the commit is not re-checked:
    rejecting then would roll the file back to HEAD and discard that edit too.
  - No wire change.

- `preview_mutation` gains a `rename` op (`path`, `to`, optional `stamps`), the
  `rename` mutation's arguments. It previews a file move or rename without
  writing or committing.
  - `rewrites`, a new product field present only for `rename`: the referrers
    whose bytes the move changes, each `{ path, links: [{ from, to }] }`. A
    referrer the move leaves byte-identical (a bare `[[name]]` on a name-keeping
    move) is absent, so the list is the actual would-change set.
  - `target` is the moved file at its destination; `blast_radius` carries every
    other file whose diagnostics change, e.g. a referrer whose re-spelled link
    becomes ambiguous. The move's source is not a blast entry.
  - refuses exactly where `rename` would: a type-def, a cross-repo move, a taken
    or case-colliding destination, a consumed-member referrer the move rewrites.
  - additive to the wire, no `schema_version` bump.

- `rename` writes only the referrers whose bytes change. A move keeping the
  file's name re-renders a bare `[[name]]` identically, so that referrer is no
  longer written, clean-checked, or committed. It is still recomputed.
  - Behaviour change: a name-keeping move (`a.md` → `sub/a.md`) no longer
    rejects over a bare-name referrer that has uncommitted changes, or that
    lives in a consumed (`discover` / `dep`) member. The human's uncommitted
    edit stays in the working tree, outside the mutation's commit.
  - A path-addressed referrer (`[[x/a]]`) is still rewritten and still gated.
  - A link without `/` is a name link, so it keeps its spelling on a move
    everywhere. A top-level file's `[[a.md]]` was re-spelled as the path
    `[[sub/a.md]]` when the file moved; it now stays `[[a.md]]`, still valid.
  - No wire change.

- `instance_counts` gains an optional `origins` filter (`["file"|"nested"|"meta"]`),
  the same filter as `instances_of`, so the count read is a true dual of the list
  read. A consumer reads a per-type COUNT over a chosen origin set in one cheap
  call: `origins:["file"]` is the file-only count that matches a file-only
  `instances_of` drill-in, without fetching the rows.
  - Behaviour change: absent `origins` now counts ALL THREE origins
    (file + nested + meta), identical to `instances_of`'s no-arg set. It was
    file + nested (meta excluded). So a no-arg `instance_counts` now INCLUDES
    meta sites (a `meta:` block on a type-def, including the builtin
    `au.engine.*` metas). Pass `origins:["file","nested"]` for the former
    default.
  - `by_type` shape (`{ name, hash, type_owners, count }`) is unchanged.
  - No `schema_version` bump: the `origins` arg is additive and the default
    change is a value-behaviour change, not a wire-shape change.

## 2026-09-20

- An inline record at a `*` (reference-only) branch of a union slot is now
  rejected as `inline-value-type-not-compatible`, instead of being accepted and
  descended into that branch type's field validation.
  - `*` is reference-only, so a union's `*` branch admits only a `[[...]]`; an
    inline value there satisfies no branch. Only a bare-record or `&` branch
    hosts an inline value. Holds on both surfaces (frontmatter and body fence),
    single-repo and cross-repo.
  - a consumer relying on the old descent (e.g. seeing the branch type's own
    field diagnostics for an inline value at a `*` branch) now sees the single
    compat error instead. No `schema_version` bump: the diagnostic codes are
    unchanged, only which one fires.

## 2026-09-19

- `semantic_tokens` now emits per-name leaves (`type-ref` / `shape-builtin` /
  `enum-member`) for QUOTED type-def field shapes too, spanned into the source
  scalar, not only the `field-shape` container.
  - the parse layer aligns each leaf to the source (an affine shift over the
    quote), so a quoted shape (`r: "decision*"`) drives go-to-def, hover, and
    per-name coloring like an unquoted one.
  - additive to the wire, no `schema_version` bump: the token kinds are
    unchanged, now populated for quoted shapes. A consumer's quoted-shape
    fallback becomes redundant. See `WIRE.md`.

## 2026-09-17

- Diagnostic `fix` grows a machine-applicable `actions` list beside `description`.
  - each action is `{ verb, title, args }`: a `mutate` verb to run plus the args
    the engine parsed (keyed by the verb's own arg names), so a consumer renders a
    generic apply button with no per-code switch. Topology args (e.g. `add_dep`'s
    `root`) stay the consumer's to complete via `resolve_member`.
  - `actions` is a list, so a fix can offer a choice of alternatives.
  - populated on six deterministic codes: `type-repo-not-a-dependency` (`add_dep`),
    `workspace-omits-containing-repo` (`add_workspace_member`),
    `discover-member-is-a-dependency` (`remove_workspace_member`), and the span-edit
    `type-repo-self` / `type-repo-empty` / `redundant-abstract-on-sealed`
    (`edit_file`). Every other code carries `description` only.
  - additive to the wire, no `schema_version` bump. See `WIRE.md`.

## 2026-09-16

- Composition mutation verbs, the governed write path for a repo's deps and a
  workspace's composition: `add_dep`, `remove_dep`, `add_workspace_member`,
  `remove_workspace_member`, `set_workspace_member_role`.
  - author `<member>/.arsumbris/repo.yaml` (`deps:`) and `.arsumbris/workspace.yaml`
    (`edit:` / `discover:`) through the channel, comment- and order-preserving,
    one commit per mutation, re-resolving the mounted set.
  - `add_workspace_member` creates `workspace.yaml` self-complete when absent.
  - `add_dep` does not fetch: the new peer stays `peer-unmounted` until `resolve`.
  - additive to the wire, no `schema_version` bump. See `WIRE.md`.

## 0.0.1-alpha

The first public release. Wire schema 29.
