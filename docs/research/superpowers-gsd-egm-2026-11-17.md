# Superpowers, GSD, and Elephant–Goldfish: fit for 2brain

Research snapshot: **2026-11-17**. This is a source review and an experiment
proposal, not an installation, benchmark, deployment, or claim that any plugin
works in allspark. External source links below were accessed on 2026-11-17.
Pinned commits identify the inspected default-branch snapshots. “Last activity”
means last default-branch commit as returned by GitHub's commits API, not the
last issue, release, or commit on another branch.

## Identification and mechanisms actually shipped

| Item | Identity, authorship, license, activity | Shipped mechanism |
|---|---|---|
| Superpowers | [obra/superpowers](https://github.com/obra/superpowers/tree/22f0b463e7a5e579904dd6211a520321035526f7), Jesse Vincent and Prime Radiant contributors; MIT; default-branch commit [2026-11-03](https://github.com/obra/superpowers/commit/22f0b463e7a5e579904dd6211a520321035526f7). The [plugin manifest](https://github.com/obra/superpowers/blob/22f0b463e7a5e579904dd6211a520321035526f7/.claude-plugin/plugin.json) names Vincent. | Claude Code plugin **and** integrations for other hosts, not just Claude. Composable skills plus startup bootstrap instruct the agent to brainstorm, plan, use red/green TDD, dispatch/review work, and verify. The README also describes host-specific bootstrap limitations and optional visual-companion telemetry. These are upstream workflow claims, not independent measurements of effectiveness. |
| GSD Path | [open-gsd/gsd-path](https://github.com/open-gsd/gsd-path/tree/d5a30f5f6cc31ca1877f492d62cbe4af45f77c1e), Open GSD contributors; Jeremy McSpadden is a principal contributor, not established here as sole author; MIT; default-branch commit [2026-11-06](https://github.com/open-gsd/gsd-path/commit/d5a30f5f6cc31ca1877f492d62cbe4af45f77c1e). | A disk-backed phase router, `.project/` handoffs and approvals; task contracts, isolated parallel worktrees, serial landing and verification. The pinned README distinguishes host support from actual milestone evidence. Its optional fail-closed native guard is restricted to particular hosts and is not a 2brain mutation guard. |
| GSD Core | [open-gsd/gsd-core](https://github.com/open-gsd/gsd-core/tree/69557a7c022a3f4f54ba63930c0ee617b82014fe), Open GSD community continuation maintained by trek-e / Tom Boucher; original `gsd-build/get-shit-done` credited to TCHES in the [fork announcement](https://github.com/open-gsd/gsd-core/discussions/109); MIT; default-branch `next` commit [2026-11-17](https://github.com/open-gsd/gsd-core/commit/69557a7c022a3f4f54ba63930c0ee617b82014fe). | Discuss → plan → execute in fresh subagents/parallel waves → verify → ship. `STATE.md` and `CONTEXT.md` preserve handoffs. It is a workflow and context framework, not GSD Path under another name; its installer transforms host-specific agent formats. |
| Google EGM | [google/elephant-goldfish-model](https://github.com/google/elephant-goldfish-model/tree/72d06f52a2482885d9a64bd24b291c6e8628942c), Google EGM team; the [2026-10-22 announcement](https://opensource.googleblog.com/2026/10/introducing-elephant-goldfish-model-plugin.html) names Brian Suchy, Jim Quinn, Elise Ewing, Oliver Newth and Dave Rensin; Apache-2.0; default-branch commit [2026-11-15](https://github.com/google/elephant-goldfish-model/commit/72d06f52a2482885d9a64bd24b291c6e8628942c). The repository says it is **not an officially supported Google product**. | A context-rich drafting session creates a design; fresh reviewers see the design and referenced sources, not the earlier conversation; the human resolves findings and approves. The repository says to stop review if fresh subagents are unavailable. Its current review supports multiple stages; the announcement describes comprehension, adversarial review, and implementation readiness. |

### What “EGM” could mean

The hint identifies the **Elephant–Goldfish Model** with high confidence. It
can refer to two related things: Dave Rensin's original
[essay](https://drensin.medium.com/elephants-goldfish-and-the-new-golden-age-of-software-engineering-c33641a48874)
(2026-04-27; a method, not a software license) or the Google team's plugin
above (Apache-2.0). They are not two competing plugins. Another expansion,
[Earth Gravitational Model](https://bgi.obs-mip.fr/grids-and-models-2/grids-and-models-2-2),
is an actual but unrelated geodesy model; no evidence connects it to the hint.
The method's metaphor does not guarantee that independent reviewers are
objective or that they find all defects.

## Fit: what exists, what is new, and what conflicts

Local basis: [naming](../naming.md), [trajectory, reconciliation](../trajectory.md#2026-11-17--reconciliation-what-moved-since-this-page-was-last-revised),
[measurement doctrine](measurement-doctrine.md), and the repository's
[agent runbook](../../AGENTS.md), all inspected 2026-11-17. The current
allspark/prime-agent session supplies Markdown and Python REPL skills,
described tool discovery, prompt notes and memories, persistent subagents,
MCP connections, and compaction. These are runtime capabilities in this
session, not evidence that an external plugin is compatible with the host.

| Mechanism | Duplicate / potentially new | Fit and conflict |
|---|---|---|
| Superpowers | Existing skills, delegation and verification duplicate the substrate. Its particular red/green instructions, skill activation discipline, and separate spec-compliance then code-quality reviews are candidate process changes, not new infrastructure. | Allspark needs native tool and skill mappings; a Claude bootstrap/hook is not automatically an allspark hook. Mandatory per-change design, TDD, branch creation, and deletion of code written before tests can be disproportionate or conflict with existing work. Never treat a prompt instruction as an authorization gate. |
| GSD Core | Fresh subagents, persisted context and parallel delegation already exist. Explicit phase artifacts and context-window checks may improve specific long tasks, unproven here. | A second phase/state authority and automatic ship flow duplicate the lane register and session state. A clean subagent has no guaranteed 200k context in this runtime. |
| GSD Path | Durable, resumable phase transitions, task preflight against the actual base tree, and serialized landing are more concrete than an informal plan; worth examining if a measured resume failure appears. | `.project/`, Git hooks, branches/worktrees and agent rules would introduce another project-state authority and workflow gates. None replaces the typed KB, engine's authoritative writer, or MCP fail-closed authorization. Path does not claim allspark host validation in its pinned README. |
| EGM | Fresh subagents already exist. The potentially useful addition is a **bounded, document-only cold review** with a comprehension check and explicit resolution of findings; current delegation alone does not establish that protocol. | A child may have project instructions and tools even without conversation history; “zero context” must be verified, not assumed. Limit reads to the frozen design and referenced fixture sources, audit tool access, and do not let reviewers write. A human, not the review, decides trade-offs. |

All four influence the agent side; none provides evidence of typed-frontmatter
validation, wiki-link resolution, wire compatibility, or 2brain write
authorization. MCP descriptions help discover tools; they are not permission
to mutate. Continue through the existing fail-closed write gate; no agent
review, pipeline phase, Git hook, or approval note is a substitute. Judgment
stays outside the serving path. Source behavior versus served behavior must
remain separate: the reconciliation explicitly says they can differ. A new
process must not declare a design or source change deployed. See the local
sources above and [answer contract](answer-contract-2026-11-16.md).

## Adaptation cost and test boundary — estimates, not completed work

These are rough engineering-order estimates, not measured labor or a commitment
to implement. Original text copied or adapted from MIT/Apache projects requires
retaining applicable notices; Apache-2.0 has additional terms. Have a human
check licensing and third-party dependency/telemetry policy before reuse.

| Option | Hypothetical placement and replacement | Cost and gates |
|---|---|---|
| One Superpowers-inspired workflow | A project-local `tb-skills/` skill or an allspark skill/prompt note, after deciding the owner; map native tool names and cite upstream. Replace a redundant instruction, not the runtime's skill registry. | Small: roughly half a day to two days for one bounded workflow and review, excluding evaluation. Test triggering, non-triggering and behavior after compaction; reproduce a failing test before trusting a TDD instruction. If Rust product code is changed later, use its normal fmt, scoped tests and clippy; no product change is needed for a prompt experiment. |
| EGM-style cold review | A brief allspark subagent instruction and ordinary design artifact; later, possibly one `tb-skills/` skill. Reuse native subagents and existing documents rather than install another orchestrator. | Small for a manual pilot; isolation and access audit add work. Test that reviewers have fresh histories, cannot access hidden scenario answers, cite evidence, and cannot write; no serving-path change. |
| GSD Core adaptation | An allspark-native mapping of commands, worker dispatch and durable handoffs, if proven necessary; decide whether existing session/lane state is replaced before adding `STATE.md`. | Medium to large: several days to weeks, highly uncertain. Test resume across compaction/restart, dispatch failures, model/tool mapping and ownership of handoff state. |
| GSD Path adaptation | External tooling and agent skills, not Rust serving code; decide whether `.project/` replaces rather than mirrors the existing lane register and who owns branches and landing. | Large: weeks or more, unestimated dependency and maintenance costs. Test project initialization, restart/crash recovery, phase transitions, permissions, branch isolation, serial landing, and hook coexistence. The upstream host tests are not proof for allspark. |

If a later change touches `tb-engine/`, run the relevant Rust 1.99.0 tests,
format and clippy gates, and compare the engine golden wire contract on a
frozen fixture; intentional wire changes require the documented contract
procedure. If it touches `tb-mcp-rs/`, run adapter fmt, locked tests and
clippy including gate and fake-daemon tests. These are relevant only to such
product changes; they do not validate a prompt's effectiveness. Never point
the proposed experiments at a live vault or production socket.

## Recommendation and a falsifiable pilot

**Do not install any of the four wholesale now.** Trial a single narrow
Superpowers-inspired review instruction using allspark's existing skill and
subagent primitives; compare it with an EGM-style cold-review instruction and
today's workflow. Path's durable router is the strongest distinct mechanism,
but its state and operations cost is not justified without a demonstrated
resume/coordination failure. These are recommendations, not shipped behavior.

Pre-register this small **screening** experiment before running it; the
thresholds below are proposed decision rules, not measured constants:

1. Prepare 12 frozen, isolated design-task fixtures from repository history or
   synthetic cases, spanning wire changes, gate failure paths, typed KB
   validation and doc/source deployment drift. No live data. An independent
   author records an evidence-backed answer key before the runs. Include
   naturally occurring omissions, not just planted markers. Do not give the
   key, prior discussion, or other arms' output to reviewers.
2. Run each fixture in three independently started sessions, same model,
   tools, time/token cap and source snapshot: **A** current allspark workflow;
   **B** a small, explicitly invoked Superpowers-inspired spec-then-quality
   review, using a fresh reviewer; **C** an EGM-style fresh reviewer given only
   the design and explicitly referenced fixture sources, required to first
   explain the design then identify gaps. B and C are adapted *protocols*, not
   installed plugins. Randomize arm order; retain prompts, traces, outputs,
   versions, model and actual token/time cost. Audit that the cold reviewer
   did not receive the preceding conversation or the answer key. If isolation
   cannot be established, label C invalid, not a pass.
3. A blinded human adjudicates each unique actionable finding against the
   source snapshot and predeclared acceptance criteria; record true defects,
   false/unsupported findings, and missed answer-key defects by fixture, plus
   time and tokens. Primary metric: paired number of fixtures where an arm
   catches at least one independently adjudicated material defect that A
   misses, minus fixtures where A catches one it misses. Report each pair and
   the denominator, not just an aggregate. The reviewer is not the resolver;
   the human adjudication and evidence are the resolver.
4. Before considering further adaptation, require **at least 3 net improved
   fixtures out of 12**, no unsafe mutation or gate bypass, and median total
   tokens at most **1.5×** A. Count a tie, fewer than 3 net improvements,
   excessive cost, broken isolation, or an unsafe action as **no adoption on
   this pilot**. These thresholds express an initial cost/benefit preference,
   not statistical significance; 12 paired examples cannot establish a
   general population effect. If promising, pre-register a larger held-out
   set with measured variance before claiming general improvement.
5. Validate the instrument: self-pair A against itself must yield zero; remove
   an answer-key defect from a review and confirm recall falls. Report the
   fixture population, all results and disagreements. A negative result
   rejects this protocol on this task set, not the entire upstream project.

**Not worth trying now:** production deployment; installing upstream hooks,
bootstraps or dashboards; a universal TDD/brainstorm mandate; a second
`.project/` source of truth; bypassing MCP for writes; or a full four-way
implementation benchmark before the cheap process pilot. A GSD Path pilot
would require a separate hypothesis about recovery and a crash/resume fixture,
not an inference from a design-review test.
