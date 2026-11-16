//! Per-verb request counters: how many requests each read (and the one control
//! verb, `shutdown`) answered, and a latency histogram for each, read back through
//! `graph_health`.
//!
//! The engine held no per-verb instrumentation, so "is `grep` slow or is the
//! vault big" was answered with a stopwatch on one call, which cannot separate a
//! slow verb from a big corpus. This module is the instrumentation the serving
//! path can afford to carry: one relaxed atomic add per request, no lock, no
//! allocation, and a fixed array so a request can never grow the daemon.
//!
//! Two facts decide the shape:
//! - the counters live in process-global statics, NOT in the held
//!   [`KnowledgeBase`](crate::KnowledgeBase), so a rebuild leaves them alone:
//!   they count REQUESTS, and a request is not a version;
//! - the histogram is fixed-size log2 buckets, so the cost per request is two
//!   atomic adds whatever the latency turns out to be. A reservoir or a sorted
//!   sample would answer percentiles more precisely and would allocate on the
//!   hot path, which is the one thing a serving loop cannot do.
//!
//! The same rows carry the VISIT counters: how many documents a read examined
//! and how many posting blocks it decoded. They exist to answer a question the
//! latency histogram cannot — whether a read's work stays bounded as the corpus
//! grows, or scales with the corpus while the answer stays one page. A read
//! declares the scope it visits inside ([`visit_scope`]) and the walks inside it
//! report what they touched ([`note_documents`], [`note_posting_blocks`]).
//!
//! The visit path is DELIBERATELY not an atomic per document. The accumulator is
//! a thread-local tally, so a walk in a loop pays a plain cell add, and the scope
//! flushes it into the per-verb atomics once, on the way out. Counting is
//! observation only: no read consults these numbers, so a wrong one can change a
//! report but never an answer.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;

/// The verb census: every [`Request`](crate::serve::Request) variant's wire
/// name, one row per verb, sorted so a lookup is a binary search and so the
/// order is a property a test can assert rather than an accident of editing.
///
/// The map itself is `Request::verb`; this table is the index the counters use.
/// A test (`serve::verb_tests::the_verb_map_agrees_with_the_wire_census`) binds
/// the map, this table, and `WIRE.md`'s census to each other.
pub(crate) const VERBS: [&str; 52] = [
    "anchors",
    "block_ids",
    "candidate_counts",
    "candidates",
    "commit_meta",
    "config",
    "content",
    "device_config",
    "diagnostic_counts",
    "diagnostics",
    "dir_entries",
    "file_history",
    "files",
    "frontmatter",
    "graph_health",
    "graph_shape",
    "grep",
    "hubs",
    "ignores",
    "imports",
    "instance",
    "instance_counts",
    "instances",
    "instances_of",
    "lifecycle",
    "link_graph",
    "members",
    "neighborhood",
    "overview",
    "pins",
    "preview_mutation",
    "recent_commits",
    "references_in",
    "references_out",
    "resolve_anchor",
    "resolve_block_id",
    "resolve_member",
    "resolve_target",
    "rollback_preview",
    "search",
    "semantic_tokens",
    "shutdown",
    "subtypes",
    "top_level_dirs",
    "type",
    "type_batch",
    "type_closure",
    "type_counts",
    "type_graph",
    "type_tree",
    "types",
    "validate_value",
];

const VERB_COUNT: usize = VERBS.len();

/// Buckets per verb. Bucket 0 is under one microsecond, bucket `i` (1..) covers
/// `[2^(i-1), 2^i)` microseconds, and the last bucket is open-ended at the top
/// (everything at or above `2^(BUCKET_COUNT-2)` µs, about 8.4 s at 25).
///
/// Octaves from 1 µs up is the useful resolution for this serving path: the
/// measured reads sit between ~100 µs and a few ms, so buckets 8–13 separate a
/// fast read from a slow one, and the open top absorbs a `grep` over a cold page
/// cache without a bucket per order of magnitude nobody will ever read.
pub(crate) const BUCKET_COUNT: usize = 25;

/// The one verb counter row: a request count, its latency histogram, and the
/// visits its answered requests made.
struct VerbCounters {
    count: AtomicU64,
    buckets: [AtomicU64; BUCKET_COUNT],
    documents_visited: AtomicU64,
    posting_blocks_decoded: AtomicU64,
}

impl VerbCounters {
    /// A zeroed row. This must stay a function: every use of a named `const`
    /// makes a fresh copy, so the counters would silently not be shared.
    const fn zero() -> Self {
        VerbCounters {
            count: AtomicU64::new(0),
            buckets: [const { AtomicU64::new(0) }; BUCKET_COUNT],
            documents_visited: AtomicU64::new(0),
            posting_blocks_decoded: AtomicU64::new(0),
        }
    }
}

static COUNTERS: [VerbCounters; VERB_COUNT] = [const { VerbCounters::zero() }; VERB_COUNT];

/// One verb's counters as served: the request count, the histogram buckets, and
/// the visits, `buckets.sum() == count` at every instant a client reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct VerbCount {
    /// Requests this verb answered since the process started.
    pub count: u64,
    /// The histogram: `buckets[i]` requests in bucket `i` (see
    /// [`BUCKET_COUNT`] for the bounds).
    pub buckets: [u64; BUCKET_COUNT],
    /// Document-visit STEPS this verb's answered requests made, summed.
    ///
    /// A step, not a distinct document: a document is counted once per pass that
    /// examines it, so a two-term `search` that scores one document twice
    /// contributes two. The number answers "how much of the corpus does one
    /// request walk", so a step per (term, document) posting is the honest unit —
    /// deduplicating would need a set and would hide the repetition.
    pub documents_visited: u64,
    /// Posting blocks this verb's answered requests decoded. One per term list
    /// walked in a scoring or adjacency pass; zero for reads that hold no
    /// postings (a backlink lookup, a graph walk).
    pub posting_blocks_decoded: u64,
}

/// The bucket a duration lands in: 0 under 1 µs, else the octave above it,
/// capped so the top bucket is open-ended.
fn bucket_for(elapsed: Duration) -> usize {
    // `as_micros` is a u128, and a `Duration` can exceed u64 microseconds; the
    // clamp keeps an absurd span in the open top bucket instead of truncating it
    // into a small one.
    let micros = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
    if micros == 0 {
        return 0;
    }
    let octave = (u64::BITS - micros.leading_zeros()) as usize;
    octave.min(BUCKET_COUNT - 1)
}

/// The slot a verb's counters live in; `None` for a name outside [`VERBS`],
/// which a caller never produces (the map returns a census literal) and which a
/// count must not panic on in any case.
fn slot_of(verb: &str) -> Option<usize> {
    VERBS.binary_search(&verb).ok()
}

/// The tally a [`visit_scope`] accumulates: the visits so far, waiting for the
/// scope to close and flush them into its verb's row.
#[derive(Clone, Copy)]
struct Tally {
    documents: u64,
    posting_blocks: u64,
}

thread_local! {
    /// The innermost open [`visit_scope`] on this thread, if any. A scope is
    /// per-request and per-thread by construction: the walks it covers run on the
    /// thread that opened it (a read that hands work to a blocking task opens its
    /// scope inside that task, see `serve`'s `grep`).
    static TALLY: Cell<Option<Tally>> = const { Cell::new(None) };
}

/// One open visit scope. Closes on drop, so an early return and a panic unwind
/// both flush what was actually walked and restore the enclosing scope.
struct VisitScope {
    previous: Option<Tally>,
    slot: usize,
}

impl Drop for VisitScope {
    fn drop(&mut self) {
        let finished = TALLY.with(Cell::get);
        TALLY.with(|c| c.set(self.previous));
        // A nested scope is not an error: a read may compose another instrumented
        // read (`overview` folds the hub ranking and the graph shape). The inner
        // one is counted for its own verb and the outer one continues with its own
        // unchanged tally.
        let Some(finished) = finished else {
            return;
        };
        if finished.documents > 0 {
            COUNTERS[self.slot]
                .documents_visited
                .fetch_add(finished.documents, Ordering::Relaxed);
        }
        if finished.posting_blocks > 0 {
            COUNTERS[self.slot]
                .posting_blocks_decoded
                .fetch_add(finished.posting_blocks, Ordering::Relaxed);
        }
    }
}

/// Attribute the visits inside `f` to `verb`.
///
/// The one call a read makes to be measurable. Outside a scope the two `note_`
/// functions are no-ops, so a path under test or reached from a verb that opens
/// no scope pays nothing and reports nothing — the counters describe only the
/// reads that asked to be described.
///
/// There is no reset verb: the counters accumulate for the process's lifetime,
/// the same contract the request counts already have, and a caller that wants a
/// per-request figure reads the snapshot before and after (the probe under
/// `tools/probe/` does exactly that).
pub(crate) fn visit_scope<T>(verb: &'static str, f: impl FnOnce() -> T) -> T {
    // An unknown verb would have nowhere to flush to; it also cannot happen (the
    // call sites are census literals), so the read simply runs unmeasured rather
    // than clobbering an enclosing scope's tally.
    let Some(slot) = slot_of(verb) else {
        return f();
    };
    let guard = VisitScope {
        previous: TALLY.with(Cell::get),
        slot,
    };
    TALLY.with(|c| {
        c.set(Some(Tally {
            documents: 0,
            posting_blocks: 0,
        }))
    });
    let value = f();
    drop(guard);
    value
}

/// Record that the walk examined `count` documents (or document-posting steps).
///
/// A no-op outside a [`visit_scope`]. Cheap by design: one thread-local cell
/// read and one add, no atomic and no shared cache line, so a caller may note in
/// a loop. Prefer noting a batch where the batch size is already known (a posting
/// list's length), which is what the search paths do.
pub(crate) fn note_documents(count: u64) {
    TALLY.with(|c| {
        if let Some(mut tally) = c.get() {
            tally.documents += count;
            c.set(Some(tally));
        }
    });
}

/// Record that the walk decoded `count` posting blocks. See [`note_documents`].
pub(crate) fn note_posting_blocks(count: u64) {
    TALLY.with(|c| {
        if let Some(mut tally) = c.get() {
            tally.posting_blocks += count;
            c.set(Some(tally));
        }
    });
}

/// Record one answered request: its count, and its handler time in a bucket.
///
/// Called once per request from the serve loop, after the handler returns, so it
/// times the HANDLER and not the frame's write or the client's round trip. A frame
/// whose `read` tag names no known verb fails deserialization before the handler,
/// so it names no verb to count — the counters answer "what did this verb cost",
/// not "what did the socket receive".
pub(crate) fn record(verb: &'static str, elapsed: Duration) {
    let Some(slot) = slot_of(verb) else {
        return;
    };
    let bucket = bucket_for(elapsed);
    COUNTERS[slot].count.fetch_add(1, Ordering::Relaxed);
    COUNTERS[slot].buckets[bucket].fetch_add(1, Ordering::Relaxed);
}

/// Every verb's counters, verb-name-ordered so a served payload is stable byte
/// for byte across calls that observed the same counts.
pub(crate) fn snapshot() -> std::collections::BTreeMap<&'static str, VerbCount> {
    VERBS
        .iter()
        .enumerate()
        .map(|(slot, verb)| {
            let row = &COUNTERS[slot];
            let buckets = std::array::from_fn(|i| row.buckets[i].load(Ordering::Relaxed));
            (
                *verb,
                VerbCount {
                    count: row.count.load(Ordering::Relaxed),
                    buckets,
                    documents_visited: row.documents_visited.load(Ordering::Relaxed),
                    posting_blocks_decoded: row.posting_blocks_decoded.load(Ordering::Relaxed),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_are_octaves_from_one_microsecond_up() {
        // Sub-microsecond durations share bucket 0: the timer's own resolution.
        assert_eq!(bucket_for(Duration::ZERO), 0);
        assert_eq!(bucket_for(Duration::from_nanos(999)), 0);
        // 1 µs opens bucket 1, and each bucket covers a doubling.
        assert_eq!(bucket_for(Duration::from_micros(1)), 1);
        assert_eq!(bucket_for(Duration::from_micros(2)), 2);
        assert_eq!(bucket_for(Duration::from_micros(3)), 2);
        assert_eq!(bucket_for(Duration::from_micros(4)), 3);
        // A typical read: 140 µs is in the 128–256 µs octave.
        assert_eq!(bucket_for(Duration::from_micros(140)), 8);
        // The top bucket is open-ended: no duration overflows the array.
        let top = BUCKET_COUNT - 1;
        assert_eq!(
            bucket_for(Duration::from_micros(1 << (top - 1))),
            top,
            "the top bucket starts at its own lower bound"
        );
        assert_eq!(bucket_for(Duration::from_secs(3600)), top);
        assert_eq!(bucket_for(Duration::MAX), top);
    }

    #[test]
    fn the_verb_table_is_sorted_and_unique() {
        // `slot_of` is a binary search, so sortedness is a precondition, not a
        // preference; uniqueness means no verb's counts are hidden beside another's.
        for pair in VERBS.windows(2) {
            assert!(
                pair[0] < pair[1],
                "VERBS must be strictly ascending: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn an_unknown_verb_is_not_recorded() {
        // The counters are process-global statics and this binary runs tests in
        // parallel, so "the whole map did not move" is not a claim this test can
        // make — another test's read lands in the same arrays. The contract is
        // narrower and exact: an unknown verb HAS NO SLOT, and recording one is a
        // no-op that cannot panic.
        assert_eq!(slot_of("not_a_verb"), None);
        record("not_a_verb", Duration::from_micros(5));
        assert_eq!(slot_of("not_a_verb"), None);
    }

    /// The tally of the innermost open scope on this thread, for the visit tests.
    fn active() -> Option<(u64, u64)> {
        TALLY
            .with(Cell::get)
            .map(|tally| (tally.documents, tally.posting_blocks))
    }

    #[test]
    fn a_visit_scope_accumulates_and_nests() {
        assert_eq!(active(), None, "no scope is open before the first one");
        visit_scope("search", || {
            note_documents(3);
            note_posting_blocks(1);
            assert_eq!(active(), Some((3, 1)));
            // A nested scope counts for ITS verb and leaves the outer tally alone.
            visit_scope("hubs", || {
                note_documents(5);
                assert_eq!(active(), Some((5, 0)));
            });
            assert_eq!(
                active(),
                Some((3, 1)),
                "the outer tally survived the inner one"
            );
            note_documents(2);
        });
        assert_eq!(active(), None, "the scope closes on the way out");
    }

    #[test]
    fn a_note_outside_a_scope_is_a_no_op() {
        assert_eq!(active(), None);
        note_documents(7);
        note_posting_blocks(3);
        assert_eq!(active(), None, "nothing was opened, so nothing accumulated");
    }

    /// The flush is one atomic add per scope, not per document: the row for the
    /// verb moves by the whole tally when the scope closes, and by nothing before
    /// it. Asserted as a LOWER bound on the delta because these statics are shared
    /// with every other test in the binary; `type_tree` is read by no test here.
    #[test]
    fn a_visit_scope_flushes_into_the_verb_row() {
        let before = snapshot()["type_tree"].documents_visited;
        visit_scope("type_tree", || {
            note_documents(11);
            note_posting_blocks(4);
            assert_eq!(
                snapshot()["type_tree"].documents_visited,
                before,
                "an open scope has not flushed yet"
            );
        });
        let after = snapshot()["type_tree"].clone();
        assert!(
            after.documents_visited >= before + 11,
            "the scope flushed its documents: {before} then {}",
            after.documents_visited
        );
        assert!(after.posting_blocks_decoded >= 4);
    }
}
