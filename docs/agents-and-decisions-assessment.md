# Agents, FSM roles, and a typed decision layer (subq / jev / ruVector)

Status: assessment, 2026-11-16. No code changed. Answers three questions that arrived together:
*how to introduce FSM/agents and what their roles are*, *whether the subq Decisions API fits*,
and *at which stages it enters the trajectory*. Every vault claim below was read through
`arsumbris-rs`.

## 1. The role split that decides everything else

An agent run contains two kinds of steps, and conflating them is the failure mode:

| role | who does it | why it cannot be the other |
|---|---|---|
| **Orchestrator** — order, retries, budgets, who may write | deterministic FSM (code) | a model choosing its own next state cannot be audited, cannot be replayed, and changes behaviour between runs |
| **Capability holders** — read, propose an edit, audit a result, classify a note | agents inside a state | the state's exit check is code, so the agent cannot declare success |
| **Judges** — bounded yes/no, choice, score | typed decision endpoint (subq or local `jev`) | free-text judgement is not machine-checkable and cannot be thresholded |
| **Truth** — what is true about the workspace | files + git + the engine's reads | no model, ever |

This is the same split our own vault recorded twice: Meta's thin orchestrator (workflow templates
as deterministic state machines, the model only at the delegation step) and ReactiveGraph
("deterministic FSM orchestrates, WASM executes, LLM is a capability, never the orchestrator").
The rule that keeps it honest: **only the runtime may declare a state complete.**

### 1.1 The four capability agents worth having here

Each is a prompt + a tool whitelist + entry/exit checks — in product form an `agent-profile`
instance, a type our SDK already defines (`tb-mcp-sdk/type/agent-profile.type.yaml`: adapter,
skills, injects, allowed tools).

| agent | state it lives in | tools it may use | exit check (code) |
|---|---|---|---|
| **librarian** | ingest a new note | `tb_grep`, `tb_content`, `tb_frontmatter`, `tb_create_file` | type is one of the nine, `source_url` present, links resolve, `tb_diagnostics` unchanged |
| **editor** | change one note | `tb_content` → `tb_write_file` with the returned hash | one commit, hash carried, no second write on `outcome_unknown` |
| **auditor** | after a mutation | `tb_content`, `tb_graph_health`, `tb_reconcile` | the file reads back with the intended text; `broken_links` did not grow |
| **scout** | answer a question | the read surface only | the answer cites note paths that exist |

None of them owns ordering. The FSM runs them: `read → propose → apply → verify → done`, with
`verify` a state whose exit is a deterministic check, not the agent's opinion.

### 1.2 FSM inside the runtime (C1) versus in the agent layer (C2)

- **C1** — the mutation gate is already a state machine (`Stage` 8, `MUTATION_STAGES` 7,
  `CREATE_STAGES` 6). P2 makes it *legible* (pure reduction, `Prepared` as a value, a refusal that
  names what unblocks it). No model is involved and none should be.
- **C2** — for repeated procedures only, and only where a state list is discoverable from the
  checks that already exist (our `scripts/*-turn.sh`). For open work (design, debugging, research)
  a declared state list is a checklist that hides the case it did not anticipate.

## 2. The subq Decisions API, measured against what we need

**What it is** (docs.subq.ai, read 2026-11-16): `POST /v1/systemone`, model `subq-snap-1.2`,
body `{model, state, questions, images?}`, 384K-token context, up to 128 named questions of three
types — `noul` (probability of yes), `choice` (2–255 options, probability + confidence), `score`
(2–10 ordered levels, probability-weighted). Text or images in, typed answers with probabilities
out; no files (extract text first). Auth is a bearer key; a request without credits is `402`.

**Why it is a candidate at all:** our vault already recorded it as a pattern
(`f/patterns/pattern-decision-layer-agent-work`) — the small decisions inside an agent run go to a
cheap structured endpoint ($0.04 per 1M tokens in the vendor formula) while the frontier model is
kept for hard reasoning — and noted that our local `jev` speaks the same `systemone` shape, so
"integration is a base_url swap". That last sentence is the important one: **the wire is already
ours; the question is only whether the vendor's model is better than what we run locally.**

**Cost of the obvious first job.** One full stale-description audit of the vault is 1 036 notes,
5.4 MiB of text ≈ 1.4 M tokens ≈ **$0.06 per pass**. Latency per call is not published; measuring
it is part of the spike, not an assumption.

## 3. Where it fits: stage by stage

| stage | question to ask | type | verdict |
|---|---|---|---|
| **ingest: stale description** (backlog P0-3) | "does this description still match the body?" | `noul` | **spike now** — offline, bounded, six cents a pass, and it is a defect we already logged |
| **ingest: type and topics** | "which of the nine types is this note?" | `choice` over 9 + `unclassified` | **spike with the same harness** — measured against the labels the notes already carry |
| **ingest: duplicate check** | "is this note the same claim as that one?" | `noul` on a pair | **after** the two above: it needs the type axis to be trustworthy |
| **retrieval: rerank top-k** | "is this note actually relevant to the query?" | `score` | **not in the serving path.** A network call cannot meet the ≤15 ms target T1 is held to; if wanted, it becomes a `tb_rerank` tool the *agent* calls, never the server |
| **agent routing** | "which capability should handle this request?" | `choice` over 4 roles | optional, in the harness, after the roles exist |
| **mutation approval** | "is this edit safe?" | any | **no.** A deterministic gate plus git is strictly stronger; adding a model to it removes the property that a refusal is reproducible |
| **schema/wire validation** | "is this answer well-formed?" | any | **no.** That is what the declared schemas and the recursive type tests are for |

## 4. The data-egress decision this forces

The vault is private. A stale-description audit sends note bodies to a third party. That is not a
technical choice, it is a policy one, and it must be made explicitly before the first real call:

- **Option A — local first.** Run the same `systemone` wire against a local judge (our `jev`
  client) and only move to the vendor if the local verdicts are measurably worse on a labelled
  sample. No egress, no key, no vendor lock.
- **Option B — vendor, offline only, opt-in per run.** The audit is batch, there is no live
  dependency, and a run can be cancelled; the cost is that note text leaves the host.
- **Option C — vendor in the serving path.** Rejected: it makes the MCP server depend on a network
  service, breaks the "files are the truth" story, and cannot meet the read budget.

**Default: A, with B as an explicit opt-in for a labelled comparison.** That default costs nothing
and answers the only question that matters — whether the vendor's probabilities are better than
ours on *our* notes.

## 5. ruVector: a pattern source, not a dependency

The vault has 449 mentions and a decision already: `knowledge/zeroclaw-stack-hnsw-rs-plus-tantivy-plus-petgraph-plus-fastembed`
records RuVector **rejected** for a production memory stack (168 downloads, single author, 90+
crates of unclear readiness) in favour of `hnsw_rs` + `tantivy` + `petgraph`. Nothing measured
since then changes that for us: our own retrieval gap is **text** (measured: 132.8 ms scan versus
0.003–0.56 ms for a BM25 prototype), and vector search is a *maybe* that the same note prices at
one crate (`hnsw_rs` 1.4 M downloads) rather than a stack.

What is worth keeping from that family is the **design rule**, which our vault also recorded:
filter by graph membership first, then rank inside the subgraph (`knowledge/hybrid-search-combines-cypher-filtering-with-hnsw-in-subgraph`),
with exact membership — the same rule HelixDB implements and the same one our T1 must follow.

## 6. What enters the trajectory

**Wave 4 — judgement layer (offline only).** Two tracks, both outside the serving path:

- **W4.1 ingest auditor.** A `systemone` client (local `jev` first) that runs three questions over
  the vault: stale description (`noul`), type (`choice`), duplicate (`noul` on candidate pairs
  produced by a BM25/T1 pass — the ranker feeds the judge, never the reverse). Deliverable: a
  report over the live vault and a hand-labelled 50-note sample with precision per question.
  Acceptance: precision ≥ 0.9 on the labelled sample at the chosen threshold, ≤ $0.10 per pass if
  the vendor is used, and no file in the vault modified by the run.
- **W4.2 role definitions.** The four agent roles from §1.1 written as `agent-profile` instances in
  `tb-mcp-sdk`/vault (documents, not code), each with its tool whitelist and its exit check, so
  P3's procedures can name a role instead of a prompt.

**Not in the trajectory:** any judge call inside the read path; any model in the mutation gate; any
vector dependency before T1 has landed and hybrid retrieval is shown to be wanted.

**Order:** W4.1 can start as soon as T1's engine module exists (it uses the same tokenisation for
its candidate pairs); W4.2 is documentation and can run in parallel with anything.
