# The axes we can move, what each costs, and the number that decides it

Written 2026-11-16, after a day that measured more than it planned. Every proposal below names the axis it moves, the
number it would change, and the smallest experiment that decides whether it is worth building. Numbers in the tables are
measured on this host unless the text says otherwise.

## The fixed cost nobody has counted: the tool surface

`tools/list` from the adapter built at `HEAD` is **58 847 bytes — about 14 700 tokens — before an agent has asked a
single question.** Split: 12 340 bytes of descriptions, 21 466 of input schemas, the rest names and framing. Thirty-six
tools are advertised; the engine behind them exposes more verbs than the adapter names, and the extra verbs cost nothing
because they are not on the wire.

That reframes "reduce the number of tools": the count matters less than the bytes, and the bytes are mostly schema. Three
levers, in order of payoff per unit of risk:

| lever | saves | cost | risk |
|---|---:|---|---|
| a one-line description on the wire, the long one behind `tb_describe_tool` (which already exists) | ~10 KB | S | a client that never calls `describe` sees less; the short line must still say when to use it |
| drop `offset`/`limit` from read schemas that never page (10 of 36), keep them where they matter | ~3 KB | S | a caller wanting the second page of a non-paged read gets a clear error |
| collapse mutate verbs that differ only in the object (`tb_set_meta`, `tb_set_slot`, `tb_toggle`, …) into one with a `field` | ~6 KB and 4 tools | M | a generic verb is easier to misuse; the gate must stay per-field |

Nothing here needs a new tool. The first two are an afternoon and would cut the per-session cost by a fifth.

## Axis 1: fewer, deeper tools

Measured today: 36 advertised, 33 read and 16 mutate at the engine, and a document (`tb-mcp-rs/docs/progressive-disclosure.md`) that already
describes a ladder nobody implemented end to end.

The shape worth building is not fewer verbs but **fewer entries with a mode** — each entry answering one question an agent
actually has:

| question an agent has | today | proposed entry |
|---|---|---|
| what is in this base | `overview`, `hubs`, `type_counts`, `top_level_dirs`, `files` | `tb_orient` (one call, all five folded, tokens budgeted) |
| what does the base know about X | `search` + N × `content` | `tb_context` (ranked evidence pack, 1 call, 2.44 KB measured) |
| read this exactly | `content`, `block`, `anchors`, `children` | `tb_read` (path + optional selector) |
| what points here | `references_in`, `references_out`, `follow`, `neighborhood` | `tb_links` (direction + depth) |
| is this consistent | `diagnostics`, `validate_value`, `graph_health` | `tb_check` (scope + what to check) |
| change it | 16 mutate verbs | `tb_write` (op + args) + `tb_preview` |
| what is my standing | `instances`, `types`, `describe_tool`, `capabilities` | `tb_schema`, `tb_help` |

That is seven entries against thirty-six. The experiment that decides it is a routing test, not an argument: 24 task
descriptions, an agent asked to pick a tool by name only, once with the current surface and once with the collapsed one.
The metric is wrong-tool rate and the count is 24 × 2. `ruvector`'s ADR-273 recommends 8–15 tools and cites a 16–23 point
routing collapse above that, but the ADR is a recommendation with no code behind it (their agent lane proved that), so our
own routing test is the evidence that counts.

## Axis 2: token-optimality per answer

We measure bytes per answer (`tools/kb/context_pack.py`) and visits per read (`graph_health {verbs:true}`), and we do not
measure **tokens per answer** anywhere. That is the number a client pays for, and the only one that justifies or kills every
"more context" idea.

Proposed metric, collected by a small extension to the probe: `tokens_per_answer = len(json)/4` (the crude constant, stated
as such), reported beside `calls_per_answer` for the twelve-task pack. Current baseline: 1.00 call, 2.44 KB ≈ 610 tokens.
The next increment is the pack's own `excerpt` width: 240 characters is 60 tokens; 400 would be 100. That is a knob with a
measurable price.

## Axis 3: non-linear logic

Right now every read is a straight line: query in, page out. Three cheap departures, each a mode of an existing entry
rather than a new tool:

1. **Abstention.** `absent` is 0/3 — the base cannot say "I do not know". The fix is not ranking: it is an answer field
   (`confidence: answered | weak | no_evidence`) computed from `matched_terms`, `best_idf` and the total, and a task set that
   scores abstention separately. Without it, every no-answer looks like an answer, which is the failure mode that poisons a
   shared memory.
2. **Next-question suggestion.** A query that matches two disjoint clusters is two questions. The index already holds the
   co-occurrence needed to say which terms would narrow it; a `suggest` field costs one pass over the postings we already
   decoded. First experiment: for the 12 tasks, does the suggested term appear in the intended note's title?
3. **Branch by budget.** `tb_context(budget_tokens=800)` returns fewer, better fragments instead of a fixed count — the pack
   already has the loop; it needs a stop rule that is a token count rather than a fragment count.

## Axis 4: the FSM, and where it actually lives

There is no state machine today; the "workflow" is prose in tool descriptions. Two places a state machine can live, and only
one of them is ours:

- **Server-side stage**: the daemon tracks a per-session stage (orient → recall → edit → verify) and refuses a mutation from a
  session that never read the target. That is a real gate, and it duplicates what the write gate already enforces (prior-read,
  freshness hash), so the marginal value is small.
- **Prompt-side FSM via MCP resources and prompts**: the protocol has `prompts` and `resources`, and we ship neither. A
  `2brain://workflow/<stage>` resource and a `prompts/get` for `intake`, `recall`, `handoff` would put the stages where an
  agent reads them — and they cost zero tools, zero schema bytes, and are versioned with the repository. This is the axis to
  move first because it is the only one that does not grow the surface.

The metric for this axis is not aesthetic: **stage conformance** = of the mutations that succeeded, how many were preceded by a
read of the target in the same session (already enforceable, so it is a checkable number), and **handoff completeness** = of the
handoff notes written, how many name a next step and an evidence path.

## Axis 5: structures — cascade, tree, matrix, temporality

| structure | what it would do | state today | decision |
|---|---|---|---|
| **cascade** | plan N edits, preview all, apply as one receipt | `preview_mutation` per verb; `rename_type` is the only cascade | build after the lineage block: a cascade without lineage cannot say what it invalidated |
| **tree** | navigate by type/topic hierarchy | `tb_types`, `tb_children`, `type_closure` exist | keep; measure depth actually used before deepening |
| **matrix** | term×note co-occurrence for query expansion and echo detection | the index holds the postings; nothing reads them as a matrix | cheap and unclaimed: one pass computes "terms that co-occur with a query's terms in the same notes"; the echo work says lexical similarity is useless for lineage, but term co-occurrence is not lexical similarity of documents |
| **temporality** | read the base as of a commit | nothing; git has the history, the engine reads the working tree | the ladder rung that does not need a new store: `tb_content(path, commit)` resolved through `git show`, and a `history` list per path. Bitemporal comes later and only if a consumer asks |

## Axis 6: SIMD and SMP — decided by a number we do not have yet

- **SIMD: not now.** Search p50 is 139 µs and p95 689 µs on 1 032 notes; the scoring loop is a varint walk, and we have no
  profile that says decode is the bottleneck rather than I/O and catalogue lookup. The honest trigger is written down:
  **p95 above 5 ms or a corpus above 50 000 notes**, whichever comes first, and then a benchmark that isolates the decode.
- **SMP: measured, not assumed, and it is the one with a product reason.** The daemon's stated job is to serve several agents
  over one base. If requests serialize, the second agent waits for the first, and that is a defect of the product direction
  rather than of the CPU. First experiment: two clients, the same scoped query, measure wall time for 1 vs 2 concurrent;
  a ratio near 2 means serialization. Only then does a read pool (shared `Arc<KnowledgeBase>`, per-request masks) become a
  question.

## Metrics per axis, and who collects them

| axis | metric | collected by | baseline today |
|---|---|---|---|
| surface | `tools/list` bytes and tools advertised | one call, scripted | 58 847 B, 36 tools |
| surface | wrong-tool rate on 24 routing tasks | new probe | not measured |
| tokens | tokens per answer, calls per answer | `context_pack.py` + `/4` | 610 tokens, 1.00 call |
| ranking | nDCG@10 / MRR on the labelled set | new harness over `labelled-queries.json` + `tasks.json` | not measured (pass/fail only) |
| ranking | bootstrap verdict | `bootstrap.py` | resolution 4 tasks at n=18 |
| abstention | absent accuracy | `tasks_eval.py` | 0/3 |
| cost | visits per read, per verb | `graph_health {verbs:true}` + `tools/probe` | search 402 mean, hubs 836 |
| lineage | echo clusters, independent observations | `lineage-echoes` method | 17 clusters, 2.1x over-count |
| concurrency | wall time at 1 vs 2 concurrent readers | new probe | not measured |
| lifecycle | days from commit to served binary | `deploy-*.md` checklist + `--version` | 7 changes unswapped |

## What we are missing, ordered by how much it costs to keep missing it

1. **A graded ranking metric, built.** Pass/fail at n=18 cannot see a one-task change; nDCG@5 with grades now exists
   (`graded.py`), and it does **not** make every later ranking argument decidable — it makes a moved rank legible. The
   difference is a second opinion's correction (`second-opinion-2026-11-16.md`, O2): the metric is expressiveness, and
   power comes from n and a measured paired σ, neither of which we have.
2. **Token accounting.** We optimise bytes because that is what we count. A client pays tokens.
3. **Abstention.** `absent` 0/3 means the base asserts on questions it cannot answer — the single worst property for a shared
   memory, and the cheapest to fix (an answer field plus a scoring rule).
4. **Lineage.** 36 notes are 17 observations and 13 clusters do not say so; without this, "how many sources support X" is a
   number the base cannot produce honestly.
5. **A deploy that lands.** Seven changes are in the tree and not in the served binary; the state of the product is older than
   its repository, which makes every user-facing claim unverified.
6. **Concurrency.** The product promises several agents on one base and we have never measured two.
