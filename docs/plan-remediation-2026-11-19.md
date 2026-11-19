# Remediation plan — the 2026-11-19 whole-tree review

Source: `docs/reviews/20261119-145319-441a7399/` (39 confirmed findings). This is the plan; the
review is the evidence. Each wave is a thin slice that can land and be gated on its own, in the
order the review recommends: data safety first, then boundaries, then contract, then payload.

## Rules

- **Debt and gaps before features.** A finding either gets a test that fails without the fix, or a
  dated decision that says why it will not be fixed. No silent "won't fix".
- **Decision gates are explicit.** A finding that changes a contract (an outcome the client must
  interpret, a page shape, a migration) is not applied automatically; it is decided here and then
  implemented.
- **No hardcoding.** Paths, names and budgets come from the existing constants (`ENGINE_RUNTIME_DIR`,
  `IDENTITY_DIR`, `SCHEMA_VERSION`) or from the engine, never from a literal copied into a second
  place. The rename already taught this: a literal in a script went stale the same day.
- **TDD.** The test asserts observable behaviour (a committed tree is not reverted, a page carries a
  usable continuation), not the implementation shape.
- **One wave, one commit, full gate run.** Engine `--lib`, `--test it`, `tb-cli --test it`, adapter,
  fmt, clippy, docs links, `legacy_audit --check`.

## Waves

### A — saga durability (in progress)

| id | finding | decision | state |
|---|---|---|---|
| #10 | a committed saga could roll back if marker removal failed | write a durable `settled` line before removal; recovery never rolls back a settled marker; a failure sentinel also stops auto-retry | **fixed, tests added** |
| #7 | a failed rollback deletes parked preimages | keep the marker and preimages; the sweep skips an id with a failure sentinel | **fixed, test added** |
| #9 | old runtime journals (`au-engine/run`) invisible | `install.sh` moves the state directory once; recovery reads only the current directory (one rule) | install path done |
| #4 | non-git `batch` is not atomic | decide: refuse a multi-op batch spanning a non-git member, or journal non-git paths | decision |
| #8 | recovery cannot see a pre-rename journal | covered by the same state-directory move; a remaining marker is refused rather than guessed | covered by #9 |

### B — access boundaries

| id | finding | notes |
|---|---|---|
| #5 | create through a symlinked parent escapes the member | canonicalize existing ancestors; verify at open |
| #6 | `tb_grep` reads outside-git symlink targets | exclude or resolve safely at catalog time |
| #16 | create wrapped in a parent bypasses the nested member's grant | authorize the actual owner |
| #17/#20 | symlink alias authorizes the lexical parent, writes the child | authorize the real target |
| #19 | symlink alias in wire grants | same rule as #17 |

Gate: a refusal leaves every file byte-identical, plus an MCP propagation test.

### C — reconcile and read contract

| id | finding | decision |
|---|---|---|
| #11 | a partial read allows a full overwrite | replace only after a full read; re-read on change |
| #14/#15 | reconcile reports `not_applied` without proof of history | introduce an engine-side mutation id / unknown outcome |
| #12 | a hung preflight holds the only write slot | apply the existing read timeout to grant preview |
| #27 | an IO error looks like NotFound | separate the two |
| #26 | delete can report `reflected` untruthfully | check the held catalog |

### D — MCP payload

| id | finding | decision |
|---|---|---|
| #13/#32/#33 | unbounded or post-projection paging (`tb_candidates`, `tb_instances`) | paginate in the engine, before projection; default bounded |
| #18 | `next_offset` cannot be used | add the `offset` argument |
| #34/#35 | output schema disagrees with the text payload / `verbs` | declare it or refuse the combination |

### E — search and tooling

| id | finding |
|---|---|
| #25 | an empty dictionary term hides a live form |
| #28 | excerpt does not show the matched form |
| #38 | `moc_links` counts links from other fields |
| #40 | `visit_bound` uses the old runtime layout |
| #42 | rollback test is unreliable under root |

## Decision gates (owner)

1. **#2 offline identity migration.** The cutover is irreversible by design (ADR-0005). For a vault
   that still holds `.arsumbris`, the options are: (a) document a manual `mv`, (b) ship a one-shot
   `mv` command that never reads the old name, (c) both.
2. **#4 non-git batch.** Refuse such a batch up front, or journal and restore non-git paths.
3. **#14/#15 outcome.** Introduce an engine-side mutation id that reconcile can query, or return
   `outcome_unknown` instead of `not_applied`.
