# Intake targets: 33 content notes nothing points at

**Question.** C3 asks which notes have no inbound edge and what, if anything, should point at them.
A note with no inbound edge is reachable only by search; a hub that should cite it does not.

**Method.** The graph read `graph_shape { orphan_paths: true }` names every file with no inbound
edge. The corpus is `f/`, `knowledge/` and `manual/`; the other 149 no-inbound paths are session
dumps, type defs, templates, scripts and dot-directories, and an inbound edge there would mean
nothing. For each content orphan the tool asks the same `search` the serving path answers, with the
file stem as the query, and keeps the top three hits that are not the orphan itself.

```sh
python3 tools/kb/intake_targets.py --socket ~/.2brain/au-engine/run/<hash>.sock
python3 tools/kb/intake_targets.py --socket ... --json
python3 tools/kb/intake_targets.py --selftest
```

**Numbers (live vault, 2026-11-17).** 33 content notes with no inbound edge; 99 proposals, three
per note; 15 of the 99 matched a single term (noise); 24 orphans have a candidate that matched
three or more terms; 9 do not. Nothing is written — a proposal is a note the ranking places near
the orphan, which is evidence, not proof.

**Worth reading (candidate matched ≥3 terms).** The link, if accepted, goes FROM the candidate TO
the orphan.

| orphan (no inbound) | candidate source | score | terms |
|---|---|---|---|
| `f/patterns/pattern-adversarial-verifier-role.md` | `knowledge/six-role-multi-agent-architecture-maps-agentic-reasoning-layers-to-web-research-workflow.md` | 55.6 | 3 |
| `f/patterns/pattern-agent-first-runtime.md` | `inbox/expo-exact-runtime.md` | 37.5 | 4 |
| `f/patterns/pattern-agents-remove-apparatus-freely.md` | `inbox/exact-discipline-deep.md` | 96.6 | 4 |
| `f/patterns/pattern-budgets-as-trades.md` | `inbox/exact-discipline-deep.md` | 64.4 | 3 |
| `f/patterns/pattern-dashboard-flows-as-mcp-tools.md` | `inbox/meta-social-technologies-mcp-app-lifecycle.md` | 80.1 | 4 |
| `f/patterns/pattern-decision-layer-agent-work.md` | `inbox/subq-snap-1-2.md` | 37.8 | 4 |
| `f/patterns/pattern-defeasible-norm-conflicts.md` | `inbox/law-ai-nlp-landscape.md` | 50.5 | 3 |
| `f/patterns/pattern-forward-budget-keep-best-edit.md` | `inbox/dspy-wsp-weight-space-programming-optimizer.md` | 44.2 | 5 |
| `f/patterns/pattern-handle-based-lazy-outputs.md` | `inbox/rust-glancer-persistent-incrementality-ajika.md` | 33.0 | 4 |
| `f/patterns/pattern-kernel-gatekeeper-comparator.md` | `inbox/alphaproof-code-libsignal.md` | 90.4 | 3 |
| `f/patterns/pattern-memory-anchored-to-code-symbols.md` | `inbox/graphicle-rust-code-graph-agent-memory.md` | 79.0 | 5 |
| `f/patterns/pattern-option-attention-choice-head.md` | `inbox/jevlike-open-option-attention-starter.md` | 77.2 | 4 |
| `f/patterns/pattern-parser-preflight-no-invention.md` | `inbox/ingar30-reviewer-economics-paper-panel.md` | 61.5 | 3 |
| `f/patterns/pattern-policy-change-from-monitor-state.md` | `inbox/policy-change-treelike-monitors.md` | 86.0 | 5 |
| `f/patterns/pattern-semantic-context-beside-schema.md` | `.claude/skills/audit/SKILL.md` | 27.9 | 4 |
| `f/patterns/pattern-static-embedding-hybrid-search.md` | `knowledge/agentdb-reasoningbank-has-four-components-pattern-matcher-experience-curator-memory-optimizer-context-synthesizer.md` | 46.9 | 4 |
| `f/patterns/pattern-team-lead-context-management.md` | `inbox/alphaproof-code-libsignal.md` | 28.6 | 3 |
| `f/patterns/pattern-zero-token-idle-message-board.md` | `inbox/gemini-4-carbon-swarm-antigravity.md` | 134.8 | 5 |
| `knowledge/decision-models-crowned-by-calibration-commodity.md` | `inbox/openai-decisions-api-validates-decision-model-category.md` | 37.2 | 3 |
| `knowledge/formal-verification-became-agentic-november-2026.md` | `knowledge/kutha-temporal-graphs.md` | 24.4 | 4 |
| `knowledge/internal-planning-tree-in-a-public-repo-buries-the-product-diff.md` | `knowledge/github-rewards-a-named-file-set-and-2026-added-agents-md-to-it.md` | 66.4 | 11 |
| `knowledge/prime-typesafe-jev-comparison-insight.md` | `knowledge/jev.md` | 51.3 | 3 |
| `knowledge/secret-capture-without-llm-pattern.md` | `knowledge/agentic-flow-agent-booster-handles-trivial-edits-via-wasm-plus-ast-at-zero-cost-and-sub-millisecond-latency.md` | 28.9 | 3 |
| `knowledge/xml-jsonl-vs-json-ld-2026.md` | `inbox/dynaflow-freeform-orchestration-dsl.md` | 35.6 | 4 |

**Weak — no candidate above two terms; listed rather than dropped.** The ranking found a neighbour,
but on one or two shared words, so a link would be a guess. These are the intake refusals until a
human finds a real relation.

- `f/patterns/pattern-conservative-normalization.md` — best candidate `knowledge/miroflow-parallel-thinking-uses-voting-for-answer-selection.md` at 2 matched term(s)
- `knowledge/creusot-verdict-insight.md` — best candidate `inbox/rust-verification-tools-family.md` at 1 matched term(s)
- `knowledge/hotpath-rs-experiment-insight.md` — best candidate `knowledge/hotpath-profiler-analysis.md` at 2 matched term(s)
- `knowledge/omp-tool-steering-insight.md` — best candidate `knowledge/omp-debugger-architecture-insight.md` at 2 matched term(s)
- `knowledge/taste-injection-loop-pattern.md` — best candidate `knowledge/agents-of-chaos-prompt-injection-structural.md` at 2 matched term(s)
- `knowledge/toolcall-repair-layers-pattern.md` — best candidate `knowledge/three-tier-subgraph-structure-separates-episodes-from-entities-from-communities.md` at 2 matched term(s)
- `knowledge/trace-capture-activity-index-pattern.md` — best candidate `.claude/skills/remember/SKILL.md` at 2 matched term(s)
- `knowledge/vendor-doctrine-converges-on-atomic-judgments.md` — best candidate `inbox/sermakarevich-sixty-ai-code-reviewers.md` at 2 matched term(s)
- `manual/manual.md` — best candidate `inbox/allspark-research-tool-descriptions-learn-from-gitnexus-and-codebase-memory-mcp.md` at 1 matched term(s)

**Decision.** Two passes wrote **19** of the 24 notes, each as one added topic in a source that
states the target's subject: the first from the source's own words, the second from the pattern
naming its provenance (expo/exact, subq, dspy/wsp, glancer/ajika, alphaproof/libsignal, ingar30, jev).
Vault commits `850e89f` and `5fff55d`.

| target | source |
|---|---|
| `pattern-adversarial-verifier-role` | `inbox/gemini-4-carbon-swarm-antigravity.md` |
| `pattern-zero-token-idle-message-board` | `inbox/gemini-4-carbon-swarm-antigravity.md` |
| `pattern-agents-remove-apparatus-freely` | `inbox/exact-discipline-deep.md` |
| `pattern-budgets-as-trades` | `inbox/exact-discipline-deep.md` |
| `pattern-defeasible-norm-conflicts` | `inbox/law-ai-nlp-landscape.md` |
| `pattern-option-attention-choice-head` | `inbox/jevlike-open-option-attention-starter.md` |
| `pattern-policy-change-from-monitor-state` | `inbox/policy-change-treelike-monitors.md` |
| `pattern-static-embedding-hybrid-search` | `knowledge/mnemosyne-hybrid-search-three-component.md` |
| `decision-models-crowned-by-calibration-commodity` | `inbox/openai-decisions-api-validates-decision-model-category.md`, `inbox/decision-models-become-a-product-category.md` |
| `pattern-memory-anchored-to-code-symbols` | `inbox/graphicle-rust-code-graph-agent-memory.md` |
| `pattern-agent-first-runtime` | `inbox/expo-exact-runtime.md` |
| `pattern-dashboard-flows-as-mcp-tools` | `inbox/meta-social-technologies-mcp-app-lifecycle.md` |
| `pattern-decision-layer-agent-work` | `inbox/subq-snap-1-2.md` |
| `pattern-forward-budget-keep-best-edit` | `inbox/dspy-wsp-weight-space-programming-optimizer.md` |
| `pattern-handle-based-lazy-outputs` | `inbox/rust-glancer-persistent-incrementality-ajika.md` |
| `pattern-kernel-gatekeeper-comparator` | `inbox/alphaproof-code-libsignal.md` |
| `pattern-team-lead-context-management` | `inbox/alphaproof-code-libsignal.md` |
| `pattern-parser-preflight-no-invention` | `inbox/ingar30-reviewer-economics-paper-panel.md` |
| `prime-typesafe-jev-comparison-insight` | `knowledge/jev.md` |

Live graph: edges **4 730 → 4 750**, no-inbound **379 → 360**, isolated unchanged at **197**,
**0 diagnostics**.

**Five refusals, with the reason each.** The candidate named the same *category*, not the same claim:

- `pattern-semantic-context-beside-schema` — the only candidate is `.claude/skills/audit/SKILL.md`, tooling
  rather than a note, and it validates schema instead of carrying semantics beside it;
- `knowledge/formal-verification-became-agentic-november-2026.md` — `kutha-temporal-graphs.md` mentions
  formal verification in passing; the claim is about four independent sources converging;
- `knowledge/internal-planning-tree-in-a-public-repo-buries-the-product-diff.md` — the candidate is about
  GitHub rewarding a named file set, a different claim about repository contents;
- `knowledge/secret-capture-without-llm-pattern.md` — the candidate is "no LLM call" for trivial edits, the
  orphan is masked secret capture; the shared idea is a category;
- `knowledge/xml-jsonl-vs-json-ld-2026.md` — the candidate is an orchestration DSL, unrelated.

**The nine without a candidate at three terms** stay as they are: a link needs a reason, and the
ranking had none. The next pass starts from these five plus those nine.
