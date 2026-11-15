# Pogocache patterns, and which of them 2brain should take

Read at `github.com/tidwall/pogocache`, commit `9ca1b8d`, `src/` (18 318 lines of C), README section
"Design details". Measured against our own numbers the same day: daemon RSS 72.6 MiB for 1 038 notes /
1 186 nodes / 4 606 edges, one engine frame 1.1 ms, `MEMORY_BUDGET_PER_DOCUMENT = 6144` bytes.

Pogocache is a cache: entries with a TTL, evictable, no authority. 2brain is the opposite — an authoritative
typed graph over files, rebuilt to an immutable version. That difference decides most of the verdicts below;
what transfers is the *data layout and budget discipline*, not the lifecycle.

## 1. What it actually does

| technique | where | what it buys |
|---|---|---|
| sharded map, 4 096 shards by default, one independent open-addressing map each | `pogocache.c:699` `struct shard`, `--shards` | contention falls with core count; no global lock |
| Robin Hood open addressing with 10-byte buckets: 1-byte dib, 3-byte hash, 6-byte pointer | `hashmap.c`, README | one 64-byte cache line holds 6 buckets; a probe touches one line |
| the 64-bit hash split: **high 32 bits choose the shard, low 32 bits the bucket** | README, `shard_get` | a key hashes once and the shard never re-hashes |
| one heap allocation per entry: header + optional fields + key + value; keys packed at 6 bits/byte ("sixpack") | `pogocache.h`, `--keysixpack` | fewer allocations and ~25% off key bytes |
| per-shard spinlock held for the operation; multi-shard work takes shards in a fixed order (`struct batch`) | `pogocache.c:699-730` | no deadlock, no global contention |
| **2-random eviction**: scan from a hash-derived offset for two live candidates, take an expired one if seen, else the older (`entry_time`) | `pogocache.c:1101` `auto_evict_entry` | eviction in a few probes, no LRU list, no per-hit bookkeeping |
| background sweep of expired entries with a **hard cap: expired entries may hold at most 10% of cache memory** | `pogocache_sweep`, `pogocache_sweep_poll` | memory is reclaimed incrementally, not in a stop-the-world pass |
| one event loop per thread, connections bound round-robin, no shared mutable state; io_uring optional | `net.c`, `main.c` | linear scaling, no runtime |
| one core, several wire protocols (memcache, RESP, HTTP, Postgres) translating into the same store API | `memcache.c`, `resp.c`, `http.c`, `postgres.c` | the protocol is a thin layer, not a second implementation |
| CAS opt-in, not default (costs 8 bytes per entry) | `--cas`, `pogocache.h` | the expensive guarantee is paid for only by the caller who needs it |
| the same core compiles into the host program, network bypassed | README "Embeddable", `pogocache.c` single file | the server is the embedding's shell, not its definition |

## 2. Verdicts for 2brain

**ADAPT — 2-random eviction under an explicit memory budget.** Our search index has a per-document budget
(`MEMORY_BUDGET_PER_DOCUMENT`) and no eviction policy: exceeding it is a test failure, which is right for a
build-time index but useless for anything bounded at runtime — a result cache, an excerpt cache, a warm
query plan. Two random candidates and a "prefer expired, else older" rule is ~20 lines and needs no per-hit
bookkeeping. *Not* for the IR: the IR is authoritative and evicting a node would be losing data.

**ADAPT — counters as first-class data.** The engine has no per-verb latency or cache-hit counters, so
"is `au_grep` slow or is the vault big" is answered with `time`. Pogocache keeps cheap per-shard counters
and aggregates on read. One histogram per verb, aggregated per version, would make the next ranking or
protocol experiment answerable without a stopwatch.

**ADAPT — the layout arithmetic, but only where the budget is the constraint.** Our BM25 index keeps
`term_ids: HashMap<String, u32>`; the vault yields **21 001 unique terms, 156 KB of key bytes**. Estimated
footprint at hashbrown's defaults — 1 control byte + 24-byte `String` header + 4-byte value per slot, plus
one heap allocation per key — is ~1.4 MiB, against **~554 KiB** for 10-byte open-addressed buckets over a
single key arena: 60% less, or **0.84 KB per document, 14% of the 6 144-byte per-document budget**. That
matters for exactly one reason — proximity, the next ranking step, needs room for per-posting positions.
Compacting the term map is how that room gets paid for. It is 1.2% of RSS, so it is *not* a memory-diet item.

**ADOPT (design validation) — CAS as an explicit, opt-in cost.** We already refuse a write without
`expected_hash`; pogocache keeps CAS off by default because it costs 8 bytes per entry. Our choice is the
stricter one and it is right for an authoritative store — no change, but it is worth knowing the trade is a
deliberate one and not an absence of alternatives.

**REJECT — sharding and spinlocks.** Our IR is immutable per version: one writer builds it, readers hold an
`Arc`. There is no lock to shard. Sharding the *map inside* one version would not raise throughput (the
version is read-only) and would cost the deterministic iteration order our tests rely on.

**REJECT — per-thread event loops without a runtime.** Measured: 1.1 ms per engine frame, of which the
socket and framing are noise. Tokio's cost is not the bottleneck and rewriting the server to epoll-per-core
would buy nothing measurable at 1 038 notes.

**REJECT — sixpack key compression.** 6 bits per byte saves 25% of key bytes: 39 KB of 72.6 MiB. The
vocabulary estimate above shows the same point; packing is an answer to a constraint we do not have.

**REJECT — io_uring.** It reorders and batches syscalls; our per-call work is ~1 ms of CPU, not syscall
latency. There is no measured queue depth to fill.

**REJECT (for now) — an HTTP read endpoint next to MCP.** The protocol-per-surface lesson is real, and
`curl` against the vault would be pleasant for debugging. But we already have MCP resources
(`2brain://<member>/<path>`) and `2brain info`; a third read surface with no named consumer is exactly the
item this project deletes on measurement. Revisit when something needs it — not before.

## 3. The three experiments worth running, ranked

1. **Compact term map in the search index** — built, measured, **reverted** (`/tmp` experiment, no commit).
   The table was 8-byte buckets (tag, dib, id) over one key arena, with spans and hashes by dense term id,
   open-addressed with Robin Hood growth; a differential test proved it answers exactly like the
   `HashMap` it replaced over 210 terms including growth past 16 buckets. Measured on the generated corpus
   the budget tests use: **4 703 → 4 695 bytes per document**, 8 bytes, not the ~840 predicted. The
   prediction was arithmetic on key bytes; the corpus's vocabulary is small and the postings dominate, so
   the term map is a smaller share than the real vault suggests. Even taking the vault's 21 001 terms and
   the measured ~11 bytes saved per term, the transfer is **0.22 KB/doc — below the 0.5 KB/doc kill
   criterion** this entry set for itself. Reverted rather than kept as a curiosity; the estimate in §2 was
   wrong about the division of weight inside the index, and the measurement is the correction.
2. **Per-verb histograms in the engine** (~80 lines). Measure: the distribution of `au_grep` vs `au_search`
   on the live vault, which today is two stopwatch numbers. Kill criterion: no verb shows a tail worth
   acting on.
3. **2-random eviction for a bounded runtime cache** — only once such a cache exists. Building it before
   there is a cache is the "item without a consumer" mistake.

## 4. What does not transfer at all

Pogocache's whole design assumes the store may forget anything: every entry is replaceable and eviction is a
feature. Our store may forget nothing — the files are the truth and the IR is derived. So expiration, TTL,
low-memory eviction, sweeps and CAS-by-default are answers to a question we are not allowed to ask. What is
left is the discipline that made those answers cheap: **compact layouts, one allocation per object, budgets
stated as constants, and eviction that costs a few probes instead of a bookkeeping structure.**
