# arscontexta skills → 2brain: what the operational layer has that the MCP successor lacks

Read 2026-11-16, read-only, from `/root/vendor-source/arscontexta` (plugin v0.8.0, 10 shipped skills,
16 skill-source templates, hooks, 3 presets, 1 agent, generators). Comparison baseline: 2brain's MCP
surface — `tb_search`, `tb_context`, `tb_read`, `tb_links`, `tb_check`, `tb_write`, the fail-closed
write gate, and advisory `diagnostics` / `graph_health` (52 tools total, single Rust daemon, one writer).

The old system is a Claude Code plugin that *generates* a per-user knowledge system from conversation;
2brain *is* the knowledge system, served over MCP. The gap is therefore not features but contracts:
the old layer wraps its primitive operations in **answer contracts, severity semantics, threshold
tables, and prescribed prompt texts** that make behavior legible to the model. 2brain's tools return
data; arscontexta's skills return *judgments with stated evidence and a fixed answer shape*. That
difference is portable as prompts, resources, and a handful of daemon-side counters.

Everything below cites paths. Items that are proposals for 2brain rather than facts about the old
system are marked **[design]**.

---

## 1. Skill table

The ten shipped skills (`skills/<name>/SKILL.md`). "Exit condition" is how the skill ends; "check" is
the internal test that says it succeeded.

| skill | trigger | steps (compressed) | exit condition | artifact | success check | transfer verdict |
|---|---|---|---|---|---|---|
| **ask** | `/ask [q]`, "why does my system…", "how should I…" | classify question (7 types) → read claim-map routing index first → search routed tier(s) → read 3-7 sources *fully* + 1 wiki-link hop → check user context (`ops/derivation.md`, `ops/methodology/`) → synthesize → cite | 6-part answer delivered in conversation | conversational answer | citations name only claims actually read; honesty standard when corpus lacks coverage; sources-consulted footer | **MCP prompt+resource** over `tb_search`+`tb_context`+`tb_read` (the answer contract IS text) |
| **health** | `/health [quick\|full\|three-space]`, "what needs fixing" | parse mode → scan vault → run applicable of 8 categories → threshold classification → condition-based maintenance signals → report | report rendered + top-3 ranked actions | `ops/health/YYYY-MM-DD-report.md` (history) | every finding names specific files; FAIL/WARN/PASS per category with stated rationale | **MCP tool** (extend `diagnostics`/`graph_health`) + **prompt** for the report shape and severity semantics |
| **help** | `/help`, `--compact`, `/help [skill]`, "what can I do" | gather vault state → resolve domain vocabulary → pick mode (narrative <5 notes / contextual / compact / skill detail) → discover commands *dynamically* from `skills/` frontmatter → render | rendered guide with one state-aware suggested next action | conversational | suggestion logic picks first matching condition and references a specific file | **MCP prompt+resource** (thin — discovery itself already covered by `tb_search_tools`/`tb_describe_tool`) |
| **recommend** | `/recommend [use case]`, `--compare A B`, "what would you recommend" | parse signals → ≤2 clarifying questions → match preset → search research → map 8 dimensions with H/M/L confidence → hard/soft/cascade constraint check → present | recommendation table; **no files created** (advisory, not generative) | conversational proposal | every position cites ≥1 research claim; defaults stated when signals insufficient | **MCP prompt+resource** (intake advisory; the signal→dimension tables and constraint cascade rules are text) |
| **reseed** | drift triggers: incoherence >3 dimensions, vocabulary mismatch, boundary dissolution, template divergence >40% | analyze (derivation, inventory, health history, observations) → drift report per dimension (none/aligned/compensated/incoherent/stagnant) → re-derive with evidence → 5-pass coherence validation → present delta → implement on approval → revalidate | kernel validation passes; content counts unchanged | new `ops/derivation.md` + restructured vault | "Content preserved: yes — N notes, N memories unchanged"; rollback file archived first | **MCP prompt+resource** (the drift taxonomy + the "NEVER deletes content" invariant language); the 8-dimension machinery itself is **not transferable** |
| **add-domain** | "add domain X" to an existing system | scan existing system → 1-3 question conversation → derive domain config (5 system-level dims fixed, 5 domain-adjustable) → **collision check** (filenames, fields, templates, folders) → composition rules → proposal → generate on approval → validate | filename uniqueness verified; hub links all domains | domain folders + template + hub MOC update + manifest update | collision check clean; kernel checks pass on new files; cross-domain test link resolves | **MCP prompt+resource**; the collision check is a cheap **tool candidate** for `tb_check` |
| **setup** | `/setup`, "set up my knowledge system" | platform detect → 3 prescribed onboarding screens (verbatim) → 2-4 turn conversation with weighted signal extraction (H 1.0 / M 0.6 / L 0.3 / inferred 0.2, resolution threshold 1.5) → derive (cascade, 3-pass coherence, failure-mode risk) → proposal → **16-step generation, `ops/derivation.md` written FIRST** → kernel validation (15 primitives) → first-success moment | kernel 15/15 PASS + pipeline smoke test | whole vault: derivation record, folders, context file, self/, templates, 16 skills, hooks, hub MOC, `.arscontexta` marker, git init | kernel validation + functional smoke test (create → verify → delete a test note) | **MCP prompt+resource** for the screens, signal tables, anti-signal table, conflict-resolution tree, and the persistence-first rule; the 16-step generator is **not transferable** |
| **tutorial** | `/tutorial`, "walk me through" | track selection (researcher/manager/personal) → 5 steps (capture, discover, process, maintain, reflect), each WHY → DO → SEE, state persisted to `ops/tutorial-state.yaml` after *every* step | step 5 complete; resume works across sessions | 2-3 **real** notes + resumable state file | real content created each step (no hypothetical examples); step-4 mini health check PASS | **MCP prompt+resource** (WHY/DO/SEE pattern + genuine-connection honesty: "forced connections pollute the graph") |
| **upgrade** | `/upgrade`, "check for improvements" | inventory (versions, git-modified skills) → consult knowledge base per skill — *methodology comparison, not hash diffing* → classify current/enhancement/correction/extension → plan → present → apply approved with archive → validate | plan applied; previous versions archived | `ops/skills-archive/<skill>-<date>.md` + updated skills + manifest | vocabulary markers resolved; pipeline handoff compatibility; never silent-overwrite user-modified skills | **MCP prompt+resource** (the consultation protocol and side-by-side rules); irrelevant to binaries, relevant to any 2brain prompt-pack refresh |
| **architect** | `/architect [area]`, evolution advice | locate → read derivation baseline → health (fresh report or live check) → friction scan (observations, methodology notes, session logs, self/) → research consult → 3-5 ranked recommendations (impact/effort/risk/evidence) → **never auto-implements** → implement on approval + log | recommendations with full evidence chains; changes logged | `ops/changelog.md` entries + updated `ops/derivation.md` | quality gates: specific file refs, ≥2 data points, ≥1 claim citation, risk + reversibility + honest time estimate, 25%-of-session budget rule | **MCP prompt+resource** over `diagnostics` — the evidence-chain standard is exactly what advisory diagnostics outputs lack |

Also present but outside the skill set: `agents/knowledge-guide.md` (a proactive methodology-advice
subagent — in 2brain this role belongs to the parent agent, so not transferable as a daemon feature)
and `scripts/sync-thinking.sh` (repo-specific vault→bundled-corpus sync, not transferable).

---

## 2. `ask` — the retrieval skill, against `tb_search` + `tb_context`

Source: `skills/ask/SKILL.md` (23 KB; `context: fork`, model opus; tools: qmd search trio + get/multi_get).

### What it does *before* searching

1. **Classifies the question** into seven types with literal signal phrases, and each type names a
   primary and secondary tier and a search modality:
   WHY → research graph, LLM-reranked deep search; HOW → guidance docs, *keyword first* ("guidance
   docs use consistent terminology"), semantic only as fallback; WHAT → domain examples, vector
   search; COMPARE → research + examples; DIAGNOSE → guidance + failure-modes doc; CONFIGURE →
   dimension/constraint reference docs; EVOLVE → evolution-lifecycle doc.
   `tb_search` takes the query it is given; nothing in the tool contract asks what kind of question
   this is or changes modality accordingly.
2. **Consults a routing index before searching.** "Read `reference/claim-map.md` first. This is the
   routing index — it shows which topic areas are relevant … Do NOT skip this step and search
   blindly." Retrieval is preceded by orientation.
3. **Loads user context into the answer frame**: `ops/derivation.md` (dimension positions,
   constraints, vocabulary) and `ops/methodology/*.md` (local operational learnings), with explicit
   *precedence rules* — local methodology wins for "why does MY system do X", research wins for
   "why is X good in general", and contradictions between them must be flagged, not averaged.

### What it does *after* searching

- **Reads fully, never skims**: top 3-7 results read whole, plus one hop of wiki links.
- **Synthesizes into a fixed 6-part shape**: (1) direct answer first — "Do not say 'I searched for X
  and found Y.' Say what the answer IS"; (2) research backing cited by `[[claim title]]`; (3)
  practical implications *in the user's own vocabulary* (a therapy user hears "reflections", never
  "claims"); (4) tensions or caveats — genuine disagreements presented as tensions, false balance
  forbidden; (5) further exploration as departure points; (6) **sources consulted** — "Research: [N]
  claims consulted. Local methodology: [M] notes consulted," naming the local notes that informed it.
- **Grades certainty** with the corpus's `confidence` field (speculative → emerging → supported →
  established; archived = superseded) and carries that grading into the prose.

### How it decides it has an answer

- **The depth principle**: "A shallow answer citing 10 claims is worse than a deep answer weaving 4
  claims into a coherent argument. Read fewer sources more deeply."
- **The honesty standard**: when the corpus does not cover a topic, say so explicitly, offer the
  nearest adjacent research, and flag the gap as a possible research direction — never extrapolate
  wildly from tangential claims.
- **An anti-fabrication gate**: only cite claims actually read; an anti-pattern table names the
  failure modes (answering without searching, listing without synthesizing, single-tier answers,
  ignoring derivation, universal vocabulary, fabricated citations, skipped claim-map, false balance).

### What our answer shape lacks

`tb_search` returns judged hits; `tb_context` returns a bounded, line-anchored evidence pack with
call links and coverage. Both are *inputs to an answer*; neither is under contract to produce one.
Concretely missing from 2brain's answer shape:

1. **An answer contract** — the 6-part structure, most importantly "direct answer first" and the
   sources-consulted footer with counts.
2. **Intent classification before retrieval** — WHY/HOW/WHAT/etc. picking search modality and
   collection. `tb_search` has rerank and filters, but the *caller* must already know what to ask for.
3. **A certainty vocabulary** — nothing in a 2brain response distinguishes an established note from a
   speculative one.
4. **The honesty standard** — 2brain tools report coverage honestly at the pack level, but there is
   no norm that says: if the graph doesn't answer this, *say so and point at the nearest neighbor*
   rather than assembling a plausible-looking pack.
5. **User-vocabulary translation** — the answer speaks in universal terms; the old system mapped
   terms per vault (see presets, §5).
6. **Accounting** — which corpus pieces and which local/operational notes informed the answer.
   `tb_context` coverage is close, but it is pack metadata, not part of an answer norm. **[design]**
   An MCP prompt+resource ("answer contract") carried alongside `tb_search`/`tb_context` buys 1-6 at
   text-only cost.

---

## 3. `health` — against advisory `diagnostics` + `graph_health`

Source: `skills/health/SKILL.md` (28 KB). 8 categories: schema compliance, orphan detection, link
health, description quality, three-space boundaries, processing throughput, stale notes, MOC
coherence; 3 modes (quick = 1-3, full = all 8, three-space = 5 only).

What the old skill has that 2brain's diagnostics do not:

- **Severity semantics with stated rationale.** FAIL is *reserved*: dangling links, persistent
  orphans, missing frontmatter, critical boundary violations — "FAIL is reserved for … broken
  promises." WARN is everything suboptimal-but-intact; INFO is context. The rationale is quotable:
  "**Why FAIL not WARN:** Dangling links are broken promises. Every `[[link]]` a reader follows that
  leads nowhere erodes trust in the graph."
- **Age-graded orphans.** Orphan < 24h = INFO (expected — awaiting the connect phase), 1-7d = WARN,
  >7d = FAIL. A binary "has zero incoming links" check mislabels yesterday's note as sick.
  `graph_health` could grade orphan age from note timestamps. **[design]**
- **Condition-based, not calendar-based staleness**: stale = modified > 30 days *AND* < 2 incoming
  links; > 90 days *AND* 0 links escalates to FAIL. Plus a priority order (oldest, least-connected
  first).
- **Throughput ratio** — inbox / (inbox + notes), thresholds 50% WARN / 75% FAIL ("collector's
  fallacy"). This is an operation the daemon can compute cheaply and 2brain has no equivalent of.
- **Description-quality check** — restatement detection (>70% title-word overlap, length bands 30-250
  chars) is a *judgment* check; keep it prompt-side or as a heuristic flag, not a hard tool check.
  **[design]**
- **Threshold table + maintenance-signal table** — observations ≥10 → rethink; tensions ≥5; inbox ≥3;
  unprocessed sessions ≥5; queue stalled >2 sessions. The hook recomputes these by `ls | wc -l` every
  session start; the daemon could keep them as incremental counters (§4).
- **The meta-trigger**: "Compare known maintenance conditions against what is actually being checked.
  If a maintenance condition has no corresponding check or trigger configured, *that gap itself is a
  finding*." Diagnostics reporting what it does not check is a norm 2brain's coverage report
  approximates but never states as a rule.
- **Report artifact + trend history** — `ops/health/YYYY-MM-DD-report.md`, written every run,
  explicitly so `architect` can read trends across reports that a single snapshot misses.
- **Ranked top-3 actions** with impact tiers (dangling links > schema/boundary > descriptions/staleness
  > MOC size/throughput), an anti-overwhelm cap (top 5-10 issues; 10 files per category then "…and N
  more"), and a health-to-action loop table mapping every finding to the skill that fixes it — with
  the separation rule: "health tells you WHAT is wrong, architect tells you WHY and HOW to fix it."
- **Platform-adaptation norm**: "Report what CAN be checked, not what the platform lacks" — degraded
  environments name the skipped checks rather than silently passing.

**Verdict split**: the checkable core (orphans with age grading, dangling links, schema fields,
throughput ratio, staleness conditions, MOC size bands) belongs in `diagnostics`/`graph_health` as
real tool output; the severity taxonomy, threshold table, ranked-actions format, and report history
are contract and belong in a prompt/resource or in the tools' documented output shape. **[design]**

---

## 4. Hooks and session lifecycle

Shipped: `hooks/hooks.json` + `hooks/scripts/{session-orient,write-validate,auto-commit,vaultguard,read_config}.sh`.
`setup` (Step 10) generates a fourth script, `session-capture.sh`, on a `Stop` event, plus additive
merge rules so generated hooks never overwrite user hooks.

What runs automatically today:

- **SessionStart → session-orient.sh (10s timeout)**. Guarded by `vaultguard.sh` (a `.arscontexta`
  marker file, which doubles as YAML config via `read_config.sh`, defaults true). It (a) silently
  tracks the session: `ops/sessions/current.json`, promoting the previous session to a timestamped
  archive when the id changes, and exports `CLAUDE_SESSION_ID`; (b) *injects orientation into the
  conversation via stdout*: 3-level `.md` directory tree, previous session record, `self/goals.md`,
  identity + methodology (if the self space is enabled), the first 3 lines of the 5 most recent
  `ops/methodology/` notes; (c) emits **condition signals** — `CONDITION: N pending observations.
  Consider /rethink.` at observations ≥10, tensions ≥5, unprocessed sessions ≥5, inbox ≥3 — plus a
  workboard reconcile call and a methodology-staleness check (config mtime minus newest methodology
  note ≥ 30 days → "Consider /rethink drift").
- **PostToolUse(Write) → write-validate.sh (5s)**. For `*/notes/*` and `*/thinking/*`: checks YAML
  frontmatter, `description:`, `topics:` in the first 20 lines; on failure prints
  `{"additionalContext": "Schema warnings for <file>: …"}` — **advisory, fail-open, never blocks**.
- **PostToolUse(Write) → auto-commit.sh (async, 5s)**. Config-gated (`git: true`); `git add -A`,
  commit `Auto: <file> | <stats>`. The script itself notes "Async hooks don't reliably receive tool
  input — commit all pending changes."
- **Stop → session-capture.sh** (generated variant only): persists what the session created/modified
  into `ops/sessions/current.json` so the next SessionStart can restore continuity across `/clear`.

### What a Rust daemon can do that a hook cannot

1. **Total coverage instead of opt-in coverage.** Hooks fire on one client's tool events on one
   platform; any other agent writing to the vault bypasses write-validate and session capture. The
   daemon is the single writer — validation and session capture at the mutation boundary cover *every*
   client by construction. This is the same argument the codebase already made for the gate.
2. **Enforcement before the write, not commentary after it.** A PostToolUse hook can only complain
   once bytes are on disk, and is fail-open by design (missing `jq`, wrong platform → exit 0). The
   daemon's policy layer refuses pre-write; an advisory schema warning belongs in the *write response*,
   where it cannot be missed, rather than in a stdout channel some clients render and some drop.
3. **Serialized auto-commit.** Two concurrent clients each firing an async `git add -A` commit race
   (the script commits "all pending changes" blind). The daemon can commit exactly once per accepted
   mutation, inside the serialized mutation path — the hook cannot, because it is neither per-client-
   aware nor serialized. **[design]** Config-gate it like the old `.arscontexta` `git: true` key.
4. **State instead of recounting.** The hook recounts files with `ls | wc -l` at session start and
   compares mtimes with `stat`. The daemon already holds the graph: pending-observation counts, inbox
   depth, orphan age, staleness are incremental state, and the condition signals can be attached to
   *every* read response (`tb_context`, `overview`, `diagnostics`) rather than announced once at
   session start, when the model may already be mid-task.
5. **Cross-agent continuity.** Session state in `ops/sessions/current.json` is only surfaced by the
   client whose hook prints it. Daemon-held session records can be served to any agent through the
   read surface, so orientation survives not just `/clear` but a *different agent entirely* — the old
   system's session lifecycle is a per-client convention; daemon-side it becomes a vault property.
6. **No shell/timer ceilings.** Hooks are 5-10s shell scripts depending on `jq`/`tree`/`git` presence;
   the equivalent daemon features are compiled, stateful, and testable (`create_race.rs`-style).

What the hook *does* better and should be kept honest about: it is user-visible, user-editable, and
fail-open by philosophy — the daemon should keep the *advisory* tier advisory (warnings in responses,
never refusals) and gate every automatic behavior (commit, capture) behind per-vault config, exactly
as the marker file did. **[design]**

---

## 5. Presets

Sources: `presets/{experimental,personal,research}/{preset,categories,vocabulary}.yaml` and
`presets/*/starter/`.

Three kinds of content, one per file type:

- **`preset.yaml` — parameters.** Eight dimensions as floats 0.0-1.0 (research: atomicity 0.8,
  linking 0.7, search 0.8 …), feature-block lists (always/conditional/optional/disabled), `self_space`,
  `qmd`, `personality` (e.g. `warm-supportive` vs `neutral-analytical`), `processing_depth`,
  `starter_mocs`. These parameterize the *generator*; 2brain has no generator, so this file type has
  no target.
- **`categories.yaml` — priors over what the vault will contain.** Extraction categories
  (research: claims, evidence, methodology-comparisons, contradictions, open-questions, design-patterns,
  design-dimensions; personal: reflections, relationship-dynamics, goals, habits, gratitude, lessons).
  This is a corpus-shaped prior: "the kinds of notes this vault expects." It is the piece of presets
  2brain's intake could actually use. **[design]** Intake could accept a category list to bias
  extraction suggestions and `tb_check` warnings toward the vault's declared intent.
- **`vocabulary.yaml` — prompts, not data.** Universal→domain term maps (note→reflection/claim,
  reduce→surface, reflect→find patterns, verify→check resonance, MOC→life area, inbox→journal) plus
  folder names and *user-facing command names* (`/surface`, `/find-patterns`). The setup skill's
  quality bar: "A therapy user never sees 'claim' or 'reduce.'" This is the same term map the `ask`
  skill applies to answers. Worth adopting as intake config: a per-vocabulary map that `tb_context`-
  assembled answers can be translated through. **[design]**
- **`starter/` — corpora.** Hand-written seed notes with correct frontmatter (`personal/starter/goals.md`
  ships a goals MOC with prose, tensions section, exploration prompts). Setup's "first-success moment"
  uses them so the user's first interaction creates *real* content. A 2brain analogue is a tiny seed
  pack written via `tb_write` on first intake — optional, low value at 2brain's maturity. **[design]**

**Verdict**: no preset-file system is needed. Adopt two of the four pieces — the vocabulary map and
extraction categories, both as intake configuration — and skip the dimension floats (they parameterize
a generator 2brain does not have) and the starter corpora (premature).

---

## 6. Ranked transfers

Ranked by value ÷ (cost × risk); costs assume one engineer familiar with the codebase.

| # | Transfer | Source | Form in 2brain | Cost | Risk |
|---|---|---|---|---|---|
| 1 | **Answer contract + honesty standard + depth principle** (6-part synthesis, direct-answer-first, sources-consulted footer, certainty levels, "don't fabricate coverage") | `skills/ask/SKILL.md` §Answer Synthesis, §Anti-Patterns | MCP prompt+resource shipped alongside `tb_search`/`tb_context` (text only) | hours | low |
| 2 | **Intent→modality routing table** (WHY/HOW/WHAT/COMPARE/DIAGNOSE/CONFIGURE/EVOLVE → tier + search mode + "consult hubs/graph_health before searching") | `ask` §Query Classification, §Search Strategy | prompt resource; optionally mirrored in `tb_search`'s tool description | hours | low |
| 3 | **Condition-based maintenance counters** (observations ≥10, tensions ≥5, inbox ≥3, sessions ≥5, methodology staleness ≥30d) surfaced on reads | `health` §Condition-Based Maintenance Signals; `hooks/scripts/session-orient.sh` CONDITION lines | daemon-side incremental counters; attached to `diagnostics`/`tb_context` output | days (engine) | low |
| 4 | **Advisory write-time schema warnings** (missing description/topics → warning in the write response) | `hooks/scripts/write-validate.sh` | adapter/engine advisory field on `tb_write_file`/`tb_create_file` responses; never blocks | 1-2 days | low (must stay advisory, unlike the gate) |
| 5 | **Severity taxonomy + ranked top-3 actions + report history** (FAIL/WARN/INFO with rationale; anti-overwhelm caps; findings→fix loop) | `health` §Quality Standards, §Output Format | documented output contract of extended `diagnostics`/`graph_health` | days | low-medium (noise control needed) |
| 6 | **Session capture + orientation as daemon state** (current/previous session records, goals surfacing, cross-agent continuity) | `session-orient.sh`, `setup` Step 10 session persistence architecture | daemon-held session records exposed via read surface; opt-in per vault | days | medium (privacy/noise; gate behind config like `.arscontexta` did) |
| 7 | **Serialized auto-commit at the mutation boundary** | `hooks/scripts/auto-commit.sh` | engine-side commit per accepted mutation, serialized with the write | days | medium (repo locking, perf; config-gated) |
| 8 | **Per-vault vocabulary map** (universal→domain terms applied to answers) | `presets/*/vocabulary.yaml`, `reference/vocabulary-transforms.md` (via `setup`) | intake config + answer-translation norm in the prompt pack | hours-days | low |
| 9 | **Onboarding/tutorial prompt texts** (setup's 3 prescribed screens; tutorial's WHY→DO→SEE and "forced connections pollute the graph"; help's narrative mode) | `skills/setup/SKILL.md` Phase 1.5, `skills/tutorial/SKILL.md`, `skills/help/SKILL.md` | MCP prompts (verbatim-capable — these texts were written to be output as-is) | hours | low |
| 10 | **Upgrade-by-consultation protocol** (compare methodology, not hashes; side-by-side for user-modified state; classify current/enhancement/correction/extension) | `skills/upgrade/SKILL.md` §Why Consultation, Not Hashing | the rule for refreshing 2brain's own prompt pack / skill resources | hours | low |

**Explicitly not transferable**: `setup`'s 16-step generator and 15-kernel validation (2brain does not
generate vaults; the kernel primitives are already structural properties of the daemon), `reseed`'s
8-dimension configuration space (same reason — but keep its drift vocabulary and its invariant), the
`knowledge-guide` proactive agent (orchestration belongs to the parent agent over MCP),
`scripts/sync-thinking.sh` (repo-specific corpus sync).

---

## Appendix: the two prompt texts worth adopting almost verbatim

**1. `ask`'s answer-quality contract** (`skills/ask/SKILL.md` §Answer Synthesis → §Quality Standards).
This is the mechanism-as-text that turns a retrieval result into an answer; it is already written to
be obeyed, not summarized:

> **Ground answers in specific claims, not general knowledge.** The knowledge base exists so answers
> are evidence-based. If you answer from general knowledge without consulting the graph, you are
> bypassing the tool.
>
> **Acknowledge gaps honestly.** When the research does not cover something, say so. "The current
> research graph doesn't have claims about X" is a legitimate answer. Do not fabricate coverage.

with the depth rule from §Search Strategy:

> **The depth principle:** A shallow answer citing 10 claims is worse than a deep answer weaving 4
> claims into a coherent argument. Read fewer sources more deeply.

**2. `architect`'s evidence-chain standard** (`skills/architect/SKILL.md` §Philosophy). This is the
contract advisory diagnostics output should meet — the difference between a warning list and advice:

> **Evidence beats intuition. Research beats habit.**
>
> You are not guessing what might help. You are diagnosing what IS happening (health + friction) and
> prescribing what research says SHOULD happen. Every recommendation traces to specific evidence. "I
> think this would be better" is not a recommendation. "Health shows 12 orphans, friction log shows
> repeated orphan complaints, research claim [[orphan notes decay faster than connected ones]]
> supports adding a condition-triggered reweave pass" — THAT is a recommendation.

Runners-up, short enough to fold into tool descriptions: health's "Dangling links are broken promises.
Every `[[link]]` a reader follows that leads nowhere erodes trust in the graph" (justifies why link
integrity is FAIL-severe), and health's "Report what CAN be checked, not what the platform lacks."
