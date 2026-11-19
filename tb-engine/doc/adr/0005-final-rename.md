---
id: ADR-0005
title: The old name is removed in one atomic migration, then the legacy readers are deleted
status: Accepted
lifecycle: "[validated]"
date: 2026-11-19
supersedes: none
related: [ADR-0001]
---

# ADR-0005: finish the rename — one migration, then delete the legacy readers

## Context

`docs/rebrand-plan.md` moved the name in tiers and deliberately kept a compatibility window: the engine
writes `.2brain/`, accepts `.arsumbris/`, emits `tb.engine.*`, accepts `au.engine.*`, emits `2brain://`,
accepts `arsumbris://`, and the engine binary is still `au`. That window was right while clients were being
migrated. It is now the only thing left, and it hides the rest of the old name behind it: a reader that
accepts both spellings keeps the old spelling alive in every fixture, type definition and message.

`tools/kb/legacy_audit.py` is the measured inventory (2026-11-19, `/root/2brain`, 2667 tracked files):

| pattern | live | format | content | history | lineage |
|---|---:|---:|---:|---:|---:|
| operator env `AU_*` | 0 | 0 | 0 | 94 | 0 |
| tool name `au_*` | 38 | 0 | 0 | 48 | 0 |
| type `au.engine.*` / `::au-engine` | 0 | 306 | 0 | 191 | 0 |
| scheme `arsumbris://` | 0 | 11 | 0 | 12 | 0 |
| identity `\.arsumbris` | 0 | 1255 | 0 | 237 | 5 |
| repo `name: au-*` | 0 | 0 | 82 | 1 | 0 |
| wikilink `::au-*` | 0 | 0 | 1561 | 89 | 0 |
| bare product name in prose | 0 | 0 | 0 | 107 | 4 |
| engine binary `au` | 21 | 0 | 0 | 36 | 0 |

Plus **288** directories named `.arsumbris` on disk (12 repository identities, the rest scenario fixtures).

The classes are not interchangeable. **Live** and **content** must be renamed. **Format** is renamed
*and then the reader that accepts the old spelling is deleted*, which is what makes the rename
irreversible. **History** and **lineage** are never edited: an unresolvable record is worse than a frozen
one, and the lineage paragraph is the one place the upstream name belongs.

## Decision

One atomic migration, one deploy window, then delete the legacy readers. There is no phase in which half
the tree says `au-` and half says `tb-` on disk, because the identity name is a join key: a `deps: - name:`
that does not match the member's `name:` is a build error, and a wikilink that does not resolve is a
diagnostic. Every name, dep, registry entry and link moves together in one commit.

### Phase 0 — freeze and back up (minutes)

```sh
git -C /root/2brain bundle create /tmp/2brain-pre-final-rename.bundle --all
git -C /root/vault  status --porcelain            # must be clean
git -C /root/vault  bundle create /tmp/vault-pre-final-rename.bundle --all
/root/.2brain/bin/au members /root/vault           # record the resolved members
cp -a /root/.2brain/au-engine/config /tmp/config-pre-final-rename
```

Gate: vault tree clean; bundles written; member list recorded.

### Phase 1 — content identity (repository, in git, reversible)

1. `name: au-*` -> `name: tb-*` in the 16 `repo.yaml` files (and the 3 templates under `tb-defaults/`).
2. `deps: - name: au-*` -> `tb-*` (79 sites) and the `workspace.yaml` `edit:` entries (4 files
   with old template names; those templates are in-tree, not served).
3. `::au-*` / `[[... ::au-* ...]]` -> `tb-*` (1561 links; the vault itself has **0**, so this is a
   repository-only change and cannot break the served vault).
4. `au.engine.*` / `::au-engine` -> `tb.engine.*` / `::tb-engine` in the content repos (`tb-type-system`,
   `tb-mcp-sdk`, `tb-base-types`, `tb-writing-style`, the `repo.yaml` files): 306 sites.
5. Directory renames: `tb-defaults/au-*-template` -> `tb-*-template`; update the template registry the
   installer reads. Repository directories are already `tb-*`.

Gate: `cargo test -p tb-engine --lib`, `--test it`, `-p tb-cli --test it`; `tb-mcp-rs` tests; the
engine loads the tree with **0** diagnostics; `tools/docs/links.py` is 0 broken.

### Phase 2 — on-disk identity directories (repository, in git, reversible)

Rename all 288 `.arsumbris/` directories to `.2brain/` inside `tb-engine/` (scenario fixtures and the
12+3 repository identities). A fixture that *tests* legacy acceptance stops carrying a legacy directory
on disk and instead creates one at runtime in a temporary root, so the acceptance test survives the
directory rename and is deleted in Phase 3.

Gate: same suite; every scenario snapshot re-blessed only where the path appears in output; diff must be
the path spelling alone.

### Phase 3 — delete the legacy readers (product code, the irreversible step)

- `tb_parser::identity`: remove `LEGACY_IDENTITY_DIR`, make `is_identity_dir_name` accept `.2brain` only,
  drop the `.arsumbris` fallback from `existing_identity_dir` / `identity_dir_for_write_with`.
- `tb_engine::repo`: remove `LEGACY_STATE_DIR`; `state_dir_in` no longer falls back to `~/.arsumbris`.
- `tb_engine::engine_schema`: remove `LEGACY_ENGINE_NAMESPACE`, `LEGACY_BUILTIN_ENGINE_REPO` and the
  canonicaliser; `ENGINE_SCHEMA_LEGACY_SPELLING` diagnostic is removed with them.
- `tb_mcp_rs::resources`: remove `LEGACY_RESOURCE_SCHEME`; `arsumbris://` becomes an error.
- Delete the migration command's legacy branch and its `LEGACY_*` test fixtures, or keep `au migrate`
  only as a no-op that reports "already current".

Gate: the whole suite green; `tools/kb/legacy_audit.py --check` returns 0 (no live); a new unit test per
removed constant proves the old spelling now *fails* (not merely "is not preferred").

### Phase 4 — the engine binary's name (product code + packaging)

- `tb-cli/Cargo.toml`: `[[bin]] name = "au"` -> `name = "tb-engine"`.
- `install.sh`: build `-p tb-cli --bin tb-engine`, install `$bin_dir/tb-engine` and
  `$state_root/bin/tb-engine`; no `au` alias (irreversible means no shadow name).
- Every `CARGO_BIN_EXE_tb-engine` -> `CARGO_BIN_EXE_tb_engine` (21 sites).
- Socket/config directory `au-engine` -> `engine` under the state root; update `state_dir_in`
  consumers, the unit's log path, and the trace directory.

Gate: full suite; `install.sh` on a clean checkout with a fresh `$HOME`; `2brain --version`; MCP smoke.

### Phase 5 — the live cutover (window; owner go, no live Hermes session)

The runbook forbids restarting the daemon while Hermes sessions are live, so this is a scheduled window:

1. Confirm no Hermes session: the daemon's `shutdown` ack is the last read (the ADR-0003 path).
2. `systemctl stop 2brain-engine`; run `tb-engine migrate identity --apply /root/vault` on a **copy**
   first, diff the build against the frozen copy, then on the real vault.
3. Update `/etc/systemd/system/2brain-engine.service`: `ExecStart=/root/.2brain/bin/tb-engine daemon
   start /root/vault`, log path `logs/engine.log`.
4. `install.sh` (builds `2brain` + `tb-engine`), `daemon-reload`, `systemctl start`, `doctor`.
5. Golden reads: capture the same probes before and after; byte-identical or the window is aborted.
6. Remove `/root/vault/.arsumbris` (empty `au-engine/run`) and the `~/.arsumbris` symlink once nothing
   reads them; the engine no longer falls back to either.

Rollback: the two bundles from Phase 0 restore both trees; the previous binary stays at
`/root/.2brain/bin/au.bak` until the window closes green.

### Phase 6 — the ratchet

`tools/kb/legacy_audit.py --check` runs in CI and fails on any `live` trace. The `format` and `content`
counts are zero after Phase 5; `history` and `lineage` are the only non-zero classes and are asserted
**not** to grow (a snapshot of the counts lives beside the tool, refreshed only when a record is added).

## Consequences

- The rename becomes irreversible at Phase 3: a binary from before the window can no longer read a tree
  written after it, and a tree from before the window is refused when it carries `.arsumbris/`.
- The `au` name leaves the codebase, the install path, the unit and every script; a host that still calls
  `au` gets "command not found", which is the intended, honest failure.
- Every remaining `arsumbris` string is a dated record or the lineage paragraph, both provably not read
  by code, so the audit's `history`/`lineage` classes become the whole remainder.
- Cost: one big commit, one window, and a snapshot re-bless whose diff must be the path spelling alone.
- Risk: a name mismatch that is half-applied. Mitigated by making Phases 1-4 a single commit that the
  full suite must pass before the window opens.

## Alternatives rejected

- **Keep the window another release.** It is the thing hiding the rest: 1255 `.arsumbris` literals and
  306 `au.engine.*` claims exist only because a reader accepts them. Keeping it keeps the rename open
  forever.
- **Rename gradually, deploy per phase.** The identity name is a join key; a per-phase deploy leaves a
  served tree whose `deps` do not match member names. The one place partial is safe is *within* git, which
  is what the single commit gives.
- **A permanent `au` symlink.** It contradicts "irreversible" and keeps the old binary name in every
  script's reach.
