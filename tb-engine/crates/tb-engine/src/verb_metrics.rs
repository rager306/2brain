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

/// The one verb counter row: a request count and its latency histogram.
struct VerbCounters {
    count: AtomicU64,
    buckets: [AtomicU64; BUCKET_COUNT],
}

impl VerbCounters {
    const ZERO: Self = VerbCounters {
        count: AtomicU64::new(0),
        buckets: [const { AtomicU64::new(0) }; BUCKET_COUNT],
    };
}

static COUNTERS: [VerbCounters; VERB_COUNT] = [VerbCounters::ZERO; VERB_COUNT];

/// One verb's counters as served: the request count and the histogram buckets,
/// `buckets.sum() == count` at every instant a client reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct VerbCount {
    /// Requests this verb answered since the process started.
    pub count: u64,
    /// The histogram: `buckets[i]` requests in bucket `i` (see
    /// [`BUCKET_COUNT`] for the bounds).
    pub buckets: [u64; BUCKET_COUNT],
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
        let before = snapshot();
        record("not_a_verb", Duration::from_micros(5));
        assert_eq!(
            snapshot(),
            before,
            "an unknown verb has no slot, so it cannot move a counter"
        );
    }
}
