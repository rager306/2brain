# The 38 refusals, decided — and the arithmetic that says the metric cannot move here

A link pass over `/root/vault` on 2026-11-16 linked 34 notes and refused the rest. This document turns every
refusal into a named decision with a reason. It is a **proposal**: nothing in the vault was written, no git command
was run, and no engine mutation was called. Every reference claim below comes from `tb_references` or
`tb_follow` on the live vault, read-only, on 2026-11-16.

Sources read first: `docs/research/intake-2026-11-16.md` (the groups and counts), `docs/research/kb-intake.md`
(the corpus facts), `tb-defaults/project-memory/README.md` (what a typed record needs).

## The count is 38, not 36

The brief says 36. The three groups it names enumerate 38: 28 single-member families + 8 without frontmatter +
2 gate-blocked. The pass table in `intake-2026-11-16.md` implies a 39th refusal — the `moc` with no family
(`No family (1)`). I decided all 39 and marked the `moc` as outside the brief's three groups rather than dropping
it. If the brief's 36 was meant to exclude the two gate-blocked notes, the rest of this document still holds: they
are rows 37–38.

State of the 38 through the engine, before any proposal (edges, not regexes):

| measure | count |
|---|---:|
| notes in the three refused groups | 38 |
| with nothing pointing at them (0 inbound) | 14 |
| with no reference either way | 12 |
| dead ends (0 outbound) | 23 |
| typed instance (`type:` in frontmatter) | 30 |

The last row is the quiet one: the 8 notes without frontmatter are invisible to `tb_instances`, `tb_types` and
every type-scoped query, whatever their content. That is a different defect from an unlinked note and the pass
had no decision for it.

## How to read the table

The decision menu is closed:

- **`link-to <path>`** — the two notes share a mechanism; a `topics:` entry must join them.
- **`adopt-into <family>`** — the family id is provenance, a real topical family exists, and the note belongs to it.
- **`frontmatter-only`** — the note is worth keeping and its type is clear; write the frontmatter block and nothing
  else. `topics:` is part of that block, because the field cannot exist without it; four of the eight rows use it.
- **`leave-alone`** — no edit. I use this literally: a draft, a duplicate, an extract that should not be linked, or
  (the case the pass had no label for) a note whose only missing edge would be a cycle, because the abstraction
  already points at it.

Two conventions the table adds, because an edge has a direction and the counts depend on it:

- **Entry owner.** `link-to` names a pair, but only one of the two notes can carry `topics: [[…]]`. The `entry`
  column says which. Where one note's own text names the other's mechanism, the text decides. Where both
  directions are true, I wrote the entry on the note that already has inbound, so the refused note gains an
  inbound edge instead of a second outbound one — in-degree is the measure the intake note says is the one that
  matters.
- **Acyclicity.** All 13 proposed entries were checked against the engine: the target resolves (`tb_follow`), no
  such edge exists yet (`tb_references`), and the target cannot reach the source within 5 hops of existing edges.
  None of the 13 creates a 2-cycle or a larger one.
  *(After the write: one of them did close a long cycle, beyond this bound — see “The cycle check” under
  “Applied” for the measurement, and for the rule that replaced this one.)*

## The 38 (plus the `moc`)

Group A is "family of one" (28 inbox extracts), B is "no frontmatter" (5 `f/patterns`, 3 inbox), C is
"gate-blocked" (2 `f/patterns`), D is the `moc` the pass also skipped.

| # | note | group | what it says | decision | entry | reason |
|---:|---|---|---|---|---|---|
| 1 | `inbox/infinite-canvas-agent-operated-workbench.md` | A · family of one | An open infinite canvas for AI image/video work; its Canvas Agent is a localhost bridge (127.0.0.1:17371) that a page connects to with a token and an Origin pinned after first use. | **leave-alone** | — | Nothing in the vault shares its mechanism. Its one transferable claim is still a plan in its own text — «Их Origin-pinning стоит оценить для au-mcp bridge» — so a link would point at a note that does not exist. Candidate for a new pattern, not for an edge. This note is fully isolated (0 in, 0 out) and stays so by decision. |
| 2 | `inbox/instabox-muse-free-agent-cloud.md` | A · family of one | A 23.10 post's scheme for a free "home IP" out of InstaBox's free agent microVMs plus Meta's Muse, with the replies explaining why the resulting IP is worthless. | **link-to** | this note | Its own body names the mechanism and the note: «Это дословно аргументация DSec §5 (high-density oversubscription) и NVX-модели». The neighbour is the microVM sandbox design, not the post's scheme. |
| 3 | `inbox/alphaproof-code-libsignal.md` | A · family of one | Google DeepMind's multi-agent formal-verification campaign: 6.5M machine-checked Lean lines over a 26K-line trusted base, a proof accepted only if kernel and comparator both check it. | **leave-alone** | — | Already has 3 inbound edges and one outbound to the swarm extract; its abstraction (`pattern-kernel-gatekeeper-comparator`) already points at it. The reverse edge would close a 2-cycle and add no reachability. |
| 4 | `inbox/little-book-generative-ai-foundations-primer.md` | A · family of one | A complete 8-chapter workthrough of a 195-page generative-AI foundations textbook (PCA→PPCA→VAE→DDPM→score/flow→GAN/EBM), derivations re-verified in sympy. | **adopt-into** | f/ml-foundations-books | The family id is a source (`f/arxiv-2605.29713`, which `source_url` already records), and a real shelf exists: 6 members that are ML-textbook workthroughs. The note is a book workthrough by form. |
| 5 | `inbox/dcoco-parallel-contextual-correction-speculative-decoding.md` | A · family of one | A draft-correction method for speculative decoding: rescore the drafter's existing top-K candidates in one batched MLP pass, pick prefix-first, because verification truncates the block. | **leave-alone** | — | Already has its one correct inbound edge: `flashraw` topics it and calls it kin («Родня к DCOCO»). No second note in the vault holds its mechanism (batched rescoring of draft candidates). |
| 6 | `inbox/dspy-wsp-weight-space-programming-optimizer.md` | A · family of one | Weight-space programming as a DSPy optimizer: a frontier model reads and edits weights like code, improving five open decision models on the Typed Decisions benchmark. | **leave-alone** | — | Two pattern notes already point here (`pattern-weights-as-editable-artifacts`, `pattern-forward-budget-keep-best-edit`) and it has 3 outbound edges. Both reciprocal directions would be cycles; nothing is missing. |
| 7 | `inbox/folio-context-folding.md` | A · family of one | Recoverable multi-resolution context folding: L1/L2/L3 records written during execution, folding as programmatic view selection with no model call, recovery through handles. | **leave-alone** | — | Two patterns already point here (`pattern-priced-landing-point`, `pattern-record-then-fold`) and it has one outbound. The abstraction layer has already absorbed it; a reciprocal edge would be a cycle. |
| 8 | `inbox/context-dot-dev-sgrep-local-semantic-grep.md` | A · family of one | A Rust local semantic grep: static per-token embeddings hybridized 75/25 with BM25, exact-identifier priority, and a piped-reranker mode. | **leave-alone** | — | Two patterns already point here (`pattern-static-embedding-hybrid-search`, `pattern-piped-reranker-over-primary-tool`); it has one outbound. Reciprocal edges would be cycles. |
| 9 | `inbox/subq-snap-1-2.md` | A · family of one | SubQ Snap 1.2: a cheap decision endpoint for the routine steps of an agent run (tool choice, context curation, document analysis) on a non-OpenAI systemone wire. | **leave-alone** | — | `pattern-decision-layer-agent-work` already points here, and the note's own outbound goes to the category note. The pair is complete in the direction that matters (abstraction → evidence). |
| 10 | `inbox/gemini-4-carbon-swarm-antigravity.md` | A · family of one | Gemini 4 Carbon plus Antigravity's /swarm: coordinator, parallel workers in git worktrees, adversarial verifiers on an append-only board, zero-token idle parking. | **leave-alone** | — | Both pattern notes it generated (`pattern-adversarial-verifier-role`, `pattern-zero-token-idle-message-board`) already point at it. Adding extract → pattern would close two 2-cycles. |
| 11 | `inbox/awesome-reverse-engineered-catalog-automation.md` | A · family of one | A second extract of the awesome-reverse-engineered catalogue, focused on its nightly production pipeline: batch re-check, a 3% chain brake, a no-op gate, PR-only updates. | **link-to** | the index card | Same `source_url` as `inbox/awesome-reverse-engineered-index.md`: two cards for one repo, neither pointing at the other. The index card already topics its sibling sources, so it is the true statement's home. |
| 12 | `inbox/hashcat-architecture-and-algorithms.md` | A · family of one | The architecture and algorithms of a reference GPU cracker: one .cl source for four backends, a fast/slow kernel split, Brain dedup, association attacks. | **link-to** | this note | Its «Независимый Python-оракул для GPU-ядра» is the rule `pattern-oracle-drift-ci` formalizes (a differential run against a second implementation). Non-reciprocal: the pattern does not point here. |
| 13 | `inbox/nachalnik-family-transparent-agent-runtime.md` | A · family of one | A small Rust agent runtime where context, tools, permissions and requests are explicit editable state, with invariants that each have a red test. | **adopt-into** | f/agent-architectures | The family id is the repo (`f/gh-ljedrz-nachalnik`), already in `source_url`. A topical family exists with 19 members of agent runtimes and harnesses, including `bashkit-virtual-bash-sandbox`. This note also carries the one entry for the nachalnik↔respan pair (row 14). |
| 14 | `inbox/respan-gateway-universal-ai-adapter-architecture.md` | A · family of one | Respan Gateway: a single-binary universal AI adapter from any client format to any model, with a four-hook plugin lifecycle and typed traffic kinds. | **link-to** | nachalnik | Same mechanism at two sizes: «3 диалекта за одним трейтом; ядро без сокетов» is respan's whole product. Entry written on nachalnik (5 in, 0 out) so respan (0 in, 0 out) gains its first inbound and nachalnik stops being a dead end. |
| 15 | `inbox/usw-40mhz-glymphatic-clearance.md` | A · family of one | A Science Advances result: 40.68 MHz non-thermal RF drives glymphatic clearance in mice through neuronal Delta activity and AQP4. | **leave-alone** | — | The note states its own situation: «Это первая биомедицинская нота vault — без wiki-связей с AI-кластером». No second biomedical note exists, so no mechanism can match. Stays isolated. |
| 16 | `inbox/graphicle-rust-code-graph-agent-memory.md` | A · family of one | A Rust program keeping a code graph and agent memory together, with memories anchored to symbols so they go stale exactly when that code changes. | **leave-alone** | — | Two patterns already point here (`pattern-memory-anchored-to-code-symbols`, `pattern-cache-aligned-context-packs`); it has one outbound. Reciprocal would be a cycle. |
| 17 | `inbox/tabular-foundation-models-got-their-jev-moment-xiaomi-catches-google-in-two-months.md` | A · family of one | Xiaomi's 70M-parameter tabular in-context model catching Google's TabFM in two months, with the architecture and pretrain-cost breakdown. | **link-to** | this note | Its own claim names the target: «третье независимое подтверждение коммодитизации frontier-категорий (decision-модели, линкеры, tabular-ML)». The category note is the one that argues that shape. |
| 18 | `inbox/ingar30-reviewer-economics-paper-panel.md` | A · family of one | A reproducible multi-agent reviewer for economics papers: deterministic PDF preprocessing that never invents a value, evidence gates, cannot_verify as a legal outcome. | **leave-alone** | — | Three patterns already point here (`pattern-evidence-gates-process-vs-completion`, `pattern-parser-preflight-no-invention`, `pattern-conservative-normalization`); it has two outbound. Nothing missing. |
| 19 | `inbox/law-compiler-deep.md` | A · family of one | A deep map of law-as-code: Catala's literate law-code and priority-graph exceptions, the OECD consultation, LegalBench, and the deontic/contract benchmark landscape. | **leave-alone** | — | `pattern-literate-law-code-case-tests` already points here; the note's outbound goes to the law landscape note. The abstraction→evidence direction is complete. |
| 20 | `inbox/peach-local-first-media-system.md` | A · family of one | A single-user local-first media system whose transferable parts are checkpoint-after-write, add-only incremental scans, cloud-move guards, and an append-only ledger. | **link-to** | pattern-persistent-incremental-computation | The pattern formalizes exactly this: state is reconciled against what is already recorded, nothing is destroyed, append-only packs. Entry on the pattern (its established style lists independent implementations); peach gains its first inbound. |
| 21 | `inbox/meta-social-technologies-mcp-app-lifecycle.md` | A · family of one | Meta's MCP server doing whole app-lifecycle setup, with four confirm-per-write tools and the app secret kept out of the agent channel. | **leave-alone** | — | Two patterns already point here (`pattern-dashboard-flows-as-mcp-tools`, `pattern-secrets-never-through-agent-channel`); it has one outbound. Reciprocal would be a cycle. |
| 22 | `inbox/nielsen-christiansen-non-hierarchical-language-structures.md` | A · family of one | A Nature Human Behaviour experiment: three-word part-of-speech sequences that are neither constituents nor constructions prime stronger than constituents do. | **link-to** | this note | Its own closing names the match: priming as a structure cache, «та же форма, что routing-lessons». The harness note is the vault's record of accumulating routing lessons. Different domains, one mechanism. |
| 23 | `inbox/neo4j-ontology-graph-specification-open-ai-first.md` | A · family of one | An open spec for machine-readable domain knowledge as one JSON/YAML document validated against an immutable published schema with a /current alias. | **adopt-into** | f/knowledge-graphs | The family id is the vendor (`f/neo4j-ontology-graph-spec`). A topical family exists with 3 members (ontology/KG builders). The two patterns that already point here stay as they are. |
| 24 | `inbox/policy-change-treelike-monitors.md` | A · family of one | When a running monitor may switch from one policy to another after a legal change, decided from stored past-time state, with the proofs in Lean 4. | **leave-alone** | — | `pattern-policy-change-from-monitor-state` already points here and the law landscape note topics it. Its own contract is the pattern's rule (whether a verdict may cross a norm change). |
| 25 | `inbox/scrapecreators-social-research-skills.md` | A · family of one | Thirteen agent skills for social-media research over a paid scrape API, with workflow-first and baseline-aware principles. | **leave-alone** | — | `pattern-baseline-aware-outliers` already points here and abstracts its one general claim. The reverse would be a cycle, and the note's remaining content is a product/skills survey. |
| 26 | `inbox/annas-archive-shadow-library-infrastructure.md` | A · family of one | Anna's Archive as infrastructure: mirror rotation, SciDB's 239M papers, the seed-health dashboard, and LLM-addressed content. | **leave-alone** | — | The index card already topics it. Its transferable claim (a "bytes living with <N copies" dashboard) is a pattern nobody has written; linking it to a nearby infra note would be a shared-words link. |
| 27 | `inbox/strands-decider-aws-open-decision-model-family.md` | A · family of one | AWS's open decision-model family: a decoder torso with the LM head replaced by a ~1M-pointer head scoring options in one forward pass, on the pa-jev wire types. | **adopt-into** | f/decision-models | It makes an existing family of one a family of two: `f/decision-models` currently holds exactly this kind of note (`subq-snap-1-2`, a decision endpoint). Three patterns already point here, so no edge is needed. |
| 28 | `inbox/fullduplex-s2s-becomes-a-category.md` | A · family of one | A category signal: four 2026 systems converged on full-duplex speech-to-speech, with only the open branch publishing latency numbers. | **leave-alone** | — | 646 characters of category signal with no mechanism of its own. The one link its text proposes («там появится наш юзкейс decision-layer») is a forecast; writing it would encode a prediction as an edge. |
| 29 | `f/patterns/pattern-alignment-collapse-eval.md` | B · no frontmatter | A pattern note: evaluate systems on questions whose surface hides the structure, using a compressibility filter and a pseudoword stress test. | **frontmatter-only** | no topics | Worth keeping and its type is unambiguous from its title and its 100 siblings in `f/patterns`. Its evidence note already points at it (`hiddenpathqa`), so a topics entry back would be a cycle; write type/description/family/source only. |
| 30 | `f/patterns/pattern-autotune-by-scaling-law.md` | B · no frontmatter | A pattern note: replace hand-set hyperparameters with measured laws, and publish every decision as a machine-readable recipe. | **frontmatter-only** | topics → e-means | The only `f/patterns` note with no edge either way. Its own implementation #1 is `inbox/e-means-faiss`, with the same formula, so the first topics entry is not a guess. One write ends the only isolated pattern note. |
| 31 | `f/patterns/pattern-deterministic-first-llm-fallback.md` | B · no frontmatter | A pattern note: compute the core with deterministic rules and let the LLM be an optional, cached enhancer; the product works without it. | **frontmatter-only** | topics → gitmanual | Worth keeping; already has two inbound edges. Its implementation #1 (`gitmanual-setup-manuals`) is zero-inbound, so the entry gives that note its first inbound without a cycle. |
| 32 | `f/patterns/pattern-filesystem-as-agent-environment.md` | B · no frontmatter | A pattern note: build the agent environment as a filesystem — typed files, index/relation files, description files, standard ls/grep/read. | **frontmatter-only** | no topics | Its own evidence note (`docfs`) already topics it. A reciprocal entry is a cycle; write the frontmatter so the note becomes a typed instance and is visible to type-scoped queries. |
| 33 | `f/patterns/pattern-uniform-capability-boundary.md` | B · no frontmatter | A pattern note: hide a whole process behind one tool interface so recursion is free, because the caller cannot tell an LLM call from a process. | **frontmatter-only** | no topics | Two notes already point here (`gitmanual`, `pattern-describe-dont-render`) and the SemTalk extract will add a third by row 36. Reciprocal entries would cycle; write the frontmatter only. |
| 34 | `inbox/e-means-faiss.md` | B · no frontmatter | faiss's e-means: k-means as a ratio of two EMAs with scaling-law-tuned batch and step, published as an em.recipe. | **frontmatter-only** | no topics | An extract with a clear type, verifying its own claims (`em.recipe` as a reproducibility contract). The pattern that abstracts it (row 30) carries the topic entry, so this note gains its first inbound without writing anything back. |
| 35 | `inbox/second-brain-v2-bm25-minin.md` | B · no frontmatter | A video-reconstructed personal second brain: a cheap LLM writes per-record cards, BM25 over SQLite FTS5 does the search with no LLM, dated freshness decay. | **frontmatter-only** | topics → pattern-deterministic-first-llm-fallback | It names the pattern by id in its own text («наш pattern-deterministic-first-llm-fallback»), so the entry is quoted, not inferred. Marked by its author as reconstructed (no code), which the frontmatter's provenance field should repeat. |
| 36 | `inbox/bpmn-agentic-workflows-semtalk.md` | B · no frontmatter | SemTalk's BPMN→LangGraph translation: explicit process model, deterministic gateways, and agentic participants as uniform capability boundaries. | **frontmatter-only** | topics → pattern-uniform-capability-boundary | It coins the phrase («— uniform capability boundary»), so the pattern it points at is its own abstraction. Already carries a navigational edge from `jevact`, but no topics field and no type. |
| 37 | `f/patterns/pattern-adversarial-verifier-role.md` | C · gate-blocked | A pattern note: make the adversarial verifier a first-class role, on the strongest model, separate from the builders. | **link-to** | pattern-external-verifier-grounded-cot | The link the pass intended: its rule (self-review is weaker than an adversarial check) and the grounded-CoT rule (never let a model verify its own reasoning) are the same rule. Blocked on the write gate, not on merit. |
| 38 | `f/patterns/pattern-zero-token-idle-message-board.md` | C · gate-blocked | A pattern note: coordinate through an append-only board and park idle agents at zero token cost, rather than routing every step through one coordinator. | **link-to** | pattern-team-lead-context-management | The link the pass intended: both remove the single context that carries every step. This entry also gives `pattern-team-lead-context-management` its first inbound edge (0 in, 2 out). |
| 39 | `inbox/hygiene-2026-11-16.md` | D · moc, no family | A generated hygiene report: graph counts, 202 unlinked files, two notes the judge says belong in a topic map; explicitly a proposal, nothing edited. | **leave-alone** | — | A `moc` with `topics: []` and `generated_from: tools/hygiene/pass.py`; it has no members to link inside and it is regenerated on each run. Not part of the brief's three groups; included because the pass table counted it as skipped. |

Decision counts: **9 `link-to`, 4 `adopt-into`, 8 `frontmatter-only`, 18 `leave-alone`** (39 rows).

## Justifications for the 13 new entries

Each block quotes both notes. The quotes are verbatim from the vault; where I compress, I say so.

### 2 · `inbox/instabox-muse-free-agent-cloud.md` ↔ `inbox/nvx-microsoft-microvm-sandbox.md` — entry on instabox

- **inbox/instabox-muse-free-agent-cloud.md** — «паузятся после 30 минут простоя и resume <1s»; and the note names the design itself: «Это дословно аргументация DSec §5 (high-density oversubscription) и NVX-модели, доведённая до free-tier для каждого».
- **inbox/nvx-microsoft-microvm-sandbox.md** — «**Capture → restoration lifecycle**: состояние песочницы снапшотится и восстанавливается (сессия агента переживает рестарт хоста)».
- **Same mechanism:** one agent's VM is suspended and resumed from captured state so CPU and RAM return to the pool. The note asserts the identity with NVX; I only name the note.

### 11 · `inbox/awesome-reverse-engineered-catalog-automation.md` ↔ `inbox/awesome-reverse-engineered-index.md` — entry on the index card

- **inbox/awesome-reverse-engineered-catalog-automation.md** — «Ночной конвейер (cron 06:17 UTC, workflow_dispatch)».
- **inbox/awesome-reverse-engineered-index.md** — «ночной refresh GitHub Actions», in a card whose `topics:` already lists its sibling source cards (`annas-archive`, `firecrawl-alexandria`).
- **Same mechanism:** one repository, one nightly pipeline: the index card names it in a sentence, this note is the deep-dive on it. Identical `source_url` on both.

### 12 · `inbox/hashcat-architecture-and-algorithms.md` ↔ `f/patterns/pattern-oracle-drift-ci.md` — entry on this note

- **inbox/hashcat-architecture-and-algorithms.md** — «Независимый Python-оракул для GPU-ядра - шаблон «второй реализации на другом языке» для проверок переносимости».
- **f/patterns/pattern-oracle-drift-ci.md** — «Один и тот же корпус тестов гоняется против oracle и против реализации; расхождения = регресс совместимости».
- **Same mechanism:** two implementations of one specification are compared on one corpus; the difference is the defect. hashcat does it inside one project, the pattern does it against a live oracle.

### 14 · `inbox/respan-gateway-universal-ai-adapter-architecture.md` ↔ `inbox/nachalnik-family-transparent-agent-runtime.md` — entry on nachalnik

- **inbox/respan-gateway-universal-ai-adapter-architecture.md** — «однобинарный универсальный AI-адаптер - любой клиентский формат (OpenAI Chat/Responses, Anthropic Messages, Gemini) к любой модели с трансляцией стриминга/тулов/reasoning».
- **inbox/nachalnik-family-transparent-agent-runtime.md** — «nachalnik-providers (3 диалекта за одним трейтом; ядро без сокетов)».
- **Same mechanism:** N provider dialects resolved behind one boundary, with the core kept off the transport. respan is the standalone form of the crate nachalnik keeps inside its runtime.

### 17 · `inbox/tabular-foundation-models-got-their-jev-moment-xiaomi-catches-google-in-two-months.md` ↔ `inbox/decision-models-become-a-product-category.md` — entry on this note

- **inbox/tabular-foundation-models-got-their-jev-moment-xiaomi-catches-google-in-two-months.md** — «третье независимое подтверждение коммодитизации frontier-категорий (decision-модели, линкеры, tabular-ML)».
- **inbox/decision-models-become-a-product-category.md** — ««decision models» (быстрые типизированные вероятности вместо текста) оформились в отдельную продуктовую категорию».
- **Same mechanism:** a capability becomes a priced category inside one season, with a fast follower inside two months. The tabular note supplies the third instance of the category note's own claim.

### 20 · `inbox/peach-local-first-media-system.md` ↔ `f/patterns/pattern-persistent-incremental-computation.md` — entry on the pattern (its `topics:` list is where its implementations live)

- **inbox/peach-local-first-media-system.md** — «инкрементальные сканы (только добавляет + size/mtime, НИКОГДА не трогает ручные правки и не удаляет)» and «чекпоинты коммитятся ТОЛЬКО после успешной записи результата».
- **f/patterns/pattern-persistent-incremental-computation.md** — «пересчитать дешёвые зависимости, сравнить значения, родителя не трогать при совпадении — пересборка всего запрещена» and «append-only pack-файлы значений».
- **Same mechanism:** the durable record is reconciled incrementally and add-only: recompute the cheap part, compare, never destroy work that already exists. Peach's scanner is a second independent implementation of the rule.

### 22 · `inbox/nielsen-christiansen-non-hierarchical-language-structures.md` ↔ `inbox/oo-harness-evolving-harness-classes-oop.md` — entry on this note

- **inbox/nielsen-christiansen-non-hierarchical-language-structures.md** — «**Структурный прайминг как кеш структур**: «услышал/прочитал раз - обрабатываешь быстрее в следующий» - та же форма, что routing-lessons».
- **inbox/oo-harness-evolving-harness-classes-oop.md** — «накапливает design + routing lessons (заморожены на деплое)».
- **Same mechanism:** a stored structure makes the next encounter of the same structure cheaper. This one is the note's own analogy, not my inference — flagged as such; the domains (language priming, harness routing) are different, the rule is not.

### 30 · `f/patterns/pattern-autotune-by-scaling-law.md` ↔ `inbox/e-means-faiss.md` — entry on the pattern

- **f/patterns/pattern-autotune-by-scaling-law.md** — «faiss e-means (2026-11): B = 0.035·k^0.135·n^0.876·T^0.813» as its implementation #1.
- **inbox/e-means-faiss.md** — «эмпирическими scaling-законами: B = 0.035·k^0.135·n^0.876·T^0.813», with everything decided published as an `em.recipe`.
- **Same mechanism:** the same formula, the same recipe-contract. The pattern is the abstraction of this extract and lists it; the entry goes on the abstraction, so the extract gains its first inbound edge.

### 37 · `f/patterns/pattern-adversarial-verifier-role.md` ↔ `f/patterns/pattern-external-verifier-grounded-cot.md` — entry on this note

- **f/patterns/pattern-adversarial-verifier-role.md** — «Не верифицировать той же моделью в том же контексте: self-review слабее adversarial-проверки».
- **f/patterns/pattern-external-verifier-grounded-cot.md** — «Никогда не использовать модель как единственного верификатора собственных рассуждений (self-reflection без внешней проверки деградирует — Huang, Stechly)».
- **Same mechanism:** self-verification is not verification: the checker must differ from the builder in context and mandate, or the check is decoration. This is the link the pass intended.

### 38 · `f/patterns/pattern-zero-token-idle-message-board.md` ↔ `f/patterns/pattern-team-lead-context-management.md` — entry on this note

- **f/patterns/pattern-zero-token-idle-message-board.md** — «Координация через append-only board: агенты публикуют результаты и запросы, никто не роутит чужие шаги через свой контекст».
- **f/patterns/pattern-team-lead-context-management.md** — «Иерархия orchestrator → team leads → workers существует ради контекста: lead абсорбирует переписку воркеров и шлёт наверх короткие сводки (открытые обязательства / завершённые леммы / блокеры) — компактирование истории не теряет состояние».
- **Same mechanism:** no single context carries every step; state lives outside it and is read on demand. The board does it by removing the router, the hierarchy by distributing it. This is the link the pass intended.

Three of the eight `frontmatter-only` rows also carry a `topics:` entry, quoted rather than inferred:

- **31 · `pattern-deterministic-first-llm-fallback` → `gitmanual-setup-manuals`.** The pattern lists it as
  implementation #1: «gitmanual (2026-11): правила детектируют 13 стеков и собирают manual; LLM «Write my guide» —
  опция с ANTHROPIC_API_KEY, раз на репо+систему, потом кэш; без ключа core-флоу полноценен». `gitmanual` has 0
  inbound today; this entry gives it its first.
- **35 · `second-brain-v2-bm25-minin` → `pattern-deterministic-first-llm-fallback`.** The note names the pattern
  by id: «Детерминизм: поиск остаётся механикой (наш pattern-deterministic-first-llm-fallback)».
- **36 · `bpmn-agentic-workflows-semtalk` → `pattern-uniform-capability-boundary`.** The note coins the phrase:
  «« вызывающий не знает, за интерфейсом один LLM-вызов или целый процесс с людьми и системами» — uniform capability
  boundary», which is the pattern's own rule.

## The `adopt-into` four

These are the only decisions that rewrite a field instead of adding one. Each names a family that already exists
with at least two members of the same kind, so the note stops being a family of one and the family stops being a
label on one file.

| note | from | to | the family's existing members say |
|---|---|---|---|
| `inbox/little-book-generative-ai-foundations-primer.md` | `f/arxiv-2605.29713` | `f/ml-foundations-books` (6) | «Hardt & Recht PPA гл.1-4 (foundations)» — a shelf of ML-textbook workthroughs |
| `inbox/nachalnik-family-transparent-agent-runtime.md` | `f/gh-ljedrz-nachalnik` | `f/agent-architectures` (19) | «виртуальный интерпретатор bash на Rust с VFS для multi-tenant запуска ненадёжных (LLM-агентных) скриптов В ПРОЦЕССЕ» — agent runtimes and their parts |
| `inbox/neo4j-ontology-graph-specification-open-ai-first.md` | `f/neo4j-ontology-graph-spec` | `f/knowledge-graphs` (3) | «ontology-first KG-билдер на Jev-decision-model» — machine-readable ontology infrastructure |
| `inbox/strands-decider-aws-open-decision-model-family.md` | `f/strands-decider` | `f/decision-models` (1) | «the routine-decision layer of agentic workflows» — the one member is a decision endpoint; this note makes it a family of two |

Each note keeps `source_url` and `provenance`, so nothing is lost by moving `family:` from a source id to a group
id. The singleton count falls from 350 to 346.

## The arithmetic: 13 edges, 4 first inbound references

Every proposed entry, with the engine's inbound count on the target before → after:

| # | entry written on | points at | target inbound | target among the 38? |
|---:|---|---|---:|---|
| 2 | `inbox/instabox-muse-free-agent-cloud.md` | `inbox/nvx-microsoft-microvm-sandbox.md` | 3 → 4 | no |
| 11 | `inbox/awesome-reverse-engineered-index.md` | `inbox/awesome-reverse-engineered-catalog-automation.md` | 0 → 1 | **yes** |
| 12 | `inbox/hashcat-architecture-and-algorithms.md` | `f/patterns/pattern-oracle-drift-ci.md` | 1 → 2 | no |
| 14 | `inbox/nachalnik-family-transparent-agent-runtime.md` | `inbox/respan-gateway-universal-ai-adapter-architecture.md` | 0 → 1 | **yes** |
| 17 | `inbox/tabular-foundation-models-got-their-jev-moment-xiaomi-catches-google-in-two-months.md` | `inbox/decision-models-become-a-product-category.md` | 7 → 8 | no |
| 20 | `f/patterns/pattern-persistent-incremental-computation.md` | `inbox/peach-local-first-media-system.md` | 0 → 1 | **yes** |
| 22 | `inbox/nielsen-christiansen-non-hierarchical-language-structures.md` | `inbox/oo-harness-evolving-harness-classes-oop.md` | 6 → 7 | no |
| 30 | `f/patterns/pattern-autotune-by-scaling-law.md` | `inbox/e-means-faiss.md` | 0 → 1 | **yes** |
| 31 | `f/patterns/pattern-deterministic-first-llm-fallback.md` | `inbox/gitmanual-setup-manuals.md` | 0 → 1 | no |
| 35 | `inbox/second-brain-v2-bm25-minin.md` | `f/patterns/pattern-deterministic-first-llm-fallback.md` | 2 → 3 | **yes** |
| 36 | `inbox/bpmn-agentic-workflows-semtalk.md` | `f/patterns/pattern-uniform-capability-boundary.md` | 2 → 3 | **yes** |
| 37 | `f/patterns/pattern-adversarial-verifier-role.md` | `f/patterns/pattern-external-verifier-grounded-cot.md` | 6 → 7 | no |
| 38 | `f/patterns/pattern-zero-token-idle-message-board.md` | `f/patterns/pattern-team-lead-context-management.md` | 0 → 1 | no |

Totals:

| measure | before | after |
|---|---:|---:|
| entries added | 0 | 13 |
| among the 38: notes with nothing pointing at them | 14 | **10** |
| among the 38: notes with no reference either way | 12 | **4** |
| among the 38: dead ends (no outbound) | 23 | 14 |
| notes among the 38 that gain an inbound reference | — | **4** |
| notes among the 38 that gain an outbound reference | — | 11 |
| distinct notes among the 38 touched at all | — | 15 |

**Answer to the question asked: 4 of the 38 would gain an inbound reference** — `awesome-reverse-engineered-catalog-automation`, `respan-gateway-universal-ai-adapter-architecture`, `peach-local-first-media-system`, and `e-means-faiss`. The four that stay with no reference either way are `infinite-canvas-agent-operated-workbench`
(row 1, no neighbour exists), `little-book-generative-ai-foundations-primer` (row 4, adopt-into only, no edge),
`usw-40mhz-glymphatic-clearance` (row 15, first biomedical note), and `fullduplex-s2s-becomes-a-category` (row 28,
a 646-character category signal). Three of those four are leave-alone by decision, not by omission.

Two things this number says, and they point in opposite directions.

1. **It is small because a per-note decision can only write one direction.** Adding `topics:` to a note gives
   *its neighbour* an inbound edge. The last pass moved the inbound count (48 → 32 across the 76) because its 34
   edges chained pattern → pattern inside `f/patterns` and the targets were already in the set. Six of my 13
   pairs point at a note in the 38, and four of those six had no inbound at all — that is this design's ceiling.
2. **The number is a choice, not a ceiling.** Six of the 13 pairs are symmetric — neither note's text names the
   other — and for those I picked the direction that repaired the worse defect. Three of the six went outward
   (hashcat, adversarial-verifier, zero-token); writing those three inward instead would raise the inbound count
   from 4 to 7 and leave three notes with no outgoing edge. Both readings are defensible; the direction is
   recorded in the `entry` column so nobody has to guess which one this document used.

What that implies for the next pass is the important part: **inbound is not something a list of unlinked notes
can grant itself.** A pass built from the leaf side moves outbound. To move inbound, start from the notes that
already have inbound (the pattern notes, the category notes, the hubs) and ask which of the zero-inbound notes
each one should point at. The candidates this pass identified and did not write:

| zero-inbound note | the note that should point at it | why it is not in this table |
|---|---|---|
| `inbox/infinite-canvas-agent-operated-workbench.md` | nothing yet | no mechanism neighbour exists; wait for a second localhost-bridge note |
| `inbox/little-book-generative-ai-foundations-primer.md` | the `f/ml-foundations-books` shelf | a family is not an edge; a shelf MOC would have to exist first |
| `inbox/usw-40mhz-glymphatic-clearance.md` | nothing yet | first biomedical note; needs a second one |
| `inbox/fullduplex-s2s-becomes-a-category.md` | nothing yet | only a forecast in its text; wait for a voice note with a mechanism |
| `f/patterns/pattern-adversarial-verifier-role.md` | its row-37 target, written the other way | row 37 writes *out*, so it stays zero-inbound; the inward edge would be the cycle that row avoids |
| `f/patterns/pattern-zero-token-idle-message-board.md` | its row-38 target, written the other way | same; and that target (`pattern-team-lead-context-management`) is now covered by this row's own outbound edge |

## Three recurring shapes, and what each implies

### 1. A family of one is a provenance id that calls itself a family

The vault has 433 distinct `family:` values; **350 of them have exactly one member.** The 28 single-member
families in this batch split into two kinds, and neither kind works as a family:

- **source names** — `f/gh-hashcat-hashcat`, `f/arxiv-2605.29713`, `f/nature-nielsen-christiansen-2026`,
  `f/strands-decider`, `f/neo4j-ontology-graph-spec`. Each duplicates a `source_url` the note already carries, so
  the field stores provenance twice.
- **subject names** — `f/voice-agents`, `f/decision-models`, `f/law-compiler`, `f/context-folding`,
  `f/policy-change-monitors`, `f/agent-canvas`. Each is a real topic with one member, so the id promises a group
  the vault does not have.

That mixture is why the refusal rule could not be applied: a source id cannot be crossed (there is no second
member to cross to), and a subject id should not veto a link to a note that shares its subject. The same
repository also produced two cards and two singleton families: `awesome-reverse-engineered-index` sits in
`f/re-tooling-index` and its pipeline deep-dive sits in `f/gh-alexgaoth-awesome-reverse-engineered`.

The decisive fact for the refusal rule: **13 of the 28 already carry an outbound `topics:` link, and all 13 point
outside their one-member family.** "Linking them would cross families" describes a boundary these notes crossed
the day they were written. A family of one cannot be crossed, because it has no inside.

Implications for the next pass:

- Report family size, and treat size 1 as *unset*. A pass must not accept "no sibling, so no link" as a reason.
- Do not invent a sibling to satisfy the rule. 28 notes did not become linkable because they were refused; they
  were never siblings.
- The real repair is schema-level: `family:` should name a group, `source_url:` and `provenance:` already name a
  source. Until that is fixed, every pass will keep re-deriving that these 28 families are provenance.
- Where a topical family already exists, adoption heals the field without a schema change — four cases here, and
  one of them (`f/decision-models`) stops being a family of one by gaining a second member.

### 2. A note without frontmatter is a type question, not a link question

All 8 are untyped, so all 8 are invisible to `tb_instances`, `tb_types` and everything else that asks by type;
30 of the 38 are typed. Their type is not in doubt: the five pattern notes open with a `# Pattern: <name>` heading and have the same five-section shape as the other 100 pattern notes in `f/patterns`; the three
inbox extracts have a title, a date and a source URL in the body.

Three of the eight are already cited by newer notes while lacking frontmatter: `docfs-filesystem-agent-environment` topics `pattern-filesystem-as-agent-environment`, `hiddenpathqa-alignment-collapse` topics
`pattern-alignment-collapse-eval`, and `jevact-contracts-for-pa-jev` links `bpmn-agentic-workflows-semtalk` in its
body. The vault has already decided these notes matter; only the file has not been told.

Implications for the next pass:

- `frontmatter-only` belongs in the menu as its own decision, and in its own queue. It is a write that changes
  what the note *is*; it should never ride along with a link pass.
- The smallest honest block: `id`, `type`, `description`, `family`, `source_url`, `provenance`, `status`,
  `topics`. Nothing in the body, no new sentence.
- The census should print *untyped* next to *unlinked*. They are different holes with different repairs, and
  the current reports only count the second.
- Four of the eight carry their only link *inside* the same frontmatter write (rows 30, 31, 35, 36), so the two
  acts cannot be separated for those notes — which is the reason to decide the frontmatter first. The other four
  get no `topics:` entry yet (rows 29, 32, 33 because their evidence note already points at them; row 34 because
  the pattern that abstracts it carries the entry).

### 3. A note owned by another session is a scheduling conflict, not a judgement

The two blocked notes were refused by the write gate, not by content: `tb_edit_file` answers "path has
uncommitted changes — the engine is the writer". Nothing about the note is in question, and both intended links
survive review: the adversarial-verifier rule *is* the external-verifier rule, and the board *is* the team lead's
context rule without the hierarchy.

I did not re-check dirtiness: no git (excluded by the brief), and the engine exposes content hashes, not
dirtiness. The check belongs at write time and will answer the same way until the owning session commits.

Implications for the next pass:

- Reserve the write set before deciding anything. A lane owns its paths for the length of a pass; a pass that
  discovers ownership at write time has wasted its reading.
- Record intended links in a document like this one, so the owning session applies them instead of re-deciding
  them. The decision is the expensive part; the write is not.
- Keep the gate. It is the only thing that stopped two sessions from losing one edit.

## What I would fix first

Three writes, each one line, each removing two defects rather than one:

1. **`nachalnik → respan` (row 14).** respan is the only note in the 28 with no edge at all *and* a mechanism
   twin; nachalnik has five inbound edges and no outbound. One entry on nachalnik gives respan its first inbound
   and turns a 5-in/0-out dead end into a note that links somewhere. Cheapest real improvement in the set.
2. **`awesome-reverse-engineered-index → awesome-reverse-engineered-catalog-automation` (row 11).** Two cards
   for one repository, neither pointing at the other, one of them isolated. One entry gives the catalog card its
   first edge and marks the pair for the merge-or-keep question a later pass must answer.
3. **`f/patterns/pattern-autotune-by-scaling-law.md` (row 30).** The only note in `f/patterns` with no edge
   either way. Its frontmatter plus one `topics:` entry (to `e-means-faiss`, same formula) removes the last
   isolated member of the largest family in the vault.

Runner-up: `zero-token → team-lead` (row 38) is the highest-value single edge here — it gives
`pattern-team-lead-context-management` its first inbound reference (0 in, 2 out) — but it is gate-blocked, so it
is the owning session's to write, not the next pass's.

## Limits of this document

- **A snapshot.** The vault is live and other sessions write to it; the counts are from one read on 2026-11-16 and
  no hashes are recorded, so every write must re-read its path.
- **Dirtiness is reported, not verified.** The two gate-blocked paths are as the pass found them; I ran no git.
- **`family` is read inductively.** 433 values, 350 of size one, shaped like source names. I did not find a
  type-def constraint that says so; "provenance, not group" is my reading of the values, and the adopt-into
  decisions are the only place it changes anything.
- **"No mechanism neighbour exists" is a search result, not a proof.** Five rows (1, 5, 15, 26, 28) rest on it;
  it means `tb_search` and `tb_grep` over 1,065 notes returned no note that shares the mechanism, not that none
  can be written.
- **The 18 `leave-alone` rows are decisions, not omissions.** Thirteen of them rest on the fact that the reciprocal
  edge would be a cycle, which is a statement about this graph, not about the note's importance.
- **Out of scope:** whether any of the 38 should be promoted to `knowledge/`, merged, or deleted; the
  `family` schema change; and writing anything at all.

## Applied — the write pass, 2026-11-16

Eleven of the thirteen decided writes landed, each through the engine
(`2brain mcp /root/vault --allow-mutations --principal intake-apply`) as its own commit. No git command was
run, no file was written on disk by this session, and no path outside the decision table was touched. Two
notes were not attempted (rows 37, 38; their lane belongs to another session). The gate refused nothing.

Every anchor was re-checked against the live text before writing: the twenty quoted fragments in the
justification blocks still matched, and every pre-write in-degree equalled the number the arithmetic table
predicted. No decision was rewritten to fit the vault.

### The thirteen

| # | decision | write | commit | target in-degree |
|---:|---|---|---|---:|
| 2 | `link-to` on instabox | `topics:` (field did not exist; added under `family:`) | `5fbd0faf` | nvx 3 → **4** |
| 11 | `link-to` on the index card | third `topics:` entry | `4a58899d` | catalog **0 → 1** |
| 12 | `link-to` on hashcat | `topics:` (new field) | `7ce50d85` | oracle-drift 1 → **2** |
| 13 | `adopt-into f/agent-architectures` | `family:` `f/gh-ljedrz-nachalnik` → `f/agent-architectures` | `14e7d5b7` | — |
| 14 | `link-to` on nachalnik | `topics:` (new field) | `87585dda` | respan **0 → 1** |
| 17 | `link-to` on the tabular note | `topics:` was present and empty; filled | `826a7d39` | category 7 → **8** |
| 20 | `link-to` on the pattern | third entry in the flow list | `6a4213f5` | peach **0 → 1** |
| 22 | `link-to` on nielsen | `topics: []` → one entry | `658a4bd9` | oo-harness 6 → **7** |
| 4 | `adopt-into f/ml-foundations-books` | `family:` `f/arxiv-2605.29713` → `f/ml-foundations-books` | `6ae2195d` | — |
| 23 | `adopt-into f/knowledge-graphs` | `family:` `f/neo4j-ontology-graph-spec` → `f/knowledge-graphs` | `bb82b787` | — |
| 27 | `adopt-into f/decision-models` | `family:` `f/strands-decider` → `f/decision-models` | `b33ff8c8` | — |
| 37 | `link-to` adversarial-verifier → external-verifier | **not attempted** — gate-blocked path, other session's lane | — | — |
| 38 | `link-to` zero-token → team-lead | **not attempted** — same | — | — |

Landed: **11** (7 edges + 4 family moves). Not attempted: **2**. Refused at write time: **0**. All four
adopt-into families exist with the membership the document counted (6, 19, 3, 1); the four notes keep
`source_url` and `provenance`, and each edit returned `diagnostics: []`. Every target gained exactly the one
inbound edge predicted, from exactly the note the `entry` column named.

### The graph, before and after

`tb_graph_health` over the vault member, both counts from the same engine:

| measure | before | after | delta |
|---|---:|---:|---:|
| nodes | 1214 | 1214 | 0 |
| edges | 4697 | 4704 | **+7** |
| isolated (no edge either way) | 204 | 198 | **−6** |
| dead ends (inbound, no outbound) | 19 | 20 | **+1** |
| sources (outbound, no inbound) | 174 | 177 | +3 |
| components | 214 | 208 | −6 |
| broken links | 0 | 0 | 0 |

The seven edges are exactly the seven added edges: no other session wrote to the edge set during the pass.
The six notes that left isolation are the six that had no edge at all and now have one — instabox,
`awesome-reverse-engineered-catalog-automation`, respan, peach, the tabular note, nielsen. The component
count falls by six for the same reason.

### The two dead ends — yes, both were repaired

- **nachalnik, 5 in / 0 out → 5 in / 1 out.** This is the note the brief names. One entry (row 14) turned
  it into a note that links somewhere, and gave respan its first inbound reference at the same time.
- **hashcat, 3 in / 0 out → 3 in / 1 out.** The second dead end in the write set; row 12 repaired it.

The net `dead_ends` count still rises by one, and the arithmetic is exact: two dead ends were repaired
(−2) and three targets became zero-outbound nodes when they gained their first inbound edge
(+3: `awesome-reverse-engineered-catalog-automation` 1/0, respan 1/0, peach 1/0). That is the trade the
"arithmetic" section predicted in the other direction — an inbound edge is not free when the receiving note
has no outgoing one. Three of the proposals deliberately paid it.

### The cycle check: what it got wrong, and the rule that replaces it

**Row 12 closed a long cycle, and the edge stays.** hashcat had no outbound edge before the write, so its new
edge necessarily closes the loop. The measured deltas:

| measure | before | after | delta |
|---|---:|---:|---:|
| cycles (SCCs with more than one node) | 31 | 29 | **−2** |
| nodes inside a cycle | 703 | 708 | **+5** |
| largest component | 619 | 630 | **+11** |

Three SCCs merged into one — the 619-node component, the 4-cycle holding `pattern-oracle-drift-ci`, and a
2-cycle — and 619 + 4 + 2 + 5 newly cyclic nodes is the 630. The five nodes that entered a cycle, hashcat first:

| node | how it entered |
|---|---|
| `hashcat-architecture-and-algorithms` | the source of the new edge; 3 in / 0 out before |
| `taffy-layout-engine-cache-and-testing` | one of hashcat's three referrers, on the return path |
| `neo4j-ontology-graph-specification-open-ai-first` | pulled in by the merged component |
| `maze-storage-engine-build-log-btrees-slotted-pages` | same |
| `pattern-immutable-published-schema-current-alias` | same |

The check could not see it: `tb_neighborhood` from `pattern-oracle-drift-ci` does not reach hashcat in five
hops across all four edge kinds, nor in six hops on the three non-commit kinds. A bounded walk is not a proof
of acyclicity in a 619-node component.

No other entry in the set moves an SCC. The two absorbed components and the five nodes that entered a cycle
are all explained by this one edge; the other six point at notes with no path back to their source.

**The criterion was wrong, not the write.** The vault is a link graph, not a DAG: 708 of its 1214 nodes sit in
cycles and there are 29 SCCs, so any edge between two such nodes merges components — "a new edge must not
create a cycle" either fails always or, as here, passes without meaning anything once it is bounded. What is
worth preventing is a short accidental loop, not membership in a 630-node component. The rule for the next
pass:

> **Reject an edge that closes a cycle of length ≤ 3; accept the rest and record the SCC delta.**

The write is right on merit: hashcat (0 out) gains its one real sibling and stops being a dead end, which is
what the row was for. It is not reverted.

### What this pass still did not do

Rows 29–36 (`frontmatter-only`) are untouched: they are a different queue by the document's own argument,
and four of them would have carried a `topics:` entry in the same write. The eighteen `leave-alone` rows
are untouched. No note was promoted, merged, or deleted.

