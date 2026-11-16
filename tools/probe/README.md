# tools/probe — measuring the engine from outside

One style, two probes, both reading the same `graph_health { verbs: true }` counters
the engine reports about its own reads. Both copy the live vault with `cp -a`, start
private engines on the copy — own `HOME`, own socket, never the production daemon and
never a write to `/root/vault` — and stop only what they started. Build the binary they
start first:

```sh
cd /root/2brain/tb-engine && RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release -p tb-cli --offline
```

## `visit_bound.py` — does a read's work grow with the corpus?

Probe for entry 1 of `docs/research/kutha-ideas-2026-11-16.md`. It derives two nested
truncations from one copy of the vault and starts one engine per corpus, so a difference
between sizes is the corpus GROWING, never the corpus changing shape.

```sh
python3 tools/probe/visit_bound.py                 # table on stdout
python3 tools/probe/visit_bound.py --json /tmp/visits.json
```

The numbers and their reading are in `docs/research/visit-bound-2026-11-16.md`.

## `candidate_set.py` — what does a SCOPED read pay for what it discards?

Probe for entry 1 of `docs/research/ruvector-adr-2026-11-16.md`: it asks whether a read
restricts its candidates before scoring, or ranks everything and filters afterwards. Same
20-query set as `visit_bound.py`, asked at `scope: own` (the default) and `scope: all`.

```sh
python3 tools/probe/candidate_set.py                            # table on stdout
python3 tools/probe/candidate_set.py --json /tmp/rows.json      # raw rows
python3 tools/probe/candidate_set.py --capture /tmp/before      # every answered page
python3 tools/probe/candidate_set.py --against /tmp/au-old      # two binaries, one corpus
```

It records, per query and scope, the `search` verb's `documents_visited` and
`posting_blocks_decoded` (sampled either side of exactly one request), the answer's
`total`, and the wall time as the median and minimum of N rounds. `--capture` writes every
answered page, paged to the end and normalized (`$VAULT` for the workspace root, sorted
keys), so a before/after comparison of the answer sets is a `diff` rather than an opinion.

`--against SECOND-BINARY` is the mode a claim should rest on: the two binaries run at the
same time on two identical copies of one frozen corpus, each query alternating which
engine answers first inside each round, and both engines' answered pages are captured and
diffed. Two runs in sequence compare two different machines — this host carries other
work — and a drift between the blocks would be attributed to the change. The probe prints
the equivalence verdict and exits non-zero when the captures differ.

The numbers and their reading are in
`docs/research/candidate-set-first-2026-11-16.md`.
