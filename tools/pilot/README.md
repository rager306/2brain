# Pre-registration: bounded design-review pilot

Status: **not run**. This is a screening experiment, not proof of general
improvement and not an installation of any upstream plugin. The thresholds
below are proposed decision preferences, not measured constants. The three
seed fixtures exist; nine further fixtures, independently authored and
evidence-checked answer keys, frozen sources, and isolation controls are
required before the pilot can run. No results may be claimed from the seeds.

## Population and construction

The population is exactly 12 diagnostic fixtures, `f01`–`f12`, not a random
sample. Each is a frozen design artifact plus a minimal allowlist of frozen
repository source files and an independently authored, evidence-backed answer
key. The following slots are registered now; the nine unbuilt slots are not
represented as existing or validated fixtures. The author must create those
artifacts and keys, and a separate human must check their source evidence and
materiality before seeing any review output. Include naturally occurring
omissions from history as well as constructed failures. If a listed source
cannot support a fixture, stop and amend the registration before the run.

| ID | Class | Construction |
|---|---|---|
| f01 | wire shape | Synthetic rename of `ready` without version or golden update, against `WIRE.md` and `AGENTS.md`. |
| f02 | write gate | Synthetic direct-write retry after prior-read/freshness denial, against `gate.rs` and `policy.rs`. |
| f03 | typed KB | Synthetic required-field omission and missing typed reference, against the three type-system atoms named in the fixture. |
| f04 | wire shape | History: reconstruct the pre/post schema v17 envelope migration from `WIRE.md` and the relevant commit; review a pre-migration compatibility plan omitting client migration. |
| f05 | wire shape | Synthetic framing proposal removing the documented length bound; use `WIRE.md` framing and identify the omitted protocol consequence. |
| f06 | write gate | Synthetic create plan that assumes a file is absent without the gate's absence check; use `policy.rs` and the implementing handler at the frozen revision. |
| f07 | write gate | Synthetic member-grant plan that ignores path scope; use `gate.rs` and `policy.rs`. |
| f08 | typed KB | Synthetic shape mismatch for a required field; use `type-instance fields.md` and the relevant type shape atom. |
| f09 | typed KB | Synthetic ambiguous validated reference incorrectly accepted; use `reference name resolution.md`. |
| f10 | source/deployment drift | History: reconstruct the default search-scope change described in `docs/research/task-set.md`; review a design claiming that a source change proves deployment. Freeze the relevant source and run record. |
| f11 | source/deployment drift | Synthetic engine swap without frozen-copy wire comparison; use `AGENTS.md` and `WIRE.md`. |
| f12 | source/deployment drift | History: reconstruct the rejected title-index experiment in `docs/research/task-set.md`; review a design claiming title weighting changes scores only, ignoring candidate-set effects. |

For history slots the author records the exact commit IDs, extracts the cited
files and design from that history into an immutable offline snapshot, and
documents any synthetic edits. For synthetic slots record the base commit and
the exact proposed text. All arms for one fixture get the same snapshot, and
each review can read only its design and allowlisted sources. No live data,
network, socket, source edits, executable mutation, or key access. A fixture's
`checks` list supplements this global no-write rule; it is not exhaustive.
Store the answer keys outside the reviewer's accessible environment. Never
pass the fixture JSON itself to a reviewer: it contains the key.

## Arms, isolation and execution

* **A**: the current workflow, reviewing the design in an independently
  started session.
* **B**: an explicitly invoked Superpowers-inspired protocol, with a fresh
  reviewer doing specification compliance first, quality and failure paths
  second. It is not the installed plugin.
* **C**: an EGM-style cold reviewer, in a new isolated session, first explains
  the design and then identifies gaps from only that design and allowlisted
  frozen sources. It is not the installed plugin.

Run one repetition of each arm per fixture (36 sessions). `plan.py` emits a
seeded random permutation of the 36 jobs when all fixtures are present; its
current output is 9 jobs for the 3 seeds, not the full pilot. The default
ordering seed is 20261117. Fix the same model and version, source snapshot,
tool access, and explicit time and token caps for every arm *before* running;
record those values, all prompts, review outputs, traces, start order, actual
elapsed time and total tokens, including drafting/reviewer overhead. The
operator must provision read-only, network-disabled isolated environments;
the prompt alone does not enforce isolation. The scripts never run a review.

Audit C's actual history, mounts, available tools and accesses for each run:
no preceding conversation, key, other arm output or writes. Verify B's fresh
reviewer too. If cold isolation cannot be established for any C run, **C is
invalid and no adoption**, even if its score would pass. A finding review is
not an authorization gate. A human decides all findings and trade-offs.

## Adjudication and decision

A human blinded to arm identity sees anonymized findings, fixture design,
frozen allowed sources, and the predeclared key. Independently adjudicate
each unique actionable finding as a material true defect or false/unsupported;
merge duplicate descriptions of the same defect across arms to the same stable
fixture-local ID. Genuine independently evidenced defects not in the key
count too. Record evidence and disagreements separately; blindness and
resolution must precede scoring. For each arm record key IDs missed, not only
the number found. False/unsupported findings are counted and reported, not
silently discarded. The reviewer does not resolve findings.

For each fixture, count a win if the arm catches at least one material true
defect A misses and a loss if A catches at least one the arm misses; both may
occur on the same fixture and cancel. Primary metric is wins minus losses,
with all 12 pairs shown. A candidate requires **net >= 3/12**, median total
tokens **<= 1.5 times A**, and **no unsafe action in any arm**. C also requires
verified isolation. Otherwise the result is **no adoption**; zero is
inconclusive, not evidence of equivalence. These thresholds are not a
statistical significance test. Report denominator, every pair, false findings,
missed keys, time, tokens, isolation audit and disagreements. Negative results
apply to this protocol on this set, not to entire upstream projects.

## Instrument and CSV

```sh
python3 tools/pilot/plan.py --json
python3 tools/pilot/score.py --selftest
python3 tools/pilot/score.py adjudication.csv --cold-isolation verified
```

CSV columns are exactly `fixture,arm,true_defects,false_findings,missed_key_defects,tokens,unsafe`.
There is one row per fixture/arm, exactly one repetition. The three finding
columns are JSON arrays of unique IDs inside CSV-quoted fields, e.g.
`"[""required-field""]"`; use `[]` for none. True IDs must be identical
across arms for the same defect. False IDs identify distinct unsupported
claims, missed IDs identify key defects. Sets in each row are disjoint.
`tokens` is a non-negative integer, `unsafe` is `true` or `false`.
Time and evidence remain in the adjudication record, not this CSV. Incomplete
or unregistered populations cannot pass; missing arm pairs are errors.
`--cold-isolation` defaults to invalid; only pass `verified` after the audit.
The scorer cannot verify human blindness, evidence, source freezes, or actual
isolation: its PASS is conditional on those controls. Selftest verifies the
self-pair, three net wins, unsafe veto and deliberately degraded input.
