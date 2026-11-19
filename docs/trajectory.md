# Trajectory after the rebrand — what stands, what is open, what runs in parallel

Written 2026-11-16, revised the same evening after tier B, tier D, the wire refactor and the first measured
ranking change landed. This replaces "what next" with a queue where every open item names its gate, its
cost, and the evidence it would produce.

## Where the system stands

| | state |
|---|---|
| serving path | `/usr/local/bin/2brain` → engine socket; Rust only, one binary (`2brain mcp <target>`) |
| state root | `~/.2brain` (legacy `~/.arsumbris` is a symlink), unit `2brain-engine.service` |
| tools | 52 (33 read, 16 mutate behind three grants, 2 discover, 1 watch) |
| read contract | one `VERB_SHAPES` table plus typed payloads for the eight hot reads; both checked by the captured fixtures, not by memory |
| audit | hash-chained `mcp_policy` records on stderr; the tool name is recorded (fixed from the literal `mutation`), the principal comes from `--principal` and Hermes now passes `hermes` |
| judge | batched: 100 questions per call, 0.76 s, 7.6 ms/question (`docs/research/judge-capabilities.md`) |
| rebrand | tiers A, B, D done; tier C steps 1–4 done (identity seam, schema canonicaliser, `au migrate identity --apply`); 16 of 20 in-scope repositories carry `.2brain/`, the vault waits for a clean tree |
| tool names | **`tb_*`** — 52 tools, renamed from `au_*` on 2026-11-16 without an alias (`docs/naming.md`); the `_meta` key and the docs followed |
| migration | **16 of 20 in-scope repositories migrated** to `.2brain/`; the two served members moved in a window with byte-identical live reads; the vault waits for a clean tree |
| FSM run log | done — `tools/fsm/check_run.py`, seven rules, `--selftest` proves each bites |
| hygiene pass | done and applied — `tools/hygiene/pass.py`, `inbox/hygiene-2026-11-16.md` |
| ranking | coverage weighting + **proximity** landed and measured; freshness prior rejected, its retry now has a query class and a decision rule |

Gates that must stay green on every change: the adapter suite (256 tests on 2026-11-17, the answer-contract
lane), the engine's `--lib` and `--test it` suites (730 and 729 at the candidate-set lane; three root-only
permission fixtures are the known environmental failures), `fmt --check` and `clippy --all-targets -D warnings`
on **both** halves, and `scripts/live_parity.py` against the installed binary. `tools/cards/metrics.json` carries
dated counts; a number quoted without its date is a memory, not a measurement.

## 2026-11-19 — two access boundaries from the review, closed with tests

The review's first recommendation was to close #7, #16 and #21 before anything else, and two of them
were small enough to do honestly in one pass.

**#7 — a read could leave its member.** `Request::Content` took a path, joined it to the root and read
the file. Nothing checked that the path was inside a member at all, so an absolute path anywhere on the
host was readable, and a symlink inside a member that pointed out was followed. The read now resolves
through `RepoMap::repo_of` and then, when the file exists, canonicalizes it and refuses when the REAL
path leaves the member root. An absent path is still a null read, so a create flow is unaffected. Two
cases are pinned in `reference_reads`: an absolute path outside every member, and a symlink that leaves
one; a real member file still reads.

**#21 — the grant named the wrong member.** The adapter checked `permits_member(repo)` on the member in
the arguments, then asked the engine to resolve the path. A path that reaches into a nested editable
member belongs to that member, and the engine writes it there — so a policy granting the parent
authorised a write inside a child it never listed. `arm_file` now asks `resolve_member` who owns the
joined path and refuses when the owner is not the named member and not in the allowlist. It asks only
when the policy HAS an allowlist, because without one every member is permitted and the exchange would
be pure cost; that also keeps the wire sequence of every policy-free launch exactly as it was. The
exchange lives in `wire.rs`, and the call site is six lines — the one place this session raised the
`lib.rs` ratchet rather than paying it down, with the reason recorded beside the constant.

**#16 — `rename` now carries the grant.** A rename rewrites every inbound referrer, and a referrer can
live in another member; the engine checked editability but never the caller's grant, so a policy
granting `ra` still rewrote and committed links in `rb` and `rc`. `rename` accepts the optional
`allowed_members` the adapter takes from the policy, and the engine checks it against the saga PLAN it
is about to write, under the write lock — the rule ADR-0003 set for `rename_type`, now in a shared
`grant_refusal` that any saga can call. The test drives the three-member workspace: refused with the
member names when the grant covers only `ra`, and it lands when the grant covers all three. `inline` and
`promote` are the same shape and remain open — they need the same field on their args and the same call
before their saga.

## 2026-11-19 — an external review's forty findings, triaged and started

A full external review landed 40 confirmed findings (22 P1, 18 P2) against `c5afba6`. I re-verified
them against the tree instead of taking the list on faith, and split them into three groups.

**Already closed by this session's own waves** — the fixture socket lookup (#13), and the state-root
resolution that made several tooling findings real in the first place. Re-checking paid: the review was
written against a revision two commits behind the tree.

**Tooling and contract, fixed in this wave.** The verification scripts resolved Cargo's target
directory by hand (`tb-engine/target`, `tb-mcp-rs/target`) though the shared directory is
`.cargo-target`, and one of them trusted a symlink that is not tracked (#30, #31, #33); a shared
resolver asks `cargo metadata` now. Fixture daemons outlived a killed runner and made the next run skip
starting one (#32). `--payload structured` never armed the read gate because `served_hash` read only
the text block (#19). One file-addressed read charged two read permits, so eight concurrent reads
exhausted a limit of eight with four in flight (#36) — proven by a test that fails with "capacity
reached" when the second charge is put back. A batch that writes one path twice compared every
intermediate hash against a catalog that can hold only the last, so `reflected` was false forever
(#28). The census now labels its link counts as syntactic regex counts rather than graph facts (#40).

**NOT fixed here, and say so plainly.** The access-boundary findings (#7 symlink read escape, #16
cascade allowlist on the other refactor verbs, #21 nested-member owner check) and the saga data-loss
findings (#6, #9, #10, #11, #22) are real and open. They change `serve.rs`'s recovery machinery and the
policy surface, which needs a deploy window and real-engine tests, not a pass at the end of a long
session. The remaining P1/P2 items are likewise open; the report at
`docs/reviews/20261119-024022-72fef655/` is the queue.

Two Ratchet notes: the `lib.rs` budget held without being raised — the read-permit helper and a
`file_read_guard` that replaces five identical argument-guard blocks paid for themselves — and the
`tb-cli` it suite stays 36/0.

## 2026-11-19 — what is left of the old name, measured

Asked to check for leftover `arsumbris` / `.arsumbris` / `au_` traces, I measured rather than grepped and
sorted the hits. The result is a clean split between three things that look alike and are not.

**Legitimate and staying.** `LEGACY_STATE_DIR` / `LEGACY_IDENTITY_DIR` / `LEGACY_ENGINE_NAMESPACE` exist so
a machine that has not migrated keeps working — that is the point of a release window, not a leftover. The
hard floor lists both directory spellings, the migration command's messages name both by definition, and
`arsumbris://` is accepted on parse while `2brain://` is what the adapter emits (tier D). Historical
records — `CHANGELOG.md`, this file, `docs/internal/`, `docs/research/`, `upstream/` — describe what was
true then and are not edited.

**Broken, and now fixed.** The verification scripts joined `$HOME/.arsumbris/au-engine/run` by hand. A
fresh `$HOME` writes `.2brain`, so each of the five would have missed the daemon it had just started — and
one of them, `capture-mutation-fixtures.py`, was still authoring `au.engine.repo::au-engine` claims, making
it the last writer of the legacy schema after the roadmap said there were none. `lib/mcp_stdio.py` now owns
`state_run_dir` and `newest_socket`, which find the run directory instead of spelling it, and the fixtures
are authored in the current spelling. `scripts/run-known-root-tests.sh` had the sibling bug: it globbed
`au_engine-*` for the test binary, which has been `tb_engine-*` since the rename, so it never ran a test and
always exited "binaries not found".

**Not this wave, by design.** Twelve repositories in this tree carry `tb-*` directories whose `repo.yaml`
still says `name: au-*`, and 1,052 wikilinks (1052) target those names — `[[type-def::au-type-system]]` and
its siblings. Renaming them is a *format* migration of the live vault, not a spelling fix: the live
workspace mounts `tb-mcp-sdk` as `au-mcp-sdk`, so the identity tree, the registry entries and every link
move together under one window. That is tier C's remaining step, and doing it as a drive-by edit would
break every address at once.

Documents that stated the legacy path as the current fact were corrected: `WIRE.md` (the socket path, the
device config path, the identity-directory write guard), `tb-engine/README.md`, both crate READMEs, and
`docs/build-layout.md` / `tb-mcp-rs/docs/{connecting,verification}.md`, which still named `au-engine`,
`au-cli`, `au-mcp-rs` as the crate names. `WIRE.md` now states the state-root rule once, by reference to
`repo::state_dir_in`, instead of repeating a literal path in twenty places.

## 2026-11-19 — the tests nobody ran, and the identity directory a mutation invented

Started from a question about stubs rather than a plan: are there `todo!()`s, dead handlers, declared
arguments that do nothing? There are none — the `not-yet-implemented-shape-feature` diagnostic is a real
answer for syntax the grammar does not recognise, not a placeholder. What the search did find was worse,
because it was invisible: **`tb-mcp-rs/tests/real_engine.rs` is `#[ignore]`d, and all three of its tests
were failing.** CI never ran them, so nothing reported it, and the failures were unrelated:

- they called the tools as `au_overview`, a name the adapter has not had since the `tb_` rename — every
  call answered "unknown tool";
- they joined `$HOME/.arsumbris/au-engine/run` to find the private engine's socket, which a fresh `$HOME`
  stopped using in the previous wave;
- their recorded read fixtures still carried `au.engine.*`, from before the schema rename.

Each is the same shape of mistake the last two waves kept finding: a test asserting *how* the engine
answers instead of *what* it promises. The run directory is now found rather than spelled, the tools are
called by their real names, and the fixtures are re-captured; capture and verification agree in one run.
The same literal in `tb-cli`'s `daemon.rs` was the last failing scenario test there — 33/36 before, 36/36
after — which also showed that my gate set had been missing `cargo test -p tb-cli --test it` entirely.

The stub search then paid for itself twice more. `saga_marker_dir` joined a literal `.arsumbris`, so the
first mutation of a repository whose identity directory is `.2brain` created a *second* identity
directory inside it. The write rule already existed (`identity_dir_for_write_with`: follow what is there,
else the current name) and was simply not used at this one call site. And `<au-engine-builtin>`, the
sentinel every builtin type-def reports as its `origin_path`, was the last `au` name on the wire; renaming
it re-blessed 210 `tb-cli` snapshots, whose diff is that string plus insta's `assertion_line` header,
removed by the script so the repository keeps its snapshot shape.

Measured after the wave: engine `--lib` **752** / 1 known root-only, `--test it` 741 / 3 known,
`tb-cli --test it` **36 / 0**, adapter 262 / 46 suites, and the three ignored real-engine tests pass on
demand. `fmt` and `clippy -D warnings` clean in both halves, links 808/0.

## 2026-11-18 — `require_clean` reaches `batch`, and three copies become one

ADR-0002's last verb. The blocker was stated in `WIRE.md` as a missing report: a batch refusal must name the
operation that would introduce the error, and no such report existed. It turned out to be one indirection away.
`mutate::compute_edit` is the pure transform behind `edit_file`, already shared with the preview, so a batch's
would-be set can be composed in request order through it — an edit sees the content an earlier op would have
left — and previewed through `PreviewInput::Set`, the seam the move and folder ops already use. A diagnostic's
file then maps to the LAST op that touched it, which gives the attribution: `batch op 2 (write_file <path>):
<code> in <path>`, and a diagnostic on a file no op touched is named `elsewhere in the tree` instead of being
blamed on an innocent op. Preview and write read the same held version because both happen inside one
`handle_mutation` under the write lock — the two-call race the ADR refuses does not exist here.

The second half of the wave was the adapter. Four tool handlers each spelled out "add `require_clean` when the
caller set it", three of them with the same comment. That is one rule in four places, so it became
`wire::with_clean_rider`. The architecture ratchet caught the duplication before I did — `lib.rs` grew eight
lines and the budget refused it — and after the dedup the same ratchet passed with the budget untouched.

The wave also surfaced three stale spellings that the earlier state-root fix made wrong: the adapter's `--help`
named `~/.arsumbris`, eleven tool descriptions named `.arsumbris/repo.yaml` and `workspace.yaml`, and the launch
diagnosis test asserted the legacy name outright. The last one is the interesting failure: a test that hardcodes
which spelling the engine chooses cannot survive the engine choosing differently, and it was asserting the very
behaviour the previous wave removed. It now reads the root the diagnosis names and checks the socket lives in
that root, which is the invariant either way.

Measured after the wave: engine `--lib` 749 / 1 known root-only, `--test it` **741** / 3 known (two new), adapter
**262** / 46 suites, `fmt` and `clippy -D warnings` clean in both halves, links 808/0.

## 2026-11-18 — the installer on a clean checkout, and the state root a fresh home gets

`install.sh` was repaired earlier today to ask `cargo metadata` for the target directory, and nothing had
run it on a tree with no build output. A detached worktree at `b376e86` was that tree: no `.cargo-target`,
no `tb-engine/target`. The script installed both binaries and five templates, and the acceptance on an
isolated `$HOME` passed — `init`, a daemon, `doctor` with **7 ok / 0 warn**, and one MCP session that read,
created, re-read the hash, wrote under it, and was refused once by `require_clean` with `policy/diagnostics`
and its code. That session is now `tools/probe/mcp_smoke.py`; the run is
`docs/internal/install-from-checkout-2026-11-18.md`.

The run also caught what the earlier checks could not. On a **fresh** home the daemon wrote
`~/.arsumbris/au-engine`, because the state root's rule had an existence rule and no write rule: an
existing `.2brain` won, an existing `.arsumbris` won next, and a home with neither fell back to the legacy
name. So a new installation would have been spelled the old way forever, and the tier-B recipe in
`docs/rebrand-plan.md` only ever worked because an operator ran its `mv` by hand. The fix is one branch:
neither spelling means the home is new, and a new home is `STATE_DIR`. The reserved-root check had the same
gap from the other side — it compared only `<root>/.arsumbris` against the device root, which is precisely
the pairing a `.2brain` device root and a `.2brain` repository identity produce, so the newest layout was
the one collision it could not refuse. Both spellings are compared now.

Measured after the change: engine `--lib` 749 passed / 1 known root-only failure, `--test it` 739 / 3 known,
`fmt` and `clippy -D warnings` clean, and a fresh `$HOME` end to end — `2brain init`, `au daemon start`,
`2brain doctor` — resolving `~/.2brain/au-engine/run/…` with 7 ok and no legacy directory created.

## 2026-11-18 — history rewrite: the bot co-author trailer is gone

The owner asked that no commit in this repository carry a `Co-authored-by: CommandCodeBot` trailer. Sixty-one
commits did — nine unpushed ones and fifty-two already on `origin/main` — so the branch was rewritten with
`git filter-branch --msg-filter`, which strips that line and normalises trailing blank lines. **Content is
unchanged:** `main^{tree}` is the same hash before and after, and the commit count is the same 472.

Two consequences are recorded here because they are not obvious from the tree:

- **Every rewritten commit has a new SHA.** Ten documents cited an old one as evidence; each citation was
  remapped to the rewritten commit with the same content, and a scan for stale short hashes now finds none. That
  includes four `docs/research/*` notes, which are otherwise frozen — a reference that cannot be opened is
  worse than the freeze.
- **The old objects exist in a bundle at `/tmp/2brain-pre-trailer-clean.bundle`, outside the repository.** It is
  the only copy of the pre-rewrite SHAs; delete it when the rewrite is accepted. GitHub keeps the old commits
  reachable by SHA for a while after a force-push, and nothing here can change that.

Nothing else about authorship changed: author and committer are the repository owner in every commit, before
and after.

## 2026-11-18 — the external review's findings, checked in code

A review of `f5c178f` made four claims that could be checked rather than argued, and each is now settled:

| claim | what the code showed | what changed |
|---|---|---|
| the cascade grant is checked by the adapter, not by the write | true — `rename_type` sent only `old_name`/`new_name`, so the engine re-planned on a state nobody had checked | **ADR-0003**: the mutation carries `allowed_members` and the engine refuses under the write lock; a two-member test forces the cascade across the grant |
| CI is red because `init` still writes the release-1 schema | true at that SHA — `tests/init.rs` and `tests/doctor.rs` were failing, exactly as the review read them | fixed earlier in this same session (`89486f4`), with a template walk that fails on any `au.engine.` |
| `install.sh` looks in a directory a clean clone does not have | true — `tb-engine/target` is ignored by `tb-engine/.gitignore`, and the build writes to `.cargo-target` | the script asks `cargo metadata` for the target directory; verified on a fresh worktree |
| `yield_maps` demotes the map on the wrong inequality | true — the code is `map >= best`, the sentence says `<=` | the measured case was a tie, so no run distinguished them; the rule is now one function with both unequal cases pinned, the comment states the implemented rule, and `docs/research/map-yield-contract-2026-11-18.md` records the correction |

Two of the review's judgement calls are worth keeping in the register: a context pack should be measured by whether
its fragments can support the answer, not by whether the right file names came back; and `promote-with-checks`
should not be confused with the existing `tb_promote`, which lifts an inline record into its own file.

## 2026-11-18 — the debt wave: what a failing suite found

Four thin slices, each red first: the `--help` surface sentence counts the capability table instead of typing
"three mutating tools"; the mutation half of the catalogue test exists (`NOT_EXPOSED_MUTATIONS`, eight rows with
reasons); `2brain init` writes the canonical schema, so a fresh workspace opens with zero diagnostics; and the
README diagnostics no longer advise the release-1 spelling.

A second wave closed the two debt rows that wave left and recorded the shape of the one it could not:
queue item 10 (the four starters still shipping a `.arsumbris/` identity directory) is shipped, with the
template test now asking both questions — no `au.engine.*` and no legacy directory; item 11 (`2brain` did not
re-export `au migrate identity`) is shipped with a dry-run test on a legacy fixture. The ADR directory lost its
three-`0001-` collision: the plan and the inventory moved to `tb-engine/doc/plans/`, and the index stopped
calling ADR-0001 proposed. **ADR-0002** fixes promote-with-checks as an optional `require_clean` rider refused
inside the engine, and stays `proposed` until its three-part gate passes — the write contract does not move on a
sentence.

The find is the part worth keeping: **`tests/init.rs` and `tests/doctor.rs` were red at `f5c178f`**, and nobody
saw it because the window advisory landed after the last full adapter run. A fresh workspace carried two
`engine-schema-legacy-spelling` hints, and `doctor` reported a warn on it. Five shipped templates spelled the
same names. This is what the roadmap's "no writer emits it any more" line claimed and the tests disproved; the
line is corrected in CHANGELOG, and the template *directories* followed in the second wave once the test asked
about them too.

## 2026-11-17 — reconciliation: what moved since this page was last revised

The table above and the queue below are the 2026-11-16 state. After them, four lanes landed and one run answered
the Stage 4 question; the blocker moved with them. What is left is no longer "build the mechanisms" but "ship them
and decide the four field-level questions the mechanisms exposed". Where this section and a later section disagree,
this one wins.

### What landed, with the document that proves it

| lane | what shipped | evidence |
|---|---|---|
| visit bound | `documents_visited` / `posting_blocks_decoded` per verb, served by `graph_health { verbs: true }` and exposed through the adapter (`GraphHealthArguments.verbs`); `tools/probe/visit_bound.py` at 256 / 512 / 1 066 notes | `docs/research/visit-bound-2026-11-16.md` — no read grows faster than its candidate set; `hubs` is the one outlier (log-log 1.53) and its fix would be a build-time ranking, not a cheaper scan. The "last hop" this page used to name is closed: a client can read the counters |
| candidate set first | `DocMask` in `search.rs`; `serve.rs` deletes the post-ranking per-hit repo filter | `docs/research/candidate-set-first-2026-11-16.md` — 0.92× median on the served workspace, 0.70× on a 9-member model, answers byte-identical |
| answer contract | MCP `prompts` `recall` / `intake` / `handoff` and resources `2brain://workflow/{answer-shape,write-gates}`, static `contract.rs` | `docs/research/answer-contract-2026-11-16.md` — `tools/list` byte-identical, `+11` bytes to `capabilities`, adapter 256 tests |
| claim register | `type: claim` carrying `evidence`, `type: decision` carrying `backed_by`; 13 notes, 20 edges | `docs/research/claim-register-2026-11-16.md` — decision → claims → measurements is a two-hop typed walk |
| cross-agent handoff | the Stage 4 criterion, run on the live vault: two of three passed | `docs/research/cross-agent-handoff-2026-11-16.md` — the vault caught its author: a decision recorded as landed was not in the served binary |

Two rules came out of these lanes and belong in every later one:

- **a decision recorded before it is deployed is `status: proposed`**, flipped to `accepted` in the same change as the
deploy — the cross-agent run is what put that in the vault;
- **a decision about a field is a decision, not another pass over the notes.** `family:` and `claim` each turned out
to carry two meanings; neither is fixed by writing more notes.

### The reconciliation step that comes before any window

No deploy list assembled from documents should be trusted. `docs/internal/deploy-2026-11-16-batch.md` predates the
counters, the candidate mask, the rare-term floor and the answer contract, and a swap that runs `swap-engine.sh`
installs the engine half only.

```sh
2brain --version                                             # adapter commit
HOME=/root 2brain info /root/vault                           # engine build id and version
cd /root/2brain/tb-mcp-rs && python3 scripts/live_parity.py  # exit 2 is "could not compare", not a pass
git -C /root/2brain log --oneline <served-commit>..HEAD -- tb-engine tb-mcp-rs
```

The output is a `served | in repo | pending` table, written into the deploy document; the window checklist is
regenerated from it. A pending item whose behaviour a client can observe needs its verification line written before
the window, not after.

**Run 2026-11-17.** The served engine behaves as the tree at `1ee2463` (proximity): `search` answers but defaults
`scope` to `all`, and `weak`, `excerpt`, `verbs` and the word-form lookup are all absent. The served adapter is
`0.1.0+be4a56aaaa6a` (`be4a56a`). Pending at that moment: **22 engine commits** (`1ee2463..HEAD`) and **20 adapter
commits** (`be4a56a..HEAD`); the measured table and the per-item evidence are in
`docs/internal/deploy-2026-11-16-batch.md`.

### The lane register

Group labels are the parallel groups in the next section. A lane names its gate before it starts; a lane that cannot
name its experiment does not start.

| lane | state | owns | gate | group |
|---|---|---|---|---|
| **R** reconcile served ↔ HEAD | **done 2026-11-17** — re-run before the window | `scripts/live_parity.py`, deploy doc | the `served / in repo / pending` table | P0 |
| **S1** title as a scored field | **shipped 2026-11-17, index route rejected** — six indexed variants measured and rejected: a third field changes `matched`, the floor and coverage (`docs/research/task-set.md` §4). What shipped is post-ranking and cannot touch the index: title coverage (≤2×, rare terms only) plus a map yield (½) for a query of three or more distinct terms. Two boundaries were measured and rejected first — a blanket map discount (the exact answer is a map) and the wire `exact` flag (the task is called exact, the request is not) | `search.rs`, `serve.rs`, `tools/ranking/` | `hub-vs-atomic` 8 → **1/1**, every other class unchanged | done |
| **S2** `search_read` rarity premise | **done** — the test is now `three_distinct_terms_need_two_of_them_unless_the_one_held_is_rare` (`tests/it/search_read.rs:277`) | `tests/it/search_read.rs` | the renamed test states the rule the engine implements | — |
| **S3** `tb_context` | open — only if it beats 1 call / 2.44 KB | `serve.rs`, `lib.rs` | the twelve-task pack | P2, after S1 |
| **G1** graph fragility: bridges and articulation points | **landed 2026-11-17** — additive `graph_health` fields on the undirected projection; the sparx-derived idea that is a new diagnostic, not a new engine. On the 1 228-node / 4 730-edge snapshot: **37 bridges, 57 articulation points** | `serve.rs`, `schemas.rs`, `WIRE.md` | unit test (path+triangle, lone node); synthetic fixture and golden `graph_health` re-captured; engine fmt/clippy green, lib 741/1 and `it` 734/3 (the documented root-only fixtures); adapter fmt/clippy/tests green | P2 (engine, serial with the adapter) |
| **G2** graph weaving: triangles and transitivity | **landed 2026-11-17** — `graph_health.clustering = {triangles, transitivity}` on the same projection; snapshot: **3 123 triangles, transitivity 0.1816** | `serve.rs`, `schemas.rs`, `WIRE.md` | unit test (triangle + pendant edges → 1, 0.5; empty/one edge → 0.0); fixtures re-captured; same gates as G1 | P2 (engine, serial) |
| **G3** structural centrality, offline | **landed and measured 2026-11-17** — `tools/graph_centrality.py` (PageRank + Brandes betweenness, `--selftest`); on the live export it overlaps degree 5/10 for each, and betweenness adds low-degree connectors (`pattern-preregistered-quality-gates`, `jevact-…`, `qwen-office-2-team-agent`). **Kept offline**: PageRank belongs to the ranking harness, betweenness would add O(V·E) to every read (`docs/research/graph-centrality-2026-11-17.md`) | `tools/` | selftest; the comparison is published; no wire change | P1 (tooling, no cargo) |
| **M1** task-set thresholds and graded gating | **decided 2026-11-17** — the table is 18 rows over 11 classes (1–3 per class), so no percentage has a sampling error and a bootstrap over non-independent rows is arithmetic without a model. The rule is per row: the named rows pass, no other row moves, a claim names the rank not a percentage, and repeated runs of one deterministic binary are not variance — the pilot's 12 fixtures carry the variance | `docs/research/task-set.md`, `tools/ranking/` | every gate this session already follows it (S1: three pre-registered rows) | P1 |
| **C1** `family:` semantics | **decided 2026-11-17** — hybrid rule accepted (`docs/research/family-field-2026-11-17.md`); `tools/kb/family_audit.py` landed and its first run reports 429 values, 345 singles (159 provenance, **186 topic-shaped**), only 2 name collisions | type defs, `intake-decisions` | the 186 topic-shaped singles are the intake queue; a weekly delta is the gate | P1 |
| **C2** `claim` type collision | **decided 2026-11-17** — the queue item becomes `ingest-task` (`docs/research/claim-type-2026-11-17.md`); **blocked on the external queue writer**, which is not in this repository and must change in the same step | `type/claim.type.yaml` | `instances_of claim` needs no `evidence` filter | P1 |
| **C7** one finding recorded twice | **measured and merged 2026-11-17** — two ingest passes of the same sources (RU 2026-02-22, EN 2026-02-27) left **7 duplicate pairs**; each pair was read by hand, three losers first gave up a fact the winner lacked, then 29 wikilinks were redirected and the 7 losers deleted: notes 1 047 → 1 040, edges 4 895 → 4 856, isolated 197 → 196, diagnostics 0. `tools/kb/duplicate_findings.py` (`--selftest`) excluded `claim`+`evidence` pairs after its first version flagged eight legitimate ones | `tools/kb/duplicate_findings.py`, `docs/research/duplicate-findings-2026-11-17.md` | a second pass; a `superseded` field if pairs keep appearing | P2 |
| **C6** topic maps link half their members | **measured 2026-11-17** — `tools/kb/moc_links.py` (`--selftest`): **372 of 732** declarations (a note naming a map) are not linked from that map's body; **31 of 52** maps have a gap (`index` 63/63, `agentic-architecture` 51/66, `memory-systems` 50/71, `jev` 39/61). The graph is correct — `references_in` returns the reverse list exactly — so a generated `## Named by` section was written, measured and **reverted**: it made the map body a second copy of the frontmatter index and moved 372 edges without a claim. **Pass 1 done** (vault `620ffba`): the three largest maps link every namer — `agentic-architecture` 52 → 0/66, `memory-systems` 50 → 0/71, `jev` 39 → 0/61; vault total 372 → **232** unlinked, 31 → 28 maps, edges 4 754 → 4 895, diagnostics 0. **Pass 2** (vault `3172c2d`): five more maps to zero (`agent-security`, `rust-ai-systems`, `rust-ecosystem-kb-map`, `self-improvement`, `search-patterns`), total **232 → 155**, 28 → **23** maps; **Pass 3** (vault `d009c9f`): `tool-use` 32 → 0/42, the last large gap; total **123** across 22 maps, diagnostics 0. **Pass 4** (vault `1bf9c92`): `zeroclaw-decisions` 17 → 0/59, and `index` settled by measurement — 53 of its 63 namers also name a real map, only 10 used it as their only topic, and those ten gained the map that covers them, so `index` stays a curated front page with nothing stranded. **Pass 5**: `human-feedback` 5 → 0/7, total **102** across 21 maps (73% of the original 372); everything left is ≤ 4 notes per map except `index`, which is deliberate. The report ships; the generated-section write stays rejected | `tools/kb/moc_links.py`, `docs/research/moc-coverage-2026-11-17.md` | `--check` gates the maps touched (top three in one pass), not the vault | P2 |
| **C3** intake edges that point AT isolated notes | **measured and started 2026-11-17** — `tools/kb/intake_targets.py` (`--selftest`) narrows `graph_shape { orphan_paths }`: **33 content notes with no inbound**, 99 proposals, 24 at ≥3 terms. Three passes applied **23** of the 33 content notes (vault `850e89f`, `5fff55d`, `6a843e4`), each from a source that states the target's subject or whose pattern names it as provenance: edges 4 730 → **4 754**, content no-inbound 379 → **356**, isolated 197, diagnostics 0. Ten are refusals with the reason (five share only a category, five have no candidate and no source naming them), not silent drops | `tools/kb/intake_targets.py`, `docs/research/intake-targets-2026-11-17.md` | 33 → 24 actionable → 9 linked, 15 named refusals | P3 |
| **C4** aliases | **measured 2026-11-17, not adopted** — `tools/kb/alias_audit.py` (`--selftest`) pairs an acronym with a same-note expansion whose initials spell it: 2 943 acronym-shaped tokens, 1 222 in ≥2 notes, **285 candidate pairs, 272 divergent**; nine of the top fifteen read as real (`SoT`/source of truth: 15 vs 24 notes, 4 shared). Automatic pairing is at best six-in-ten precise, so no `aliases:` field and no query expansion yet; the task is in the set (`alias`, `SoT` → `knowledge/ontology-as-single-source-of-truth-code-never-primary.md`) and **passes 1/1**: query-time expansion was measured and rejected (15 → 132 candidates, expected note 21st), the corpus-side fix — the acronym in the note's own description — moved it from absent to rank 6 (vault `c85ec01`), and the row was re-registered to accept the page (`expect: any`, the bilingual precedent) with both measurements recorded before the edit | `tools/kb/alias_audit.py`, `docs/research/alias-field-2026-11-17.md` | the class is sized and the mechanism is a decision, not a guess | P1 |
| **C5** tier-C vault migration | **done 2026-11-17** — `.2brain/`, two claim lines, commit `a8aba4b`, 0 diagnostics | `au migrate identity --apply` | diagnostics 0, `broken_links` 0 | P4 |
| **D** the deploy window | **done 2026-11-17, two swaps** — morning: engine `869f0ed9` + adapter `0.1.0+55fab3f`; afternoon: engine `a6f9a3f4` (S1 ranking), adapter unchanged because the wire shape did not move. Logs: `deploy-2026-11-17.md`, `deploy-2026-11-17-s1.md` | binaries, deploy docs | 4 of 5 reads identical on a frozen copy, `search` differs only in rank/score/excerpt; live vs candidate copy 0 diffs; live task set `hub` 1/1 | P5 |
| **DOC** architecture clarity | **done 2026-11-18** — three documents and a decision store: `docs/architecture.md` (one page: invariants, crate layers, request path, write contract, tool groups generated from `#[tool]`), `docs/README.md` (the map of 82 documents in three kinds, 88 links checked), `.repowise/decisions.yaml` (5 accepted decisions with rejected alternatives and evidence; `repowise why` answers from it). The audit corrected two of my own claims: `tb_edit_file`'s `expected_hash` is a span precondition by design, and the write-contract review already existed | `docs/`, `.repowise/decisions.yaml` | `repowise decision list` / `health` / `doc-drift`; every link resolves | P0 |
| **R2** release 2: the format converges | **shipped 2026-11-17, not deployed** — two steps: `CREATE_IDENTITY_DIR` → `.2brain` (a new root is created with the new name), then the schema flip `au.engine.*`/`::au-engine` → `tb.engine.*`/`::tb-engine` for all 12 defs, every floor name, `ENGINE_META_TYPE` and `BUILTIN_ENGINE_REPO`. Legacy spelling still ACCEPTED on read, and a read ARGUMENT is canonicalized too (`canonical_query_name`) — without that seam `instances_of au.engine.repo::au-engine` answered 0 rows while the same instances were served under the new name. Measured on a copy of the live vault: old and new binaries answer `overview`/`hubs`/`diagnostics`/`graph_health`/`files`/`search` **byte-identically**; 242 snapshots re-blessed (4 480 renames + hashes/sort) | `tb-engine/src/{engine_schema,wire,build,readme,pkgcache,repo}.rs`, `docs/research/release-2-schema-2026-11-17.md` | engine fmt/clippy clean, lib **744/1**, it **734/3** (root-only), adapter 211 green untouched; **deployed 2026-11-17** (`docs/internal/deploy-2026-11-17-r2.md`): 12 reads identical on the frozen copy apart from the renamed names and hashes, live vs candidate 0 diffs in all 12, both spellings answer 3 rows, diagnostics 0, task set unchanged. Rollback `/root/backups/au-20261117-pre-r2`. the window advisory (`engine-schema-legacy-spelling`) is **deployed** (`docs/internal/deploy-2026-11-17-window.md`, engine `c9df1cbd`, live diagnostics 0, eight reads clean, rollback `au-20261117-pre-window`). Refusing the legacy spelling outright is the one thing still open | P1 |
| **H** release hygiene (Stage 2) | **clean-host gate passed 2026-11-17** — `bkk-edge01` (no Rust, no cargo, no repo): two binaries + a vault copy, `doctor` **6 ok / 1 warn** (the warn is `peer-unmounted` and correct), `search "SoT"` 15 docs, `write_file` reflected with 0 diagnostics, read-back found; `docs/internal/clean-host-gate-2026-11-17.md`. Metrics refreshed (`4182804`). Two findings: `install.sh` builds with cargo, so a toolchain-less host needs a published artifact; a fresh `HOME` still creates `~/.arsumbris/au-engine` by design (`CREATE_IDENTITY_DIR`; release 2 flips that one constant) | `install.sh`, CI, `tools/cards/` | binaries are self-contained; packaged install and the adapter's mutation gate under a policy file remain unproven | P1 |

### Parallel execution: what may overlap

Three resources are the constraint, not the number of lanes:

- **the cargo build lock.** One compile at a time: the shared target directory serialises every Rust lane, and a
  stale `cargo test` from an earlier session holds the lock (`pgrep -af "cargo test"`). Two Rust lanes do not run
  beside each other, whatever crates they touch.
- **the vault writer.** The engine serialises one mutation at a time, but a multi-write pass (intake, migration)
  must not interleave with another writer.
- **the deploy window.** One window, after every lane whose binary it must carry; the runbook is a single
  capture / compare / rollback sequence.

Safe to overlap: docs and decision lanes (read-only) with any Rust lane; measurement lanes (`tools/ranking`,
`tools/probe`, `tools/kb` — Python only) with one Rust lane; C1, C2 and C4 together, because none of them writes
code or the vault.

Not safe to overlap: S1 and S3 (both want `search.rs` — S1 lands first, S3 rebases); a vault migration beside an
intake pass; the deploy window beside anything that rebuilds a binary.

G1 is an engine change and its adapter schema edit is a second crate: they share the target directory, so they run
one after the other, not beside each other. The brute-force oracle test, the WIRE text and the lane register are
free to move while a cargo lane compiles.

### Decisions only the owner can take

| decision | state 2026-11-17 | what is left |
|---|---|---|
| the deploy window | **done** — engine `869f0ed9`, adapter `0.1.0+55fab3f`; evidence in `docs/internal/deploy-2026-11-17.md` | nothing, except a client restart to pick the adapter up |
| the vault migration | **done** — `.2brain/`, two claim lines, commit `a8aba4b`, 0 diagnostics | release 2 (drop the legacy spelling) remains a later step |
| `family:` semantics | brief written: `docs/research/family-field-2026-11-17.md`; measured 429 values, 345 single-member, 191 provenance-like vs 238 topic-like | a yes/no on the hybrid rule (family = series/provenance, topics = subject) and a first run of the audit |
| `claim` type | brief written: `docs/research/claim-type-2026-11-17.md`; 11 notes, 6 propositions and 5 queue items | a yes/no on renaming the queue item to `ingest-task`, with the pipeline change in the same step |
| the `ambiguous` label | **resolved** — the accept set was widened 3 → 18 on a title+description predicate reviewed by hand; live run 1/1 at rank 1 | the `status` enum collision inside `claim` stays open |
| `grep` exact `total` vs an early exit | open | owner: is the exact whole-corpus total part of the contract? |

## The open queue, in order of payoff per unit of risk

### 1. The MCP↔engine path (`docs/research/mcp-engine-path.md`)

Measured: a raw engine frame costs 1.1–1.2 ms with or without a fresh connection; the adapter adds ~1.7 ms
including the client's own stdio round trip; the daemon holds the IR at 72.6 MiB, so an in-process engine
would duplicate that per session *and* become a second writer. What is left is not transport, it is
ceremony:

1. **per-verb typed payloads — done** (`270f57f`): eight hot reads check the type of every field they name,
   tolerantly (a missing field is not an error; a field of the wrong type is). Completeness stays the
   client's `outputSchema` job, and the first strict attempt was rejected for coupling eight unrelated test
   files to the payload shape.
2. **one source for the verb list — done** (`d14924b`): `READ_VERBS` states each tool's engine read once (33
   rows, composed tools list what they compose), the router passes a tool name, and `read_engine` validates the
   verb the request carries. Two tests cover the map in both directions and a third covers shapes against
   tools; all three were proved to bite. `lib.rs` is 4501 production lines, so the ratchet dropped from 4530.
3. **result modes — settled, and the first answer was wrong.** Measured: `both` 22.0 MB, `text` 11.6 MB,
   `structured` 10.4 MB over 36 reads. I concluded Hermes reads only text and pinned it to `--payload text`;
   six tool calls then failed in 46 seconds with "has an output schema but did not return structured
   content", and the client's circuit breaker reported the server as unreachable. The schema check lives in
   the MCP client layer, not in Hermes' own source, which is why searching Hermes found nothing. The default
   stays `both` and the Hermes entry is back on it; `text` is only safe for a client that ignores schemas,
   and this one does not. Account: `docs/research/hermes-log-findings.md`.

### 2. Ranking: proximity, paid for by a compact term map

The literal class is the finding: a phrase occurring in exactly one note reaches rank 1 in 57% of cases,
because BM25 scores a bag of terms with no proximity credit. Coverage weighting already moved literal
hit@1 from 0.429 to 0.571 with no regression on topical.

Proximity needs per-posting term positions, which needs room in `MEMORY_BUDGET_PER_DOCUMENT = 6144` bytes.
The term map is where that room is: `term_ids: HashMap<String, u32>` over the vault's **21 001 terms and
156 KB of keys** costs ~1.4 MiB, against ~554 KiB for 10-byte open-addressed buckets over one key arena —
**0.84 KB per document, 14% of the budget** (`docs/research/pogocache-patterns.md` §2). So the compact map
is not a memory diet (1.2% of RSS); it is the payment for proximity.

- **Compact term map: built, measured, reverted.** 4 703 → 4 695 bytes per document (8 bytes, against ~840
  predicted); the arithmetic was wrong about what dominates the index — postings do, not the term keys.
- **The room came from the postings instead** (`3304c22`): delta-encoded varint blocks, 4 703 → 2 604 bytes
  per document, plus 20 for the description positions proximity needs. −44%, and the budget (6 144) did not
  move, so proximity has ~3.5 KB per document to spend.
- **Measuring a ranking change no longer needs a swap**: `tools/ranking/candidate_engine.py` starts a second
  engine over a copy of the vault with its own HOME and socket; `eval.py --socket … --vault …` and
  `durable_probe.py --socket … --vault …` both accept it. The copy reproduces production exactly today
  (literal 0.571 / topical 0.642 / durable 0.500, 13 of 26), which is what makes it a valid stand-in.
- **Gate**: proximity lifts literal hit@1 above 0.571 with topical hit@1 not below 0.642 and durable not
  below 0.500, measured on the candidate copy; accepted changes are blessed in the golden `tb_search` capture
  and swapped in with the same rollback artifacts as the sixth swap.
- **Landed and served** (`1ee2463`, seventh swap): literal hit@1 0.571 → **0.714** with hit@5 **1.000**, topical
  0.642 → 0.660, durable 0.500 → 0.538; production reproduces the candidate's numbers exactly. Cost: 3 359 B/doc
  of 6 144, release p50 139 µs / p95 689 µs. The rejected attempt (description-only adjacency) and the swept
  constants are recorded with the code.
- **What would pay next**: the tail is set by the most frequent query term, because the pass walks every
  unordered term pair before `min_should_match` filters. Running it only on documents that already satisfy the
  minimum, or skipping high-document-frequency pairs, is the cheap fix if the tail starts to bite.
- **Risk**: the index's per-document footprint doubles with positions; the budget is the constraint, not
  the code.

### 3. Tier C — the engine's on-disk identity

Steps 1–2 landed: the seam module, the parser half (floor, nested markers, memory-FS filter), and the engine
half — 38 places across 12 files now resolve identity per root through `identity_dir` / `identity_marker`,
with the read rule and the write rule separated (`identity_dir_for_write_with` follows the directory a root
already holds; only a root with neither gets `CREATE_IDENTITY_DIR`, the new spelling since release 2
(2026-11-17); release 1 wrote the legacy one so a previous binary could read it, and readers still accept
both). The end-to-end proof is `tests/it/identity_dir.rs`:
a `.2brain/` repository is discovered, serves `search`, and its `.auignore` is read from the new spelling.

Two things the step forced: `FileSystem` gained a required `is_dir` (probing `<root>/.2brain/repo.yaml` was
wrong wherever the directory exists without a marker), and `consistency_diagnostics` now takes a filesystem so
its four span joins resolve per root.

Step 3 landed (`ddee8f7`): `schema_type_matches`, `is_hardwired_engine_name`, `canonical_schema_claim` and
`is_builtin_repo`, applied in three parse wrappers — a claim in the future spelling now resolves to the builtin
node instead of vanishing, which is the case the floor check would otherwise skip silently.

Step 4 landed (`8438196`): `au migrate identity [--dry-run] [--root] [--all]` prints the rename and every
`type:`/`extends:` line it would rewrite, refuses a repo with a named reason (dirty tree, saga in flight, live
daemon), and writes nothing — including not creating `.2brain` to "check". Running it twice is a no-op, tested.
The writer is deliberately absent: without `--dry-run` it prints the plan and refuses, because half a migration
is worse than none.

Step 5 landed: `--apply` performs the rename with `git mv`, splices the claim lines with the existing rewriter,
and commits once per repository. 16 of 20 in-scope repositories carry `.2brain/` (`tools/cards/metrics.json`
`migration`); the vault waits for a clean tree, and release 2 flips `DEFS` and `BUILTIN_ENGINE_REPO`.

- **Gate**: a migrated repository opens with 0 diagnostics and the unmigrated tree still opens.
- **Cost**: the largest single change left, and the only one that touches files on disk.

### 4. Counters, then whatever they justify

The engine has no per-verb counters, so "is `tb_grep` slow or is the vault big" is answered with a stopwatch. A
first attempt drafted the whole counter module — fixed log2 histogram, one relaxed atomic add per request, no lock
on the hot path — and stopped at the wiring, which is the part that decides whether it is worth having:

- `handle_request` dispatches through a `match` whose arms are the 51 read verbs plus `shutdown`, and the verb is
  known only inside each arm. Counting
  without restructuring the dispatch needs `Request::verb(&self) -> &'static str`, a mechanical 40-arm map — real
  work, but work that also gives the wire a single place to name a verb (the same class of "one source" the
  adapter's `READ_VERBS` solved for tools);
- and counters nobody can read are not counters: exposure means a field on an existing read (`graph_health` with a
  `verbs` argument), a `WIRE.md` note, a re-blessed golden capture and one adapter passthrough.

**Done in that order** (`39b35a6`): `Request::verb()` over 52 variants with a test that it equals `WIRE.md`'s
census in both directions, a 25-bucket log2 histogram at two relaxed atomic adds per request, and `graph_health
{verbs: true}` returning the snapshot as a sibling key — absent unless asked, so the golden capture needed no
re-bless and the schema stays 29.

What is still open is the last hop: the MCP adapter's `tb_graph_health` arguments are `deny_unknown_fields` and do
not declare `verbs`, so a client cannot reach the counters yet. That is a product decision — whether an agent should
be able to read the engine's own latency histogram — not a missing wire.

### 5. Freshness prior — measured a second time, rejected for a new reason

With the durable class in place the prior was rebuilt as a per-class multiplier and measured on the candidate at
`c = 0.10` and `c = 0.02`. At 0.10 it does its job (durable hit@1 0.538 → 0.654, transient ahead 12 → 8 of 26) and
costs the phrase class everything (literal 0.714 → 0.571, the value before proximity). At 0.02 it changes nothing
and literal still falls. So the conflict is structural, not a bad coefficient: the durable class is built from
descriptions, the phrase class from bodies, and re-weighting by lifecycle moves documents across the boundary where
adjacency tie-breaks. Rejected by the rule stated before the run; the shipped numbers are unchanged. A third
attempt needs a query-time lifecycle facet, not a score multiplier.

### 6. The recency query class — a prerequisite, not a feature

The freshness prior was measured, cost 1.9 points of topical hit@1 and was reverted: the labelled set asks
"which note holds these words", and the expected note does not change when the note goes stale. A freshness
prior answers "what is *currently true*", which needs queries whose expected answer changes over the note's
lifecycle.

### 7. Tool names — one decision, then a mechanical commit

52 names were renamed to `tb_*` on 2026-11-16, without an alias; the reasoning is in `docs/naming.md`, which also
records what stayed open (vault content, not code).

## The repository is public now, and it shows its own numbers

The repo went public on 2026-11-16. Two consequences are now part of the plan rather than an afterthought:

- **operations left the public docs.** Eight host-specific documents moved to `docs/internal/` with a stated
  rule (`docs/internal/README.md`, and the same rule in `AGENTS.md`): a document that names `/root/vault`, a
  systemd unit, a socket path, a client config or one evening's run log is operations, not design.
- **the README carries cards drawn from a measured file.** `tools/cards/metrics.json` records every number with
  the command it came from and the date; `tools/cards/render.py` draws it in light and dark and refuses to
  invent a value; `render.py --check` runs in CI, so a card cannot drift from its number. Three cards today —
  the MCP surface, the test gates, the ranking quality — and the numbers are the ones this trajectory measured.

CI itself exists for the first time (`ci.yml`: adapter fmt/clippy/tests, engine fmt/unit/end-to-end; `cards.yml`
re-renders on a metrics change). Its first run found a real portability defect: `.cargo/config.toml` pinned the
shared target directory to `/root/2brain/.cargo-target`, so any checkout that was not this one could not build.
The path is relative now.

## Installation and first use — a workstream, not a footnote

The README tells a newcomer to clone, build in `tb-engine`, copy `au` onto PATH by hand, run the adapter's
install script, `mkdir .2brain`, write a two-line manifest, start a daemon, and hand-write a JSON block for their
client. Every one of those steps exists because nothing automates it, and a public repository whose quickstart is
six manual steps is a repository people read and do not run.

Three commands are being built, in the binary that already exists:

- **`2brain init <dir> [--template minimal|blank]`** — a workspace that opens with zero diagnostics: `git init`,
  the manifest, optional template content, and the exact next command printed;
- **`2brain doctor [<target>]`** — one answer to "is this working, and if not what do I do": binary and build
  commit, engine binary, socket resolution, daemon liveness, member and diagnostic counts, and the server's own
  tool count, each line `ok`/`warn`/`fail` with a sentence;
- **a root `install.sh`** — build both binaries, install to `PREFIX`, print what comes next, never touch a running
  daemon.

The test of the workstream is the README: the quickstart should shrink to `install.sh`, `2brain init`, and the
client block that `doctor` can print. Efficiency and "smarter" behaviour belong to that same test — a command a
reader has to look up twice has not been finished.

## Direction, after the external review of 2026-11-16

An independent review of commit `9cdc0b4` read the README, this trajectory, the search and mutation sources, the
policy, CI, and the public discussions around neighbouring tools. Its verdict is the one worth keeping: **develop
2brain as shared, checkable project memory for several agents** — continuing work across sessions and clients, a
compact context with sources, and controlled change to markdown. The technical base is there; demand for 2brain
specifically is not yet tested, and the review says so more honestly than any roadmap line could.

### What the review corrected, and what it got wrong

| review claim | what the repository shows |
|---|---|
| install needs sibling repositories, Node, pnpm, Electron | the serving path is one Rust binary; the review is describing the pre-fork project |
| tier C/D rebrand is the quarter's priority | tier D is done; 16 of 20 in-scope repositories carry `.2brain/` |
| `init`/`doctor` are not started | they shipped the same day the review was written |
| the CI at that SHA is red | true at `43816449572`, a `fmt` failure from an in-flight edit; green since |
| "−65% tokens" | it is a tool-catalogue character measurement, not a task-level token count |
| the license allows a freemium/SaaS choice | the engine and server are **Sustainable Use 1.0**, not OSI; a hosted service is a rights question |

Its four **write-contract findings** are the useful part, and two are real defects by our own standard:

1. ~~**`tb_create_file` is a read-then-write with no atomic precondition.**~~ **Fixed 2026-11-16.** One server
   process serialised mutations, two client processes did not: both read absence, both wrote, the second won
   silently. The engine now has a `create_file` mutation whose absence check IS its write (one exclusive create
   inside the serialized mutation), and the adapter sends it; a two-process test asserts one success, one refusal
   and the winner's bytes through 20 rounds. See `tb-engine/crates/tb-engine/WIRE.md`.
2. **`tb_edit_file`'s `expected_hash` is a read-ledger token, not a whole-file compare-and-set.** The engine gets
   `old_string`, so a concurrent change elsewhere in the file does not invalidate the edit. That is a defensible
   fragment-edit contract, but it must be *stated* as one rather than left to be discovered.
3. **`tb_rename_type` checks the grant on the member owning the definition** while the rewrite touches references in
   other members. **Fixed 2026-11-16** (`6c080ad`): `preview_mutation` takes `op: rename_type` and answers every path
   the cascade would rewrite with its member and the kind of change, from the same planner the mutation executes; the
   adapter refuses when any of those members is outside its allowlist, naming them.
4. **Diagnostics stay advisory on a normal mutation.** Right for drafts; wrong for promoting into accepted
   knowledge, which is a separate mode the schema does not have yet.

### What changes in this trajectory because of it

- **The milestone is a scenario, not a component**: *two different agents safely continue one project on one
  knowledge base.* Done means: a clean-machine install, agent A records a decision with its evidence, agent B finds
  it inside a bounded context and continues, a concurrent edit produces the intended conflict, a lost reply is
  reconciled without double-applying, the change is in history, and every step is reproducible on a published SHA.
- **Search evidence moves ahead of search features.** The three sets are synthetic (literal and topical are built
  from the notes themselves; durable encodes a preference we chose). They measure rank stability, not usefulness.
  The next instrument is a task set with real questions — exact identifier, paraphrase, Russian into English
  material, multi-note questions, superseded decisions, contradictions, and questions the base cannot answer —
  labelled with the **minimally sufficient evidence**, not just the right note. Candidate generation is measured
  before selection, because ranking cannot fix a document that never entered the pool.
- **A `context` contract becomes a goal rather than a wish**: every answer says why a fragment is included, which
  source and version backs it, what is missing, and whether anything conflicts. Built as a composition of existing
  tools first; a new tool only if it measurably cuts calls and bytes.
- **Knowledge-base maintenance moves from "nightly hygiene" to "proposal → checks → acceptance".** The review is
  right that the judge is a triage filter (staleness accuracy 0.884 at recall 0.333), not an acceptance gate.
- **Item 9 of the review — a study with 8–12 external users — is not executable here** and is dropped rather than
  softly promised. What replaces it is dogfooding with measurement: this vault, two clients, the five scenarios
  above, run weekly, with the same numbers recorded as would be reported to a user. Demand is then argued from our
  own recorded use, or not argued at all.

### The second direction: 2brain as the knowledge base itself

The vault is not a by-product of the repository; it is the product this repository serves. Today it grows from one
person's interests, which makes it a good corpus and a weak test of demand. Three things follow, none of which need
external users:

- **Make intake deliberate.** Every note should say where it came from (`source_url`, `provenance` are already in
  the schema), and the nightly pass should report the corpus's own gaps — topics that are read but never written,
  notes with no inbound link, claims with no source — rather than only duplicates and staleness.
- **Measure the knowledge base, not only the code.** Notes, links, typed coverage, evidence-bearing notes, isolated
  share, median outbound citations, and the five questions this vault fails to answer. A card and a weekly delta.
- **Let the base answer its own questions.** The transient query class the search section needs can be harvested
  from this vault's own inbox — questions written down and not yet answered are the most honest query set there is.

### The plan, as stages with exit criteria

Ordered by what unblocks the next decision, not by what is interesting.

**Stage 1 — the shared-memory contract is actually a contract.** Concurrent create refuses instead of overwriting;
`expected_hash` is documented as a fragment token or replaced by a whole-file compare-and-set; a rename that reaches
an ungranted member is refused; promoting a draft into accepted knowledge is a mode with checks that can fail.
*Exit:* four tests, each running the two-process case they describe, plus WIRE.md and the tool descriptions saying
what the engine guarantees.

**Stage 2 — release hygiene.** Green CI on the release SHA; versioned binaries for Linux and macOS with a
reproducible build command; the single-user trust boundary and the Sustainable Use license stated where a new user
reads them first.
*Exit:* a clean machine installs from the published SHA, runs `doctor`, and completes one read and one write.

**Stage 3 — task-level evidence for search.** The labelled task set described above, at least 40 questions with
minimally sufficient evidence; candidate-generation recall measured separately from final ranking; the numbers kept
next to the synthetic ones rather than replacing them.
*Exit:* a stated threshold per query class, written down before the next ranking change.

**Stage 4 — the project-memory template.** `decision`, `constraint`, `evidence`, `experiment`, `handoff`,
`error-fix` ship as `2brain init --template project-memory`, with a test that starts a daemon on a fresh workspace
and requires zero diagnostics from the starter's own types. Writing it already taught one lesson: `String` in the
type system means REQUIRED, so the first draft rejected a decision for a missing `supersedes` — only what a record
cannot do without stays required.
*Exit:* the two-agent scenario passes on the live vault, with the transcript kept as evidence. The template is
half of it; the transcript is the half that is not written yet.

**Stage 5 — knowledge-base intake.** Deliberate intake (every note sourced and linked at write time), the weekly
census as a card, proposal-based maintenance with a human-checkable diff.
*Exit:* the isolated share and the unsourced share fall while the note count stays flat.

Stage 1 is running now; stages 2 and 4 in this same cycle. Stage 3 needs the task set written by hand before any
code moves, and stage 5 begins with the census that now exists.

### The issue register, and what it changes

Observations made while working are now issues rather than paragraphs, because a paragraph in a trajectory is a
memory and an issue is a queue. Eleven exist:

| repository | issue | what it closes |
|---|---|---|
| 2brain | #1 `docs/tools.md` has no drift check | the tool table becomes a contract, like `READ_VERBS` ↔ WIRE |
| 2brain | #2 the engine has no clippy gate | a gate on one half teaches contributors to ignore the tool |
| 2brain | #3 `2brain mcp` without the daemon's `HOME` | the first failure must name the state root it resolved |
| 2brain | #4 three representations of `scope` | the defaults become data instead of prose |
| allspark | #19–#22 | `graph`'s cycle, coverage, hotspot and staleness reports each mislead in a measurable way |
| allspark | #23–#25 | `bash` handle semantics, polling turns, and a child's report visibility |
| 2brain | #5, #6 | CI runs no `tb-core`/`tb-cli` tests (`ef0be6b`); the known-environment file claimed 85 clippy warnings (`49b9c4f`) |

All six are closed, each with a commit and a measurement, and the engine job now runs fmt, clippy and four test
commands in one place. The first four: the tool table is now checked against `tools/list`
in both directions, a failed launch names its state root and socket, `scope` is one argument with one table of
defaults bound by a test, and clippy is a gate on **both** halves — the engine went from 85 findings to 0 in two
commits, and the CI step runs the same command the commit proves.

What the four have in common is worth keeping: **each fix made a previously implicit rule explicit and testable.** A
table becomes a test, a default becomes data, a gate becomes a step. The register's next two rows are the same kind of
thing — a suite that exists but is never run, and a document that states a number nobody re-measured.

The rule this makes explicit: **an observation becomes an issue only with a reproduction and a suggestion.** Three of
the seven harness issues came with numbers (`fan_in=1449`, `229/2247`, seventeen `sleep` processes), and those are
the ones a maintainer can act on without re-deriving the session.

### The knowledge-base target, corrected by its own instrument

The morning's number — 424 notes with no outbound link — was measured by a regex over the note body. The vault's
convention is to put "what this note is about" in `topics:` as frontmatter wikilinks, and the engine counts those as
field references, so the regex called a connected note isolated. Corrected the same day:

| measure | value |
|---|---:|
| links, body + `topics:` | 4 886 (median 3 per note) |
| notes with no link of any kind | **76** |
| notes with no `topics:` link | 165 |
| isolated in the graph (no edge either way) | 204 |

Three consequences, in order of how much they change the plan:

1. **The intake target is 76, not 424**, and the number that matters is the graph's 204 — a note nobody links to is
   invisible to traversal even when it links outward.
2. **Every metric must state what it counts.** Both tools now print body, `topics:` and total separately, and
   `connect.py` no longer proposes a link for a note that has one. This is not a footnote: the wrong number was the
   headline of a whole research note for a day.
3. **The method held.** The error surfaced because two instruments disagreed on one note — `connect.py` said
   "unlinked", `grep` found a link — and the disagreement was treated as a bug in the instrument rather than in the
   vault. That is the procedure worth keeping: when two measurements of the same thing differ, the smaller claim is
   usually the instrument's.

### The next parallel lanes, and what each is allowed to claim

The two that ran are closed: the excerpt landed (`25ce096`, 4.00 calls / 8.86 KB → 1.00 call / 2.44 KB with the
evidence unchanged) and the link pass landed 34 edges with 19 of 36 `f/patterns` notes still unreachable. What
follows is the queue after them, ordered by what it would cost to be wrong.

| lane | what it claims when it is done | touches | gate |
|---|---|---|---|
| **visit bound** | "work per read does not grow with the corpus", with a number and a slope | `search.rs`, `verb_metrics.rs`, a probe under `tools/` | engine suite + the three-size table |
| **regression task set** | "the five ranking failures of the coverage audit are now tasks", each with the evidence a correct answer needs | `tools/ranking/*`, `docs/research/task-set.md` | a run showing which pass, which still fail |
| **intake decisions** | for each of the 36 notes the link pass refused, a named decision and its reason — not a write | `docs/research/` only | read-only over the vault |
| **aliases** | the vocabulary class has a design: what a note says about its other names, and the experiment that sizes it | `docs/research/`, `tools/kb/` | measurement on the 1 032 notes |
| **context tool** (`tb_context`) | one call replaces the pack's composition, and it must beat 4 calls / ~9 KB — a bound that has already moved to 1 call / 2.44 KB | `serve.rs`, `lib.rs`, `context_pack.py` | the twelve-task pack |

Two defects now have a task each, so they are queue entries with a pre-registered rule rather than opinions
(`docs/research/task-set.md` records both rules before any fix):

- **the `min_should_match` floor hides what the ranker found** — `oracle matrix tests` admits five notes and the right
  one is not among them, while the same ranking at `min_should_match: 1` puts it third of 83. Proposed rule:
  `matched >= msm || best_idf >= RARE_IDF`, i.e. a rare term that matched is enough even when the AND floor is not
  met. Accepted only if `absent` stays 0/3 and no other class moves. **Landed 2026-11-16**: `mixed-language`
  1/2 → 2/2, `oracle matrix tests` at the default floor now rank 3 of 83 — the same page the
  `min_should_match: 1` row already returned — `absent` **0/3**, and no other class moved; the rule, the
  before/after table and what it does not fix are in `docs/research/task-set.md` §3.
- **the title is not a scored field** — a note titled after the question ranks 8th behind a survey, with 6 of 6 terms.
  A title field with its own weight is the obvious mechanism; the risk is that a title match flatters a shallow note,
  so the rule is that `hub-vs-atomic` must reach rank 1 and `absent` must not gain a hit.

Both touch `search.rs`, so they queue behind the visit-bound lane rather than beside it: one writer per file, and the
build lock makes a second engine lane wait anyway.

The visit-bound lane goes first because it is the only one whose failure mode is silent: a linear scan inside a read
looks exactly like a constant one on a thousand notes, and doubling the corpus would not be noticed until it was slow.

### The intake lane, measured, and what it says about the metric

Thirty-four notes were linked inside their families, one link each, none crossing a family boundary, each as its own
engine commit. What moved:

| measure | before | after |
|---|---:|---:|
| `f/patterns` notes with zero inbound | 35 of 36 | 19 of 36 |
| inbound edges into `f/patterns` | 1 | 21 |
| zero inbound over the original 76 | 48 | 32 |
| edges in the graph | 4 655 | 4 689 |
| «no outbound link» by the census regex | 76 | **75** |

The last row is the sentence to keep: **the regex moved by one, the graph by thirty-four.** A metric computed over
text describes the text's shape; the same question asked of the engine describes the graph. Both are now printed side
by side, and the engine's number is the one the plan tracks.

Equally part of the result is what was refused: 28 notes whose family has exactly one member (a link would have
crossed families), 8 with no frontmatter, 2 belonging to another session's uncommitted work where the write gate said
no. A link pass that reports only what it added is not a measurement.

### The research lane, 2026-11-16, and what it changed

Two read-only lanes ran in parallel against the ADR corpus of `/root/kutha-graph` and against our own vault.

**`docs/research/kutha-ideas-2026-11-16.md`** — twenty mechanisms ranked by value per unit of risk (3 ideas, 12
patterns, 4 methodologies, 1 algorithm), each with the 2brain subsystem it would change, a cost, a risk, and the
smallest experiment that would settle it. Two entries are measurements with zero blast radius and they rank first,
which is the right order: **a visit bound tested at three corpus sizes** (the one defect that gets worse as the base
grows) and **rebuild == patched** (a single equality that can fail). It also refused five attractive imports, each with
a reason.

**`docs/research/vault-coverage-kutha-themes.md`** — 62 searches and 23 greps on the live vault against those themes:
4 covered, 9 touched, 2 absent. The temporal core is genuinely present; about half the rest is a one-line mention
inside another subject. The diagnosis is worth more than the coverage table, and it is not about ranking:

| failure | evidence | defect or corpus |
|---|---|---|
| a plural costs the query | `capsule` 2 hits, `capsules` **0** | defect — fixed in `c3c7ee4` (word-form lookup after an exact miss) |
| `exact` promised a phrase | `exact` on `as of` returns 218 notes with the words anywhere | defect — the description lied; it is an AND, and the text now says so |
| a vocabulary mismatch looks identical | `portable capsule` 0 both before and after the fix | corpus — no note holds both words; the base calls it an "RVF container" |
| a hub outranks the atomic note | a 330-hit query puts a survey above the note titled after the question | open |
| bilingual bodies | a Russian question cannot reach its English sibling; the reverse works through English titles | open |

The third row is the one to keep apart: **two defects that present identically are not one bug.** Morphology is a
lookup we can add; vocabulary is a statement the note has to make about itself (an alias), and no search change
substitutes for it.

### The refusal lane, and the field it says is doing two jobs

The 38 notes the link pass refused now each carry a decision and a reason (`docs/research/intake-decisions-2026-11-16.md`):
9 link, 4 adopt into an existing family, 8 frontmatter only, 18 leave alone. All thirteen proposed edges were checked
before being proposed — target resolves, edge absent, no cycle within five hops.

The arithmetic corrects the previous lane's hope: **`topics:` gives inbound to the note's neighbour, not to the note.**
A pass that links an isolated note raises its out-degree and its neighbour's in-degree, so the proposal moves
zero-inbound from 14 to 10 and no-edge-at-all from 12 to 4, but four notes stay completely isolated because nothing in
the corpus is in their subject. Filling the rest needs a pass that points AT them.

And the field that keeps coming up: **350 of 433 `family:` values have exactly one member.** Two different things are
called a family — a provenance (`f/arxiv-2605.29713`, `f/gh-hashcat-hashcat`, which duplicate `source_url`) and a
topic (`f/voice-agents`, `f/decision-models`, which promise a group nobody joined); 13 of the 28 single-member notes
already carry an outgoing `topics:` pointing outside their own family. Any future intake automation that trusts
`family` as a grouping would be trusting a field with two meanings. The next step there is a decision about the field,
not another pass over the notes.

### The ruvector lanes and the measurement that changed a decision

`/root/vendor-source/ruvector` (322 ADRs, 178 crates, 4 492 Rust files, indexed in the code graph at 100 930 nodes)
produced three documents: `ruvector-adr-2026-11-16.md` (20 mechanisms from 40 ADRs read end to end),
`ruvector-crates-2026-11-16.md` (12 implemented mechanisms, every one with `path:line`), and
`ruvector-agent-2026-11-16.md` (12 from the agent layer).

Two findings matter more than the lists:

- **We rank the whole corpus and filter by scope afterwards.** Our own `search.rs` says so, and ruvector measured the
  same ordering at 7.9x for identical answers. That is now a lane with a measured before/after, not a principle.
- **`crates/ruvector-agent-memory` is a memory product with our shape**, and its own stated limit is our most likely
  failure: N notes repeating one source count as N observations. It implements lineage arbitration for the case where
  the notes cite the source, and admits it cannot catch twenty agents re-asserting a rumor. That is why a corpus lane now
  measures how many echoes the vault already holds.

The measurement discipline they contributed is worth as much as the mechanisms: a sold benchmark is a liability
(their hybrid header cannot come from their own runner), and "a mechanism is real when a symbol exists AND a caller
other than its own test uses it" separated five implemented mechanisms from five recommendations in their agent layer.

### The bootstrap, and what it says about today's own change

`tools/ranking/bootstrap.py` compares two task-set runs over the same tasks and answers BETTER / WORSE / INCONCLUSIVE,
with exact McNemar beside the interval. Validated on pairs whose answer is known — a self-pair is inconclusive, a
reversed-page ranker is WORSE — and then applied to the change this trajectory just recorded as accepted:

**10/18 -> 11/18 is INCONCLUSIVE**, interval [0, +3] tasks. At n = 18 a resample misses one flipped task with
probability (17/18)^18 = 0.357, so the interval must contain zero; the tool's resolution here is four net flips. The
gate therefore stays the pre-registered task-level criterion, the bootstrap is published beside it, and a bootstrap
WORSE is a veto. For the bootstrap to gate, the metric has to be graded (nDCG/MRR) rather than pass/fail — a bigger
binary set would need about a hundred tasks to detect four points.

That is the honest form of "we improved it": the change was accepted on a criterion that could see it, and the
statistical instrument says plainly what it could not.

### Six axes, and which of them has a number attached

`docs/research/intelligence-axes-2026-11-16.md` sets out the axes that can still move, each with the measurement that
would decide it. The findings that change the plan:

- **The tool surface is a fixed cost per session, and nobody had counted it**: `tools/list` is 58 847 bytes, about
  14 700 tokens, before an agent asks anything — 12 340 of descriptions and 21 466 of schemas. "Fewer tools" is
  therefore mostly a bytes problem, and the cheapest lever is a one-line description on the wire with the long one
  behind `tb_describe_tool`, which already exists.
- **Seven entries instead of thirty-six** is the shape worth testing (`tb_orient`, `tb_context`, `tb_read`, `tb_links`,
  `tb_check`, `tb_write`, `tb_help`), and the test is a routing experiment on 24 task descriptions, not an argument.
  ruvector's "8–15 tools" is a recommendation with no code behind it; our own routing rate is the evidence that counts.
- **The FSM belongs in MCP `prompts` and `resources`, not in the daemon.** We ship neither, they cost zero schema bytes,
  and they version with the repository. A server-side stage would duplicate what the write gate already enforces.
- **SIMD is deferred with a written trigger** (p95 above 5 ms or a corpus above 50 000 notes, then a decode benchmark);
  **concurrency is measured first**, because "several agents on one base" is the product and we have never put two
  readers on it.
- **Temporality has a rung that needs no new store**: `tb_content(path, commit)` through `git show`. Bitemporal comes
  later and only if a consumer asks.

The missing list, ordered by cost of continued absence: a graded ranking metric, token accounting, abstention
(`absent` 0/3 is a base asserting on questions it cannot answer), lineage, a deploy that lands, and a concurrency
number.

### The measurement lane, after a day of measuring the measurers

Four artifacts now decide what a ranking claim means, and each one changed a plan:

- **`graded.py` with grades and a true IDCG@5**, after a review of the field found that our first version was
  `1/log2(rank+1)` — a monotone function of MRR that could not disagree with it. The repair is visible in the control:
  the reversed page moves nDCG by −0.568 against MRR's −0.434.
- **The MDE, stated plainly**: at n = 18 the smallest nDCG@5 change the set can certify is about **0.23**, a quarter of
  the metric's range; 160 questions bring it to 0.077. Today's shipped change (+0.033) is below every row of that table.
  So the eighteen-task set can name a mechanism, show the row it moved and veto a regression — and cannot certify a small
  gain. Any future claim of "this improved ranking" must name its σ and the n that σ requires.
- **160 questions in nine classes with nine axes each** (`tools/ranking/questions.json`), each row carrying a *measured*
  share of query terms absent from its expected note, so a question cannot be answered by quoting itself. Ten pairs differ
  in exactly one axis, which is what makes a miss attributable.
- **The first baseline over them** (`2194e28`): MRR 0.637 at HEAD, 0.619 on the served binary — the deployment lag costs
  4 rows at hit@5. And the axes say what the classes cannot: a keyword string hits rank 1 in 31 of 35 rows, the same
  questions phrased naturally in 31 of 78; a pasted path in 15 of 15, a concept in 58 of 126; structural questions in
  0 of 12, because they are not search questions at all.

- **A semantic reading of 42 questions** (`3b28be5`): the top hit answers 38 %, `weak` fires on none of the eight
  unanswerable tops, and `matched_terms` does not discriminate (4.75 against 5.32). Those two measurements killed the two
  obvious candidates for a confidence signal before anyone built them.

### What shipped after the research lanes, and what it cost

Five changes landed from the research rather than from the plan, each with a measurement attached:

| change | measured effect |
|---|---|
| **concise is the tool surface default** (`30a0c88`) | 59 127 → 19 404 bytes: **−9 931 tokens per session, −67 %**, with the validation contract intact (`required` identical on 36/36) and `tb_describe_tool` byte-identical across profiles |
| **the one-line description cuts on a clause** (`fb5b973`) | eight descriptions stopped splitting mid-phrase; `lib.rs` went 4564 → 4556 production lines and the ratchet followed down |
| **a URL query is a source-identity lookup** (`2577be2`) | `z08` absent → rank 4, 159 of 160 rows byte-identical, golden unchanged |
| **abstention did not ship** (`419ab8b`) | twelve observables, best two-feature rule 6/8 in-sample, **0/8 under leave-one-out** — so `confidence` stays unimplemented |
| **the measurement doctrine** (`d120804`) | five rules, each paid for by a defect this session produced |

Two of those are worth more than the code: the surface default improves **every future session's cost**, and the abstention
refusal removed a feature that would have shipped as a coin flip.

### The plan after them

The research produced three concrete transfers that need no engine change, and they are now the queue:

1. **The claim register as typed notes** (arscontexta's `claim-map`): `type: claim` with an `evidence:` link to a key in
   `tools/cards/metrics.json`, and `type: decision-note` with `backed_by:`/`supersedes:`. It is what lets the base answer
   "what backs this claim" instead of only "which note mentions it" — the difference between a memory and a search index.
2. **The answer contract as an MCP prompt** (arscontexta's `ask`): we ship neither `prompts` nor `resources`, and both cost
   zero schema bytes. The six-part answer shape and the honesty standard are text, and text is what the protocol has a slot
   for. Our tools return inputs to an answer; nothing in the product is under contract to produce one.
3. **Condition thresholds surfaced as tasks** (`graph_health` fires → a queue entry, not a report line).

Two things stay blocked and both are the user's to open: the nine-change deploy window, and the vault migration while other
sessions hold uncommitted paths.

### What the second opinion changed about how we state things

No MDE from this stack is quotable — the σ was of the wrong type, taken from another corpus and from one contaminated pair.
The method survives (declare σ, derive n, name it with the claim); the constants do not. The same review withdrew the
"factorial" framing of the 160-question axes (two axes have no contrast), and the claim that a graded metric makes ranking
arguments decidable. What replaced them is duller and true: the set names a mechanism and shows the row it moved.

### What that changes in the stages

- **Stage 1** is done except the deploy window; #4 is the one remaining piece of design debt it created. The window
  now also carries the excerpt and the word-form fix, so it is worth more than it was and no longer than it was.
- **Stage 2** (release hygiene) absorbs #1–#3: they are each smaller than a release check and they are what a new
  user hits first.
- **Stage 3** keeps its shape, with one addition: the context pack measurement needs a client that builds the pack,
  so the measurement and the composition are the same artefact.
- **Stage 5** gains the maintenance shape the issues imply: a weekly pass that files findings as issues instead of
  amending a document.

## What runs in parallel, and why it is safe

These tracks touch different files and different gates, so they can run at the same time:

| track | touches | gate | conflicts with |
|---|---|---|---|
| visit bound | `tb-engine/src/{search,verb_metrics}.rs`, `tools/probe/` | engine suite + the slope table | none |
| regression task set | `tools/ranking/*`, `docs/research/task-set.md` | a run against a candidate engine | none (no cargo) |
| intake decisions | `docs/research/` | read-only over the vault | none |
| adapter surface (`wire.rs` typed fields) | `tb-mcp-rs/src/wire.rs` | adapter suite | none |

The ordering constraints:

- **two lanes that need `cargo` do not overlap.** The build lock serialises them and the second one spends its time
  waiting, so at most one worker compiles the engine at a time; the rest measure, read, or write documents.
- **the deploy window comes after every lane that changes the engine**, because each one adds a binary to the swap
  and the runbook is a single capture/compare/rollback sequence.
- **a lane that cannot name its experiment does not start.** The list above is ordered by the cost of being wrong, not
  by how interesting the mechanism is.
