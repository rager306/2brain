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
| tool names `au_*` | 52 | **done 2026-11-16** — renamed to `tb_*` without an alias (`docs/naming.md`) |

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
   gate is `tb_diagnostics == 0` on the live vault afterwards. This is the only tier that changes files
   users have on disk, so it gets its own ADR.

### Tier D — wire contract (alias period)

8. **Resource scheme** — **done 2026-11-16**: resources are emitted as `2brain://<member>/<path>` and the
   legacy `arsumbris://` is **accepted** on parse (a cached URI keeps working). Two tests pin it: emission
   uses the new scheme only, and both schemes parse to the same member/path. The drop is a later versioned
   change, and the fixture/golden regeneration belongs to it, not to this commit.

## Order and gates

| step | gate | rollback |
|---|---|---|
| repo rename | `git remote -v`; `gh repo view` | `gh repo rename arsumbris` |
| **state root + unit (tier B) — done 2026-11-16** | `2brain-engine.service` active, socket under `~/.2brain`, `2brain info` 3 members / 0 diagnostics, health `engine=1 bin=1 action=ok` | restore `/root/backups/arsumbris-engine.service.20261116`, `mv ~/.2brain ~/.arsumbris` |
| local dir | `au members /root/vault` (3 members), `2brain info` | `mv` back |
| dirs `au-*` | `cargo check --workspace`; `au members`; diagnostics 0 | `git revert` |
| prose | `git grep -i arsumbris` lists only dated records + README attribution | `git revert` |
| state dir | engine restarts, socket answers, one read + one write | symlink back |
| format | `tb_diagnostics == 0` on the vault after migration | restore the vault from git |
| scheme | both schemes answer; fixtures blessed | revert the commit |

## What stays spelled `arsumbris`, and why

- **README attribution**: the project is based on the upstream `arsumbris` project, with a link. One
  paragraph, and it is the only place the word survives in the tree.
- **Dated records** (`docs/architecture-assessment.md`, the swap runbooks, ADRs, `docs/w4.1-…`): they
  measured a named system on a date. Renaming inside them would make the numbers unattributable.
- **`au.engine.*` and `.arsumbris/`** until Tier C lands: they are a file format.

## Executed in this pass

See the commits after this file; the table above is updated as each tier lands.

## The trap this pass discovered

A directory rename must stop at the state root. `au-engine` means two different things:

- `tb-engine/` — a repository directory, renamed;
- `~/.arsumbris/au-engine/` — the device layout name the engine hardcodes (`run/`, `config/`, logs).

The first pass renamed both, because the rule was "a path segment". `swap-engine.sh --check` caught it
with a socket path that does not exist; 116 occurrences in 53 files were reverted. The symptom was
otherwise invisible: the running daemon still used the old binary, so every test stayed green while the
next build would have started with an empty state root and resolved zero members.

The rule for future passes: a rename may cross a repository directory; it may not cross a *storage
layout*, and the two are recognised by what reads them, not by their spelling.

## Recipes for the open tiers

### Tier B — state root and unit — **executed 2026-11-16**

The engine now resolves the state root as: `$2BRAIN_HOME` → an existing `$HOME/.2brain` → the legacy
`$HOME/.arsumbris`, so a machine that has not been migrated keeps working, and after the move both
spellings reach the same inode through a symlink.

```sh
systemctl stop arsumbris-engine.service
mv ~/.arsumbris ~/.2brain                       # sockets, config, logs, the installed `au`
ln -s ~/.2brain ~/.arsumbris                    # one release of compatibility
# engine must accept both: `$ARSUMBRIS_HOME` or the new default, tested by a unit test
systemctl start 2brain-engine.service           # after renaming the unit file + daemon-reload
HOME=/root 2brain info /root/vault              # 3 members, 0 diagnostics
```

Rollback: stop, remove the symlink name, `mv ~/.2brain ~/.arsumbris`, start the old unit. Gate: the
four reads in `verification.md` and one write, because the write path reads the registry.

### Tier C — the on-disk format

1. Engine release 1: accept `.arsumbris/` **and** `.2brain/` for the identity directory, and both
   `au.engine.*` and `tb.engine.*` for the schema types; `BUILTIN_ENGINE_REPO` matches either name.
2. A migration command (`au migrate identity`) that rewrites every `repo.yaml` this registry knows,
   committing per repository, and refuses when the tree is dirty.
3. Run it over the monorepo and the vault; gate on `tb_diagnostics == 0` and `graph_health.broken_links == 0`.
4. Release 2 drops the old spelling, with a diagnostic that names the migration.

Content that cites `type: mcp.skill::au-mcp-sdk` moves in step 2, in the same commit as the member
rename — a half-renamed tree cannot resolve.

### Tier D — the resource scheme

Serve `2brain://` and `arsumbris://` from the same handler for one release, publish the deprecation in
`WIRE.md`, then drop the old scheme and regenerate the golden captures in the drop commit. Fixtures that
compare bytes will fail on the drop unless they are blessed in the same change — the reason this is a
tier and not a sed.
