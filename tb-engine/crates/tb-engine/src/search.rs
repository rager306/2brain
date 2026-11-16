//! BM25 lexical search over an in-memory document set.
//!
//! The engine's other lexical search is `grep`, a literal byte scan measured at
//! 132.8 ms over the live vault. This module replaces it for the ranking half: an
//! inverted index over note text that a query answers in microseconds.
//!
//! [`Bm25Index`] is the index itself, a plain value with no knowledge-base
//! coupling. [`SearchIndex`] is the served form the engine holds: the same index
//! plus the document-id to path table, built from the held catalogue in the same
//! pass that fills `instances` and `backlinks`, patched by the incremental delta
//! on a content edit, and rebuilt from the patched catalogue when the path set
//! changes. The `search` read is its only caller.
//!
//! # Documents and fields
//!
//! A document is a caller-assigned `usize` id and `N` text fields. Field 0 is
//! the body and field 1 the description; weights are per field and default to
//! 1.0 and 3.0. A note's description is hand-written and short, so a match
//! there is stronger evidence than the same term repeated in prose — the
//! prototype's finding, and the reason `description` is a field of its own.
//!
//! Ids are dense-ish: the per-document tables grow to the largest id ever
//! added, so a sparse id pays for the gap. They must fit `u32`, which the
//! postings store.
//!
//! # Tokenization
//!
//! Lowercase, then a run of Unicode alphanumerics is a term and every other
//! character separates. There is no stemming and no stopword removal: a stemmer
//! merges `error` and `errors`, which the exact path must keep apart, and a
//! stopword list silently changes what a conjunction requires. `to_lowercase`
//! is the Unicode simple lowercase mapping, not full case folding; for note
//! text the difference does not arise.
//!
//! The split is deliberately coarse: `error->fix`, `error_fix` and
//! `error fix` all tokenize to the same two terms, which is what the
//! punctuation case wants. Underscored identifiers split too, and the exact
//! path still finds them as a conjunction; a caller that needs order needs
//! positions, which this module stores in a delta-encoded posting block (see
//! [`PostingBlock`]) but not yet in a form the ranking reads.
//!
//! # Postings
//!
//! A term's postings are one byte block, not a `Vec` of structs: the document ids
//! ascend as varint deltas, each posting's per-field frequencies follow as
//! varints, and the description's term positions follow those. Scoring decodes
//! the block on the fly, one [`Posting`] at a time, and pays nothing for the
//! positions it does not read.
//!
//! # Ranking
//!
//! BM25 with `k1 = 1.2` and `b = 0.75`, applied per field: each field's term
//! frequency normalises against that field's own average length, so a short
//! description is not penalised for being short. Inverse document frequency is
//! per term over the documents holding it in any field, and field contributions
//! are summed with their weights.
//!
//! A plain OR query over two or three common terms matched 335–343 documents in
//! the prototype, so [`Bm25Index::search`] takes a minimum should match and the
//! caller chooses between OR (`0` or `1`) and AND (the distinct query-term
//! count).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tb_core::{InstanceField, InstanceValue};

use crate::ir::{FileEntry, OrdMap};
use crate::parse::FileParse;

/// BM25 term-frequency saturation.
const K1: f64 = 1.2;

/// BM25 length-normalisation strength.
const B: f64 = 0.75;

/// How much a document's term coverage lifts its BM25 sum.
///
/// BM25 sums per-term contributions, so a document matching one rare term can outrank one matching all
/// four query terms. The eval set in `tools/ranking/` measured the cost of that: a phrase occurring in
/// exactly one note reached rank 1 only 43% of the time. Multiplying by `1 + COVERAGE * matched/total`
/// rewards the document that actually holds the query, without positions and without a second index.
/// The value is chosen against that set, not by taste; see `docs/trajectory.md`, track 3.
const COVERAGE: f64 = 0.6;

/// Default weight of field 0, the note body.
const BODY_WEIGHT: f64 = 1.0;

/// Default weight of field 1, the note description.
const DESCRIPTION_WEIGHT: f64 = 3.0;

/// The field whose term positions a posting carries: the description, which a
/// note writes by hand and which the proximity step ranks on first.
const DESCRIPTION_FIELD: usize = 1;

/// One ranked document returned by [`Bm25Index::search`] or
/// [`Bm25Index::exact`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hit {
    /// The caller's document id.
    pub doc_id: usize,
    /// The sum of the matched terms' BM25 contributions. Higher is better.
    pub score: f64,
    /// How many distinct query terms occur in this document, in any field.
    ///
    /// This is what the minimum should match is compared against; it never
    /// exceeds the number of distinct query terms.
    pub matched_terms: usize,
}

/// One term's occurrence count in one document, per field.
///
/// This is the decoder boundary: no posting is stored in this shape, and
/// [`Postings`] produces one per step through a term's block. The fields are the
/// same ones the ranking reads, so scoring is written against the posting and
/// knows nothing about the encoding. A count saturates at `u16::MAX`: BM25's
/// saturation has flattened long before.
#[derive(Clone, Copy, Debug)]
struct Posting<const N: usize> {
    doc: u32,
    tf: [u16; N],
}

/// One term's postings, encoded as a single byte block.
///
/// Layout: the posting count as a varint, the last posting's document id as a
/// varint, then one entry per posting in ascending document order. The header's
/// document id is the delta base an append starts from; it costs a byte or two
/// per term and lets a build, whose ids ascend, extend a block without decoding
/// it.
///
/// A posting entry is the document id as a varint delta from the previous
/// posting's id (the first from zero), one varint per field holding that field's
/// term frequency, then the description positions: one varint per occurrence of
/// the term in field 1, each a delta from the previous position. A document id
/// therefore costs its gap rather than its value, which is most of what the
/// layout buys, and a phrase walk reads a document's positions in order without
/// a second table.
///
/// Positions cover the description only. The body is where a byte-cost decision
/// has to wait for a consumer: the proximity step ranks on the hand-written
/// description first, and a one-field index stores no positions at all.
///
/// The block is rebuilt whole on an insert below its last id or on a remove —
/// one linear pass over the term's list, the same cost the vector's own
/// `insert`/`remove` paid — and its capacity is what [`Bm25Index::memory_bytes`]
/// counts.
#[derive(Clone, Debug)]
struct PostingBlock<const N: usize> {
    bytes: Vec<u8>,
}

impl<const N: usize> PostingBlock<N> {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// The number of postings, read from the block header.
    fn posting_count(&self) -> usize {
        self.header().0
    }

    /// The header's `(posting count, last document id)`; `(0, 0)` when empty.
    fn header(&self) -> (usize, u32) {
        if self.bytes.is_empty() {
            return (0, 0);
        }
        let mut at = 0;
        let count = read_varint(&self.bytes, &mut at) as usize;
        let last = read_varint(&self.bytes, &mut at);
        (count, last)
    }

    /// Where the posting entries start.
    fn header_len(&self) -> usize {
        if self.bytes.is_empty() {
            return 0;
        }
        let mut at = 0;
        read_varint(&self.bytes, &mut at);
        read_varint(&self.bytes, &mut at);
        at
    }

    /// The postings, decoded one at a time, ascending by document id.
    fn postings(&self) -> Postings<'_, N> {
        Postings {
            walk: PostingWalk::new(&self.bytes),
        }
    }

    /// The description positions of `doc`, ascending, empty when it holds none.
    ///
    /// The proximity step's input. Ranking does not read it yet, which is why
    /// only the tests call it today.
    #[cfg(test)]
    fn positions(&self, doc: u32) -> Vec<u32> {
        let mut decoded = Vec::new();
        let mut walk: PostingWalk<'_, N> = PostingWalk::new(&self.bytes);
        while let Some(posting) = walk.next_posting(Some(&mut decoded)) {
            if posting.doc == doc {
                return decoded;
            }
            if posting.doc > doc {
                break;
            }
        }
        Vec::new()
    }

    /// Insert a posting, keeping the block ascending by document id.
    ///
    /// A document id above the block's last one — the build case, where ids
    /// arrive in order — rewrites the header and appends the entry. Anything
    /// else re-encodes the block in one pass.
    fn insert(&mut self, doc: u32, tf: [u16; N], positions: &[u32]) {
        let (count, last) = self.header();
        if count == 0 || doc > last {
            self.append(doc, tf, positions);
        } else {
            self.rebuild_insert(doc, tf, positions);
        }
    }

    /// Append a posting whose document id is above every id in the block.
    fn append(&mut self, doc: u32, tf: [u16; N], positions: &[u32]) {
        let (count, last) = self.header();
        let header_len = self.header_len();
        let mut header = Vec::with_capacity(8);
        put_varint(&mut header, count as u32 + 1);
        put_varint(&mut header, doc);
        if header.len() == header_len {
            self.bytes[..header_len].copy_from_slice(&header);
        } else {
            let mut bytes = Vec::with_capacity(header.len() + self.bytes.len() - header_len);
            bytes.extend_from_slice(&header);
            bytes.extend_from_slice(&self.bytes[header_len..]);
            self.bytes = bytes;
        }
        self.bytes
            .reserve_exact(posting_len(last, doc, &tf, positions));
        write_posting(&mut self.bytes, last, doc, &tf, positions);
    }

    /// Insert a posting in the middle of the block, re-encoding it.
    fn rebuild_insert(&mut self, doc: u32, tf: [u16; N], positions: &[u32]) {
        let (count, last) = self.header();
        let mut bytes = Vec::with_capacity(self.bytes.len() + 8);
        put_varint(&mut bytes, count as u32 + 1);
        put_varint(&mut bytes, last.max(doc));
        let mut prev = 0;
        let mut placed = false;
        let mut decoded = Vec::new();
        let mut walk: PostingWalk<'_, N> = PostingWalk::new(&self.bytes);
        while let Some(posting) = walk.next_posting(Some(&mut decoded)) {
            debug_assert!(
                posting.doc != doc,
                "a document is removed before it is added"
            );
            if !placed && posting.doc > doc {
                write_posting(&mut bytes, prev, doc, &tf, positions);
                prev = doc;
                placed = true;
            }
            write_posting(&mut bytes, prev, posting.doc, &posting.tf, &decoded);
            prev = posting.doc;
        }
        if !placed {
            write_posting(&mut bytes, prev, doc, &tf, positions);
        }
        bytes.shrink_to_fit();
        self.bytes = bytes;
    }

    /// Remove a posting, if the document has one.
    fn remove(&mut self, doc: u32) {
        let (count, _) = self.header();
        if count == 0 {
            return;
        }
        let mut body = Vec::new();
        let mut prev = 0;
        let mut last = 0;
        let mut kept = 0u32;
        let mut decoded = Vec::new();
        let mut walk: PostingWalk<'_, N> = PostingWalk::new(&self.bytes);
        while let Some(posting) = walk.next_posting(Some(&mut decoded)) {
            if posting.doc == doc {
                continue;
            }
            write_posting(&mut body, prev, posting.doc, &posting.tf, &decoded);
            prev = posting.doc;
            last = posting.doc;
            kept += 1;
        }
        if kept == 0 {
            // The term's last document left: drop the block's allocation too, so
            // an emptied term costs its map entry and nothing else.
            self.bytes = Vec::new();
            return;
        }
        let mut bytes = Vec::with_capacity(body.len() + 8);
        put_varint(&mut bytes, kept);
        put_varint(&mut bytes, last);
        bytes.extend_from_slice(&body);
        bytes.shrink_to_fit();
        self.bytes = bytes;
    }

    /// The bytes this block's allocation holds.
    fn memory_bytes(&self) -> usize {
        self.bytes.capacity()
    }
}

/// The decoded postings of one term, as scoring walks them.
struct Postings<'a, const N: usize> {
    walk: PostingWalk<'a, N>,
}

impl<const N: usize> Iterator for Postings<'_, N> {
    type Item = Posting<N>;

    fn next(&mut self) -> Option<Posting<N>> {
        self.walk.next_posting(None)
    }
}

/// A cursor over one encoded posting block.
struct PostingWalk<'a, const N: usize> {
    bytes: &'a [u8],
    at: usize,
    remaining: usize,
    doc: u32,
}

impl<'a, const N: usize> PostingWalk<'a, N> {
    fn new(bytes: &'a [u8]) -> Self {
        if bytes.is_empty() {
            return Self {
                bytes,
                at: 0,
                remaining: 0,
                doc: 0,
            };
        }
        let mut at = 0;
        let count = read_varint(bytes, &mut at) as usize;
        let _last = read_varint(bytes, &mut at);
        Self {
            bytes,
            at,
            remaining: count,
            doc: 0,
        }
    }

    /// The next posting, or `None` at the end of the block.
    ///
    /// The description positions are collected into `positions` when a buffer is
    /// given and skipped otherwise: scoring reads the frequencies, and a rewrite
    /// has to carry the positions through unchanged.
    fn next_posting(&mut self, positions: Option<&mut Vec<u32>>) -> Option<Posting<N>> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        self.doc += read_varint(self.bytes, &mut self.at);
        let tf: [u16; N] = std::array::from_fn(|_| read_varint(self.bytes, &mut self.at) as u16);
        let occurrences = position_count(&tf) as usize;
        match positions {
            Some(decoded) => {
                decoded.clear();
                let mut position = 0;
                for _ in 0..occurrences {
                    position += read_varint(self.bytes, &mut self.at);
                    decoded.push(position);
                }
            }
            None => {
                for _ in 0..occurrences {
                    skip_varint(self.bytes, &mut self.at);
                }
            }
        }
        Some(Posting { doc: self.doc, tf })
    }
}

/// How many description positions a posting carries: one per occurrence of the
/// term in field 1, zero when the index has no description field.
fn position_count<const N: usize>(tf: &[u16; N]) -> u32 {
    u32::from(tf.get(DESCRIPTION_FIELD).copied().unwrap_or(0))
}

/// Append `value` as a LEB128 varint: seven bits per byte, low group first, a
/// continuation bit on every byte but the last.
fn put_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Advance `*at` past the varint there, without decoding it.
fn skip_varint(bytes: &[u8], at: &mut usize) {
    while bytes[*at] & 0x80 != 0 {
        *at += 1;
    }
    *at += 1;
}

/// The varint at `*at`, advancing `*at` past it.
///
/// The block is written only by this module, so a read past the end is a bug in
/// the writer, not input to be tolerated.
fn read_varint(bytes: &[u8], at: &mut usize) -> u32 {
    let mut value = 0u32;
    let mut shift = 0;
    loop {
        let byte = bytes[*at];
        *at += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// How many bytes `value` takes as a varint.
fn varint_len(mut value: u32) -> usize {
    let mut len = 1;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

/// Write one posting entry, given the document id of the entry before it.
fn write_posting<const N: usize>(
    out: &mut Vec<u8>,
    prev_doc: u32,
    doc: u32,
    tf: &[u16; N],
    positions: &[u32],
) {
    put_varint(out, doc - prev_doc);
    for &frequency in tf {
        put_varint(out, u32::from(frequency));
    }
    // A frequency saturates, so a term repeated past that limit leaves its later
    // positions out rather than writing more than the decoder will read.
    let mut prev_position = 0;
    for &position in positions.iter().take(position_count(tf) as usize) {
        put_varint(out, position - prev_position);
        prev_position = position;
    }
}

/// How many bytes one posting entry takes, given the entry before it.
fn posting_len<const N: usize>(prev_doc: u32, doc: u32, tf: &[u16; N], positions: &[u32]) -> usize {
    let mut len = varint_len(doc - prev_doc);
    for &frequency in tf {
        len += varint_len(u32::from(frequency));
    }
    let mut prev_position = 0;
    for &position in positions.iter().take(position_count(tf) as usize) {
        len += varint_len(position - prev_position);
        prev_position = position;
    }
    len
}

/// One document's field lengths, plus whether it is currently in the index.
///
/// The slot outlives a removal so that re-adding the same id reuses it; a
/// dead slot's lengths are zero and contribute to no average.
#[derive(Clone, Copy, Debug)]
struct DocSlot<const N: usize> {
    lens: [u32; N],
    alive: bool,
}

impl<const N: usize> DocSlot<N> {
    fn empty() -> Self {
        Self {
            lens: [0; N],
            alive: false,
        }
    }
}

/// Per-document running score while one query gathers its candidates.
#[derive(Default)]
struct Accumulator {
    score: f64,
    matched: usize,
}

/// A BM25 inverted index over `N` text fields per document.
///
/// The field count is a compile-time constant, so the engine's two-field
/// `body`/`description` index and a one-field test index share one
/// implementation without boxing.
///
/// Build with [`Bm25Index::from_docs_with`] (explicit weights), or start empty
/// with [`Bm25Index::new`] and grow it with [`Bm25Index::add`]. An index is a
/// plain value: not shared and not locked. `Clone` is a deep copy of every
/// table, so the held state reaches it through an `Arc` and copies it once per
/// patch rather than per read.
#[derive(Debug, Clone)]
pub(crate) struct Bm25Index<const N: usize> {
    weights: [f64; N],
    /// Term text to term id. The only owner of the term text; the postings and
    /// the per-document term lists refer to it by id.
    term_ids: HashMap<String, u32>,
    /// Term id to that term's postings, one encoded block each, ordered by
    /// document id. One slot per id ever assigned, so ids stay stable when a
    /// term's list empties.
    postings: Vec<PostingBlock<N>>,
    /// Per-document field lengths, indexed by document id.
    docs: Vec<DocSlot<N>>,
    /// Term ids of each live document, in no particular order, so removing a
    /// document touches only that document's posting lists.
    doc_terms: Vec<Vec<u32>>,
    /// Field lengths summed over live documents, for the per-field average.
    total_len: [u64; N],
    live_docs: usize,
}

impl<const N: usize> Bm25Index<N> {
    /// An empty index with the given per-field weights.
    ///
    /// [`default_weights`] supplies the body/description defaults; this
    /// constructor always takes the weights explicitly.
    pub(crate) fn new(weights: [f64; N]) -> Self {
        Self {
            weights,
            term_ids: HashMap::new(),
            postings: Vec::new(),
            docs: Vec::new(),
            doc_terms: Vec::new(),
            total_len: [0; N],
            live_docs: 0,
        }
    }

    /// Build from `(doc_id, fields)` pairs with explicit per-field weights.
    pub(crate) fn from_docs_with<'a>(
        weights: [f64; N],
        docs: impl IntoIterator<Item = (usize, [&'a str; N])>,
    ) -> Self {
        let mut index = Self::new(weights);
        for (doc_id, fields) in docs {
            index.add(doc_id, fields);
        }
        index
    }

    /// Number of live documents, for the module's tests and the size budget.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.live_docs
    }

    /// Add a document, replacing it if the id is already live.
    ///
    /// Cost is one pass over the document's terms, an append to each distinct
    /// term's block (ids ascend during a build, so nothing is re-encoded), and
    /// one allocation for the document's term-id list. A brand-new term also
    /// allocates its block and its map entry.
    ///
    /// # Panics
    ///
    /// If `doc_id` does not fit `u32`, the width of a posting's document field.
    pub(crate) fn add(&mut self, doc_id: usize, fields: [&str; N]) {
        assert!(
            doc_id <= u32::MAX as usize,
            "document id {doc_id} does not fit the postings' u32"
        );
        self.remove(doc_id);
        if doc_id >= self.docs.len() {
            self.docs.resize(doc_id + 1, DocSlot::empty());
            self.doc_terms.resize(doc_id + 1, Vec::new());
        }

        // One pass over the fields, counting per-field term frequencies into a
        // scratch map, and remembering where in the description each term
        // occurred: the positions the ranking's next step reads.
        let mut counts: HashMap<u32, [u32; N]> = HashMap::new();
        let mut positions: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut lens = [0u32; N];
        for (field, text) in fields.iter().enumerate() {
            for (at, term) in tokenize(text).into_iter().enumerate() {
                let term_id = self.intern(&term);
                lens[field] += 1;
                counts.entry(term_id).or_insert([0; N])[field] += 1;
                if field == DESCRIPTION_FIELD {
                    positions.entry(term_id).or_default().push(at as u32);
                }
            }
        }

        let doc = doc_id as u32;
        for (term_id, tf) in &counts {
            let tf = std::array::from_fn(|field| u16::try_from(tf[field]).unwrap_or(u16::MAX));
            let term_positions = positions.get(term_id).map_or(&[][..], Vec::as_slice);
            self.postings[*term_id as usize].insert(doc, tf, term_positions);
        }
        self.doc_terms[doc_id] = counts.into_keys().collect();
        self.docs[doc_id] = DocSlot { lens, alive: true };
        for (field, length) in lens.iter().enumerate() {
            self.total_len[field] += u64::from(*length);
        }
        self.live_docs += 1;
    }

    /// Remove a document, if it is live.
    ///
    /// Cost is one re-encode per distinct term of the document — a linear pass
    /// over each posting list, proportional to how common the document's terms
    /// are, not to the whole index. The vocabulary does not shrink: a term whose
    /// last document leaves keeps its map entry, and its block drops to no
    /// allocation, which is what makes re-adding an id cheap.
    pub(crate) fn remove(&mut self, doc_id: usize) {
        let Some(slot) = self.docs.get(doc_id) else {
            return;
        };
        if !slot.alive {
            return;
        }
        let lens = slot.lens;
        self.docs[doc_id].alive = false;
        self.docs[doc_id].lens = [0; N];

        let doc = doc_id as u32;
        for term_id in std::mem::take(&mut self.doc_terms[doc_id]) {
            self.postings[term_id as usize].remove(doc);
        }
        for (field, length) in lens.iter().enumerate() {
            self.total_len[field] -= u64::from(*length);
        }
        self.live_docs -= 1;
    }

    /// Rank the documents matching `query`, best first.
    ///
    /// A document is a candidate when it holds at least `min_should_match`
    /// distinct query terms, so `0` is an OR and the distinct term count is an
    /// AND. The bound is clamped to the number of distinct query terms, so
    /// `usize::MAX` also means AND. A query term the index has never seen
    /// counts as a distinct term that no document matches: an AND query that
    /// includes it returns nothing, which is what AND means.
    ///
    /// Ties in score are broken by ascending [`Hit::doc_id`], so the order is
    /// total and identical across builds of the same corpus. `limit == 0`
    /// returns nothing.
    pub(crate) fn search(&self, query: &str, min_should_match: usize, limit: usize) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        let (distinct_terms, term_ids) = self.query_terms(query);
        if term_ids.is_empty() {
            return Vec::new();
        }
        let required = min_should_match.min(distinct_terms);
        let average = self.average_lengths();

        // Accumulate each candidate's contributions as the term lists are
        // walked: one posting per (term, document), so `matched` counts
        // distinct terms without a second pass. The upper bound on distinct
        // candidates is the sum of the list lengths, which keeps the map from
        // rehashing as it fills.
        let bound: usize = term_ids
            .iter()
            .map(|&term_id| self.postings[term_id as usize].posting_count())
            .sum();
        let mut accumulator: HashMap<u32, Accumulator> = HashMap::with_capacity(bound);
        for &term_id in &term_ids {
            let postings = &self.postings[term_id as usize];
            let idf = self.idf(postings.posting_count());
            let weight: [f64; N] = std::array::from_fn(|field| self.weights[field] * idf);
            for posting in postings.postings() {
                let slot = &self.docs[posting.doc as usize];
                let mut score = 0.0;
                for (field, &tf) in posting.tf.iter().enumerate() {
                    if tf == 0 || weight[field] == 0.0 {
                        continue;
                    }
                    score += weight[field]
                        * tf_component(u32::from(tf), slot.lens[field], average[field]);
                }
                let entry = accumulator.entry(posting.doc).or_default();
                entry.score += score;
                entry.matched += 1;
            }
        }

        let mut hits: Vec<Hit> = accumulator
            .into_iter()
            .filter(|(_, accumulator)| accumulator.matched >= required)
            .map(|(doc, accumulator)| Hit {
                doc_id: doc as usize,
                score: accumulator.score
                    * (1.0
                        + COVERAGE * (accumulator.matched as f64) / (distinct_terms.max(1) as f64)),
                matched_terms: accumulator.matched,
            })
            .collect();
        sort_hits(&mut hits);
        hits.truncate(limit);
        hits
    }

    /// The documents holding every term of `needle`, best first.
    ///
    /// This is the phrase and punctuation path: `error->fix` tokenizes to
    /// `error` and `fix`, and the answer is the intersection of their posting
    /// lists, so no document is scanned and no punctuation is compared. If any
    /// term is absent from the index the conjunction is empty. Candidates are
    /// scored with the same BM25 sum [`Bm25Index::search`] uses and returned in
    /// its order.
    ///
    /// It is a term conjunction, not an ordered phrase: `fix` before `error`
    /// matches too. Order would need a position per posting on both fields, and
    /// the blocks carry them for the description only, so the engine's phrase
    /// queries stay punctuation-shaped rather than order-sensitive until the
    /// proximity step reads them.
    pub(crate) fn exact(&self, needle: &str, limit: usize) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        // A term the index never saw cannot be intersected, and a conjunction
        // that requires it has no answer.
        let (distinct_terms, term_ids) = self.query_terms(needle);
        if term_ids.is_empty() || term_ids.len() != distinct_terms {
            return Vec::new();
        }

        // Each term's posting list with its idf, so the intersection and the
        // scoring share one lookup. Decoding a list once into a local vector is
        // what buys the binary search back: a conjunctive needle is short, so
        // the transient lists are a few kilobytes.
        let terms: Vec<(Vec<Posting<N>>, f64)> = term_ids
            .iter()
            .map(|&term_id| {
                let postings = &self.postings[term_id as usize];
                (
                    postings.postings().collect(),
                    self.idf(postings.posting_count()),
                )
            })
            .collect();

        // The shortest list bounds the answer and is the cheapest probe: every
        // document in the intersection holds every term, hence every document
        // of the probe list.
        let probe = terms
            .iter()
            .enumerate()
            .min_by_key(|(_, (postings, _))| postings.len())
            .map(|(at, _)| at)
            .expect("a non-empty term list");
        let mut candidates: Vec<u32> = terms[probe].0.iter().map(|posting| posting.doc).collect();
        for (at, (postings, _)) in terms.iter().enumerate() {
            if at == probe {
                continue;
            }
            candidates.retain(|doc| {
                postings
                    .binary_search_by_key(doc, |posting| posting.doc)
                    .is_ok()
            });
            if candidates.is_empty() {
                return Vec::new();
            }
        }

        let average = self.average_lengths();
        let mut hits = Vec::with_capacity(candidates.len());
        for doc in candidates {
            let slot = &self.docs[doc as usize];
            let mut score = 0.0;
            for (postings, idf) in &terms {
                let at = postings
                    .binary_search_by_key(&doc, |posting| posting.doc)
                    .expect("an intersecting document is in every term's postings");
                let posting = postings[at];
                for (field, &tf) in posting.tf.iter().enumerate() {
                    if tf == 0 || self.weights[field] == 0.0 {
                        continue;
                    }
                    score += self.weights[field]
                        * idf
                        * tf_component(u32::from(tf), slot.lens[field], average[field]);
                }
            }
            hits.push(Hit {
                doc_id: doc as usize,
                score,
                matched_terms: terms.len(),
            });
        }
        sort_hits(&mut hits);
        hits.truncate(limit);
        hits
    }

    /// The bytes this index's own allocations hold, for the size budgets.
    ///
    /// An estimate: it counts capacity rather than length for every vector and
    /// string and one control byte per term-map slot, and it excludes allocator
    /// bookkeeping and the caller's own document table.
    #[cfg(test)]
    pub(crate) fn memory_bytes(&self) -> usize {
        let mut bytes = std::mem::size_of::<Self>();
        bytes += self.term_ids.capacity()
            * (std::mem::size_of::<String>() + std::mem::size_of::<u32>() + 1);
        for term in self.term_ids.keys() {
            bytes += term.capacity();
        }
        bytes += self.postings.capacity() * std::mem::size_of::<PostingBlock<N>>();
        for postings in &self.postings {
            bytes += postings.memory_bytes();
        }
        bytes += self.docs.capacity() * std::mem::size_of::<DocSlot<N>>();
        bytes += self.doc_terms.capacity() * std::mem::size_of::<Vec<u32>>();
        for terms in &self.doc_terms {
            bytes += terms.capacity() * std::mem::size_of::<u32>();
        }
        bytes
    }

    /// The term id for `term`, assigning one if the index has not seen it.
    fn intern(&mut self, term: &str) -> u32 {
        if let Some(&term_id) = self.term_ids.get(term) {
            return term_id;
        }
        let term_id = u32::try_from(self.postings.len()).expect("term id space exhausted");
        self.term_ids.insert(term.to_owned(), term_id);
        self.postings.push(PostingBlock::new());
        term_id
    }

    /// The query's distinct terms: how many there are, and the ids of those the
    /// index knows, in first-appearance order.
    ///
    /// The count includes terms the index never saw, so a caller asking for an
    /// AND still requires them.
    fn query_terms(&self, query: &str) -> (usize, Vec<u32>) {
        let tokens = tokenize(query);
        let mut distinct: Vec<&str> = Vec::with_capacity(tokens.len());
        for token in &tokens {
            if !distinct.contains(&token.as_str()) {
                distinct.push(token);
            }
        }
        let ids = distinct
            .iter()
            .filter_map(|term| self.term_ids.get(*term).copied())
            .collect();
        (distinct.len(), ids)
    }

    /// Each field's average length per live document, for length normalisation.
    fn average_lengths(&self) -> [f64; N] {
        let documents = self.live_docs.max(1) as f64;
        std::array::from_fn(|field| self.total_len[field] as f64 / documents)
    }

    /// Inverse document frequency of a term seen in `document_frequency`
    /// documents. Lucene's positive form, so a term in most documents still
    /// contributes a small positive amount.
    fn idf(&self, document_frequency: usize) -> f64 {
        let documents = self.live_docs as f64;
        let frequency = document_frequency as f64;
        (1.0 + (documents - frequency + 0.5) / (frequency + 0.5)).ln()
    }

    /// The description positions of `term` in `doc_id`, ascending, empty when the
    /// term is absent there. The proximity step's input; ranking does not read it
    /// yet, so only the tests do.
    #[cfg(test)]
    pub(crate) fn term_positions(&self, term: &str, doc_id: usize) -> Vec<u32> {
        let Some(&term_id) = self.term_ids.get(term) else {
            return Vec::new();
        };
        self.postings[term_id as usize].positions(doc_id as u32)
    }

    /// One document's parity facts: its per-field token counts and whether the
    /// slot is live. Read by [`SearchIndex::parity_facts`].
    #[cfg(test)]
    pub(crate) fn doc_fact(&self, doc_id: usize) -> ([u32; N], bool) {
        match self.docs.get(doc_id) {
            Some(slot) => (slot.lens, slot.alive),
            None => ([0; N], false),
        }
    }
}

/// The servable search index: the BM25 index plus the document-id to path table.
///
/// The engine holds one in [`crate::KnowledgeBase::search`]. Document ids are
/// assigned in the catalogue's path order at build time, which buys three things
/// at once: a rebuild over the same content produces the same ids, a hit's path is
/// one indexed lookup, and a score tie breaks by path (through
/// [`Bm25Index::search`]'s ascending-doc-id rule) rather than arbitrarily.
///
/// The two fields are the note body (weight 1.0) and its frontmatter
/// `description:` (weight 3.0). The weighting is the prototype's finding: a
/// description is hand-written and short, so a match there is stronger evidence
/// than the same term repeated in prose.
#[derive(Debug, Clone)]
pub struct SearchIndex {
    index: Bm25Index<2>,
    /// Document id to path, in the path order the ids were assigned in. Sorted,
    /// and an edit never renumbers, so a path's id is a binary search away.
    paths: Vec<PathBuf>,
}

impl SearchIndex {
    /// Build over a catalogue, in path order.
    ///
    /// One pass over the catalogue in its own order: an entry with no body
    /// contributes no document, and every other entry takes the next id.
    pub(crate) fn build(catalog: &OrdMap<PathBuf, FileEntry>) -> Self {
        let mut paths = Vec::new();
        let mut docs: Vec<(usize, [&str; 2])> = Vec::new();
        for (path, entry) in catalog {
            let Some(fields) = indexed_fields(entry.parse.as_ref()) else {
                continue;
            };
            docs.push((paths.len(), fields));
            paths.push(path.clone());
        }
        Self {
            index: Bm25Index::from_docs_with(default_weights(), docs),
            paths,
        }
    }

    /// Re-index one path from its new parse, at the id it already holds.
    ///
    /// The content-edit case of the incremental patch: the path keeps its rank,
    /// so only its own postings move. An add or a delete renumbers every later
    /// path, so the caller rebuilds instead (see `apply_recompute`).
    ///
    /// # Panics
    ///
    /// If `path` is not indexed. A path enters the index through a build, so a
    /// caller reaching a missing one skipped the renumbering rebuild.
    pub(crate) fn update(&mut self, path: &Path, parse: &FileParse) {
        let id = self
            .id_of(path)
            .expect("an updated path is indexed; an add rebuilds the index");
        self.index.remove(id);
        if let Some(fields) = indexed_fields(parse) {
            self.index.add(id, fields);
        }
    }

    /// The ranked documents matching `query`, best first, unfiltered and unpaged.
    ///
    /// `exact` takes the conjunction path ([`Bm25Index::exact`]); otherwise
    /// `min_should_match` decides how many distinct query terms a document must
    /// hold. The whole answer comes back because scope (`repo` / `scope`) is a
    /// query-side filter this index knows nothing about: the caller filters, then
    /// totals and pages over what it kept.
    pub(crate) fn query(
        &self,
        query: &str,
        exact: bool,
        min_should_match: usize,
    ) -> Vec<SearchHit<'_>> {
        let hits = if exact {
            self.index.exact(query, usize::MAX)
        } else {
            self.index.search(query, min_should_match, usize::MAX)
        };
        hits.into_iter()
            .map(|hit| SearchHit {
                path: self.paths[hit.doc_id].as_path(),
                score: hit.score,
                matched_terms: hit.matched_terms,
            })
            .collect()
    }

    /// The id of an indexed path, `None` when the index holds no document for it.
    fn id_of(&self, path: &Path) -> Option<usize> {
        self.paths
            .binary_search_by(|probe| probe.as_path().cmp(path))
            .ok()
    }

    /// One line per document, for the identical-rebuild parity oracle: the id, its
    /// path, whether the slot is live, and its per-field token counts. A full
    /// build and an incremental patch must agree on all of them, which is what
    /// makes the ranking, including its ties, identical.
    #[cfg(test)]
    pub(crate) fn parity_facts(&self) -> Vec<String> {
        (0..self.paths.len())
            .map(|id| {
                let (lens, live) = self.index.doc_fact(id);
                format!("search {id}|{}|{live}|{lens:?}", self.paths[id].display())
            })
            .collect()
    }
}

/// One ranked document, with the path its id stands for.
pub(crate) struct SearchHit<'a> {
    pub path: &'a Path,
    pub score: f64,
    pub matched_terms: usize,
}

/// Whether a parse contributes a document to the index. See [`indexed_fields`].
pub(crate) fn indexed(parse: &FileParse) -> bool {
    indexed_fields(parse).is_some()
}

/// The index text of one parse: `[body, description]`, `None` when the parse
/// carries no body.
///
/// A parse with no body is skipped: a type-def, an unread asset, a pure-YAML
/// instance. The read answers "which note says this", so a file whose only text
/// is a frontmatter declaration is not an answer.
fn indexed_fields(parse: &FileParse) -> Option<[&str; 2]> {
    match parse {
        FileParse::Instance { body, instance, .. } if !body.is_empty() => {
            let description = instance
                .as_ref()
                .map(|inst| description_of(&inst.fields))
                .unwrap_or("");
            Some([body, description])
        }
        FileParse::Note { body, fields, .. } if !body.is_empty() => {
            Some([body, description_of(fields)])
        }
        _ => None,
    }
}

/// The `description:` frontmatter value, empty when absent or not a string.
fn description_of(fields: &[InstanceField]) -> &str {
    fields
        .iter()
        .find_map(|field| match (&*field.key, &field.value) {
            ("description", InstanceValue::String(text)) => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or("")
}

/// The default minimum should match for a query: how many distinct terms a
/// document must hold to be a candidate.
///
/// One or two distinct terms require all of them; three or more require two. A
/// plain OR over two or three common terms matched 335–343 documents in the
/// prototype — an OR answers a third of the vault — while requiring every term of
/// a long natural-language query answers nothing. Two is the middle: one stray
/// word cannot flood the page, and a caller need not phrase a query like a search
/// engine.
pub(crate) fn default_min_should_match(query: &str) -> usize {
    let mut terms = tokenize(query);
    terms.sort();
    terms.dedup();
    match terms.len() {
        count if count <= 2 => count,
        _ => 2,
    }
}

/// BM25's saturated, length-normalised term frequency for one field.
fn tf_component(tf: u32, field_length: u32, average_length: f64) -> f64 {
    let tf = f64::from(tf);
    // A field with no text at all has an average of zero; nothing in that field
    // can match, and the guard keeps the ratio finite for the others.
    let average = if average_length > 0.0 {
        average_length
    } else {
        1.0
    };
    let normalisation = 1.0 - B + B * f64::from(field_length) / average;
    tf * (K1 + 1.0) / (tf + K1 * normalisation)
}

/// Rank descending by score, then ascending by document id.
fn sort_hits(hits: &mut [Hit]) {
    hits.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then(left.doc_id.cmp(&right.doc_id))
    });
}

/// The default weights: body 1.0, description 3.0, anything else 1.0.
fn default_weights<const N: usize>() -> [f64; N] {
    std::array::from_fn(|field| match field {
        0 => BODY_WEIGHT,
        1 => DESCRIPTION_WEIGHT,
        _ => BODY_WEIGHT,
    })
}

/// The index's terms of `text`, in order.
///
/// Lowercase, then a maximal run of Unicode alphanumerics is one term and every
/// other character separates. No stemming, no stopwords: see the module docs.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut start: Option<usize> = None;
    for (at, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            start.get_or_insert(at);
        } else if let Some(from) = start.take() {
            terms.push(text[from..at].to_lowercase());
        }
    }
    if let Some(from) = start {
        terms.push(text[from..].to_lowercase());
    }
    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test that bounds index size states this ceiling, in bytes per
    /// document, for a generated two-field corpus of ~400-term bodies.
    ///
    /// Measured at 2.6–2.7 KiB per document on 2026-11-16 in a debug build: 4 703
    /// bytes before the postings became delta-encoded varint blocks and 2 624
    /// after, for the 1 000-document corpus `measure_build_and_query_latency`
    /// prints, with the 500-document corpus below at 2 711. The assertion is a
    /// tripwire against a structure change that doubles the index, not a
    /// performance target; the headroom is what a proximity pass may spend.
    const MEMORY_BUDGET_PER_DOCUMENT: usize = 6_144;

    /// Build with explicit weights from an owned two-field corpus.
    fn index_of(weights: [f64; 2], docs: &[(usize, [String; 2])]) -> Bm25Index<2> {
        Bm25Index::from_docs_with(
            weights,
            docs.iter()
                .map(|(doc_id, fields)| (*doc_id, [fields[0].as_str(), fields[1].as_str()])),
        )
    }

    /// Hit ids, sorted, so a test asserts membership rather than tie order.
    fn sorted_ids(hits: Vec<Hit>) -> Vec<usize> {
        let mut ids: Vec<usize> = hits.into_iter().map(|hit| hit.doc_id).collect();
        ids.sort_unstable();
        ids
    }

    /// Deterministic xorshift64, so a generated corpus is identical on every
    /// run and every machine and the measured latencies stay comparable.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    /// Words at the head of the generated vocabulary that carry half of every
    /// document.
    const COMMON_WORDS: usize = 50;

    /// One generated term: half the draws come from the small common set, half
    /// are uniform over the rest of the vocabulary.
    ///
    /// Prose repeats a few function words and mentions most others rarely; a
    /// uniform draw would make almost every token new, inflating both the
    /// posting count and each document's term list.
    fn pick_term(rng: &mut Rng, vocabulary: usize) -> usize {
        if rng.below(100) < 50 {
            rng.below(COMMON_WORDS)
        } else {
            COMMON_WORDS + rng.below(vocabulary - COMMON_WORDS)
        }
    }

    /// A two-field corpus: bodies of roughly `body_tokens` terms, descriptions
    /// of 20.
    fn generated_corpus(
        documents: usize,
        body_tokens: usize,
        vocabulary: usize,
    ) -> Vec<(usize, [String; 2])> {
        assert!(
            vocabulary > COMMON_WORDS,
            "a generated vocabulary needs room for the common words"
        );
        let words: Vec<String> = (0..vocabulary).map(|id| format!("w{id:05}")).collect();
        let mut rng = Rng(0x5eed_1234_abcd_ef01);
        (0..documents)
            .map(|doc_id| {
                let mut body = String::with_capacity(body_tokens * 8);
                let length = body_tokens * (8 + rng.below(5)) / 10;
                for _ in 0..length {
                    let pick = pick_term(&mut rng, vocabulary);
                    body.push_str(&words[pick]);
                    body.push(' ');
                }
                let mut description = String::with_capacity(20 * 8);
                for _ in 0..20 {
                    let pick = pick_term(&mut rng, vocabulary);
                    description.push_str(&words[pick]);
                    description.push(' ');
                }
                (doc_id, [body, description])
            })
            .collect()
    }

    /// Min, p50, p95, max of a sample in microseconds.
    fn percentiles(mut samples: Vec<f64>) -> (f64, f64, f64, f64) {
        samples.sort_by(f64::total_cmp);
        let last = samples.len() - 1;
        let at = |quantile: f64| samples[((last as f64) * quantile).round() as usize];
        let p50 = at(0.5);
        let p95 = at(0.95);
        (samples[0], p50, p95, samples[last])
    }

    /// A corpus document's term frequencies and field lengths, straight from the
    /// text: what the encoded blocks are decoded against.
    struct ReferenceDoc {
        tf: [HashMap<String, u32>; 2],
        lens: [u32; 2],
    }

    fn reference_docs(corpus: &[(usize, [String; 2])]) -> Vec<ReferenceDoc> {
        corpus
            .iter()
            .map(|(_, fields)| {
                let mut tf: [HashMap<String, u32>; 2] = Default::default();
                let mut lens = [0u32; 2];
                for (field, text) in fields.iter().enumerate() {
                    for term in tokenize(text) {
                        *tf[field].entry(term).or_insert(0) += 1;
                        lens[field] += 1;
                    }
                }
                ReferenceDoc { tf, lens }
            })
            .collect()
    }

    /// The distinct query terms, in first-appearance order: the order the index
    /// sums its contributions in, which floating-point addition is sensitive to.
    fn reference_terms(query: &str) -> Vec<String> {
        let mut distinct: Vec<String> = Vec::new();
        for term in tokenize(query) {
            if !distinct.contains(&term) {
                distinct.push(term);
            }
        }
        distinct
    }

    /// How many documents hold each term in either field, in `terms` order.
    fn reference_frequencies(docs: &[ReferenceDoc], terms: &[String]) -> Vec<usize> {
        terms
            .iter()
            .map(|term| {
                docs.iter()
                    .filter(|doc| doc.tf[0].contains_key(term) || doc.tf[1].contains_key(term))
                    .count()
            })
            .collect()
    }

    /// The weights, per-field averages and per-term idf both reference paths use.
    fn reference_setup(
        docs: &[ReferenceDoc],
        frequency: &[usize],
    ) -> ([f64; 2], [f64; 2], Vec<f64>) {
        let documents = docs.len() as f64;
        let average: [f64; 2] = std::array::from_fn(|field| {
            let total: u64 = docs.iter().map(|doc| u64::from(doc.lens[field])).sum();
            total as f64 / documents
        });
        let idf = frequency
            .iter()
            .map(|&frequency| {
                let frequency = frequency as f64;
                (1.0 + (documents - frequency + 0.5) / (frequency + 0.5)).ln()
            })
            .collect();
        (default_weights::<2>(), average, idf)
    }

    /// Rank descending by score, then ascending by document id.
    fn sort_reference(scored: &mut [(usize, f64)]) {
        scored
            .sort_unstable_by(|left, right| right.1.total_cmp(&left.1).then(left.0.cmp(&right.0)));
    }

    /// BM25 over the corpus text: the same formula in the same accumulation
    /// order, computed without the index, so a document id, a field frequency or
    /// a length the encoding loses changes the answer.
    fn reference_search(
        docs: &[ReferenceDoc],
        query: &str,
        min_should_match: usize,
    ) -> Vec<(usize, f64)> {
        let terms = reference_terms(query);
        let frequency = reference_frequencies(docs, &terms);
        let (weights, average, idf) = reference_setup(docs, &frequency);
        let mut scored = Vec::new();
        for (doc_id, doc) in docs.iter().enumerate() {
            let mut score = 0.0;
            let mut matched = 0;
            for (term, (&term_idf, &frequency)) in terms.iter().zip(idf.iter().zip(&frequency)) {
                // A term the index never saw is not in its posting table, so it
                // contributes nothing; it still counts towards the requirement.
                if frequency == 0 {
                    continue;
                }
                // A posting exists exactly for the documents holding the term in
                // some field, and that is what `matched` counts.
                if doc.tf[0].get(term).is_none() && doc.tf[1].get(term).is_none() {
                    continue;
                }
                matched += 1;
                // The index sums one term's field contributions into a local
                // score and adds that to the document's, so the rounding here
                // has to group the fields the same way.
                let mut term_score = 0.0;
                for field in 0..2 {
                    let tf = doc.tf[field].get(term).copied().unwrap_or(0);
                    if tf == 0 || weights[field] == 0.0 {
                        continue;
                    }
                    term_score += weights[field]
                        * term_idf
                        * tf_component(tf, doc.lens[field], average[field]);
                }
                score += term_score;
            }
            if matched < min_should_match.min(terms.len()) {
                continue;
            }
            scored.push((
                doc_id,
                score * (1.0 + COVERAGE * matched as f64 / (terms.len().max(1) as f64)),
            ));
        }
        sort_reference(&mut scored);
        scored
    }

    /// The conjunction path over the corpus text, in the same order.
    fn reference_exact(docs: &[ReferenceDoc], needle: &str) -> Vec<(usize, f64)> {
        let terms = reference_terms(needle);
        let frequency = reference_frequencies(docs, &terms);
        if frequency.iter().any(|&frequency| frequency == 0) {
            return Vec::new();
        }
        let (weights, average, idf) = reference_setup(docs, &frequency);
        let mut scored = Vec::new();
        for (doc_id, doc) in docs.iter().enumerate() {
            if terms
                .iter()
                .any(|term| !doc.tf[0].contains_key(term) && !doc.tf[1].contains_key(term))
            {
                continue;
            }
            let mut score = 0.0;
            for (term, &term_idf) in terms.iter().zip(&idf) {
                for field in 0..2 {
                    let tf = doc.tf[field].get(term).copied().unwrap_or(0);
                    if tf == 0 || weights[field] == 0.0 {
                        continue;
                    }
                    score += weights[field]
                        * term_idf
                        * tf_component(tf, doc.lens[field], average[field]);
                }
            }
            scored.push((doc_id, score));
        }
        sort_reference(&mut scored);
        scored
    }

    #[test]
    fn tokenizer_lowercases_and_splits_on_punctuation() {
        assert_eq!(
            tokenize("Error->Fix, ERROR_fix!"),
            vec!["error", "fix", "error", "fix"]
        );
        assert_eq!(tokenize("  --  "), Vec::<String>::new());
        assert_eq!(tokenize("Привет, мир"), vec!["привет", "мир"]);
    }

    #[test]
    fn field_weights_decide_between_a_body_and_a_description_match() {
        let docs = vec![
            (
                0,
                [
                    "alpha alpha alpha in the body".to_string(),
                    "nothing".to_string(),
                ],
            ),
            (1, ["plain body text".to_string(), "alpha".to_string()]),
            (
                2,
                ["plain body text again".to_string(), "notes".to_string()],
            ),
            (3, ["more plain body text".to_string(), "notes".to_string()]),
        ];

        // Body only: the document that repeats the term wins.
        let body_only = index_of([1.0, 0.0], &docs);
        assert_eq!(body_only.search("alpha", 1, 1)[0].doc_id, 0);

        // Description only: the one occurrence there wins.
        let description_only = index_of([0.0, 1.0], &docs);
        assert_eq!(description_only.search("alpha", 1, 1)[0].doc_id, 1);

        // The defaults weight the description above the body, so the short
        // hand-written mention beats the three body repeats.
        let default = index_of(default_weights(), &docs);
        assert_eq!(default.search("alpha", 1, 1)[0].doc_id, 1);
    }

    #[test]
    fn min_should_match_turns_or_into_and() {
        let docs = vec![
            (0, ["alpha beta".to_string(), String::new()]),
            (1, ["alpha".to_string(), String::new()]),
            (2, ["beta".to_string(), String::new()]),
            (3, ["gamma".to_string(), String::new()]),
        ];
        let index = index_of(default_weights(), &docs);

        assert_eq!(sorted_ids(index.search("alpha beta", 1, 10)), vec![0, 1, 2]);
        assert_eq!(sorted_ids(index.search("alpha beta", 2, 10)), vec![0]);
        assert_eq!(
            sorted_ids(index.search("alpha beta", usize::MAX, 10)),
            vec![0],
            "a bound past the term count is clamped to AND"
        );

        // An unseen term still counts towards the requirement, so it can only
        // be satisfied by an OR.
        assert!(index.search("alpha absent", 2, 10).is_empty());
        assert_eq!(sorted_ids(index.search("alpha absent", 1, 10)), vec![0, 1]);
    }

    #[test]
    fn exact_intersects_postings_for_a_punctuation_needle() {
        let docs = vec![
            (
                0,
                ["error error error error error".to_string(), String::new()],
            ),
            (
                1,
                [
                    "error one two three four five six seven eight nine fix".to_string(),
                    String::new(),
                ],
            ),
            (2, ["a fix only".to_string(), String::new()]),
            (3, ["nothing here".to_string(), String::new()]),
        ];
        let index = index_of(default_weights(), &docs);

        // BM25 over the two terms ranks the document that repeats one of them,
        // which is the prototype's `error->fix` failure.
        assert_eq!(index.search("error->fix", 1, 1)[0].doc_id, 0);

        // The exact path intersects the postings, so only the document holding
        // both terms answers.
        let hits = index.exact("error->fix", 10);
        assert_eq!(sorted_ids(hits.clone()), vec![1]);
        assert_eq!(hits[0].matched_terms, 2);

        // A term the index never saw makes the conjunction empty.
        assert!(index.exact("error->absent", 10).is_empty());
        assert_eq!(sorted_ids(index.exact("error", 10)), vec![0, 1]);
    }

    #[test]
    fn remove_then_add_matches_a_fresh_build() {
        let mut corpus = vec![
            (0, ["alpha beta gamma".to_string(), "delta".to_string()]),
            (1, ["beta gamma".to_string(), "alpha".to_string()]),
            (2, ["gamma delta".to_string(), "beta".to_string()]),
            (3, ["alpha delta".to_string(), String::new()]),
            (4, ["beta delta".to_string(), "gamma".to_string()]),
        ];
        let mut patched = index_of(default_weights(), &corpus);
        patched.remove(2);

        // A removed document leaves the ranking.
        assert_eq!(sorted_ids(patched.search("gamma", 1, 10)), vec![0, 1, 4]);

        corpus[2].1 = ["alpha".to_string(), "gamma delta".to_string()];
        let fresh = index_of(default_weights(), &corpus);
        patched.add(2, [corpus[2].1[0].as_str(), corpus[2].1[1].as_str()]);
        assert_eq!(patched.len(), fresh.len());

        for query in [
            "alpha",
            "gamma",
            "alpha beta",
            "delta gamma",
            "beta gamma alpha",
        ] {
            assert_eq!(
                patched.search(query, 1, 10),
                fresh.search(query, 1, 10),
                "OR ranking differs for {query}"
            );
            assert_eq!(
                patched.search(query, 2, 10),
                fresh.search(query, 2, 10),
                "AND ranking differs for {query}"
            );
            assert_eq!(
                patched.exact(query, 10),
                fresh.exact(query, 10),
                "exact answer differs for {query}"
            );
        }

        // Removal alone also lands on the fresh build of the shrunken corpus.
        let mut shrunk = index_of(default_weights(), &corpus);
        shrunk.remove(2);
        let without: Vec<(usize, [String; 2])> = corpus
            .iter()
            .filter(|(doc_id, _)| *doc_id != 2)
            .cloned()
            .collect();
        let fresh_without = index_of(default_weights(), &without);
        for query in ["gamma", "delta gamma", "alpha beta"] {
            assert_eq!(
                shrunk.search(query, 1, 10),
                fresh_without.search(query, 1, 10),
                "ranking after a removal differs for {query}"
            );
        }

        // A re-encode carries the description positions through unchanged, so the
        // patched index and the fresh one agree term by term.
        for (doc_id, [body, description]) in &corpus {
            for term in tokenize(body).into_iter().chain(tokenize(description)) {
                assert_eq!(
                    patched.term_positions(&term, *doc_id),
                    fresh.term_positions(&term, *doc_id),
                    "positions of {term} in document {doc_id} differ after a patch"
                );
            }
        }
    }

    /// Description positions are the term's token indices in the description, for
    /// every term of every document: the shape the proximity step needs and the
    /// encoding has to keep. A term occurs in the body only when its list here is
    /// empty.
    #[test]
    fn description_positions_are_the_token_indices() {
        let corpus = generated_corpus(40, 40, 200);
        let index = index_of(default_weights(), &corpus);

        let mut checked = 0;
        for (doc_id, [_, description]) in &corpus {
            let mut expected: HashMap<String, Vec<u32>> = HashMap::new();
            for (at, term) in tokenize(description).into_iter().enumerate() {
                expected.entry(term).or_default().push(at as u32);
            }
            for (term, positions) in expected {
                assert_eq!(
                    index.term_positions(&term, *doc_id),
                    positions,
                    "positions of {term} in document {doc_id} differ"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "the generated corpus holds no description positions"
        );
    }

    #[test]
    fn ranking_is_deterministic_across_builds() {
        let corpus = generated_corpus(60, 30, 200);
        let first = index_of(default_weights(), &corpus);
        let second = index_of(default_weights(), &corpus);
        for query in [
            "w00001",
            "w00001 w00002",
            "w00001 w00002 w00003",
            "w00007 w00011",
        ] {
            let hits = first.search(query, 1, 10);
            assert_eq!(
                hits,
                second.search(query, 1, 10),
                "two builds disagree on {query}"
            );
            assert_eq!(
                hits,
                first.search(query, 1, 10),
                "two queries disagree on {query}"
            );
            assert_eq!(first.exact(query, 10), second.exact(query, 10));
        }

        // Equal scores fall back to the document id, so the order is total.
        let tied = vec![
            (0, ["same".to_string(), String::new()]),
            (1, ["same".to_string(), String::new()]),
            (2, ["same".to_string(), String::new()]),
        ];
        let index = index_of(default_weights(), &tied);
        let order: Vec<usize> = index
            .search("same", 1, 10)
            .into_iter()
            .map(|hit| hit.doc_id)
            .collect();
        assert_eq!(order, vec![0, 1, 2]);
    }

    /// The encoded blocks must answer what the corpus text answers: every term's
    /// decoded postings, and every score the two ranking paths produce, bit for
    /// bit. A decode that shifts a document id, drops a field frequency or drifts
    /// a length fails here rather than as a ranking difference nobody reads.
    #[test]
    fn encoded_blocks_decode_to_the_corpus_answer() {
        let corpus = generated_corpus(120, 60, 600);
        let index = index_of(default_weights(), &corpus);
        let docs = reference_docs(&corpus);

        // Every term's posting list, rebuilt from the text and compared with the
        // decoder's.
        let mut expected: HashMap<String, Vec<(usize, [u16; 2])>> = HashMap::new();
        for (doc_id, fields) in &corpus {
            let mut counts: HashMap<String, [u16; 2]> = HashMap::new();
            for (field, text) in fields.iter().enumerate() {
                for term in tokenize(text) {
                    counts.entry(term).or_default()[field] += 1;
                }
            }
            for (term, tf) in counts {
                expected.entry(term).or_default().push((*doc_id, tf));
            }
        }
        assert_eq!(
            expected.len(),
            index.term_ids.len(),
            "the index's vocabulary differs from the corpus's"
        );
        for (term, mut wanted) in expected {
            wanted.sort_unstable_by_key(|(doc, _)| *doc);
            let term_id = index.term_ids[&term];
            let got: Vec<(usize, [u16; 2])> = index.postings[term_id as usize]
                .postings()
                .map(|posting| (posting.doc as usize, posting.tf))
                .collect();
            assert_eq!(got, wanted, "decoded postings differ for {term}");
        }

        // Scores and order, over single terms, a conjunction and a query with a
        // term the corpus does not hold.
        for query in [
            "w00001",
            "w00003 w00004",
            "w00007 w00021 w00033",
            "w00001 w00049 absent",
        ] {
            for min_should_match in [1, 2, 3] {
                let wanted = reference_search(&docs, query, min_should_match);
                let got: Vec<(usize, f64)> = index
                    .search(query, min_should_match, usize::MAX)
                    .into_iter()
                    .map(|hit| (hit.doc_id, hit.score))
                    .collect();
                assert_eq!(
                    got, wanted,
                    "ranking differs for {query} at {min_should_match}"
                );

                // The truncated answer is the head of the same order.
                let top = wanted.len().min(5);
                let paged: Vec<(usize, f64)> = index
                    .search(query, min_should_match, top)
                    .into_iter()
                    .map(|hit| (hit.doc_id, hit.score))
                    .collect();
                assert_eq!(paged, wanted[..top], "top-{top} differs for {query}");
            }

            let wanted = reference_exact(&docs, query);
            let got: Vec<(usize, f64)> = index
                .exact(query, usize::MAX)
                .into_iter()
                .map(|hit| (hit.doc_id, hit.score))
                .collect();
            assert_eq!(got, wanted, "exact answer differs for {query}");
        }
    }

    #[test]
    fn generated_index_stays_within_its_memory_budget() {
        let corpus = generated_corpus(500, 400, 5_000);
        let index = index_of(default_weights(), &corpus);
        assert_eq!(index.len(), corpus.len());

        let per_document = index.memory_bytes() / corpus.len();
        assert!(
            per_document <= MEMORY_BUDGET_PER_DOCUMENT,
            "the index costs {per_document} bytes per document, over the \
             {MEMORY_BUDGET_PER_DOCUMENT}-byte budget"
        );

        // The generated corpus is searchable, not just buildable.
        let first_term = tokenize(&corpus[0].1[0])
            .into_iter()
            .next()
            .expect("a non-empty body");
        assert!(!index.search(&first_term, 1, 5).is_empty());
    }

    #[test]
    #[ignore = "measurement: cargo test --offline -p au-engine --lib search -- --ignored --nocapture"]
    fn measure_build_and_query_latency() {
        use std::time::Instant;

        const DOCUMENTS: usize = 1_000;
        const BODY_TOKENS: usize = 500;
        const VOCABULARY: usize = 5_000;
        const QUERIES: usize = 200;

        let corpus = generated_corpus(DOCUMENTS, BODY_TOKENS, VOCABULARY);
        let terms: usize = corpus
            .iter()
            .map(|(_, [body, description])| tokenize(body).len() + tokenize(description).len())
            .sum();

        let started = Instant::now();
        let index = index_of(default_weights(), &corpus);
        let build = started.elapsed();
        let memory = index.memory_bytes();
        println!(
            "corpus: {DOCUMENTS} documents, {terms} terms ({} per document)",
            terms / DOCUMENTS
        );
        println!(
            "build: {build:?} ({:.0} terms/s)",
            terms as f64 / build.as_secs_f64()
        );
        println!(
            "index: {memory} bytes, {} bytes per document",
            memory / DOCUMENTS
        );

        // Realistic queries: one to three terms taken from a document's body.
        let mut rng = Rng(0x1234_5678_9abc_def0);
        let mut queries = Vec::with_capacity(QUERIES);
        for _ in 0..QUERIES {
            let (_, [body, _]) = &corpus[rng.below(corpus.len())];
            let width = 1 + rng.below(3);
            let query = tokenize(body)
                .into_iter()
                .take(width)
                .collect::<Vec<_>>()
                .join(" ");
            queries.push(query);
        }

        let mut samples = Vec::with_capacity(QUERIES);
        let mut hits_seen = 0;
        for query in &queries {
            let started = Instant::now();
            let hits = index.search(query, 1, 10);
            samples.push(started.elapsed().as_secs_f64() * 1e6);
            hits_seen += hits.len();
        }
        let (min, p50, p95, max) = percentiles(samples);
        println!(
            "search (OR, top 10): min {min:.1} us, p50 {p50:.1} us, \
             p95 {p95:.1} us, max {max:.1} us; {hits_seen} hits over {QUERIES} queries"
        );

        // A conjunction written with a punctuation separator, the `error->fix`
        // shape the BM25 path answers poorly.
        let needles: Vec<String> = queries
            .iter()
            .filter_map(|query| {
                let mut terms = query.split(' ');
                let first = terms.next()?;
                let second = terms.next()?;
                Some(format!("{first}->{second}"))
            })
            .collect();
        let mut samples = Vec::with_capacity(needles.len());
        for needle in &needles {
            let started = Instant::now();
            let hits = index.exact(needle, 10);
            samples.push(started.elapsed().as_secs_f64() * 1e6);
            std::hint::black_box(hits);
        }
        let (min, p50, p95, max) = percentiles(samples);
        println!(
            "exact (two-term needle, {} queries): min {min:.1} us, \
             p50 {p50:.1} us, p95 {p95:.1} us, max {max:.1} us",
            needles.len()
        );

        assert!(
            memory / DOCUMENTS <= MEMORY_BUDGET_PER_DOCUMENT,
            "{} bytes per document exceeds the {MEMORY_BUDGET_PER_DOCUMENT}-byte budget",
            memory / DOCUMENTS
        );
    }
}
