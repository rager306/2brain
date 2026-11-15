# Rebrand plan: from `arsumbris` to `2brain`

Written 2026-11-16, after the crate rename (`au-*` → `tb-*`) and before any directory or repo rename.
It exists because "rename everything" is four different operations with four different risk profiles, and
doing them as one find-and-replace breaks the live vault.

## What is actually in the name today, measured

| surface | count | can a rename be a find-and-replace? |
|---|---:|---|
| repo files mentioning `arsumbris` | 451 | no — the word appears in identifiers, paths, prose and dated records |
| vault notes/files mentioning it | 108 | no — content, and some of it is history |
| GitHub slug, local directory | 1 + 1 | yes, with reference updates |
| directories `au-*` | 15 | yes, with reference updates (member `name:` is a separate field) |
| engine state dir `~/.arsumbris/` | 1 | **no** — the engine refuses a repo rooted there and hardcodes it (`tb-engine/src/build.rs:61`) |
| engine schema types `au.engine.*` | 6 | **no** — hardcoded in `engine_schema.rs` and declared in every `repo.yaml` |
| systemd units | 1 (`arsumbris-engine.service`) | yes, with a restart |
| resource scheme `arsumbris://` | 1 | yes, but it is a wire contract: fixtures and clients compare it |
| tool names `au_*` | 52 | not in scope: `au` is the engine's short name, not the brand |

## The tiers

### Tier A — brand and layout (safe; no live format)

1. **GitHub**: `gh repo rename 2brain` (keeps redirects), then `git remote set-url`.
2. **Local directory**: `/root/arsumbris` → `/root/2brain`. References to update: the workspace registry
   (`~/.arsumbris/au-engine/config/workspaces.yaml`), `au-health.sh` (install path), `swap-engine.sh`,
   `install.sh` (its own location is derived), docs.
3. **Directories `au-*` → `tb-*`**, plus `arsumbris/` → `upstream/`. The member `name:` in
   `.arsumbris/repo.yaml` is the workspace identity; the directory is a path. Renaming the directory is
   safe when the registry is updated in the same change. Renaming a member `name:` is **not** Tier A —
   content cites `type: mcp.skill::tb-mcp-sdk`.
4. **Prose**: product name in docs, README, skill text. Dated records (ADRs, assessments, run logs) keep
   the word, because rewriting history makes the evidence lie.

### Tier B — live identifiers (needs a window)

5. **State directory** `~/.arsumbris/` → `~/.2brain/`: the engine hardcodes it as the device root, so it
   is an engine change plus a move of sockets, config, logs and the installed `au`; every client derives
   its socket from it. Do it with the daemon stopped, and keep a symlink from the old path for one
   release so an old client keeps working.
6. **systemd unit** `arsumbris-engine.service` → `2brain-engine.service`: stop, rename the unit file,
   `daemon-reload`, start; `au-health.sh` and the swap script name it.

### Tier C — engine file format (a migration, not a rename)

7. **`.arsumbris/` repo identity dir** → `.2brain/`, and **`au.engine.*` schema types** → `tb.engine.*`,
   including `BUILTIN_ENGINE_REPO = "tb-engine"`. These are the on-disk contract: every repository's
   identity file, the vault, and the type names that content cites (`au.engine.repo::tb-engine`). The
   engine must accept both spellings in one release, the migration must rewrite every `repo.yaml`, and the
   gate is `au_diagnostics == 0` on the live vault afterwards. This is the only tier that changes files
   users have on disk, so it gets its own ADR.

### Tier D — wire contract (alias period)

8. **Resource scheme** `arsumbris://<member>/<path>` → `2brain://…`: serve both for one release, publish
   the deprecation, then drop. Fixtures and the golden captures regenerate in the same commit as the drop.

## Order and gates

| step | gate | rollback |
|---|---|---|
| repo rename | `git remote -v`; `gh repo view` | `gh repo rename arsumbris` |
| local dir | `au members /root/vault` (3 members), `2brain info` | `mv` back |
| dirs `au-*` | `cargo check --workspace`; `au members`; diagnostics 0 | `git revert` |
| prose | `git grep -i arsumbris` lists only dated records + README attribution | `git revert` |
| state dir | engine restarts, socket answers, one read + one write | symlink back |
| format | `au_diagnostics == 0` on the vault after migration | restore the vault from git |
| scheme | both schemes answer; fixtures blessed | revert the commit |

## What stays spelled `arsumbris`, and why

- **README attribution**: the project is based on the upstream `arsumbris` project, with a link. One
  paragraph, and it is the only place the word survives in the tree.
- **Dated records** (`docs/architecture-assessment.md`, the swap runbooks, ADRs, `docs/w4.1-…`): they
  measured a named system on a date. Renaming inside them would make the numbers unattributable.
- **`au.engine.*` and `.arsumbris/`** until Tier C lands: they are a file format.

## Executed in this pass

See the commits after this file; the table above is updated as each tier lands.
