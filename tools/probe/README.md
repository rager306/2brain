# tools/probe — measuring the engine from outside

`visit_bound.py` is the probe for entry 1 of `docs/research/kutha-ideas-2026-11-16.md`:
it asks whether a read's work grows with the corpus while its answer does not.

```sh
cd /root/2brain/tb-engine && RUSTUP_HOME=/root/.rustup cargo +1.99.0 build --release -p tb-cli --offline
python3 tools/probe/visit_bound.py                 # table on stdout
python3 tools/probe/visit_bound.py --json /tmp/visits.json
```

It copies the live vault once with `cp -a`, derives two nested truncations from that
copy, and starts one private engine per corpus — own `HOME`, own socket, never the
production daemon and never a write to `/root/vault`. The counters come from
`graph_health` with `verbs: true`, sampled before and after each query.

Requires an engine binary that carries the visit counters (the release build above).
The numbers and their reading are in `docs/research/visit-bound-2026-11-16.md`.
