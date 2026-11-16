# FSM run logs

A run log is the audit trail of one agent run: which state it entered, which guard let it leave, and
whether the guard's answer came from a tool, the engine, or the judge. The checker enforces the rules in
`docs/research/meta-prompt-fsm.md`; it needs no model and no network.

```sh
python3 tools/fsm/check_run.py --selftest      # proves each rule can fail
python3 tools/fsm/check_run.py run.jsonl       # 0 clean, 1 violation, 2 unreadable
```

## Line shapes

```json
{"run":"r1","state":"ORIENT","event":"enter","at":"2026-11-16T10:00:00Z"}
{"run":"r1","state":"ORIENT","event":"guard","guard":"overview_answered","source":"tool","call":"tb_overview","ok":true}
{"run":"r1","state":"ORIENT","event":"exit","to":"LOCATE","at":"2026-11-16T10:00:02Z"}
{"run":"r1","state":"LOCATE","event":"guard","guard":"same_subject","source":"judge","question":"same_subject","probability":0.91,"threshold":0.8,"model":"subq-snap-1.2","batch_id":"b7","ok":true}
{"run":"r1","state":"APPLY","event":"exit","to":"RECOVER","outcome":"outcome_unknown","at":"…"}
```

## Rules the checker enforces

1. states come from the nine-state vocabulary;
2. a state cannot be left without at least one **passing** guard recorded in that state;
3. a guard names its source (`tool`, `engine`, `judge`); a judged guard also carries `question`,
   `probability`, `threshold`, `model` and an explicit `comparison` — `gt` when "true" means the good
   state, `lt` for an inverted question ("is this stale?"). A threshold nobody can audit is not
   evidence, and a checker that guesses the polarity silently blesses a wrong gate;
4. judged guards are only allowed in `ORIENT`, `LOCATE`, `PROPOSE`, `CONSOLIDATE`;
5. transitions must be edges the document declares — an invented edge is the first symptom of an FSM that
   drifted from its design;
6. `outcome_unknown` must leave `APPLY` for `RECOVER`, never for `VERIFY`.

## Why a log at all

The FSM is only a contract if a run can be checked after the fact. The log is cheap to write (one line per
event), it survives the session that produced it, and it converts "the agent decided" into "the agent
decided, here is the probability and the threshold it cleared".

`examples/hygiene-run.jsonl` is a complete, passing log of the nightly hygiene pass the trajectory
proposes — the shape a real run should have.
