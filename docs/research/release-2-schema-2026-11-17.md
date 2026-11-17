# Release 2, tier C step 7: the engine schema moved to `tb.engine.*` and `::tb-engine`

**What changed.** The engine's hardwired schema namespace and the builtin repo identity were
renamed: `au.engine.*` → `tb.engine.*` and `::au-engine` → `::tb-engine`. The 12 hardwired type
defs (`repo`, `repos`, `workspaces`, `workspace`, `repo-lock`, `workspace-lock`, `readme`, `dep`,
`registry-entry`, `workspace-entry`, `locked-package`, `meta`) are now named in the new spelling, and
so is every floor name the file kinds assign.

**Why it is safe.** The legacy spelling is still ACCEPTED on every path:

- a file that writes `type: au.engine.repo::au-engine` is canonicalized forward on read
  (`canonical_schema_claim`), so a release-1 tree reads unchanged and reports no diagnostic;
- a READ argument is canonicalized too (`canonical_query_name`, new in this change): before it,
  `instances_of {type: "au.engine.repo::au-engine"}` answered **0 rows** while the same three
  instances were served under the new name. That is the regression the migration window exists to
  prevent, and it was found by comparing the two binaries on the live vault, not by a test.

**Measured.** On a copy of the live vault (`/root/vault`, 1 040 notes), the old binary
(`869f0ed9`) and the new one produce **byte-identical** answers for `overview`, `hubs`,
`diagnostics`, `graph_health`, `files` and `search` (0 diffs in all six; `version` excluded). Both
spellings of `instances_of` answer the same 3 rows; diagnostics 0.

Gates: fmt clean, clippy `-D warnings` clean, engine lib **744 / 1** (the root-only unreadable
fixture), `it` **734 / 3** (the three root-only fixtures). 242 scenario snapshots re-blessed: the
diff is 4 480 replacements of `au.` → `tb.` plus the `crates/au-cli` → `crates/tb-cli` header path,
with 32 further lines that are hashes and sort positions — `au.engine.*` sorted before the
knowledge base's own names, `tb.engine.*` sorts after them.

**What this is not.** The vault format's *file* spellings are unchanged in meaning: a note that
claimed `tb.engine.repo::tb-engine` before this change claimed the same identity after it. The
`DEFS` bodies now say `tb.engine.*` in their own field shapes, which is what makes the builtin's own
defs self-consistent.

**Remaining for release 2** (not in this change): dropping the legacy acceptance entirely, which is
what closes the migration window, and the adapter's own `au`-spelled fixtures.
