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
| rebrand | tiers A, B, **D done**; tier C steps 1–3 done (38 identity sites, schema seams, canonicaliser), step 4 is the migration command |
| tool names | **`tb_*`** — 52 tools, renamed from `au_*` on 2026-11-16 without an alias (`docs/naming.md`); the `_meta` key and the docs followed |
| migration | **16 of 20 in-scope repositories migrated** to `.2brain/`; the two served members moved in a window with byte-identical live reads; the vault waits for a clean tree |
| FSM run log | done — `tools/fsm/check_run.py`, seven rules, `--selftest` proves each bites |
| hygiene pass | done and applied — `tools/hygiene/pass.py`, `inbox/hygiene-2026-11-16.md` |
| ranking | coverage weighting + **proximity** landed and measured; freshness prior rejected, its retry now has a query class and a decision rule |

Gates that must stay green on every change: adapter 207 tests, engine `--lib` 685/1 (the one failure is the
documented root-runner), `search_read` 6/6, `fmt --check`, `clippy -D warnings` on the adapter, and
`scripts/live_parity.py` against the installed binary.

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
already holds; only a root with neither gets `CREATE_IDENTITY_DIR`, the legacy spelling in release 1 so a
previous-release binary still reads what this one writes). The end-to-end proof is `tests/it/identity_dir.rs`:
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

Step 5, the writer, is in flight: `--apply` performs the rename with `git mv`, splices the claim lines with the
existing rewriter, and commits once per repository. The order after it: one leaf member, then the monorepo, then
the vault, then release 2 flips `DEFS` and `BUILTIN_ENGINE_REPO`.

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

### 6. Tool names — one decision, then a mechanical commit

52 names, 618 occurrences in 81 files, 34 fixtures named after them, 91 vault mentions. The safe form is
`tb_*`. That decision was taken on 2026-11-16 and executed without an alias; the reasoning is in
chosen; `docs/naming.md` records the open state.

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
