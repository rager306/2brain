//! Optional pure-domain hybrid reranker (`--features hybrid-rerank`, U14, 0 I/O, Miri-clean).
//!
//! Model2Vec-compatible static token-embedding reranker:
//! - Lookup static token vectors (`vocab: token -> [f32; dim]`), plus optional deterministic
//!   character n-gram projection for OOV morphological / paraphrase resilience.
//! - Mean-pool + L2-normalize into dense unit vectors (`[f32; dim]`).
//! - Bounded RAM accounting (`estimated_ram_bytes`, fail-closed `max_ram_bytes`).
//! - Negation & qualifier polarity guard (`SEM-48` protection): dampens semantic boost when
//!   query and candidate disagree on negation polarity so embeddings cannot invert negative
//!   control assertions.
//! - Preserves freshness multipliers and deterministic canonical `Locator` tie-breaking.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use tb_contracts::Locator;
use tb_syntax::tokenize;

use crate::SearchHit;

const DEFAULT_MAX_RAM_BYTES: usize = 32 * 1024 * 1024; // 32 MiB default hard ceiling
pub const MAX_EMBEDDING_DIM: usize = 1024;

const NEGATION_TOKENS: &[&str] = &[
    "not",
    "no",
    "never",
    "without",
    "cannot",
    "cant",
    "dont",
    "doesnt",
    "didnt",
    "wont",
    "isnt",
    "arent",
    "disabled",
    "deprecated",
    "forbidden",
    "forbid",
    "refuse",
    "refuses",
    "reject",
    "rejects",
    "prohibit",
    "prohibited",
    "avoid",
    "except",
    "unless",
    "не",
    "нет",
    "без",
    "никогда",
    "запрещено",
    "отключено",
];

/// Configuration for the optional hybrid static-embedding reranker (U14).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HybridRerankConfig {
    /// Blend weight `alpha` in `[0.0, 1.0]` for semantic cosine similarity vs normalized BM25.
    /// Effective score: `((1.0 - alpha) * norm_bm25 + alpha * pos_cosine) * freshness_multiplier`.
    pub alpha: f64,
    /// Maximum BM25 candidate pool size to rerank.
    pub candidate_pool: usize,
    /// Enable `SEM-48` negation/polarity guard (dampens semantic lift on polarity mismatch).
    pub negation_guard: bool,
    /// Hard RAM ceiling for model + cached document vectors (bytes).
    pub max_ram_bytes: usize,
}

impl Default for HybridRerankConfig {
    fn default() -> Self {
        Self {
            alpha: 0.25,
            candidate_pool: 32,
            negation_guard: true,
            max_ram_bytes: DEFAULT_MAX_RAM_BYTES,
        }
    }
}

/// Pure-domain Model2Vec-compatible static embedding table (`token -> [f32; dim]`).
#[derive(Clone, Debug)]
pub struct StaticEmbeddingModel {
    dim: usize,
    vocab: HashMap<String, Vec<f32>>,
    use_char_ngram_fallback: bool,
}

impl StaticEmbeddingModel {
    /// Load a validated static embedding vocabulary (e.g., exported from Model2Vec `potion-*`).
    ///
    /// Fails closed on zero dimension, mismatched vector lengths, non-finite weights, or
    /// exceeding `max_ram_bytes`.
    pub fn from_vectors(
        dim: usize,
        vectors: BTreeMap<String, Vec<f32>>,
        use_char_ngram_fallback: bool,
        max_ram_bytes: usize,
    ) -> Result<Self, String> {
        if dim == 0 || dim > MAX_EMBEDDING_DIM {
            return Err(format!(
                "invalid embedding dim {dim} (expected 1..={MAX_EMBEDDING_DIM})"
            ));
        }
        let est_bytes = vectors
            .len()
            .saturating_mul(dim.saturating_mul(std::mem::size_of::<f32>()) + 48);
        if est_bytes > max_ram_bytes {
            return Err(format!(
                "static embedding table ({est_bytes} bytes) exceeds RAM budget ({max_ram_bytes} bytes)"
            ));
        }
        let mut vocab = HashMap::with_capacity(vectors.len());
        for (tok, vec) in vectors {
            if vec.len() != dim {
                return Err(format!(
                    "token '{tok}' has dimension {}, expected {dim}",
                    vec.len()
                ));
            }
            if vec.iter().any(|v| !v.is_finite()) {
                return Err(format!(
                    "token '{tok}' contains non-finite embedding weight"
                ));
            }
            vocab.insert(tok.to_ascii_lowercase(), l2_normalize(&vec));
        }
        Ok(Self {
            dim,
            vocab,
            use_char_ngram_fallback,
        })
    }

    /// Construct a deterministic zero-weight hashed subword/char-trigram projection model
    /// (plus optional synonym clusters mapped to shared directions) for offline U14 experiments,
    /// CI benchmarks, and Miri tests without external model downloads.
    #[must_use]
    pub fn deterministic_subword(dim: usize, synonym_clusters: &[&[&str]]) -> Self {
        let d = dim.clamp(16, MAX_EMBEDDING_DIM);
        let mut vocab: HashMap<String, Vec<f32>> = HashMap::new();
        for (cluster_idx, cluster) in synonym_clusters.iter().enumerate() {
            let mut proto = vec![0.0f32; d];
            // Deterministic orthogonal-like cluster basis vector via splitmix64 PRNG.
            let mut state = 0x9e37_79b9_7f4a_7c15u64
                .wrapping_mul((cluster_idx as u64).wrapping_add(1))
                ^ 0xbf58_476d_1ce4_e5b9u64;
            for slot in &mut proto {
                state = splitmix64(state);
                let signed = ((state >> 40) as f32 / ((1u32 << 24) as f32)) * 2.0 - 1.0;
                *slot = signed;
            }
            let norm_proto = l2_normalize(&proto);
            for &word in *cluster {
                vocab.insert(word.to_ascii_lowercase(), norm_proto.clone());
            }
        }
        Self {
            dim: d,
            vocab,
            use_char_ngram_fallback: true,
        }
    }

    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    #[must_use]
    pub fn vocab_len(&self) -> usize {
        self.vocab.len()
    }

    /// Estimated heap footprint of the static vocabulary table in bytes.
    #[must_use]
    pub fn estimated_ram_bytes(&self) -> usize {
        self.vocab.len() * (self.dim * std::mem::size_of::<f32>() + 48)
    }

    /// Encode arbitrary text via Model2Vec static mean-pooling + L2 normalization.
    #[must_use]
    pub fn encode(&self, text: &str) -> Vec<f32> {
        let mut acc = vec![0.0f32; self.dim];
        let mut count = 0usize;

        for tok in tokenize(text) {
            if let Some(vec) = self.vocab.get(&tok.term) {
                for (a, &v) in acc.iter_mut().zip(vec.iter()) {
                    *a += v;
                }
                count += 1;
            } else if self.use_char_ngram_fallback && tok.term.chars().count() >= 3 {
                let mut oov = vec![0.0f32; self.dim];
                project_char_trigrams(&tok.term, &mut oov);
                let norm_oov = l2_normalize(&oov);
                // Weight OOV character-trigram fallback below exact static vocab vectors
                // so domain synonym vectors are not drowned out by long OOV prose tokens.
                for (a, &v) in acc.iter_mut().zip(norm_oov.iter()) {
                    *a += 0.25 * v;
                }
                count += 1;
            }
        }

        if count == 0 {
            return acc;
        }
        let inv = 1.0 / (count as f32);
        for v in &mut acc {
            *v *= inv;
        }
        l2_normalize(&acc)
    }
}

/// Precomputed per-document dense embedding cache for fast hybrid reranking.
#[derive(Clone, Debug)]
pub struct HybridDocVectors {
    dim: usize,
    vectors: HashMap<Locator, Vec<f32>>,
    negation_flags: HashMap<Locator, bool>,
}

impl HybridDocVectors {
    /// Build per-document static vectors for a corpus under the configured RAM cap.
    pub fn build<'a>(
        model: &StaticEmbeddingModel,
        docs: impl IntoIterator<Item = (Locator, &'a str)>,
        max_ram_bytes: usize,
    ) -> Result<Self, String> {
        let mut vectors = HashMap::new();
        let mut negation_flags = HashMap::new();
        let per_doc_bytes = model.dim() * std::mem::size_of::<f32>() + 64;
        let mut total_bytes = model.estimated_ram_bytes();

        for (loc, text) in docs {
            total_bytes = total_bytes.saturating_add(per_doc_bytes);
            if total_bytes > max_ram_bytes {
                return Err(format!(
                    "hybrid doc vector index ({total_bytes} bytes) exceeds RAM budget ({max_ram_bytes} bytes)"
                ));
            }
            let vec = model.encode(text);
            let has_neg = contains_negation_cue(text);
            vectors.insert(loc.clone(), vec);
            negation_flags.insert(loc, has_neg);
        }

        Ok(Self {
            dim: model.dim(),
            vectors,
            negation_flags,
        })
    }

    #[must_use]
    pub fn estimated_ram_bytes(&self) -> usize {
        self.vectors.len() * (self.dim * std::mem::size_of::<f32>() + 64)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }
}

/// Rerank BM25 [`SearchHit`]s using static-embedding cosine similarity with `SEM-48` negation
/// polarity guard, freshness preservation, and deterministic `Locator` tie-breaking.
#[must_use]
pub fn rerank_hits(
    query: &str,
    bm25_hits: &[SearchHit],
    model: &StaticEmbeddingModel,
    doc_vectors: &HybridDocVectors,
    cfg: &HybridRerankConfig,
    limit: usize,
) -> Vec<SearchHit> {
    if bm25_hits.is_empty() || limit == 0 {
        return Vec::new();
    }
    let alpha = if cfg.alpha.is_finite() {
        cfg.alpha.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let pool_len = cfg.candidate_pool.max(limit).min(bm25_hits.len());
    let pool = &bm25_hits[..pool_len];

    let q_vec = model.encode(query);
    let q_has_neg = contains_negation_cue(query);
    let safe_fmult = |f: f64| {
        if f.is_finite() {
            f.clamp(0.01, 1.0)
        } else {
            1.0
        }
    };
    let max_raw_bm25 = pool
        .iter()
        .filter_map(|h| {
            if !h.score.is_finite() || h.score <= 0.0 {
                return None;
            }
            let f = safe_fmult(h.freshness_multiplier);
            let raw = h.score / f;
            raw.is_finite().then_some(raw)
        })
        .fold(0.0f64, f64::max)
        .max(1e-9);

    let mut reranked: Vec<SearchHit> = Vec::with_capacity(pool.len());
    for hit in pool {
        let fmult = safe_fmult(hit.freshness_multiplier);
        let norm_bm25 = if hit.score.is_finite() && hit.score > 0.0 {
            ((hit.score / fmult) / max_raw_bm25).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let raw_cos = doc_vectors.vectors.get(&hit.locator).map_or(0.0, |dv| {
            let c = dot(&q_vec, dv);
            if c.is_finite() {
                c.clamp(0.0, 1.0)
            } else {
                0.0
            }
        });

        // SEM-48 negation & qualifier guard: keep the lexical term fixed at `(1 - alpha) * norm_bm25`
        // and zero ONLY the semantic boost when query and candidate disagree on negation polarity.
        // That way an opposite-polarity doc never receives an unintended lexical jump (`1.0 > 1 - alpha`).
        let doc_has_neg = doc_vectors
            .negation_flags
            .get(&hit.locator)
            .copied()
            .unwrap_or(false);
        let semantic_weight = if cfg.negation_guard && q_has_neg != doc_has_neg {
            0.0
        } else {
            alpha
        };

        let blended = ((1.0 - alpha) * norm_bm25 + semantic_weight * raw_cos) * fmult;
        let mut next = hit.clone();
        next.score = if blended.is_finite() { blended } else { 0.0 };
        reranked.push(next);
    }

    reranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.locator.cmp(&b.locator))
    });
    reranked.truncate(limit);
    reranked
}

#[must_use]
pub fn contains_negation_cue(text: &str) -> bool {
    if tokenize(text)
        .into_iter()
        .any(|tok| NEGATION_TOKENS.contains(&tok.term.as_str()))
    {
        return true;
    }
    // Also detect English contractions (`don't`, `can't`, `doesn't`, `isn't`, `won't`, etc.)
    // whose apostrophe (`'` or `’`) is split by `tb_syntax::tokenize`.
    const CONTRACTION_STEMS: &[&str] = &[
        "cant", "dont", "doesnt", "didnt", "wont", "isnt", "arent", "wasnt", "werent", "shouldnt",
        "wouldnt", "couldnt", "mustnt", "havent", "hasnt", "hadnt",
    ];
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '\u{2019}') {
        if raw.contains('\'') || raw.contains('\u{2019}') {
            let collapsed: String = raw
                .chars()
                .filter(|&c| c != '\'' && c != '\u{2019}')
                .flat_map(char::to_lowercase)
                .collect();
            if CONTRACTION_STEMS.contains(&collapsed.as_str()) {
                return true;
            }
        }
    }
    false
}

fn dot(a: &[f32], b: &[f32]) -> f64 {
    let mut sum = 0.0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        sum += f64::from(x) * f64::from(y);
    }
    sum
}

fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm_sq: f64 = v.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    if norm_sq <= 1e-12 || !norm_sq.is_finite() {
        return vec![0.0; v.len()];
    }
    let inv = 1.0 / norm_sq.sqrt();
    v.iter().map(|&x| (f64::from(x) * inv) as f32).collect()
}

fn project_char_trigrams(term: &str, acc: &mut [f32]) {
    let chars: Vec<char> = term.chars().collect();
    if chars.len() < 3 || acc.is_empty() {
        return;
    }
    let dim = acc.len();
    for win in chars.windows(3) {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &c in win {
            h ^= u64::from(c as u32);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let idx = (h as usize) % dim;
        let sign = if (h >> 63) == 0 { 1.0f32 } else { -1.0f32 };
        acc[idx] += sign;
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SearchIndex;

    #[test]
    fn hybrid_rerank_lifts_paraphrase_match_and_respects_ram_budget() {
        let model = StaticEmbeddingModel::deterministic_subword(
            64,
            &[
                &["backoff", "retry", "exponential", "jitter"],
                &["socket", "ipc", "unix", "daemon"],
            ],
        );

        let a = Locator::new("vault", "a-generic.md").unwrap();
        let b = Locator::new("vault", "b-backoff.md").unwrap();
        let body_a = "Transient network status overview covering general diagnostics.";
        let body_b = "Network exponential backoff with jitter prevents thundering herds.";

        let idx = SearchIndex::build([(a.clone(), body_a, ""), (b.clone(), body_b, "")]);
        let bm25_hits = idx.query("network retry", false, Some(1), None, 10);
        assert_eq!(bm25_hits.len(), 2);

        let doc_vecs = HybridDocVectors::build(
            &model,
            [(a.clone(), body_a), (b.clone(), body_b)],
            1024 * 1024,
        )
        .expect("within RAM budget");

        let reranked = rerank_hits(
            "network retry",
            &bm25_hits,
            &model,
            &doc_vecs,
            &HybridRerankConfig::default(),
            10,
        );
        assert_eq!(reranked[0].locator, b);

        // Fail-closed RAM budget check.
        let too_small = HybridDocVectors::build(&model, [(a, body_a), (b, body_b)], 64);
        assert!(too_small.is_err());
    }

    #[test]
    fn sem48_negation_guard_prevents_embedding_inversion() {
        let model = StaticEmbeddingModel::deterministic_subword(
            64,
            &[&["symlink", "traversal", "escape", "confinement"]],
        );
        let neg_loc = Locator::new("vault", "policy-forbids-symlinks.md").unwrap();
        let pos_loc = Locator::new("vault", "symlinks-allowed.md").unwrap();
        let neg_body = "Workspace confinement must never follow symlink escapes outside root.";
        let pos_body = "Symlink traversal confinement symlink escape symlink guide.";

        let idx = SearchIndex::build([
            (neg_loc.clone(), neg_body, ""),
            (pos_loc.clone(), pos_body, ""),
        ]);
        let bm25_hits = idx.query("never follow symlink", false, Some(1), None, 10);
        let doc_vecs = HybridDocVectors::build(
            &model,
            [(neg_loc.clone(), neg_body), (pos_loc.clone(), pos_body)],
            1024 * 1024,
        )
        .unwrap();

        let reranked = rerank_hits(
            "never follow symlink",
            &bm25_hits,
            &model,
            &doc_vecs,
            &HybridRerankConfig {
                alpha: 0.45,
                negation_guard: true,
                ..HybridRerankConfig::default()
            },
            10,
        );
        assert_eq!(
            reranked[0].locator, neg_loc,
            "negation guard must keep negated policy note ranked first"
        );

        // Regression (Finding #2a & #2b): contractions (`don't`) are detected and opposite-polarity
        // docs never receive an accidental lexical weight jump `(1 - 0) > (1 - alpha)`.
        assert!(contains_negation_cue("don't follow symlinks"));
        assert!(contains_negation_cue("can\u{2019}t escape root"));
        let synthetic_hits = vec![
            SearchHit {
                locator: neg_loc.clone(),
                score: 10.0,
                matched_terms: 2,
                weak: false,
                freshness_multiplier: 1.0,
            },
            SearchHit {
                locator: pos_loc.clone(),
                score: 8.0,
                matched_terms: 2,
                weak: false,
                freshness_multiplier: 1.0,
            },
        ];
        let guarded = rerank_hits(
            "don't follow symlink",
            &synthetic_hits,
            &model,
            &doc_vecs,
            &HybridRerankConfig {
                alpha: 0.45,
                negation_guard: true,
                ..HybridRerankConfig::default()
            },
            2,
        );
        assert_eq!(guarded[0].locator, neg_loc);

        // Regression (Finding #7): non-finite scores/freshness never corrupt finite hits.
        let bad_loc = Locator::new("vault", "bad-inf.md").unwrap();
        let with_inf = vec![
            SearchHit {
                locator: bad_loc,
                score: f64::INFINITY,
                matched_terms: 1,
                weak: false,
                freshness_multiplier: f64::NAN,
            },
            synthetic_hits[0].clone(),
        ];
        let sanitized = rerank_hits(
            "never follow symlink",
            &with_inf,
            &model,
            &doc_vecs,
            &HybridRerankConfig::default(),
            2,
        );
        assert_eq!(sanitized[0].locator, neg_loc);
        assert!(sanitized[0].score.is_finite() && sanitized[0].score > 0.0);
    }
}
