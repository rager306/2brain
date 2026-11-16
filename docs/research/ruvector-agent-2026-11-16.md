# What the RuVector agent layer knows that a memory product needs

Read 2026-11-16 from `/root/vendor-source/ruvector`, read-only. Scope: `.claude/` (117 agent files, 103 unique
role names; 37 skills; 127 command files; hooks; `intelligence/`; `settings.json`), `harnesses/timesfm-harness/`,
`METAHARNESS-README.md`, `docs/METAHARNESS-ARCHITECTURE-SUMMARY.md`, `docs/rvagent/`, `docs/evidence/`, `CLAUDE.md`,
and the ADRs those documents lean on (134, 273, 312, 317).

Two housekeeping findings first, because they change how the rest should be read.

**The agent layer is mostly generated documentation, and the mechanisms are unevenly real.** The `.claude/agents/`
tree carries 117 files for 103 role names; several are byte-level duplicates in two paths
(`analysis/code-review/analyze-code-quality.md` and `analysis/analyze-code-quality.md`;
`testing/validation/production-validator.md` and `testing/production-validator.md`). The core role cards
(`coder`, `planner`, `reviewer`, `tester`, `researcher`) repeat the same "V3 self-learning" blocks with claims like
"150x-12,500x faster" and "+12.4% accuracy" asserted in the role card, with no dataset, configuration, or run
cited there. The genuinely implemented pieces
are elsewhere: `helpers/hook-handler.cjs`, `helpers/intelligence.cjs`, the rvagent middleware docs, and the four
ADRs. I have taken the mechanisms from those and named the rest as text.

**Their own citations do not resolve.** `METAHARNESS-README.md:16` advertises "ADR-265 ... (280 lines)";
`docs/adr/ADR-265-ruvector-comprehensive-benchmark-suite.md` is 136 lines. Two different files both claim to be
ADR-266 (`...-darwin-ann-optimization.md`, 331 lines, and `...-darwin-integration.md`, 513 lines). The memory
specialist cites ADR-009 as "Hybrid Memory Backend" while `ADR-009-structured-output.md` is structured output; the
claims authorizer cites ADR-010 as "Claims-Based Authorization" while `ADR-010-function-calling.md` is function
calling. One code comment cites "ADR-174: capture FAILURES" (`.claude/helpers/hook-handler.cjs:347`) while
`ADR-174-ruos-thermal-overclock-pi5.md` is a Raspberry Pi thermal ADR. This is the defect a shared memory product
exists to prevent, committed by the project that is otherwise most careful about provenance. It is evidence for
Entry 1, not a reason to discard the layer.

The twelve entries below are ranked by value per unit of risk for **2brain as it stands**: an engine that is the
only writer, a fail-closed gate with prior-read and freshness hashes, per-member grants, one saga per mutation, a
hash-chained policy ledger, `preview_mutation`, visit counters, a `proposed → accepted` lifecycle, 52 MCP tools, and
no LLM or vector index in the serving path. I have marked as "already there" the parts 2brain has, and stated the
delta. Nothing proposed here puts an LLM or an embedding on the read path; where a proposal would, it says so.

| # | Entry | Changes | Cost | Why it ranks here |
|---|---|---|---|---|
| 1 | Evidence grade per claim | type system, `search.rs`, `tools/` | M | Every other entry depends on a reader being able to tell assertion from fact |
| 2 | Read delimiter and trust flag | `serve.rs`, `search.rs`, `tools/` | S | Cheapest protection against the largest failure mode: a note that instructs its reader |
| 3 | Loop detection | adapter / `serve.rs` | S | Small, measured elsewhere, and 2brain has no read-loop guard at all |
| 4 | Tool surface 8-15 | tool surface, `docs/tools.md` | M | 52 tools is the largest measured liability in the product |
| 5 | No witness, no mutation | `mutate.rs`, ledger | M | The ledger exists; the binding to state does not |
| 6 | Measure the task set first | `tools/ranking/`, `docs/` | S | Gates every ranking claim made after it |
| 7 | Allowed/blocked dossier | type system, `mutate.rs` | M | High value, but needs the lifecycle discipline first |
| 8 | Four-condition promotion | type system, `mutate.rs`, `tools/` | M | Depends on 1 and 7 to be more than ceremony |
| 9 | Doctor pre-flight | `tools/`, `serve.rs` | S | Useful, not differentiating |
| 10 | Manifest over a batch | `mutate.rs`, `tools/` | S/M | Extends `preview_mutation`; no new concept |
| 11 | Trajectory note type | type system, `tools/` | M | Real, but the reward is self-reported without a gate |
| 12 | Session-end handoff | `docs/`, `tools/` | S | Convenience; the transport should not be copied |

---

## 1. Evidence grade on every claim

**Mechanism.** Every term that authorizes a record is tagged with how it is known: `recomputed`,
`signature-verified`, or `trusted-assertion`. A reader can then tell which parts of a claim were independently
re-derived and which were merely asserted. From ADR-312: "each authorizing term is tagged with 322C's evidence
grade (`recomputed`/`signature-verified`/`trusted-assertion`) so a reader can tell which parts of the claim were
independently recomputed versus merely asserted" (`docs/adr/ADR-312-shared-witness-schema-anchoring-contract.md:103-106`).
The ADR applies the vocabulary to authorizing terms in a receipt; the transfer is to apply it to the claims a note
makes. The vocabulary is three values, not a score; it is orthogonal to 2brain's lifecycle `status`, which says
where a note is in its life, not how its content is known.

**Source.** `docs/adr/ADR-312-shared-witness-schema-anchoring-contract.md:51,103-106`. The vocabulary is also
implemented, not only specified: `crates/ruvector-agent-memory/src/ops.rs:221` defines the three-variant enum and
`:230` its compact code, and the code is bound into the witness record's `flags` nibble so it is part of the hash
preimage (`ops.rs:254-258`). The transfer is therefore a port, not an invention.

**What it would change in 2brain.** The type system: a per-claim `evidence:` field, either in frontmatter for a
single-claim note or on the block that carries the claim. `search.rs`: return the grade beside the hit so a reader
sees it before opening the note. `tools/`: `tb_search`/`tb_content`/`tb_context` carry the grade; `tb_validate_value`
rejects an unknown grade. `docs/`: the three definitions, with one worked example each. `mutate.rs` only later, to
require a grade on promotion.

**Cost.** M. One frontmatter field, one validation enum, response fields in three read tools, and a doc page;
roughly 3-4 files and no new state or crate. The expensive part is the authoring convention, not the code.

**Risk.** Grade inflation: if the writer sets the grade by feel, almost everything becomes `trusted-assertion`
(honest but inert), or everything becomes `recomputed` (dishonest and worse than no grade). A filter on grade can
hide the one note that answers a question. The grade must be checked by the reader's ability to re-run the
recomputation, not trusted from the field.

**First experiment.** Take 30 claims from 10 existing notes and attempt to recompute each one from the repo or the
vault (hash a file, re-run a grep, re-count a test result). Report the split across the three grades and the median
time per recomputation. If fewer than ~30% are recomputable at all, the grade's value is diagnostic — it shows how
much of the base is assertion — and it should not gate retrieval yet.

---

## 2. A delimiter and a trust flag around every read

**Mechanism.** Tool output is wrapped in a block that names the tool and the call, so the model can see where
untrusted content starts and ends: `<tool_output tool="read_file" id="call_abc123"> … </tool_output>`
(`docs/rvagent/security.md:80-92`). Memory files loaded into a prompt are trusted only when a signed manifest
verifies their hash, and `trust_agents_md` defaults to `false`
(`docs/rvagent/security.md:94-103`). Frontmatter is capped at 4 KB, skill files at 1 MB, and YAML expansion is
bounded.

**Source.** `docs/rvagent/security.md:80-103`; pipeline order in `docs/rvagent/architecture.md:98-111`
(`ToolResultSanitizerMiddleware` at :110, `WitnessMiddleware` at :109). Implemented at
`crates/rvAgent/rvagent-middleware/src/tool_sanitizer.rs:14` (struct) and `:22`
(`sanitize_tool_result`), which escapes `</tool_output>` before wrapping; the audit sibling is
`crates/rvAgent/rvagent-middleware/src/witness.rs:301` (`WitnessMiddleware::wrap_model_call`).

**What it would change in 2brain.** `serve.rs`/`search.rs` response formatting: every note body returned by
`tb_content`, `tb_search` excerpt, and `tb_context` fragment is emitted inside a delimited block carrying the note
path, its content hash, and its evidence grade. The delimiter itself must be escaped in the body, or a note can
close the block and inject text. The context pack already carries `hash` and `provenance` (`docs/research/context-pack.md`);
this adds the boundary and the escape rule. `tools/` docs state the contract; no vault change.

**Cost.** S. A formatting function and an escape rule, one response-shape test, one doc paragraph. The JSON schema
does not change if the block lives in the text field.

**Risk.** A client that parses the text field instead of `structuredContent` breaks (the Hermes incident in
`docs/research/hermes-log-findings.md` is exactly this class of break). Delimiter injection if the escape is
incomplete. Byte overhead on every answer; keep it under ~3%.

**First experiment.** Twenty notes whose bodies deliberately contain the delimiter, fenced code, and a closing tag.
Assert that every emitted block is balanced (a parser that counts opens and closes), measure the byte overhead, and
run the existing client once to confirm the schema is unchanged. Decisive: zero broken blocks and <5% overhead.

---

## 3. Loop detection by consecutive identical calls

**Mechanism.** Fingerprint each tool call by `(name, args)`; refuse it once it repeats consecutively past a
threshold (default 3), substituting an actionable message. "Counting is consecutive, not windowed" — an agent
re-running the same check between edits is doing legitimate work. "Refused calls still emit exactly one tool result
each, in the model's original call order" (`docs/adr/ADR-273-rvagent-harness-reliability-floor.md:83-98`). The ADR
also says the thing most harness authors get wrong: "Raising `max_iterations` does **not** fix loops; it makes them
more expensive. `max_iterations` is a cost cap, not a loop guard" (:94-95).

**Source.** `docs/adr/ADR-273-rvagent-harness-reliability-floor.md:83-98`; status "shipped" at :178. The
implementation is `crates/rvAgent/rvagent-core/src/graph.rs:116` (private `LoopDetector`), `:141` (fingerprint over
name + canonical JSON args), `:150` (`observe`), `:169` (the refusal message), call site `:330`; default threshold 3
at `:96`. The code refuses on the third identical call, not the fourth — the ADR's "past a threshold" is looser than
the shipped behaviour.

**What it would change in 2brain.** `tb-mcp-rs/src/lib.rs` (the adapter) or `serve.rs`, before dispatch: keep the
last fingerprint and a consecutive counter per session; on the third consecutive identical call, return a refusal that names
the repeated call and suggests the deterministic alternative (`tb_reconcile`, `tb_diagnostics`, a narrower query).
No vault change, no new state on disk. 2brain has a mutation budget (K=3 per run) in the prompt FSM but no
read-loop guard; a model can call `tb_search` with the same query indefinitely and pay for it.

**Cost.** S. One small stateful guard plus tests; less than 100 lines.

**Risk.** Refusing a legitimate repeated read — hence consecutive, not windowed, and reset on any different call.
A loop that alternates between two calls is not caught; the ADR says so and leaves `max_iterations` as the
backstop. If the refusal does not keep tool-call/result pairing, the client's protocol breaks.

**First experiment.** Replay a week of the policy ledger (or `mcp-stderr.log`) and count runs containing three or
more consecutive identical `(tool, args)` pairs. If the count is zero, the guard is cheap insurance but not a
measured win; if it is non-trivial, record which tool and which argument repeated — that names a tool whose error
message is not actionable.

---

## 4. Hold the tool surface to 8-15 tools

**Mechanism.** "Hold the builtin surface at 8-15 tools. Routing accuracy degrades 16-23 points across large
catalogs; vendors document degradation past 30-50 tools" (`docs/adr/ADR-273-rvagent-harness-reliability-floor.md:63`,
§3.4). MCP servers are gated behind explicit per-session enablement; "exposing an MCP firehose directly into the
tool list forfeits this" (§3.4).

**Source.** `docs/adr/ADR-273-rvagent-harness-reliability-floor.md:63`; full section at §3.4.

**What it would change in 2brain.** The tool surface, not the engine: a small default profile (orient, search,
content, references, types/frontmatter, validate, diagnostics, write, edit, batch, reconcile — about 11), with the
other 41 available only after an explicit `profile` call or a per-session flag. This is a `tb-mcp-rs` change plus
`docs/tools.md` plus client configuration. The engine keeps all 52 implementations; the model sees a menu it can
route over.

**Cost.** M. Profiles, two modes, a compatibility pass over every existing caller, and tests that the hidden tools
still work when enabled. No new crate and no new state.

**Risk.** An agent cannot discover a capability it needs; a profile that omits `tb_reconcile` turns an
`outcome_unknown` into a stuck run. Splitting read and mutate profiles can break a workflow that alternates them.
The 8-15 figure is from coding harnesses, not from memory reads; the direction is corroborated, the exact number is
not.

**First experiment.** Run the twelve tasks in `tools/ranking/tasks.json` twice against the live vault: once with the
full 52-tool surface, once with an 11-tool profile. Count (a) tool-selection errors (wrong tool, missing required
argument, retry after a refusal) and (b) calls per answered task. If the profile is not worse on evidence and uses
fewer calls, the 52-tool default is a liability with a number attached.

---

## 5. No witness, no mutation

**Mechanism.** The audit record is emitted before the mutation commits, and the mutation call is unreachable if
emission fails: "If `emit()` returns `Err`, the partition is not created. The invariant **no witness, no mutation**
is enforced by control flow" (`docs/adr/ADR-134-witness-schema-log-format.md:317-340`; the invariant is stated at
:17). The record is fixed-width and hash-chained — sequence, timestamp, action kind, proof tier, actor, target,
capability hash, payload, `prev_hash`, `record_hash` (:66-86) — and replay verifies the chain before applying
(:394-420).

**Source.** `docs/adr/ADR-134-witness-schema-log-format.md:17,66-86,317-340,394-420`. Three implementations
exist, and their differences matter:

- `crates/ruvector-agent-memory/src/ledger.rs:651` (`TransactionalLedger::emit_and_apply`) is the closest to a
  memory product: chain the witness batch, `self.sink.emit_batch(&witnesses)?`, and only then apply effects. A
  refused sink returns `LedgerError::WitnessRejected` (`ops.rs:142-144`) and no state is applied. `MemoryWitnessLog::verify_chain`
  (`ops.rs:415`) checks the chain; `LedgerWitnessRecord::to_bytes` (`ops.rs:304`) keeps the ADR-134 64-byte shape.
- `crates/rvm/crates/rvm-witness` implements the invariant for the hypervisor, but the current write format is v2
  (96-byte, keyed-BLAKE3 chain MAC, `v2.rs:320 try_append -> Result`), and the ADR-134 64-byte record is now
  verify-only legacy (`lib.rs:1-24`). The ADR describes v1; the code has moved.
- `crates/cognitum-gate-tilezero/src/receipt.rs:9` (`WitnessReceipt`, BLAKE3) is a receipt log, not a fail-closed
  gate: `decide()` signs the token, calls `emit_receipt(...)` and returns the token regardless
  (`lib.rs:204-260`), and `verify_receipt_chain` (`lib.rs:294`) has exactly one caller — an example
  (`examples/receipt_audit.rs:16`). Its `replay` is a stub (`lib.rs:270-280`).

**What it would change in 2brain.** `mutate.rs` and the policy ledger. 2brain already writes a hash-chained ledger
(`docs/research/hermes-log-findings.md` §2) and already has a saga per mutation. The delta is three things: (a) the
record names the **read set** — which note hashes the writer read to justify the write — not only the policy
decision; (b) the record carries the content hash of the note **after** the write, so replay compares state, not
just intent; (c) ledger-append failure blocks the commit instead of logging after it. `serve.rs` exposes a
`tb_witness` read for a range or a note; `tools/` documents it.

**Cost.** M. A record-shape change, a replay checker, a fail-closed path in the saga, and tests that inject a
ledger failure. No new crate; the ledger exists.

**Risk.** Availability: a full or read-only ledger stops all writes, which is the point but is also an outage mode.
Disk growth if every read is recorded — record the read set only for mutations, not every `tb_content`. Replay is
only deterministic if the record captures enough to reconstruct the write; a partial payload makes the checker
lie.

**First experiment.** Perform 50 mutations; for each, replay the ledger against the tree and compare the recorded
post-write hash with the actual file hash. Then make the ledger directory read-only and attempt one mutation:
it must refuse with a policy-shaped message and leave the tree byte-identical. Decides whether today's ledger is a
log (written after) or a witness (required before).

---

## 6. Measure the task set before trusting any change to it

**Mechanism.** "Before trusting any A/B of a harness change, **verify test determinism** — run F2P/P2P repeatedly (fail-to-pass / pass-to-pass tests)
under gold and base patches. Weak tests inflate resolve rates by ~6.4 pp, and 1 in 5 'solved' patches on a
saturated benchmark are semantically incorrect. A 3-point improvement sits inside the flaky-test noise band and
means nothing" (`docs/adr/ADR-273-rvagent-harness-reliability-floor.md:165-171`). And from the Darwin run notes:
"Only trust batch evaluation of the final artifact — in-loop counters drift 1.5-5×"
(`harnesses/timesfm-harness/.claude/skills/evolve/SKILL.md:51`).

The Darwin driver makes the same point operationally: a fixed seed (7), fixed generations and children, an
explicit `promotionDelta: 0.05`, and a checkpoint per generation
(`harnesses/timesfm-harness/scripts/evolve-openrouter.mjs:47-63`). Reproducibility is a small set of knobs written
down, not a claim that the run was careful.

**Source.** `docs/adr/ADR-273-rvagent-harness-reliability-floor.md:165-174`;
`harnesses/timesfm-harness/.claude/skills/evolve/SKILL.md:43-52`;
`harnesses/timesfm-harness/scripts/evolve-openrouter.mjs:47-63`.

**What it would change in 2brain.** `tools/ranking/` (task set and harness) and `docs/`. Not the engine. Add a
determinism pass to the task set: every task runs twice under identical conditions and the harness refuses to
report a delta when the two runs disagree. Add a batch mode that scores the final artifact only (the answered
pack), never in-loop counters.

**Cost.** S. A runner flag, a diff check, and a report column.

**Risk.** A slower loop; an over-strict test that rejects a real but small improvement. The twelve-task set is
small, so the confidence interval is wide no matter what — the discipline tells you that instead of hiding it.

**First experiment.** Run each of the twelve tasks three times and compute the spread of hit@1 and of
`evidence_paths`. If a 3-point delta is inside that spread, no ranking change of that size is measurable on this
set, and the next ranking argument is settled by argument rather than by data.

---

## 7. An evidence dossier with allowed and blocked claims

**Mechanism.** A note states a question, a grade (A-D), whether human review is required, and two explicit lists:
what may be claimed and what may not, with citations. The file is small enough to quote whole
(`docs/evidence/mri-prior.md:1-17`): grade B, "claim allowed with citations", `Allowed claims` containing one
sentence, `Blocked claims` empty, one citation, and the line "Candidate evidence. Promotion to the curated cache is
a reviewed step." `pathology-review.md:5` shows the other axis: `Human review required: true`.

**Source.** `docs/evidence/mri-prior.md:1-17`; `docs/evidence/pathology-review.md:1-9`.

**What it would change in 2brain.** The type system (`evidence` note type with `grade`, `human_review`, `allowed`,
`blocked`, `citations`), `search.rs` (annotate a hit whose claim is blocked), `mutate.rs` (refuse a write whose
claim the dossier blocks — a new refusal stage beside `prior_read` and `freshness`), and `docs/`. It is the
strongest small idea in the corpus: a memory that records what you may **not** say is more trustworthy than one
that only records what you know.

**Cost.** M. A type definition, a validation rule, a refusal stage, and the authoring of the first dossiers. No new
state beyond the notes.

**Risk.** A stale dossier silently blocks a true claim; the grade letters invite false authority; maintaining the
allowed/blocked lists is real work that will be skipped. The dossier's timestamp (`Refreshed`) must be checked, or
a six-month-old grade becomes a fact.

**First experiment.** Write five dossiers for 2brain's own contested claims (the `scope` default, the `weak` flag
result, the deployed-binary lag, the rename cascade, the tool count). Take ten questions from the task set and
count how many answers would be blocked or annotated. If the answer is zero, the lists are empty formality; if it
is high, the dossiers are doing work no search rank can do.

---

## 8. A promotion gate with four independent conditions, and an evaluator that cannot promote

**Mechanism.** Promotion requires four separately measured conditions, any one of which vetoes: utility above 90%
of baseline, attack success below 5%, persistence of a compromise below 1%, and demonstrated recovery to a
verified-clean state. "A mutation failing any one of the four is vetoed ... regardless of its performance on other
axes" (`docs/adr/ADR-317-harnessrisk-lifecycle-security-benchmark-gate.md:87-96`). And the gate has no promotion
authority: "this benchmark gate is evaluative only. It has no promotion authority of its own — a passing ... rerun
is one necessary input ..., never a self-sufficient approval" (:101-104).

**Source.** `docs/adr/ADR-317-harnessrisk-lifecycle-security-benchmark-gate.md:87-104`. Two code counterparts exist
and are closer to the memory use case than the security ADR:

- `crates/ruvector-agent-memory/src/diagnostic.rs:556` (`evaluate_promotion`) is a joint **three**-condition bar for
a memory-strategy artifact: (a) paired improvement passes the outcome gate; (b) a protected held-out slice does not
regress; (c) a diagnostic trace localizes the change to exactly one of four pipeline stages. It has a third outcome
besides promote and block — `MonitoredFlag`, a plausible but unproven candidate retained for observation and never
served as accepted knowledge (`PromotionDecision`, `BlockReason`).
- `crates/sona/src/darwin_guard.rs:45` (`IntentJudge::veto`) and `:54` (`NoJudge`) encode the separation in one
type: a frozen judge "may only VETO a candidate, never set its reward"; the test is literally
`judge_vetoes_but_does_not_set_reward` (`:255`). `Guard::screen` also rejects a NaN candidate rather than scoring
it zero, because zero can still win (`:59-120`).

**What it would change in 2brain.** The type system and `mutate.rs`: a `tb_promote` operation that moves
`status: proposed → accepted` only when four independent checks pass — (a) evidence grade is `recomputed` or
`signature-verified`; (b) `tb_diagnostics` is clean; (c) the claim was re-read from the deployed artifact, not only
from the note; (d) a rollback preview reports safe. Borrow the third verdict as well: a note that fails only (a)
becomes a monitored `proposed` note with a reason, not a rejected one. `tools/` exposes the four checks separately, never as one
blended score, and the evaluator (a client or a judge) can veto but cannot itself flip `status`.

**Cost.** M. One new mutation op, four checks, a report shape, tests. No new crate.

**Risk.** Perpetual `proposed`: a note that cannot demonstrate recovery stays proposed forever, which is correct
but will feel like a stuck queue. The gate becomes ceremony if the four checks are computed by the same code that
wants the promotion; the separation is the mechanism, not the four names.

**First experiment.** Take ten `proposed` notes and compute the four pass counts independently. If all ten pass all
four, the gate is too weak to be worth building; if none pass, it is too strict and the checks need loosening
before the op exists.

---

## 9. A doctor call as a pre-flight gate

**Mechanism.** Four PASS/FAIL checks run before work starts — kernel loads, version is a non-empty string, backend
is one of the allowed values, host adapter has a name — each printed, and the process exits non-zero if any fails
(`harnesses/timesfm-harness/bin/cli.js:26-45`). It needs no provider and no network, so it can run in CI and before
a batch.

**Source.** `harnesses/timesfm-harness/bin/cli.js:26-45`; the command contract in
`harnesses/timesfm-harness/.claude/commands/doctor.md`.

**What it would change in 2brain.** `tools/` plus `serve.rs`: a `tb_doctor` that checks daemon reachability, lock
ownership, index freshness (last commit vs index build), grant-file parse, ledger chain continuity, and
engine/client version agreement, and returns a table with a non-zero exit. This is not `tb_diagnostics`, which
checks the vault's content; it checks the service around the vault.

**Cost.** S. Six checks and a table; most of the data already exists.

**Risk.** A false FAIL blocks a batch that would have worked; checks that take seconds defeat the purpose. Fatal
and degraded must be distinct: a stale index is degraded (reads work), a broken ledger is fatal (writes must stop).

**First experiment.** Run the check twenty times on the live vault; record wall time and any flaky FAIL. Then inject
each fault once (stop the daemon, touch a grant file, truncate a ledger line, deploy an older binary) and confirm
the matching check fails and the others pass. Decides in under an hour whether it is a gate or a dashboard.

---

## 10. A per-file manifest with a witness over the manifest

**Mechanism.** A generated bundle records a SHA-256 for every emitted file and a witness hash over the manifest
itself; provenance records the generator, version, command line, and a feasibility score. The manifest lists
`.claude/commands/doctor.md`, `src/agents/architect.ts`, and the rest with their hashes
(`harnesses/timesfm-harness/.harness/manifest.json:14-35`); `.harness/manifest.sha256` is the witness, verified with
`sha256sum` (`PROVENANCE.md:55-65`). A reader can prove the set it received is the set that was produced.

**Source.** `harnesses/timesfm-harness/.harness/manifest.json:1-36`;
`harnesses/timesfm-harness/PROVENANCE.md:55-67`.

**What it would change in 2brain.** `mutate.rs` batch response and `tools/`: `tb_batch` returns a manifest of
`path → pre-hash, post-hash, action` plus a digest over the manifest. `tb_content` can verify a note against the
manifest of the commit that wrote it. This extends `preview_mutation`, which already lists the affected paths and
the kind of change; the manifest adds the hashes a third party can check without trusting the writer.

**Cost.** S/M. A response shape, a digest, and one verifier; no new state if the manifest is returned rather than
stored. Storing it costs a file per batch.

**Risk.** A manifest generated before the commit drifts from what landed; a manifest generated after it is a claim,
not a witness. Hash-algorithm churn breaks old manifests. Clients ignore a field they did not ask for.

**First experiment.** Run one 20-file batch, generate the manifest, then verify independently with `git status`,
`git diff`, and `sha256sum`. Any mismatch is the drift the manifest exists to catch. Decides whether the batch
response alone is enough or the manifest must be committed.

---

## 11. A trajectory note type with an outcome

**Mechanism.** A pattern record is a fixed shape — task, approach, steps, outcome (`success`/`failure`), a numeric
reward, metadata (agent type, duration, files changed, tests passed), and evidence — and the pipeline around it is
RETRIEVE → JUDGE → DISTILL → CONSOLIDATE (`.claude/agents/v3/reasoningbank-learner.md:163-172`;
pipeline at :37-60). The value is not the RL; it is that an attempt has a recorded verdict, so a later reader can
ask "what was tried and what happened" without replaying a transcript.

**Source.** `.claude/agents/v3/reasoningbank-learner.md:37-60,163-172`. The stored shape is real in
`crates/ruvllm/src/reasoning_bank/pattern_store.rs:220` (`Pattern::from_trajectory`): a pattern keeps
`success_count`, `avg_quality`, `source_trajectories`, `lessons`, and `example_actions` beside its embedding. Note
that this store is HNSW/768-dim vector-backed — the shape transfers, the vector index does not.

**What it would change in 2brain.** The type system (`trajectory` note: `task`, `approach`, `steps[]`, `outcome`,
`evidence`, optional `duration`), `tools/` (a `tb_trajectory` writer that links the notes it touched), and `docs/`.
The evidence field must be a note hash or a command output hash, not a free sentence; otherwise the outcome is a
self-report.

**Cost.** M. A type, a writer, link handling, and the first ten notes. No new state.

**Risk.** Self-reported reward is the classic Goodhart target; an agent that writes "success" without evidence
poisons the type. Bloat: trajectories are numerous and short-lived, and a 1,032-note vault will grow faster than it
consolidates. The type needs a retention rule from day one.

**First experiment.** Write ten trajectories from the existing cross-agent run (including the failed retrieval and
the deployed-binary finding), then give a fresh agent only those notes and ask what was tried, what failed, and
what the next step is. If it cannot reproduce the failed step, the schema is missing the field that matters.

---

## 12. A session-end handoff note with four headings and a delta hash

**Mechanism.** The session-end command specifies a summary with work accomplished, key decisions, problems solved,
and next steps, plus metrics (`.claude/commands/hooks/session-end.md`); what is actually wired at session end is a
git branch/commits/diff digest posted to a remote brain (`.claude/settings.json:80-96`). The transferable part is
the four headings and the delta — the note says what changed and what is next, not what the agent felt.

**Source.** `.claude/settings.json:80-96`; `.claude/commands/hooks/session-end.md`.

**What it would change in 2brain.** `docs/agent-roles.md` (the convention), `tools/` (a `tb_handoff` template), and
optionally a `handoff` type. The note links every decision it names and carries the commit range it covers. The
transport should not be copied: ruvector posts to a remote service with a hardcoded bearer token in
`settings.json:46,90,197`, and the post is wrapped in `|| true`.

**Cost.** S. A template and a convention; no engine change.

**Risk.** Note spam: one per session in a 1,032-note vault is noise unless handoffs are a separate, prunable
series. Summaries that assert rather than link duplicate the claim problem. A handoff written from memory rather
than from the ledger will disagree with the ledger.

**First experiment.** Five sessions, one note each, then a fresh agent reads only the note (not the transcript) and
states the next step and the commit it applies to. Score correct-next-step out of five and note length; if it is
5/5 at under 400 words, the transcript is no longer the handoff.

---

## Two prompt texts to adopt almost verbatim

**The reviewer role card** (`harnesses/timesfm-harness/src/agents/reviewer.ts:4`). The whole prompt is five
sentences, and one of them is a veto rule:

> You review diffs for correctness, security, and reuse. Report only high-confidence findings, each with a file:line
> and a concrete fix. Distinguish a bug (will break) from a nit (style). Never approve a change that widens a
> permission, swallows an error, or ships a secret. You operate inside the timesfm-harness harness; defer
> destructive actions to the user.

For 2brain, three terms change and nothing else: diffs become proposed memory writes, `file:line` becomes the note
path and block id, and bug/nit becomes false claim/style. The veto clause becomes: "Never approve a write that
widens a grant, hides its evidence, or ships a secret." That sentence is the whole review policy, and every clause
in it is checkable by the gate.

**The plan-change procedure** (`harnesses/timesfm-harness/.claude/skills/plan-change/SKILL.md:10-13`):

> 1. Restate the goal in one sentence.
> 2. List the files to touch and why.
> 3. Name the smallest interface that satisfies it.
> 4. Flag anything that ripples beyond three files or widens a permission.

Change "files" to "notes" and "interface" to "claim", and it is a memory-write plan with a built-in scope gate:
a change that touches more than three notes, or widens a grant, stops for review before the write. It costs four
lines and it is the only workflow in the corpus that fits inside a system prompt.

A third line belongs beside them, from `harnesses/timesfm-harness/src/agents/test-writer.ts:4`:
"A test that cannot fail is worse than no test — assert behaviour, not implementation." Its memory form is the whole
of Entry 1 in one sentence: a claim that cannot be checked is worse than no claim.

---

## Prompts and workflows we should not copy

**1. The Rust coder/planner role cards and their performance claims.** `coder.md` advertises "150x-12,500x faster
pattern search", "+12.4% accuracy", "2.49x-7.47x speedup", "SONA <0.05ms adaptation" as if measured; the same
sentences appear under `planner`, `reviewer`, `tester`, and `researcher`. Not transferable on two counts: the
content is Rust implementation advice, and the numbers are stated without a dataset, configuration, or run, then
repeated until they read as fact. A memory product whose differentiator is verifiability cannot ship a prompt that
asserts measurements it cannot show.

**2. The keyword router with fixed confidence.** `helpers/router.js:18-52` maps regexes to agent names and returns
`confidence: 0.8` for a regex match, `0.5` for the default, and routes unknown work to `coder`. The confidence is a
constant, not a probability; there is no calibration and no way for the number to be wrong. 2brain's FSM already
does better: deterministic guards where code can decide, and the engine's own words in the refusal.

**3. Implicit relevance feedback and self-reported reward.** `helpers/intelligence.cjs:670-707` boosts the
confidence of the "last matched patterns" by `+0.05` on task success and `-0.02` on failure, where success means
the session continued. `.claude/intelligence/index.js:403-470` runs a Q-table whose reward comes from the same
session. The repository's own calibration file shows the result: every prediction at confidence 0.9 is recorded as
correct (`4,369 / 4,369`), and the 0.6 bucket is `0 / 3`. That is not calibration; it is a mirror. 2brain's
ranking is deterministic and auditable on purpose; adding a learned weight without a ground-truth signal would
make every rank unexplainable.

**4. Mandatory status writes every 30-60 seconds.** The worker protocol requires a status record at task start,
"every 30-60 seconds", on each step, and at completion, into the shared coordination namespace
(`.claude/agents/hive-mind/worker-specialist.md:19-60,177`). In 2brain that would be a write every minute into the
same tree the gate protects, each one a saga, each one a commit, each one a note a reader must skip. If in-flight
state is needed, it belongs in an ephemeral tier with a TTL, not in the durable vault.

**5. Prose workflows with no machine-checkable exit, orphaned hooks, and unresolvable citations.** The SPARC
command set defines eleven modes and instructs the agent to end with `attempt_completion`; nothing checks it
(`.claude/commands/sparc/sparc.md:10-45`). The five scripts in `.claude/hooks/` (`crate-context.sh`,
`rust-check.sh`, `post-rust-edit.sh`, `bench-runner.sh`, `wasm-size-check.sh`) are not referenced by
`settings.json`, which routes every hook to `hook-handler.cjs`; they also hardcode `/workspaces/ruvector`. A
mechanism that is not wired is documentation. And the ADR citations above show the same failure at the level of
provenance: an id that does not resolve is a claim with no evidence. 2brain's FSM is the correct contrast — every
exit is a tool call or an engine read.

---

## What their agent layer reveals about the shape of a multi-agent memory product

RuVector's agent layer is two things that are usually confused: an agent runtime (the rvagent middleware pipeline,
the hooks, the loop) and a memory sidecar (the intelligence store, the namespaces, the Q-table). The runtime is
where the engineering is real, and the sidecar is where the claims degrade. Every mechanism in that layer that
survived contact with a reader binds a claim to a check it cannot perform itself — a witness emitted before the
mutation, a manifest hash over the emitted files, an evidence grade that says who recomputed what, a promotion
gate whose four conditions are measured separately and whose evaluator is forbidden to promote. Every mechanism
that did not survive is one where the agent grades itself: the confidence constants, the Q-values, the
"150x" claims, the calibration file that is 100% correct at every bucket. The distinction is not stylistic. A
shared memory is read by an agent that cannot see the writer's session, so the only thing that transfers is what
the note can prove on its own; a system that lets the writer assert its own quality has moved the trust problem,
not solved it. That is also the answer to the product question. Their layer shows that multi-agent memory is not a
store with a search box; it is an evidence ledger with a deliberately small tool surface, where the writer's read
set is part of the record, the reader's inputs are delimited and hashed, the claims carry how they are known, and
promotion is decided by something that is not the promoter. 2brain already has the substrate for that — git, typed
notes, one writer, a fail-closed gate, a hash-chained ledger, a `proposed → accepted` lifecycle. What it lacks is
the epistemic layer (grades, blocked claims, independent promotion) and the reader protections (delimiters, loop
detection, a tool menu small enough to route over). Those are the twelve entries above, and they are cheaper than
the vector index the project deliberately did without.

---

## Addendum (same day): the code-graph check

After the first pass I ran the project's code graph (4,492 files; 21,785 edges unresolved, 8,050 of them
external/std) against the two mechanisms that mattered most, then against the rest. The point was to separate
"written down" from "called by something other than its own test". The index cannot see JavaScript, Python, or
prose, so "no symbol" below means "not found in Rust", not "does not exist".

| Entry | Symbol | Verdict |
|---|---|---|
| 1 Evidence grade | `crates/ruvector-agent-memory/src/ops.rs:221` (enum), `:230` (`code`), `:254-258` (bound into witness `flags`) | **Implemented**, and stronger than the ADR: the grade is in the hash preimage |
| 2 Delimiter / trust | `crates/rvAgent/rvagent-middleware/src/tool_sanitizer.rs:14,22`; `.../witness.rs:301`; `crates/rvAgent/rvagent-core/src/config.rs:50,60` (`trust_agents_md: false`) | **Implemented** |
| 3 Loop detection | `crates/rvAgent/rvagent-core/src/graph.rs:116,141,150,169,330`; default `:96` | **Implemented**; refuses the third identical call, not the fourth |
| 4 Tool surface 8-15 | none | **Text only**: a recommendation in ADR-273 §3.4, enforced nowhere |
| 5 No witness, no mutation | `crates/ruvector-agent-memory/src/ledger.rs:651`; `ops.rs:142-144,304,415`; `crates/rvm/.../v2.rs:320`; TileZero `receipt.rs:9` | **Implemented in three shapes** with different guarantees (below) |
| 6 Measure the task set | `crates/ruvector-sota-bench/harness/src/statistics.ts` (paired bootstrap); determinism check is policy | **Partially code**, mostly discipline |
| 7 Allowed/blocked dossier | none; `docs/evidence/*.md` is a format | **Document only**; the refusal stage is our design |
| 8 Promotion gate | `crates/ruvector-agent-memory/src/diagnostic.rs:556`; `crates/sona/src/darwin_guard.rs:45,54,255` | **Implemented**, with a third verdict and a veto-only judge |
| 9 Doctor | `harnesses/timesfm-harness/bin/cli.js:26-45` | Implemented in the generated harness, not in core |
| 10 Manifest | `.harness/manifest.json` + `manifest.sha256` | **Artifact**, not a symbol; the generator is external |
| 11 Trajectory | `crates/ruvllm/src/reasoning_bank/pattern_store.rs:220` | **Implemented**, vector-backed |
| 12 Handoff | `.claude/settings.json:80-96` + a command doc | **Hook/config only** |

### What the graph found that the `.claude/` slice missed

`crates/ruvector-agent-memory` is itself a memory product, and it is closer to 2brain than anything under
`.claude/`: a transactional ledger with five operations over `pending` / `accepted` / `rejected`, a proof gate
before acceptance, witness-first application, replay, evidence grades, a staged promotion gate, and
correlation-aware arbitration. Three parts deserve to be read even though they did not make the twelve:

- **Evidence lineage, not vote count** (`crates/ruvector-agent-memory/src/arbitration.rs:1-90`). N memories that
  declare derivation from one source are "one observation repeated N times, not N independent observations". The
  module clusters remembered items into lineages by declared `causal_parents` and scores effective evidence per
  lineage. Its own limitation statement is the best sentence in the repository: correlation is detected from
  declared parents, never from content, so "twenty agents that each re-assert one rumor's content without citing
  it" are treated as twenty independent roots; the diagnostic to watch is `independent_lineages`, not the score
  delta. If the twelve were re-opened, this would rank around fifth: a shared memory's characteristic failure is
  not a wrong fact, it is one fact counted many times.
- **Atomic observations with signatures** (`observation.rs`, `fusion.rs`): source-tagged observations fuse into a
  causal episodic graph, with SHA-256/Ed25519 content addressing and provenance back to each source. That is the
  schema a multi-agent memory needs; 2brain's note + block ids can carry it without a vector index.
- **A three-way promotion verdict** (`diagnostic.rs`): `Promote` / `MonitoredFlag` / `Blocked`, each with a reason
  and a coverage number. A plausible but unproven candidate is kept as a monitored flag and "never served as
  accepted knowledge" — better than the binary promote/reject I described in Entry 8.

### What the code corrects in the documents

- **ADR-134 describes the legacy format.** `crates/rvm/crates/rvm-witness/src/lib.rs:1-24` says the 64-byte v1
  record is now verify-only and "writing new v1 logs is removed"; the current write format is 96-byte v2 with
  keyed-BLAKE3 chain MACs and `try_append -> Result` (`v2.rs:320`). The invariant and the field list survive; the
  byte layout in the ADR does not.
- **A receipt log is not a gate.** TileZero's `decide()` signs a permit token, emits the receipt, and returns the
  token whether or not the receipt was recorded (`crates/cognitum-gate-tilezero/src/lib.rs:204-260`);
  `verify_receipt_chain` (`:294`) has one caller, an example (`examples/receipt_audit.rs:16`), and `replay` is a
  stub (`:270-280`). It records decisions; it does not require the record before deciding. The fail-closed shape
  is in `ruvector-agent-memory` and `rvm-witness`, not here.
- **The loop threshold is three, not "past three".** `observe()` increments from one and refuses at
  `consecutive >= threshold` (`graph.rs:150-166`), so the third identical call is refused.

The rule I would carry forward: a mechanism is real when a symbol exists **and** a caller other than its own test
or example uses it. By that rule, loop detection, the memory ledger, the tool-result sanitizer, the memory
promotion gate, and the veto-only judge are real; the 8-15 tool budget, the allowed/blocked dossier, and the
four-condition security gate are recommendations. That distinction is worth more than any single entry above,
because it is the same distinction a shared memory product sells.
