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

1. **`tb_create_file` is a read-then-write with no atomic precondition.** One server process serialises mutations,
   two client processes do not: both read absence, both write, the second wins silently. Being fixed with an
   engine-side create verb whose absence check happens where the write is serialised.
2. **`tb_edit_file`'s `expected_hash` is a read-ledger token, not a whole-file compare-and-set.** The engine gets
   `old_string`, so a concurrent change elsewhere in the file does not invalidate the edit. That is a defensible
   fragment-edit contract, but it must be *stated* as one rather than left to be discovered.
3. **`tb_rename_type` checks the grant on the member owning the definition** while the rewrite touches references in
   other members. A rename that reaches an ungranted member must be refused or the affected set must be checked.
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

## What runs in parallel, and why it is safe

These tracks touch different files and different gates, so they can run at the same time:

| track | touches | gate | conflicts with |
|---|---|---|---|
| MCP path (typed payloads) | `tb-mcp-rs/src/{wire,schemas,lib}.rs` | adapter suite + live parity | none |
| compact term map, then proximity | `tb-engine/crates/tb-engine/src/search.rs` | ranking eval + engine suite | none |
| tier C step 2 | `tb-parser` (`scope.rs`, `fs.rs`), `tb-engine` repo discovery, `wire.rs` floor | vault opens with 0 diagnostics | none (different crates) |
| verb counters | `tb-engine/src/serve.rs` | engine suite | tier C (same crate) |
| tool names | every tool string | adapter suite + live server | MCP path (same files) |


The one ordering constraint: **the tool rename comes after the MCP path**, because both rewrite the router
and the schemas, and a rename on top of an unfinished refactor makes the diff unreadable.
