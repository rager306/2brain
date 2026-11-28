//! Pure-domain BM25 lexical search + literal grep over in-memory documents (0 I/O, Miri-clean).
//!
//! Architecture:
//! - Delta-varint [`PostingBlock`] with per-field frequencies and positions
//! - Zero-alloc [`PositionCursor`] merge walk for body proximity scoring
//! - Configurable 4-field BM25 (`k1 = 1.2`, `b = 0.75`; default weights: `body 1.0`,
//!   `description 3.0`, `keywords/aliases 2.2`, `search_questions 1.4`) with sparse-field
//!   non-empty-doc length normalization so partially populated fields are never penalized
//! - Configurable type-aware [`FreshnessPolicy`] (durable types do not decay; transient types
//!   decay with a configurable half-life)
//! - Coverage bonus (`0.6`), body adjacency proximity (`0.3`, window `5`),
//!   rare-IDF escape (`2.0`), post-ranking rare-term title multiplier (`1.0`),
//!   exact-miss plural/singular `word_forms` fallback, and 240-char word-snapped excerpts
//! - Separated literal `grep_text` (SEM-09: never conflated with conjunctive BM25 search)

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tb_contracts::Locator;
use tb_syntax::tokenize;

#[cfg(feature = "hybrid-rerank")]
pub mod hybrid;

/// BM25 term-frequency saturation.
const K1: f64 = 1.2;

/// BM25 length-normalisation strength.
const B: f64 = 0.75;

/// How much a document's term coverage lifts its BM25 sum.
const COVERAGE: f64 = 0.6;

/// How much a note's title lifts it after ranking when it states the query's rare terms.
const TITLE_COVERAGE: f64 = 1.0;

/// How much two query terms stated near each other in the body lift a document's score.
const PROXIMITY: f64 = 0.3;

/// Maximum token distance between two query terms in the body that counts as near.
const PROXIMITY_WINDOW: u32 = 5;

/// Body field index whose positions the adjacency pass reads.
const PROXIMITY_FIELD: usize = 0;

/// Default weight of field 0 (body).
const BODY_WEIGHT: f64 = 1.0;

/// Default weight of field 1 (frontmatter description).
const DESCRIPTION_WEIGHT: f64 = 3.0;

/// Default weight of field 2 (frontmatter keywords & aliases).
const KEYWORDS_WEIGHT: f64 = 2.2;

/// Default weight of field 3 (frontmatter search_questions).
const SEARCH_QUESTIONS_WEIGHT: f64 = 1.4;

/// Configurable per-field BM25 weights (zero hardcoded ranking lock-in).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchWeights {
    pub body: f64,
    pub description: f64,
    pub keywords: f64,
    pub search_questions: f64,
}

impl Default for SearchWeights {
    fn default() -> Self {
        Self {
            body: BODY_WEIGHT,
            description: DESCRIPTION_WEIGHT,
            keywords: KEYWORDS_WEIGHT,
            search_questions: SEARCH_QUESTIONS_WEIGHT,
        }
    }
}

impl SearchWeights {
    #[must_use]
    pub fn sanitized(self) -> Self {
        let clamp_w = |w: f64| if w.is_finite() && w >= 0.0 { w } else { 0.0 };
        Self {
            body: clamp_w(self.body),
            description: clamp_w(self.description),
            keywords: clamp_w(self.keywords),
            search_questions: clamp_w(self.search_questions),
        }
    }

    #[must_use]
    pub fn as_array(self) -> [f64; 4] {
        let clean = self.sanitized();
        [
            clean.body,
            clean.description,
            clean.keywords,
            clean.search_questions,
        ]
    }
}

/// Half-life in days for transient notes (inbox, sessions, daily logs).
const TRANSIENT_HALF_LIFE_DAYS: f64 = 120.0;

/// Lower floor for transient time decay so old unique notes remain discoverable.
const MIN_TRANSIENT_TIME_FACTOR: f64 = 0.15;

/// IDF threshold at which a matched term counts as rare rather than common.
pub const RARE_IDF: f64 = 2.0;

/// Default character width for a search match excerpt.
pub const EXCERPT_CHARS: usize = 240;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub doc_id: usize,
    pub score: f64,
    pub matched_terms: usize,
    pub best_idf: f64,
}

impl Hit {
    #[must_use]
    pub fn weak(&self) -> bool {
        self.best_idf < RARE_IDF
    }
}

#[derive(Clone, Copy, Debug)]
struct Posting<const N: usize> {
    doc: u32,
    tf: [u16; N],
}

#[derive(Clone, Debug)]
struct PostingBlock<const N: usize> {
    bytes: Vec<u8>,
}

impl<const N: usize> PostingBlock<N> {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn posting_count(&self) -> usize {
        self.header().0
    }

    fn header(&self) -> (usize, u32) {
        if self.bytes.is_empty() {
            return (0, 0);
        }
        let mut at = 0;
        let count = read_varint(&self.bytes, &mut at) as usize;
        let last = read_varint(&self.bytes, &mut at);
        (count, last)
    }

    fn header_len(&self) -> usize {
        if self.bytes.is_empty() {
            return 0;
        }
        let mut at = 0;
        read_varint(&self.bytes, &mut at);
        read_varint(&self.bytes, &mut at);
        at
    }

    fn postings(&self) -> Postings<'_, N> {
        Postings {
            walk: PostingWalk::new(&self.bytes),
        }
    }

    fn for_each_shared(
        &self,
        other: &Self,
        mut visit: impl FnMut(u32, [u16; N], PositionCursor<'_>, [u16; N], PositionCursor<'_>),
    ) {
        let mut left = PostingWalk::<N>::new(&self.bytes);
        let mut right = PostingWalk::<N>::new(&other.bytes);
        let mut left_entry = left.next_entry();
        let mut right_entry = right.next_entry();
        while let (Some((left_doc, left_tf, left_at)), Some((right_doc, right_tf, right_at))) =
            (left_entry, right_entry)
        {
            match left_doc.cmp(&right_doc) {
                std::cmp::Ordering::Less => left_entry = left.next_entry(),
                std::cmp::Ordering::Greater => right_entry = right.next_entry(),
                std::cmp::Ordering::Equal => {
                    visit(
                        left_doc,
                        left_tf,
                        PositionCursor::new(&self.bytes, left_at),
                        right_tf,
                        PositionCursor::new(&other.bytes, right_at),
                    );
                    left_entry = left.next_entry();
                    right_entry = right.next_entry();
                }
            }
        }
    }

    fn insert(&mut self, doc: u32, tf: [u16; N], positions: &[Vec<u32>; N]) {
        let (count, last) = self.header();
        if count == 0 || doc > last {
            self.append(doc, tf, positions);
        } else {
            self.rebuild_insert(doc, tf, positions);
        }
    }

    fn append(&mut self, doc: u32, tf: [u16; N], positions: &[Vec<u32>; N]) {
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

    fn rebuild_insert(&mut self, doc: u32, tf: [u16; N], positions: &[Vec<u32>; N]) {
        let (count, last) = self.header();
        let mut bytes = Vec::with_capacity(self.bytes.len() + 8);
        put_varint(&mut bytes, count as u32 + 1);
        put_varint(&mut bytes, last.max(doc));
        let mut prev = 0;
        let mut placed = false;
        let mut decoded: [Vec<u32>; N] = std::array::from_fn(|_| Vec::new());
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

    fn remove(&mut self, doc: u32) {
        let (count, _) = self.header();
        if count == 0 {
            return;
        }
        let mut body = Vec::new();
        let mut prev = 0;
        let mut last = 0;
        let mut kept = 0u32;
        let mut decoded: [Vec<u32>; N] = std::array::from_fn(|_| Vec::new());
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
}

struct Postings<'a, const N: usize> {
    walk: PostingWalk<'a, N>,
}

impl<const N: usize> Iterator for Postings<'_, N> {
    type Item = Posting<N>;

    fn next(&mut self) -> Option<Posting<N>> {
        self.walk.next_posting(None)
    }
}

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

    fn next_posting(&mut self, positions: Option<&mut [Vec<u32>; N]>) -> Option<Posting<N>> {
        let (doc, tf, position_at) = self.next_entry()?;
        if let Some(decoded) = positions {
            let mut at = position_at;
            for (field, &frequency) in tf.iter().enumerate() {
                decoded[field].clear();
                let mut position = 0;
                for _ in 0..frequency {
                    position += read_varint(self.bytes, &mut at);
                    decoded[field].push(position);
                }
            }
        }
        Some(Posting { doc, tf })
    }

    fn next_entry(&mut self) -> Option<(u32, [u16; N], usize)> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        self.doc += read_varint(self.bytes, &mut self.at);
        let tf: [u16; N] = std::array::from_fn(|_| read_varint(self.bytes, &mut self.at) as u16);
        let position_at = self.at;
        for _ in 0..position_count(&tf) {
            skip_varint(self.bytes, &mut self.at);
        }
        Some((self.doc, tf, position_at))
    }
}

struct PositionCursor<'a> {
    bytes: &'a [u8],
    at: usize,
    remaining: u32,
    position: u32,
}

impl<'a> PositionCursor<'a> {
    fn new(bytes: &'a [u8], at: usize) -> Self {
        Self {
            bytes,
            at,
            remaining: 0,
            position: 0,
        }
    }

    fn begin(&mut self, occurrences: u32) {
        self.remaining = occurrences;
        self.position = 0;
    }

    fn next(&mut self) -> Option<u32> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        self.position += read_varint(self.bytes, &mut self.at);
        Some(self.position)
    }

    fn skip_run(&mut self) {
        while self.next().is_some() {}
    }
}

fn min_gap(left: &mut PositionCursor<'_>, right: &mut PositionCursor<'_>) -> Option<u32> {
    let mut left_at = left.next()?;
    let mut right_at = right.next()?;
    let mut best = u32::MAX;
    loop {
        best = best.min(left_at.abs_diff(right_at));
        if best <= 1 {
            break;
        }
        if left_at <= right_at {
            match left.next() {
                Some(position) => left_at = position,
                None => break,
            }
        } else {
            match right.next() {
                Some(position) => right_at = position,
                None => break,
            }
        }
    }
    left.skip_run();
    right.skip_run();
    Some(best)
}

fn closeness<const N: usize>(
    left_tf: &[u16; N],
    mut left: PositionCursor<'_>,
    right_tf: &[u16; N],
    mut right: PositionCursor<'_>,
) -> f64 {
    let left_count = u32::from(left_tf.get(PROXIMITY_FIELD).copied().unwrap_or(0));
    let right_count = u32::from(right_tf.get(PROXIMITY_FIELD).copied().unwrap_or(0));
    if left_count == 0 || right_count == 0 {
        return 0.0;
    }
    debug_assert_eq!(PROXIMITY_FIELD, 0, "the cursor starts at field 0");
    left.begin(left_count);
    right.begin(right_count);
    min_gap(&mut left, &mut right)
        .filter(|gap| *gap <= PROXIMITY_WINDOW)
        .map_or(0.0, |gap| {
            1.0 - f64::from(gap.saturating_sub(1)) / f64::from(PROXIMITY_WINDOW)
        })
}

fn position_count<const N: usize>(tf: &[u16; N]) -> u32 {
    tf.iter().map(|&frequency| u32::from(frequency)).sum()
}

fn put_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn skip_varint(bytes: &[u8], at: &mut usize) {
    while bytes[*at] & 0x80 != 0 {
        *at += 1;
    }
    *at += 1;
}

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

fn varint_len(mut value: u32) -> usize {
    let mut len = 1;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

fn write_posting<const N: usize>(
    out: &mut Vec<u8>,
    prev_doc: u32,
    doc: u32,
    tf: &[u16; N],
    positions: &[Vec<u32>; N],
) {
    put_varint(out, doc - prev_doc);
    for &frequency in tf {
        put_varint(out, u32::from(frequency));
    }
    for (field, &frequency) in tf.iter().enumerate() {
        let mut prev_position = 0;
        for &position in positions[field].iter().take(frequency as usize) {
            put_varint(out, position - prev_position);
            prev_position = position;
        }
    }
}

fn posting_len<const N: usize>(
    prev_doc: u32,
    doc: u32,
    tf: &[u16; N],
    positions: &[Vec<u32>; N],
) -> usize {
    let mut len = varint_len(doc - prev_doc);
    for &frequency in tf {
        len += varint_len(u32::from(frequency));
    }
    for (field, &frequency) in tf.iter().enumerate() {
        let mut prev_position = 0;
        for &position in positions[field].iter().take(frequency as usize) {
            len += varint_len(position - prev_position);
            prev_position = position;
        }
    }
    len
}

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

#[derive(Default)]
struct Accumulator {
    score: f64,
    matched: usize,
    closeness: f64,
    best_idf: f64,
}

#[derive(Clone, Copy)]
pub struct DocMask<'a> {
    admitted: Option<&'a [bool]>,
}

impl<'a> DocMask<'a> {
    #[must_use]
    pub const fn all() -> Self {
        Self { admitted: None }
    }

    #[must_use]
    pub const fn new(admitted: &'a [bool]) -> Self {
        Self {
            admitted: Some(admitted),
        }
    }

    fn admits(&self, doc: u32) -> bool {
        match self.admitted {
            None => true,
            Some(admitted) => admitted.get(doc as usize).copied().unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Bm25Index<const N: usize> {
    weights: [f64; N],
    term_ids: HashMap<String, u32>,
    postings: Vec<PostingBlock<N>>,
    docs: Vec<DocSlot<N>>,
    doc_terms: Vec<Vec<u32>>,
    total_len: [u64; N],
    non_empty_docs: [usize; N],
    live_docs: usize,
}

impl<const N: usize> Bm25Index<N> {
    #[must_use]
    pub fn new(weights: [f64; N]) -> Self {
        let sanitized = std::array::from_fn(|i| {
            if weights[i].is_finite() && weights[i] >= 0.0 {
                weights[i]
            } else {
                0.0
            }
        });
        Self {
            weights: sanitized,
            term_ids: HashMap::new(),
            postings: Vec::new(),
            docs: Vec::new(),
            doc_terms: Vec::new(),
            total_len: [0; N],
            non_empty_docs: [0; N],
            live_docs: 0,
        }
    }

    pub fn from_docs_with<'a>(
        weights: [f64; N],
        docs: impl IntoIterator<Item = (usize, [&'a str; N])>,
    ) -> Self {
        let mut index = Self::new(weights);
        for (doc_id, fields) in docs {
            index.add(doc_id, fields);
        }
        index
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.live_docs
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live_docs == 0
    }

    pub fn add(&mut self, doc_id: usize, fields: [&str; N]) {
        assert!(
            doc_id <= u32::MAX as usize,
            "document id {doc_id} does not fit u32"
        );
        self.remove(doc_id);
        if doc_id >= self.docs.len() {
            self.docs.resize(doc_id + 1, DocSlot::empty());
            self.doc_terms.resize(doc_id + 1, Vec::new());
        }

        let mut counts: HashMap<u32, [u32; N]> = HashMap::new();
        let mut positions: HashMap<u32, [Vec<u32>; N]> = HashMap::new();
        let mut lens = [0u32; N];
        for (field, text) in fields.iter().enumerate() {
            for (at, tok) in tokenize(text).into_iter().enumerate() {
                let term_id = self.intern(&tok.term);
                lens[field] = lens[field].saturating_add(1);
                counts.entry(term_id).or_insert([0; N])[field] += 1;
                positions
                    .entry(term_id)
                    .or_insert_with(|| std::array::from_fn(|_| Vec::new()))[field]
                    .push(at as u32);
            }
        }

        let doc = doc_id as u32;
        for (term_id, tf) in &counts {
            let tf = std::array::from_fn(|field| u16::try_from(tf[field]).unwrap_or(u16::MAX));
            if let Some(term_positions) = positions.get(term_id) {
                self.postings[*term_id as usize].insert(doc, tf, term_positions);
            }
        }
        self.doc_terms[doc_id] = counts.into_keys().collect();
        self.docs[doc_id] = DocSlot { lens, alive: true };
        for (field, length) in lens.iter().enumerate() {
            self.total_len[field] += u64::from(*length);
            if *length > 0 {
                self.non_empty_docs[field] += 1;
            }
        }
        self.live_docs += 1;
    }

    pub fn remove(&mut self, doc_id: usize) {
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
            if *length > 0 {
                self.non_empty_docs[field] -= 1;
            }
        }
        self.live_docs -= 1;
    }

    #[must_use]
    pub fn search(
        &self,
        query: &str,
        min_should_match: usize,
        limit: usize,
        mask: DocMask<'_>,
    ) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        let (distinct_terms, term_ids) = self.query_terms(query);
        if term_ids.is_empty() {
            return Vec::new();
        }
        let required = min_should_match.min(distinct_terms);
        let average = self.average_lengths();

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
                if !mask.admits(posting.doc) {
                    continue;
                }
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
                entry.best_idf = entry.best_idf.max(idf);
            }
        }

        if PROXIMITY > 0.0 && term_ids.len() > 1 {
            for (at, &left) in term_ids.iter().enumerate() {
                for &right in &term_ids[at + 1..] {
                    self.postings[left as usize].for_each_shared(
                        &self.postings[right as usize],
                        |doc, left_tf, left_positions, right_tf, right_positions| {
                            if !mask.admits(doc) {
                                return;
                            }
                            let near =
                                closeness(&left_tf, left_positions, &right_tf, right_positions);
                            if near > 0.0
                                && let Some(entry) = accumulator.get_mut(&doc)
                            {
                                entry.closeness = entry.closeness.max(near);
                            }
                        },
                    );
                }
            }
        }

        let mut hits: Vec<Hit> = accumulator
            .into_iter()
            .filter(|(_, acc)| acc.matched >= required || acc.best_idf >= RARE_IDF)
            .map(|(doc, acc)| Hit {
                doc_id: doc as usize,
                score: acc.score
                    * (1.0 + COVERAGE * (acc.matched as f64) / (distinct_terms.max(1) as f64))
                    * (1.0 + PROXIMITY * acc.closeness),
                matched_terms: acc.matched,
                best_idf: acc.best_idf,
            })
            .collect();
        sort_hits(&mut hits);
        hits.truncate(limit);
        hits
    }

    #[must_use]
    pub fn exact(&self, needle: &str, limit: usize, mask: DocMask<'_>) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        let (distinct_terms, term_ids) = self.query_terms(needle);
        if term_ids.is_empty() || term_ids.len() != distinct_terms {
            return Vec::new();
        }

        let terms: Vec<(Vec<Posting<N>>, f64)> = term_ids
            .iter()
            .map(|&term_id| {
                let postings = &self.postings[term_id as usize];
                let decoded: Vec<Posting<N>> = postings
                    .postings()
                    .filter(|posting| mask.admits(posting.doc))
                    .collect();
                (decoded, self.idf(postings.posting_count()))
            })
            .collect();

        let Some((probe, _)) = terms
            .iter()
            .enumerate()
            .min_by_key(|(_, (postings, _))| postings.len())
        else {
            return Vec::new();
        };

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
                if let Ok(at) = postings.binary_search_by_key(&doc, |posting| posting.doc) {
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
            }
            hits.push(Hit {
                doc_id: doc as usize,
                score,
                matched_terms: terms.len(),
                best_idf: terms.iter().map(|(_, idf)| *idf).fold(0.0, f64::max),
            });
        }
        sort_hits(&mut hits);
        hits.truncate(limit);
        hits
    }

    #[must_use]
    pub fn term_idf(&self, term: &str) -> Option<f64> {
        let id = self.resolve_term(term)?;
        Some(self.idf(self.postings[id as usize].posting_count()))
    }

    fn intern(&mut self, term: &str) -> u32 {
        if let Some(&term_id) = self.term_ids.get(term) {
            return term_id;
        }
        let term_id = u32::try_from(self.postings.len()).expect("term id space exhausted");
        self.term_ids.insert(term.to_owned(), term_id);
        self.postings.push(PostingBlock::new());
        term_id
    }

    fn query_terms(&self, query: &str) -> (usize, Vec<u32>) {
        let distinct = distinct_terms(query);
        let mut ids: Vec<u32> = Vec::with_capacity(distinct.len());
        for term in &distinct {
            let Some(id) = self.resolve_term(term) else {
                continue;
            };
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        (distinct.len(), ids)
    }

    fn resolve_term(&self, term: &str) -> Option<u32> {
        let live = |id: u32| self.postings[id as usize].posting_count() > 0;
        if let Some(&id) = self.term_ids.get(term)
            && live(id)
        {
            return Some(id);
        }
        word_forms(term)
            .iter()
            .find_map(|form| self.term_ids.get(form).filter(|&id| live(*id)).copied())
    }

    fn average_lengths(&self) -> [f64; N] {
        // Normalize each field against the count of documents that actually populate it so
        // sparse optional fields (e.g. partial enrichment on 1% of notes) are not penalized
        // by a near-zero corpus-wide average length.
        std::array::from_fn(|field| {
            let docs = self.non_empty_docs[field].max(1) as f64;
            self.total_len[field] as f64 / docs
        })
    }

    fn idf(&self, document_frequency: usize) -> f64 {
        let documents = self.live_docs as f64;
        let frequency = document_frequency as f64;
        (1.0 + (documents - frequency + 0.5) / (frequency + 0.5)).ln()
    }
}

/// Type-aware freshness metadata per indexed document (Backlog P1-5, 0 I/O, Miri-clean).
///
/// Status multipliers: `current=1.0`, `aging=0.7`, `stale=0.3`, `superseded=0.05`.
/// Durable notes (patterns, ADRs, references, or explicit `freshness: durable`) never decay with age;
/// transient notes (`freshness: transient`, `inbox/`, sessions) decay with a 120-day half-life.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreshnessMeta {
    pub status_mult: f64,
    pub is_transient: bool,
    pub doc_epoch_days: Option<i64>,
}

impl Default for FreshnessMeta {
    fn default() -> Self {
        Self {
            status_mult: 1.0,
            is_transient: false,
            doc_epoch_days: None,
        }
    }
}

/// Configurable policy controlling transient half-life and floor for [`FreshnessMeta`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FreshnessPolicy {
    pub transient_half_life_days: f64,
    pub min_transient_time_factor: f64,
}

impl Default for FreshnessPolicy {
    fn default() -> Self {
        Self {
            transient_half_life_days: TRANSIENT_HALF_LIFE_DAYS,
            min_transient_time_factor: MIN_TRANSIENT_TIME_FACTOR,
        }
    }
}

impl FreshnessMeta {
    #[must_use]
    pub fn from_fields(
        status: Option<&str>,
        freshness: Option<&str>,
        doc_type: Option<&str>,
        rel_path: &str,
        date_str: Option<&str>,
    ) -> Self {
        let status_mult = match status.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("aging" | "draft" | "review") => 0.7,
            Some("stale" | "deprecated") => 0.3,
            Some("superseded" | "archived" | "rejected") => 0.05,
            _ => 1.0,
        };

        let explicit_freshness = freshness.map(|s| s.trim().to_ascii_lowercase());
        let is_transient = match explicit_freshness.as_deref() {
            Some("durable" | "evergreen" | "permanent") => false,
            Some("transient" | "ephemeral" | "daily") => true,
            _ => {
                let dtype = doc_type.map(|s| s.trim().to_ascii_lowercase());
                match dtype.as_deref() {
                    Some(
                        "pattern" | "adr" | "decision" | "reference" | "rule" | "architecture"
                        | "concept" | "guide" | "standard" | "skill" | "mcp.skill" | "mcp.tool"
                        | "error-fix",
                    ) => false,
                    Some("inbox" | "session" | "daily" | "log" | "meeting" | "scratch") => true,
                    _ => {
                        let lower_path = rel_path.to_ascii_lowercase();
                        lower_path.starts_with("inbox/")
                            || lower_path.starts_with("sessions/")
                            || lower_path.starts_with("daily/")
                            || lower_path.starts_with("journal/")
                    }
                }
            }
        };

        let doc_epoch_days = date_str.and_then(parse_iso_epoch_days);
        Self {
            status_mult,
            is_transient,
            doc_epoch_days,
        }
    }

    #[must_use]
    pub fn multiplier_as_of(&self, reference_epoch_days: Option<i64>) -> f64 {
        self.multiplier_as_of_with_policy(reference_epoch_days, FreshnessPolicy::default())
    }

    #[must_use]
    pub fn multiplier_as_of_with_policy(
        &self,
        reference_epoch_days: Option<i64>,
        policy: FreshnessPolicy,
    ) -> f64 {
        let half_life = if policy.transient_half_life_days.is_finite()
            && policy.transient_half_life_days > 0.0
        {
            policy.transient_half_life_days
        } else {
            TRANSIENT_HALF_LIFE_DAYS
        };
        let floor = if policy.min_transient_time_factor.is_finite() {
            policy.min_transient_time_factor.clamp(0.01, 1.0)
        } else {
            MIN_TRANSIENT_TIME_FACTOR
        };
        let time_factor = if self.is_transient {
            match (reference_epoch_days, self.doc_epoch_days) {
                (Some(ref_d), Some(doc_d)) => {
                    let age = ref_d.saturating_sub(doc_d);
                    if age <= 0 {
                        1.0
                    } else {
                        0.5_f64.powf((age as f64) / half_life).max(floor)
                    }
                }
                _ => 1.0,
            }
        } else {
            1.0
        };
        let status_mult = if self.status_mult.is_finite() {
            self.status_mult
        } else {
            1.0
        };
        (status_mult * time_factor).clamp(0.01, 1.0)
    }
}

/// Parse an exact ISO-8601 `YYYY-MM-DD` string into Unix epoch days (1970-01-01 = 0).
/// Rejects trailing garbage, signs (`+`/`-` in digits), and invalid calendar dates.
#[must_use]
pub fn parse_iso_date_exact(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if trimmed.len() != 10 {
        return None;
    }
    parse_iso_ymd_checked(trimmed.as_bytes())
}

/// Parse an ISO-8601 `YYYY-MM-DD` date or `YYYY-MM-DD[T ]...` timestamp into Unix epoch days.
/// Strictly validates ASCII digits and calendar bounds including leap years.
#[must_use]
pub fn parse_iso_epoch_days(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    if bytes.len() > 10 && !matches!(bytes[10], b'T' | b't' | b' ') {
        return None;
    }
    parse_iso_ymd_checked(&bytes[..10])
}

fn parse_iso_ymd_checked(bytes: &[u8]) -> Option<i64> {
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    for &idx in &[0usize, 1, 2, 3, 5, 6, 8, 9] {
        if !bytes[idx].is_ascii_digit() {
            return None;
        }
    }
    let s = std::str::from_utf8(bytes).ok()?;
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || day == 0 {
        return None;
    }
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap => 29,
        2 => 28,
        _ => return None,
    };
    if day > max_day {
        return None;
    }
    // Howard Hinnant's civil-to-days algorithm (exact over Gregorian proleptic calendar).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u64;
    let m = u64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + u64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + (doe as i64) - 719_468)
}

/// Rich document input for [`SearchIndex::build_rich`] and [`SearchIndex::upsert_rich_document`].
#[derive(Clone, Debug)]
pub struct SearchDocInput<'a> {
    pub locator: Locator,
    pub body: &'a str,
    pub description: &'a str,
    pub keywords: &'a str,
    pub search_questions: &'a str,
    pub freshness: FreshnessMeta,
}

/// Servable four-field (`body`, `description`, `keywords`, `search_questions`) BM25 index with
/// type-aware freshness bound to canonical [`Locator`]s.
#[derive(Debug, Clone)]
pub struct SearchIndex {
    index: Bm25Index<4>,
    weights: SearchWeights,
    freshness_policy: FreshnessPolicy,
    locators: Vec<Locator>,
    body_hashes: Vec<u64>,
    freshness: Vec<FreshnessMeta>,
    locator_to_doc: HashMap<Locator, usize>,
    default_reference_days: Option<i64>,
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub locator: Locator,
    pub score: f64,
    pub matched_terms: usize,
    pub weak: bool,
    pub freshness_multiplier: f64,
}

impl SearchIndex {
    /// Build from an iterator of `(Locator, body, description)` sorted by `Locator`
    /// (backwards-compatible shorthand with empty enrichment and neutral freshness).
    pub fn build<'a>(docs: impl IntoIterator<Item = (Locator, &'a str, &'a str)>) -> Self {
        Self::build_rich(
            docs.into_iter()
                .map(|(locator, body, description)| SearchDocInput {
                    locator,
                    body,
                    description,
                    keywords: "",
                    search_questions: "",
                    freshness: FreshnessMeta::default(),
                }),
            None,
        )
    }

    /// Build from rich documents carrying separate `keywords` and `search_questions` fields plus
    /// type-aware [`FreshnessMeta`], sorted deterministically by `Locator`.
    pub fn build_rich<'a>(
        docs: impl IntoIterator<Item = SearchDocInput<'a>>,
        default_reference_days: Option<i64>,
    ) -> Self {
        Self::build_rich_with_weights(docs, default_reference_days, SearchWeights::default())
    }

    /// Build from rich documents with explicit per-field [`SearchWeights`].
    pub fn build_rich_with_weights<'a>(
        docs: impl IntoIterator<Item = SearchDocInput<'a>>,
        default_reference_days: Option<i64>,
        weights: SearchWeights,
    ) -> Self {
        let mut collected: Vec<SearchDocInput<'a>> = docs.into_iter().collect();
        collected.sort_by(|a, b| a.locator.cmp(&b.locator));

        let mut locators = Vec::with_capacity(collected.len());
        let mut body_hashes = Vec::with_capacity(collected.len());
        let mut freshness = Vec::with_capacity(collected.len());
        let mut locator_to_doc = HashMap::with_capacity(collected.len());
        let clean_weights = weights.sanitized();
        let mut index = Bm25Index::new(clean_weights.as_array());
        for doc in collected {
            if doc.body.is_empty()
                && doc.description.is_empty()
                && doc.keywords.is_empty()
                && doc.search_questions.is_empty()
            {
                continue;
            }
            let doc_id = locators.len();
            index.add(
                doc_id,
                [
                    doc.body,
                    doc.description,
                    doc.keywords,
                    doc.search_questions,
                ],
            );
            locator_to_doc.insert(doc.locator.clone(), doc_id);
            locators.push(doc.locator);
            body_hashes.push(fnv1a64(doc.body.as_bytes()));
            freshness.push(doc.freshness);
        }
        Self {
            index,
            weights: clean_weights,
            freshness_policy: FreshnessPolicy::default(),
            locators,
            body_hashes,
            freshness,
            locator_to_doc,
            default_reference_days,
        }
    }

    /// Return the default reference day configured for transient freshness decay.
    #[must_use]
    pub fn default_reference_days(&self) -> Option<i64> {
        self.default_reference_days
    }

    /// Return the active per-field BM25 weights.
    #[must_use]
    pub fn weights(&self) -> SearchWeights {
        self.weights
    }

    /// Return the active [`FreshnessPolicy`] used for transient age decay.
    #[must_use]
    pub fn freshness_policy(&self) -> FreshnessPolicy {
        self.freshness_policy
    }

    /// Configure the [`FreshnessPolicy`] used by [`SearchIndex::query`] and [`SearchIndex::query_with_options`].
    pub fn set_freshness_policy(&mut self, policy: FreshnessPolicy) {
        self.freshness_policy = policy;
    }

    /// Update the default reference day used by [`SearchIndex::query`] for transient decay.
    pub fn set_default_reference_days(&mut self, days: Option<i64>) {
        self.default_reference_days = days;
    }

    /// Incrementally insert or replace a single document (empty enrichment, neutral freshness).
    pub fn upsert_document(&mut self, locator: Locator, body: &str, description: &str) {
        self.upsert_rich_document(SearchDocInput {
            locator,
            body,
            description,
            keywords: "",
            search_questions: "",
            freshness: FreshnessMeta::default(),
        });
    }

    /// Incrementally insert or replace a single rich document without re-tokenizing the corpus.
    pub fn upsert_rich_document(&mut self, doc: SearchDocInput<'_>) {
        if doc.body.is_empty()
            && doc.description.is_empty()
            && doc.keywords.is_empty()
            && doc.search_questions.is_empty()
        {
            self.remove_document(&doc.locator);
            return;
        }
        let hash = fnv1a64(doc.body.as_bytes());
        let fields = [
            doc.body,
            doc.description,
            doc.keywords,
            doc.search_questions,
        ];
        if let Some(&doc_id) = self.locator_to_doc.get(&doc.locator) {
            self.index.add(doc_id, fields);
            self.body_hashes[doc_id] = hash;
            self.freshness[doc_id] = doc.freshness;
        } else {
            let doc_id = self.locators.len();
            self.index.add(doc_id, fields);
            self.locator_to_doc.insert(doc.locator.clone(), doc_id);
            self.locators.push(doc.locator);
            self.body_hashes.push(hash);
            self.freshness.push(doc.freshness);
        }
    }

    /// Incrementally remove a single document from the live index.
    pub fn remove_document(&mut self, locator: &Locator) {
        if let Some(&doc_id) = self.locator_to_doc.get(locator) {
            self.index.remove(doc_id);
            self.body_hashes[doc_id] = 0;
            self.freshness[doc_id] = FreshnessMeta::default();
        }
    }

    /// Returns true when deleted tombstones exceed compaction thresholds and callers should rebuild
    /// from live documents to reclaim dead slots and orphaned vocabulary.
    #[must_use]
    pub fn needs_compaction(&self) -> bool {
        let total = self.locators.len();
        let live = self.index.len();
        let dead = total.saturating_sub(live);
        dead >= 64 && dead.saturating_mul(4) >= total
    }

    /// Returns true if `locator` is currently live in this search index.
    #[must_use]
    pub fn contains_locator(&self, locator: &Locator) -> bool {
        self.locator_to_doc
            .get(locator)
            .is_some_and(|&doc_id| self.index.docs.get(doc_id).is_some_and(|slot| slot.alive))
    }

    /// Returns true iff this index's stored body postings for `locator` match `body` byte-for-byte
    /// (by FNV-1a64 digest; empty bodies match when absent or indexed with an empty body).
    #[must_use]
    pub fn body_matches(&self, locator: &Locator, body: &str) -> bool {
        let hash = fnv1a64(body.as_bytes());
        match self.locator_to_doc.get(locator) {
            Some(&doc_id) if self.index.docs.get(doc_id).is_some_and(|slot| slot.alive) => {
                self.body_hashes.get(doc_id) == Some(&hash)
            }
            _ => body.is_empty(),
        }
    }

    /// Visit every indexed term with at least one live body-field (`field 0`) posting.
    /// Reuses an internal scratch buffer to avoid per-term heap allocations.
    pub fn for_each_body_term<'a>(&'a self, mut visit: impl FnMut(&'a str, &[(&'a Locator, u16)])) {
        let mut buf: Vec<(&'a Locator, u16)> = Vec::new();
        for (term, &term_id) in &self.index.term_ids {
            buf.clear();
            let block = &self.index.postings[term_id as usize];
            if block.posting_count() == 0 {
                continue;
            }
            for posting in block.postings() {
                if posting.tf[0] > 0 {
                    let doc_idx = posting.doc as usize;
                    if self.index.docs.get(doc_idx).is_some_and(|slot| slot.alive) {
                        buf.push((&self.locators[doc_idx], posting.tf[0]));
                    }
                }
            }
            if !buf.is_empty() {
                visit(term.as_str(), &buf);
            }
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    #[must_use]
    pub fn query(
        &self,
        query: &str,
        exact: bool,
        min_should_match: Option<usize>,
        repo_filter: Option<&str>,
        limit: usize,
    ) -> Vec<SearchHit> {
        self.query_with_options(
            query,
            exact,
            min_should_match,
            repo_filter,
            self.default_reference_days,
            true,
            limit,
        )
    }

    /// Query BM25 with explicit type-aware freshness controls (`reference_days` and `apply_freshness`).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn query_with_options(
        &self,
        query: &str,
        exact: bool,
        min_should_match: Option<usize>,
        repo_filter: Option<&str>,
        reference_days: Option<i64>,
        apply_freshness: bool,
        limit: usize,
    ) -> Vec<SearchHit> {
        let admitted_buf: Option<Vec<bool>> =
            repo_filter.map(|repo| self.locators.iter().map(|loc| loc.repo == repo).collect());
        let mask = match &admitted_buf {
            Some(buf) => DocMask::new(buf),
            None => DocMask::all(),
        };

        let mut hits = if exact {
            self.index.exact(query, usize::MAX, mask)
        } else {
            let floor = min_should_match.unwrap_or_else(|| default_min_should_match(query));
            self.index.search(query, floor, usize::MAX, mask)
        };

        let rare: Vec<String> = distinct_terms(query)
            .into_iter()
            .filter(|term| self.index.term_idf(term).is_some_and(|idf| idf >= RARE_IDF))
            .collect();
        let mut mults: Vec<f64> = Vec::with_capacity(hits.len());
        for hit in &mut hits {
            if !rare.is_empty() {
                let stem = file_stem(&self.locators[hit.doc_id].path);
                hit.score *= title_multiplier(title_coverage(&rare, stem));
            }
            let fmult = if apply_freshness {
                self.freshness
                    .get(hit.doc_id)
                    .copied()
                    .unwrap_or_default()
                    .multiplier_as_of_with_policy(reference_days, self.freshness_policy)
            } else {
                1.0
            };
            hit.score *= fmult;
            mults.push(fmult);
        }

        let mut paired: Vec<(Hit, f64)> = hits.into_iter().zip(mults).collect();
        // Always break score ties deterministically by canonical Locator (independent of incremental doc_id order).
        paired.sort_by(|(a, _), (b, _)| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| self.locators[a.doc_id].cmp(&self.locators[b.doc_id]))
        });

        paired
            .into_iter()
            .take(limit)
            .map(|(hit, freshness_multiplier)| SearchHit {
                locator: self.locators[hit.doc_id].clone(),
                score: hit.score,
                matched_terms: hit.matched_terms,
                weak: hit.weak(),
                freshness_multiplier,
            })
            .collect()
    }
}

/// Literal substring match inside a single document (SEM-09: strictly separate from BM25).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GrepLineMatch {
    pub line: usize,
    pub column: usize,
    pub snippet: String,
}

/// Literal substring search over a document's text (case-insensitive by default unless `case_sensitive`).
#[must_use]
pub fn grep_text(text: &str, needle: &str, case_sensitive: bool) -> Vec<GrepLineMatch> {
    if needle.is_empty() {
        return Vec::new();
    }
    let needle_lower = if case_sensitive {
        String::new()
    } else {
        needle.to_lowercase()
    };
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let col = if case_sensitive {
            line.find(needle)
        } else {
            line.to_lowercase().find(&needle_lower)
        };
        if let Some(column) = col {
            out.push(GrepLineMatch {
                line: idx + 1,
                column: column + 1,
                snippet: line.chars().take(EXCERPT_CHARS).collect(),
            });
        }
    }
    out
}

/// Distinct lowercase query terms in first-appearance order.
#[must_use]
pub fn distinct_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for tok in tokenize(query) {
        if !terms.contains(&tok.term) {
            terms.push(tok.term);
        }
    }
    terms
}

/// Default minimum-should-match floor: 1..=2 terms require all, 3+ require 2 (with rare-IDF escape).
#[must_use]
pub fn default_min_should_match(query: &str) -> usize {
    let distinct = distinct_terms(query);
    match distinct.len() {
        count if count <= 2 => count,
        _ => 2,
    }
}

/// Word-snapped excerpt window around the first query term in `body`.
#[must_use]
pub fn excerpt(body: &str, terms: &[String], width: usize) -> String {
    if width == 0 || body.is_empty() {
        return String::new();
    }
    let start = window_start(body, terms, width);
    let mut end = start;
    for (taken, (at, ch)) in body[start..].char_indices().enumerate() {
        if taken == width {
            break;
        }
        end = start + at + ch.len_utf8();
    }

    let mut from = start;
    if from > 0
        && !body[..from]
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
        && let Some((at, _)) = body[from..end]
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
    {
        from += at;
    }
    let mut to = end;
    if to < body.len() && !body[to..].chars().next().is_some_and(char::is_whitespace) {
        match body[from..to]
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
        {
            Some((at, _)) => to = from + at,
            None => {
                if let Some((at, _)) = body[to..]
                    .char_indices()
                    .take(width)
                    .find(|(_, ch)| ch.is_whitespace())
                {
                    to += at;
                }
            }
        }
    }
    if to < from {
        to = from;
    }

    let text = body[from..to]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = String::with_capacity(text.len() + 8);
    if from > 0 {
        out.push('…');
    }
    out.push_str(&text);
    if to < body.len() {
        out.push('…');
    }
    out
}

fn window_start(body: &str, terms: &[String], width: usize) -> usize {
    let Some(at) = first_term(body, terms) else {
        return 0;
    };
    body[..at]
        .char_indices()
        .rev()
        .nth(width / 3)
        .map_or(0, |(index, _)| index)
}

fn first_term(body: &str, terms: &[String]) -> Option<usize> {
    if terms.is_empty() {
        return None;
    }
    let forms: Vec<String> = terms.iter().flat_map(|term| word_forms(term)).collect();
    let stated = |token: &str| {
        terms.iter().any(|term| term_matches(token, term))
            || forms.iter().any(|form| term_matches(token, form))
    };
    let mut start: Option<usize> = None;
    for (at, ch) in body.char_indices() {
        if ch.is_alphanumeric() {
            start.get_or_insert(at);
        } else if let Some(from) = start.take()
            && stated(&body[from..at])
        {
            return Some(from);
        }
    }
    start.filter(|from| stated(&body[*from..]))
}

fn term_matches(token: &str, term: &str) -> bool {
    if token.is_ascii() {
        token.eq_ignore_ascii_case(term)
    } else {
        token.to_lowercase() == term
    }
}

fn word_forms(term: &str) -> Vec<String> {
    let mut forms = Vec::new();
    if term.chars().count() < 4 {
        return forms;
    }
    if let Some(stem) = term.strip_suffix("ies").filter(|stem| stem.len() >= 2) {
        forms.push(format!("{stem}y"));
    }
    if let Some(stem) = term.strip_suffix("es").filter(|stem| stem.len() >= 3) {
        forms.push(stem.to_owned());
    }
    if let Some(stem) = term.strip_suffix('s').filter(|stem| stem.len() >= 3)
        && !stem.ends_with('s')
    {
        forms.push(stem.to_owned());
    }
    if let Some(stem) = term.strip_suffix('y') {
        forms.push(format!("{stem}ies"));
    }
    forms.push(format!("{term}s"));
    forms.push(format!("{term}es"));
    forms
}

fn tf_component(tf: u32, field_length: u32, average_length: f64) -> f64 {
    let tf = f64::from(tf);
    let average = if average_length > 0.0 {
        average_length
    } else {
        1.0
    };
    let normalisation = 1.0 - B + B * f64::from(field_length) / average;
    tf * (K1 + 1.0) / (tf + K1 * normalisation)
}

fn sort_hits(hits: &mut [Hit]) {
    hits.sort_unstable_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then(left.doc_id.cmp(&right.doc_id))
    });
}

fn file_stem(rel_path: &str) -> &str {
    let file = rel_path.rsplit('/').next().unwrap_or(rel_path);
    file.strip_suffix(".md").unwrap_or(file)
}

fn title_coverage(rare: &[String], title: &str) -> f64 {
    if rare.is_empty() {
        return 0.0;
    }
    let terms = tokenize(title);
    let matched = rare
        .iter()
        .filter(|term| terms.iter().any(|tok| tok.term == **term))
        .count();
    matched as f64 / rare.len() as f64
}

fn title_multiplier(coverage: f64) -> f64 {
    1.0 + TITLE_COVERAGE * coverage * coverage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sem09_bm25_finds_split_and_phrase_while_grep_finds_literal_only() {
        let split_body =
            "zqvalpha appears in paragraph one.\n\nParagraph two mentions zqvomega separately.\n";
        let phrase_body = "Exact phrase zqvalpha zqvomega appears together here.\n";

        let l_split = Locator::new("vault", "ops/tck-sem09-split.md").unwrap();
        let l_phrase = Locator::new("vault", "ops/tck-sem09-phrase.md").unwrap();

        let index = SearchIndex::build([
            (l_split.clone(), split_body, ""),
            (l_phrase.clone(), phrase_body, ""),
        ]);

        let hits = index.query("zqvalpha zqvomega", false, None, Some("vault"), 10);
        assert_eq!(hits.len(), 2);
        // Proximity bonus ranks the adjacent phrase above the split note.
        assert_eq!(hits[0].locator, l_phrase);
        assert_eq!(hits[1].locator, l_split);

        assert!(grep_text(split_body, "zqvalpha zqvomega", false).is_empty());
        let g = grep_text(phrase_body, "zqvalpha zqvomega", false);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].line, 1);
    }

    #[test]
    fn word_forms_and_description_weighting_work() {
        let a = Locator::new("vault", "a.md").unwrap();
        let b = Locator::new("vault", "b.md").unwrap();
        let index = SearchIndex::build([
            (
                a.clone(),
                "Long prose mentioning capsule once among many other filler words here.",
                "",
            ),
            (b.clone(), "Short body.", "Authoritative capsule note"),
        ]);
        let hits = index.query("capsules", false, None, None, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].locator, b,
            "description weight 3.0 should outrank body mention"
        );
    }

    #[test]
    fn excerpt_windows_around_first_query_term() {
        let body = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega";
        let terms = distinct_terms("lambda");
        let snip = excerpt(body, &terms, 36);
        assert!(snip.contains("lambda"), "excerpt was: {snip}");
        assert!(snip.starts_with('…'));
        assert!(snip.ends_with('…'));
    }

    #[test]
    fn excerpt_never_panics_on_long_punctuation_separated_text() {
        let prefix = "a".repeat(120);
        let suffix = "b".repeat(120);
        let body = format!("{prefix}-needle-{suffix}");
        let terms = distinct_terms("needle");
        let snip = excerpt(&body, &terms, 40);
        assert!(snip.contains("needle"), "excerpt was: {snip}");
    }

    #[test]
    fn incremental_upsert_and_remove_match_full_rebuild() {
        let a = Locator::new("vault", "a.md").unwrap();
        let b = Locator::new("vault", "b.md").unwrap();
        let c = Locator::new("vault", "c.md").unwrap();

        // Insert `b` first and `a` later with identical score to verify Locator tie-breaking parity.
        let mut inc = SearchIndex::build([
            (b.clone(), "graph modularity bridge", "topology"),
            (c.clone(), "old content", "old"),
        ]);
        inc.upsert_document(a.clone(), "quantum lattice mechanics", "notes");
        inc.upsert_document(c.clone(), "quantum lattice mechanics", "notes");
        inc.remove_document(&b);

        let rebuilt = SearchIndex::build([
            (a.clone(), "quantum lattice mechanics", "notes"),
            (c.clone(), "quantum lattice mechanics", "notes"),
        ]);

        assert_eq!(inc.len(), 2);
        assert_eq!(rebuilt.len(), 2);
        assert!(!inc.contains_locator(&b));
        assert!(inc.body_matches(&a, "quantum lattice mechanics"));
        assert!(!inc.body_matches(&a, "mutated body text"));
        assert!(inc.body_matches(&b, ""));
        assert!(!inc.body_matches(&b, "non-empty"));

        for q in ["quantum", "lattice", "modularity", "bridge"] {
            let lhs = inc.query(q, false, None, None, 10);
            let rhs = rebuilt.query(q, false, None, None, 10);
            assert_eq!(lhs.len(), rhs.len(), "query '{q}' len mismatch");
            for (l, r) in lhs.iter().zip(rhs.iter()) {
                assert_eq!(l.locator, r.locator, "query '{q}' locator order mismatch");
                assert_eq!(l.matched_terms, r.matched_terms);
                assert_eq!(l.weak, r.weak);
                assert!(
                    (l.score - r.score).abs() < 1e-12,
                    "query '{q}' score drift: {} vs {}",
                    l.score,
                    r.score
                );
            }
        }
    }

    #[test]
    fn type_aware_freshness_and_enrichment_fields_rank_correctly() {
        assert_eq!(parse_iso_epoch_days("1970-01-01"), Some(0));
        assert_eq!(parse_iso_date_exact("1970-01-01"), Some(0));
        assert_eq!(parse_iso_date_exact("2026-11-28garbage"), None);
        assert_eq!(parse_iso_date_exact("2026-+1-01"), None);
        assert_eq!(
            parse_iso_epoch_days("2026-11-28T12:00:00Z"),
            parse_iso_date_exact("2026-11-28")
        );
        assert_eq!(parse_iso_epoch_days("2026-11-28garbage"), None);
        assert_eq!(parse_iso_epoch_days("2026-02-30"), None);
        assert_eq!(parse_iso_epoch_days("2026-13-01"), None);

        let ref_day = parse_iso_epoch_days("2026-11-28").unwrap();
        let current_durable = Locator::new("vault", "patterns/retry.md").unwrap();
        let superseded_durable = Locator::new("vault", "patterns/retry-old.md").unwrap();
        let aged_transient = Locator::new("vault", "inbox/retry-log.md").unwrap();
        let explicit_durable_inbox = Locator::new("vault", "inbox/backlog.md").unwrap();

        let idx = SearchIndex::build_rich(
            [
                SearchDocInput {
                    locator: current_durable.clone(),
                    body: "Backoff jitter prevents thundering herds.",
                    description: "Retry pattern",
                    keywords: "exponential backoff circuit breaker",
                    search_questions: "how to stop retry storms",
                    freshness: FreshnessMeta::from_fields(
                        Some("current"),
                        None,
                        Some("pattern"),
                        "patterns/retry.md",
                        Some("2025-01-01"),
                    ),
                },
                SearchDocInput {
                    locator: superseded_durable.clone(),
                    body: "Backoff jitter prevents thundering herds.",
                    description: "Retry pattern",
                    keywords: "",
                    search_questions: "",
                    freshness: FreshnessMeta::from_fields(
                        Some("superseded"),
                        None,
                        Some("pattern"),
                        "patterns/retry-old.md",
                        Some("2026-11-27"),
                    ),
                },
                SearchDocInput {
                    locator: aged_transient.clone(),
                    body: "Backoff jitter prevents thundering herds.",
                    description: "Retry pattern",
                    keywords: "",
                    search_questions: "",
                    freshness: FreshnessMeta::from_fields(
                        Some("current"),
                        None,
                        Some("inbox"),
                        "inbox/retry-log.md",
                        Some("2026-03-31"), // ~242 days old (~2 half-lives -> ~0.25)
                    ),
                },
                SearchDocInput {
                    locator: explicit_durable_inbox.clone(),
                    body: "Backoff jitter prevents thundering herds.",
                    description: "Retry pattern",
                    keywords: "",
                    search_questions: "",
                    freshness: FreshnessMeta::from_fields(
                        Some("current"),
                        Some("durable"),
                        Some("extract"),
                        "inbox/backlog.md",
                        Some("2025-01-01"),
                    ),
                },
            ],
            Some(ref_day),
        );

        let hits = idx.query("thundering herds", false, None, None, 10);
        assert_eq!(hits.len(), 4);
        // Durable notes keep 1.0 despite being from 2025; aged transient decays (~0.25); superseded is 0.05.
        let by_loc: HashMap<&Locator, &SearchHit> = hits.iter().map(|h| (&h.locator, h)).collect();
        assert!((by_loc[&current_durable].freshness_multiplier - 1.0).abs() < 1e-12);
        assert!((by_loc[&explicit_durable_inbox].freshness_multiplier - 1.0).abs() < 1e-12);
        assert!(by_loc[&aged_transient].freshness_multiplier < 0.30);
        assert!(by_loc[&aged_transient].freshness_multiplier > 0.20);
        assert!((by_loc[&superseded_durable].freshness_multiplier - 0.05).abs() < 1e-12);
        assert_eq!(hits.last().unwrap().locator, superseded_durable);

        // Enrichment field (field 2) matches paraphrase terms absent from body & description!
        let enrich_hits = idx.query("circuit breaker storms", false, None, None, 5);
        assert_eq!(enrich_hits.len(), 1);
        assert_eq!(enrich_hits[0].locator, current_durable);

        // ignore_freshness=true (apply_freshness=false) restores 1.0 multiplier across all docs.
        let raw_hits = idx.query_with_options(
            "thundering herds",
            false,
            None,
            None,
            Some(ref_day),
            false,
            10,
        );
        assert!(
            raw_hits
                .iter()
                .all(|h| (h.freshness_multiplier - 1.0).abs() < 1e-12)
        );
    }

    #[test]
    fn sparse_optional_enrichment_field_is_not_penalized_by_empty_corpus_docs() {
        let target = Locator::new("vault", "patterns/target.md").unwrap();
        let mut sparse_docs: Vec<(Locator, String, String, String)> = Vec::new();
        sparse_docs.push((
            target.clone(),
            "Generic architecture note body.".to_owned(),
            "Architecture note".to_owned(),
            "anti-accretion governance gate".to_owned(),
        ));
        let filler_count = if cfg!(miri) { 16 } else { 200 };
        for i in 0..filler_count {
            sparse_docs.push((
                Locator::new("vault", format!("filler/doc-{i:03}.md")).unwrap(),
                format!("Unrelated note number {i} discussing ordinary workflows."),
                "Filler note".to_owned(),
                String::new(),
            ));
        }

        let sparse_idx = SearchIndex::build_rich(
            sparse_docs
                .iter()
                .map(|(loc, body, desc, kw)| SearchDocInput {
                    locator: loc.clone(),
                    body: body.as_str(),
                    description: desc.as_str(),
                    keywords: kw.as_str(),
                    search_questions: if loc == &target {
                        "How do you prevent governance accretion in agent repos?"
                    } else {
                        ""
                    },
                    freshness: FreshnessMeta::default(),
                }),
            None,
        );

        let hits = sparse_idx.query("anti-accretion governance", false, None, None, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].locator, target);
        // With sparse-field non-empty-doc avg length, field_len == avg_len (normalisation = 1.0),
        // so the BM25 score stays strong instead of collapsing under an empty-doc penalty.
        assert!(
            hits[0].score > 8.0,
            "sparse enrichment score collapsed to {}",
            hits[0].score
        );

        // Verify separate keywords vs search_questions weights: keywords (2.2) outranks questions (1.4).
        let kw_doc = Locator::new("vault", "kw.md").unwrap();
        let sq_doc = Locator::new("vault", "sq.md").unwrap();
        let field_idx = SearchIndex::build_rich(
            [
                SearchDocInput {
                    locator: kw_doc.clone(),
                    body: "neutral prose",
                    description: "note",
                    keywords: "quorum consensus",
                    search_questions: "",
                    freshness: FreshnessMeta::default(),
                },
                SearchDocInput {
                    locator: sq_doc.clone(),
                    body: "neutral prose",
                    description: "note",
                    keywords: "",
                    search_questions: "quorum consensus",
                    freshness: FreshnessMeta::default(),
                },
            ],
            None,
        );
        let ranked = field_idx.query("quorum consensus", false, None, None, 2);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].locator, kw_doc);
        assert!(ranked[0].score > ranked[1].score);

        // Regression: NaN/Inf SearchWeights & FreshnessMeta::status_mult never poison scores,
        // and SearchIndex::set_freshness_policy adjusts transient age decay dynamically.
        let bad_w = SearchWeights {
            body: f64::NAN,
            description: f64::INFINITY,
            keywords: -5.0,
            search_questions: 1.5,
        };
        let mut safe_idx = SearchIndex::build_rich_with_weights(
            [SearchDocInput {
                locator: kw_doc.clone(),
                body: "quorum",
                description: "quorum",
                keywords: "quorum",
                search_questions: "quorum",
                freshness: FreshnessMeta {
                    status_mult: f64::NAN,
                    is_transient: true,
                    doc_epoch_days: Some(100),
                },
            }],
            Some(160),
            bad_w,
        );
        assert_eq!(safe_idx.weights().body, 0.0);
        assert_eq!(safe_idx.weights().description, 0.0);
        assert_eq!(safe_idx.weights().keywords, 0.0);
        assert_eq!(safe_idx.weights().search_questions, 1.5);
        let h_default = safe_idx.query("quorum", false, None, None, 1);
        assert_eq!(h_default.len(), 1);
        assert!(h_default[0].score.is_finite());
        assert!(h_default[0].freshness_multiplier.is_finite());
        // Shorten half-life from 120d to 30d -> age 60d decays from ~0.707 to 0.25.
        safe_idx.set_freshness_policy(FreshnessPolicy {
            transient_half_life_days: 30.0,
            min_transient_time_factor: 0.1,
        });
        let h_fast = safe_idx.query("quorum", false, None, None, 1);
        assert!((h_fast[0].freshness_multiplier - 0.25).abs() < 1e-6);

        // Regression: incremental sparse-enrichment toggle matches full rebuild bit-for-bit.
        let mut inc_sparse = SearchIndex::build_rich(
            [
                SearchDocInput {
                    locator: kw_doc.clone(),
                    body: "alpha",
                    description: "note",
                    keywords: "",
                    search_questions: "",
                    freshness: FreshnessMeta::default(),
                },
                SearchDocInput {
                    locator: sq_doc.clone(),
                    body: "beta",
                    description: "note",
                    keywords: "",
                    search_questions: "",
                    freshness: FreshnessMeta::default(),
                },
            ],
            None,
        );
        inc_sparse.upsert_rich_document(SearchDocInput {
            locator: kw_doc.clone(),
            body: "alpha",
            description: "note",
            keywords: "quorum consensus",
            search_questions: "how does quorum work",
            freshness: FreshnessMeta::default(),
        });
        let full_sparse = SearchIndex::build_rich(
            [
                SearchDocInput {
                    locator: kw_doc.clone(),
                    body: "alpha",
                    description: "note",
                    keywords: "quorum consensus",
                    search_questions: "how does quorum work",
                    freshness: FreshnessMeta::default(),
                },
                SearchDocInput {
                    locator: sq_doc,
                    body: "beta",
                    description: "note",
                    keywords: "",
                    search_questions: "",
                    freshness: FreshnessMeta::default(),
                },
            ],
            None,
        );
        let inc_h = inc_sparse.query("quorum consensus", false, None, None, 5);
        let full_h = full_sparse.query("quorum consensus", false, None, None, 5);
        assert_eq!(inc_h.len(), 1);
        assert!((inc_h[0].score - full_h[0].score).abs() < 1e-12);
    }
}
