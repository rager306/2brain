# Tier C inventory: what the on-disk format change actually touches

Measured 2026-11-16 by grep over `tb-engine/crates/*/src`. Tier C of `docs/rebrand-plan.md`: move the
engine's identity directory `.arsumbris/` → `.2brain/` and its schema types `au.engine.*` → `tb.engine.*`,
in a way that a running system survives.

## The surface, counted

| what | occurrences | files |
|---|---:|---|
| the literal `".arsumbris"` (identity dir inside a repo, state root, ignore markers) | 79 | 12 |
| `"au.engine.*"` schema type names | 52 | — |
| `BUILTIN_ENGINE_REPO` references | 22 | — |

By file, the identity directory concentrates in `serve.rs` (17), `tb-parser/src/fs.rs` (15),
`pkgcache.rs` (15), `repo.rs` (12), `build.rs` (8).

The two kinds of `.arsumbris` must not be confused — this is the trap that already bit the directory
rename (`docs/rebrand-plan.md`):

- **inside a repo**: `<repo>/.arsumbris/repo.yaml`, `.auignore`, `workspace.yaml` — the identity marker.
  This is what tier C migrates.
- **under `$HOME`**: `~/.arsumbris/au-engine/{run,config}` — the device state root, already moved to
  `~/.2brain` in tier B with a legacy symlink. Nothing in tier C touches it.

## The schema type names

`au.engine.repo`, `au.engine.repo-lock`, `au.engine.workspace`, `au.engine.workspaces`,
`au.engine.workspace-lock`, `au.engine.repos`, `au.engine.readme`, `au.engine.meta`,
`au.engine.locked-package`, `au.engine.workspace-entry`, `au.engine.widget`, `au.engine.registry-entry`.
They live in `engine_schema.rs` as string matches, and `BUILTIN_ENGINE_REPO = "au-engine"` names the repo
those definitions belong to. Every repository's `repo.yaml` declares `type: au.engine.repo::au-engine`, so a
rename is a migration of **content**, not of code alone: 314 `repo.yaml` files in this monorepo, plus every
repository outside it that a user has.

## What a two-release migration looks like

Release 1 (accept both):

1. `identity_dir(root) -> PathBuf` prefers `.2brain/repo.yaml` and falls back to `.arsumbris/repo.yaml`;
   every direct `join(".arsumbris")` goes through it. `fs.rs` and `pkgcache.rs` need the same helper, because
   they classify files by path.
2. `schema_type_matches(name) -> bool` accepts `au.engine.*` and `tb.engine.*`; `BUILTIN_ENGINE_REPO`
   matches `au-engine` and `tb-engine`. A type name is matched in several places, so the predicate is the
   only safe seam.
3. `au migrate identity [--dry-run]`: for each repository in the registry, rewrite `type:` values, move
   `.arsumbris/` to `.2brain/`, commit per repository, refuse on a dirty tree. Prints what it changed.

Release 2 (drop the old spelling) with a diagnostic that names the migration.

## Gate

`au_diagnostics == 0` and `graph_health.broken_links == 0` on the live vault after migration, and the
pre-migration tree still opens with the same diagnostics using the release-1 binary. Rollback is
`git revert` in each repository (the vault is a git repo; identity files are committed).

## Ordering that keeps the tree buildable

1. helper + call sites (no behaviour change, tests unchanged);
2. accept-both matching in the schema layer, with a test that both spellings resolve;
3. the migration command, tested against a fixture repository first;
4. the monorepo migration in one commit, then the vault;
5. release 2 drops the legacy spelling.

## Not determined

- Whether `engine_schema.rs` matches type names by string equality everywhere or has a resolution layer
  already — the 52 occurrences suggest several independent matches, which is exactly why the predicate is
  the first step rather than a sed.
- Snapshot tests under `crates/tb-engine/tests/it/` that print paths would change with the directory rename;
  they were not enumerated.
