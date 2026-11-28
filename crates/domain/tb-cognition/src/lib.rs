//! Pure-domain InfraNodus & cognitive analytics over `u32` CSR graphs (0 I/O, Miri-clean).
//!
//! All structural signals are hypotheses, never proof of truth or authorization to mutate
//! (Issue #7). Algorithms implemented:
//! - Exact Brandes betweenness centrality on CSR + Conceptual Gateways (`bc / degree`)
//! - Peel-the-Onion hub masking (`masked: &[bool]`, zero graph mutation)
//! - Deterministic multi-seed connected modularity (Leiden-style refinement) + NMI stability
//! - Configuration null-model Z-score (Maslov-Sneppen degree-preserving double-edge swaps)
//! - Callon Strategic Diagram quadrants (`Motor`, `BasicTransversal`, `Niche`, `EmergingDeclining`)
//! - Dual-Layer Discrepancy (`TypedLinkGraph ⊕ CoWord/Rare-IDF`) + Template Detector +
//!   Anti-Tautology filter + deterministic `QuestionId`
//! - LLM Bias Audit (`audit_response_bias`: source-cluster coverage vs omitted blind spots)

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tb_contracts::{Locator, NodeId};
use tb_graph::TypedLinkGraph;
use tb_search::SearchIndex;
use tb_syntax::tokenize;

pub mod claims;
pub mod evolution;
pub mod experience;
pub mod experiment;
pub mod investigation;

pub use claims::{
    AmbiguousClaimRef, ClaimRecord, ContradictionAuditReport, ContradictionItem, ContradictionKind,
    ContradictionPole, Modality, Polarity, TemporalStatus, TemporalSummary, audit_contradictions,
    conflict_id, is_claim_relevant, parse_iso_date_prefix,
};
pub use evolution::{
    BetweennessMover, DecayCandidate, EdgeDiffEntry, FragilityReport, HubFragilityEntry,
    SnapshotDiffReport, SnapshotNodeMeta, TopologyDelta, analyze_fragility, diff_snapshots,
};
pub use experience::{
    EpisodeObservation, ExperienceArm, LessonCandidate, PROMOTION_SCHEMA_VERSION,
    PromotionEvaluation, PromotionPolicy, PromotionReason, PromotionVerdict,
    evaluate_experience_promotion,
};
pub use experiment::{
    EXPERIMENT_SCHEMA_VERSION, ExperimentEvaluation, ExperimentPolicy, ExperimentVerdict,
    PairedTaskOutcome, PprConfig, PprRankEntry, PprSolverReport, SolverStopReason,
    evaluate_paired_experiment, run_bounded_ppr_solver,
};
pub use investigation::{
    CandidateRoute, ControllerDecision, InvestigationAttempt, InvestigationBudget,
    InvestigationSession, TerminationReason, route_fingerprint,
};

/// One node's Brandes centrality and conceptual gateway metrics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GatewayRow {
    pub locator: Locator,
    pub betweenness: f64,
    pub degree: usize,
    /// `betweenness / max(degree, 1)`: surfaces tight conceptual bridges over star hubs.
    pub gateway_ratio: f64,
}

/// Callon strategic quadrant classification of a community.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallonQuadrant {
    Motor,
    BasicTransversal,
    Niche,
    EmergingDeclining,
}

/// Summary of one connected community discovered in the graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommunitySummary {
    pub id: usize,
    pub size: usize,
    pub internal_edges: usize,
    pub external_edges: usize,
    pub density: f64,
    pub centrality: f64,
    pub quadrant: CallonQuadrant,
    pub representatives: Vec<Locator>,
}

/// Partition quality report with multi-seed NMI stability and null-model significance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommunityReport {
    pub modularity: f64,
    pub null_mean_modularity: f64,
    pub null_std_modularity: f64,
    pub modularity_z_score: f64,
    pub mean_nmi: f64,
    pub min_nmi: f64,
    pub community_count: usize,
    pub communities: Vec<CommunitySummary>,
}

/// Peel-the-Onion report after masking the top-K highest-betweenness hubs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PeelReport {
    pub masked_hubs: Vec<Locator>,
    pub residual_gateways: Vec<GatewayRow>,
    pub residual_community_count: usize,
}

/// Combined topology analysis report (`tb_analyze_topology`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TopologyReport {
    pub node_count: usize,
    pub edge_count: usize,
    pub top_betweenness: Vec<GatewayRow>,
    pub top_gateways: Vec<GatewayRow>,
    pub communities: CommunityReport,
    pub peel: PeelReport,
    pub fragile_hubs: Vec<FragileHub>,
}

/// Articulation point with high degree whose removal disconnects parts of the knowledge graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FragileHub {
    pub locator: Locator,
    pub degree: usize,
}

/// One candidate structural gap from Dual-Layer Discrepancy (`TypedLinkGraph ⊕ Rare-IDF`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuralGap {
    pub question_id: String,
    pub from: Locator,
    pub to: Locator,
    pub graph_distance: Option<usize>,
    pub lexical_similarity: f64,
    pub shared_terms: Vec<String>,
    pub tautology_filtered: bool,
}

/// Full structural gap detection report (`tb_detect_gaps`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GapReport {
    pub candidate_pairs_evaluated: usize,
    pub suppressed_by_negative_memory: usize,
    pub filtered_by_anti_tautology: usize,
    pub template_terms_filtered: usize,
    pub gaps: Vec<StructuralGap>,
}

/// Deterministic LLM Bias Audit comparing source context fragments against a draft answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BiasAuditReport {
    pub sources_total: usize,
    pub sources_covered: usize,
    pub coverage_ratio: f64,
    pub covered: Vec<SourceCoverage>,
    pub blind_spots: Vec<SourceCoverage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceCoverage {
    pub locator: Locator,
    pub matched_terms: usize,
    pub signature_terms: Vec<String>,
    pub overlap_ratio: f64,
}

/// Compact (~400-token) structural contextual hint (`tb_orient`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrientHint {
    pub node_count: usize,
    pub edge_count: usize,
    pub community_count: usize,
    pub modularity: f64,
    pub modularity_z_score: f64,
    pub mean_nmi: f64,
    pub top_communities: Vec<CommunitySummary>,
    pub top_gateways: Vec<GatewayRow>,
    pub peel_hidden_bridges: Vec<GatewayRow>,
    pub top_gaps: Vec<StructuralGap>,
    pub fragile_hubs: Vec<FragileHub>,
}

/// Exact unweighted Brandes betweenness centrality on CSR undirected adjacency,
/// respecting an optional `masked` node slice (Peel-the-Onion).
#[must_use]
pub fn brandes_betweenness(graph: &TypedLinkGraph, masked: Option<&[bool]>) -> Vec<f64> {
    let n = graph.node_count();
    let mut cb = vec![0.0f64; n];
    if n < 3 {
        return cb;
    }

    let is_masked = |u: usize| masked.is_some_and(|m| m.get(u).copied().unwrap_or(false));

    let mut stack: Vec<u32> = Vec::with_capacity(n);
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut sigma = vec![0.0f64; n];
    let mut dist = vec![-1i32; n];
    let mut delta = vec![0.0f64; n];
    let mut queue: VecDeque<u32> = VecDeque::with_capacity(n);
    let mut touched: Vec<u32> = Vec::with_capacity(n);

    for s in 0..n {
        if is_masked(s) {
            continue;
        }
        let s_u32 = s as u32;
        if graph.undirected_neighbors(NodeId(s_u32)).is_empty() {
            continue;
        }

        stack.clear();
        touched.clear();
        queue.clear();

        sigma[s] = 1.0;
        dist[s] = 0;
        queue.push_back(s_u32);
        touched.push(s_u32);

        while let Some(v) = queue.pop_front() {
            stack.push(v);
            let d_v = dist[v as usize];
            let sigma_v = sigma[v as usize];
            for &w_id in graph.undirected_neighbors(NodeId(v)) {
                let w = w_id.0 as usize;
                if is_masked(w) {
                    continue;
                }
                if dist[w] < 0 {
                    dist[w] = d_v + 1;
                    queue.push_back(w_id.0);
                    touched.push(w_id.0);
                }
                if dist[w] == d_v + 1 {
                    sigma[w] += sigma_v;
                    preds[w].push(v);
                }
            }
        }

        while let Some(w) = stack.pop() {
            let wi = w as usize;
            let coeff = (1.0 + delta[wi]) / sigma[wi].max(1.0);
            for &v in &preds[wi] {
                delta[v as usize] += sigma[v as usize] * coeff;
            }
            if wi != s {
                cb[wi] += delta[wi];
            }
        }

        // Reset only touched nodes for O(component) cleanup on sparse graphs.
        for &u in &touched {
            let ui = u as usize;
            preds[ui].clear();
            sigma[ui] = 0.0;
            dist[ui] = -1;
            delta[ui] = 0.0;
        }
    }

    // Undirected graph counts each shortest path twice; normalize by 0.5 and by (n-1)(n-2)/2.
    let active = (0..n).filter(|&u| !is_masked(u)).count();
    let scale = if active > 2 {
        1.0 / ((active - 1) * (active - 2)) as f64
    } else {
        1.0
    };
    for v in &mut cb {
        *v *= scale;
    }
    cb
}

/// Run full CSR topology analysis: Brandes, Conceptual Gateways, Leiden-connected communities
/// with NMI stability + null-model Z-score + Callon quadrants, Peel-the-Onion, and hub fragility.
#[must_use]
pub fn analyze_topology(
    graph: &TypedLinkGraph,
    peel_top_k: usize,
    top_limit: usize,
) -> TopologyReport {
    let n = graph.node_count();
    let cb = brandes_betweenness(graph, None);

    let mut rows: Vec<GatewayRow> = (0..n)
        .filter_map(|i| {
            let loc = graph.locator(NodeId(i as u32))?.clone();
            let deg = graph.undirected_neighbors(NodeId(i as u32)).len();
            let bc = cb[i];
            let gateway_ratio = if deg > 0 { bc / (deg as f64) } else { 0.0 };
            Some(GatewayRow {
                locator: loc,
                betweenness: bc,
                degree: deg,
                gateway_ratio,
            })
        })
        .collect();

    rows.sort_by(|a, b| {
        b.betweenness
            .total_cmp(&a.betweenness)
            .then_with(|| a.locator.cmp(&b.locator))
    });
    let top_betweenness: Vec<GatewayRow> = rows.iter().take(top_limit).cloned().collect();

    let mut by_gateway = rows.clone();
    by_gateway.sort_by(|a, b| {
        b.gateway_ratio
            .total_cmp(&a.gateway_ratio)
            .then_with(|| b.betweenness.total_cmp(&a.betweenness))
            .then_with(|| a.locator.cmp(&b.locator))
    });
    let top_gateways: Vec<GatewayRow> = by_gateway
        .into_iter()
        .filter(|r| r.betweenness > 0.0)
        .take(top_limit)
        .collect();

    // Peel-the-Onion: mask top-K highest-betweenness hubs without mutating the graph.
    let mut mask = vec![false; n];
    let mut masked_hubs = Vec::new();
    for r in rows.iter().take(peel_top_k) {
        if r.betweenness <= 0.0 {
            break;
        }
        if let Some(id) = graph.node_id(&r.locator) {
            mask[id.0 as usize] = true;
            masked_hubs.push(r.locator.clone());
        }
    }
    let peeled_cb = brandes_betweenness(graph, Some(&mask));
    let mut residual_rows: Vec<GatewayRow> = (0..n)
        .filter(|&i| !mask[i] && peeled_cb[i] > 0.0)
        .filter_map(|i| {
            let loc = graph.locator(NodeId(i as u32))?.clone();
            let deg = graph
                .undirected_neighbors(NodeId(i as u32))
                .iter()
                .filter(|nid| !mask[nid.0 as usize])
                .count();
            let bc = peeled_cb[i];
            let gateway_ratio = if deg > 0 { bc / (deg as f64) } else { 0.0 };
            Some(GatewayRow {
                locator: loc,
                betweenness: bc,
                degree: deg,
                gateway_ratio,
            })
        })
        .collect();
    residual_rows.sort_by(|a, b| {
        b.gateway_ratio
            .total_cmp(&a.gateway_ratio)
            .then_with(|| b.betweenness.total_cmp(&a.betweenness))
            .then_with(|| a.locator.cmp(&b.locator))
    });
    let residual_adj = build_masked_adj(graph, Some(&mask));
    let (residual_labels, _) = detect_connected_communities(&residual_adj, 0x5eed_cafe_0001);
    let residual_community_count = count_active_communities(&residual_labels, Some(&mask));

    let peel = PeelReport {
        masked_hubs,
        residual_gateways: residual_rows.into_iter().take(top_limit).collect(),
        residual_community_count,
    };

    // Multi-seed connected communities + NMI stability + null-model Z-score + Callon quadrants.
    let adj = build_masked_adj(graph, None);
    let communities = analyze_communities(graph, &adj, &cb, 5, 5, top_limit);

    // Hub fragility: articulation points ordered by undirected degree descending.
    let health = graph.health();
    let mut fragile_hubs: Vec<FragileHub> = health
        .articulation_points
        .into_iter()
        .filter_map(|loc| {
            let id = graph.node_id(&loc)?;
            let degree = graph.undirected_neighbors(id).len();
            Some(FragileHub {
                locator: loc,
                degree,
            })
        })
        .collect();
    fragile_hubs.sort_by(|a, b| {
        b.degree
            .cmp(&a.degree)
            .then_with(|| a.locator.cmp(&b.locator))
    });
    fragile_hubs.truncate(top_limit);

    TopologyReport {
        node_count: graph.node_count(),
        edge_count: graph.edge_count(),
        top_betweenness,
        top_gateways,
        communities,
        peel,
        fragile_hubs,
    }
}

/// Dual-Layer Discrepancy (`TypedLinkGraph ⊕ Rare-IDF`) with Template Detector,
/// Anti-Tautology check, and Negative-Memory suppression (`suppressed_question_ids`).
#[must_use]
pub fn detect_structural_gaps<'a>(
    graph: &TypedLinkGraph,
    search: &SearchIndex,
    docs: impl IntoIterator<Item = (&'a Locator, &'a str)>,
    suppressed_question_ids: &HashSet<String>,
    limit: usize,
) -> GapReport {
    let mut doc_list: Vec<(&Locator, &str)> = docs.into_iter().collect();
    doc_list.sort_by(|a, b| a.0.cmp(b.0));
    let n_docs = doc_list.len();
    if n_docs < 2 || limit == 0 {
        return GapReport {
            candidate_pairs_evaluated: 0,
            suppressed_by_negative_memory: 0,
            filtered_by_anti_tautology: 0,
            template_terms_filtered: 0,
            gaps: Vec::new(),
        };
    }

    // 1. Document frequency + Template Detector (reject boilerplate & >25% DF terms)
    //    + compact top-16 rare-term IDF vectors per document.
    //    Fast path: reuse pre-tokenized body postings from `SearchIndex` with zero re-tokenization.
    let max_df = ((n_docs as f64) * 0.25).ceil().max(2.0) as usize;
    let mut template_terms_filtered = 0usize;
    let mut doc_vecs: Vec<Vec<(String, f64)>> = Vec::with_capacity(n_docs);
    let mut doc_norms: Vec<f64> = vec![0.0; n_docs];

    let can_use_search_index = !search.is_empty()
        && doc_list
            .iter()
            .all(|(loc, body)| search.body_matches(loc, body));

    let mut used_fast_path = false;
    if can_use_search_index {
        let loc_to_idx: HashMap<&Locator, usize> = doc_list
            .iter()
            .enumerate()
            .map(|(i, (loc, _))| (*loc, i))
            .collect();
        let mut raw_doc_vecs: Vec<Vec<(&str, f64)>> = vec![Vec::new(); n_docs];
        let mut matched: Vec<(usize, u16)> = Vec::new();
        let mut saw_saturated_tf = false;
        let mut fast_template_filtered = 0usize;
        search.for_each_body_term(|term, postings| {
            if saw_saturated_tf || term.chars().count() < 4 || is_template_token(term) {
                return;
            }
            matched.clear();
            for &(loc, tf) in postings {
                if let Some(&idx) = loc_to_idx.get(loc) {
                    if tf == u16::MAX {
                        saw_saturated_tf = true;
                        return;
                    }
                    matched.push((idx, tf));
                }
            }
            let freq = matched.len();
            if freq > max_df {
                fast_template_filtered += 1;
                return;
            }
            if freq < 2 {
                return;
            }
            let idf = ((n_docs as f64 + 1.0) / (freq as f64 + 1.0)).ln() + 1.0;
            for &(idx, tf) in &matched {
                let w = (1.0 + f64::from(tf).ln()) * idf;
                raw_doc_vecs[idx].push((term, w));
            }
        });
        if !saw_saturated_tf {
            template_terms_filtered = fast_template_filtered;
            for (idx, mut weighted) in raw_doc_vecs.into_iter().enumerate() {
                weighted.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
                weighted.truncate(16);
                doc_norms[idx] = weighted.iter().map(|(_, w)| w * w).sum::<f64>().sqrt();
                doc_vecs.push(
                    weighted
                        .into_iter()
                        .map(|(t, w)| (t.to_owned(), w))
                        .collect(),
                );
            }
            used_fast_path = true;
        }
    }
    if !used_fast_path {
        let mut doc_term_sets: Vec<HashMap<String, u32>> = Vec::with_capacity(n_docs);
        let mut df: HashMap<String, usize> = HashMap::new();
        for &(_, body) in &doc_list {
            let mut counts: HashMap<String, u32> = HashMap::new();
            for tok in tokenize(body) {
                if tok.term.chars().count() < 4 || is_template_token(&tok.term) {
                    continue;
                }
                *counts.entry(tok.term).or_insert(0) += 1;
            }
            for term in counts.keys() {
                *df.entry(term.clone()).or_insert(0) += 1;
            }
            doc_term_sets.push(counts);
        }

        let mut term_idf: HashMap<String, f64> = HashMap::new();
        for (term, freq) in &df {
            if *freq < 2 || *freq > max_df {
                if *freq > max_df {
                    template_terms_filtered += 1;
                }
                continue;
            }
            let idf = ((n_docs as f64 + 1.0) / (*freq as f64 + 1.0)).ln() + 1.0;
            term_idf.insert(term.clone(), idf);
        }

        for (idx, counts) in doc_term_sets.iter().enumerate() {
            let mut weighted: Vec<(String, f64)> = counts
                .iter()
                .filter_map(|(t, &tf)| {
                    let &idf = term_idf.get(t)?;
                    let w = (1.0 + (tf as f64).ln()) * idf;
                    Some((t.clone(), w))
                })
                .collect();
            weighted.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            weighted.truncate(16);
            doc_norms[idx] = weighted.iter().map(|(_, w)| w * w).sum::<f64>().sqrt();
            doc_vecs.push(weighted);
        }
    }

    let mut postings: HashMap<&str, Vec<(usize, f64)>> = HashMap::new();
    for (idx, vec) in doc_vecs.iter().enumerate() {
        for (term, weight) in vec {
            postings
                .entry(term.as_str())
                .or_default()
                .push((idx, *weight));
        }
    }

    // 3. Accumulate candidate pairs sharing >= 2 rare signature terms (zero String cloning until top-K).
    let mut pair_dots: HashMap<(usize, usize), (f64, Vec<&str>)> = HashMap::new();
    for (&term, list) in &postings {
        // Bound hub-term pair explosion deterministically.
        if list.len() > 40 {
            continue;
        }
        for i in 0..list.len() {
            let (d1, w1) = list[i];
            for &(d2, w2) in &list[i + 1..] {
                let key = if d1 < d2 { (d1, d2) } else { (d2, d1) };
                let entry = pair_dots.entry(key).or_insert_with(|| (0.0, Vec::new()));
                entry.0 += w1 * w2;
                entry.1.push(term);
            }
        }
    }

    let candidate_pairs_evaluated = pair_dots.len();
    let mut scored: Vec<(usize, usize, f64, Vec<&str>)> = pair_dots
        .into_iter()
        .filter_map(|((i, j), (dot, mut terms))| {
            if terms.len() < 2 {
                return None;
            }
            let denom = doc_norms[i] * doc_norms[j];
            if denom <= 0.0 {
                return None;
            }
            let sim = dot / denom;
            if sim < 0.15 {
                return None;
            }
            terms.sort_unstable();
            terms.dedup();
            Some((i, j, sim, terms))
        })
        .collect();

    scored.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| doc_list[a.0].0.cmp(doc_list[b.0].0))
            .then_with(|| doc_list[a.1].0.cmp(doc_list[b.1].0))
    });

    // 4. Filter by graph distance > 2 (Dual-Layer Discrepancy), Negative Memory, and Anti-Tautology.
    let mut suppressed_by_negative_memory = 0usize;
    let mut filtered_by_anti_tautology = 0usize;
    let mut gaps = Vec::new();

    for (i, j, sim, shared_terms) in scored {
        if gaps.len() >= limit {
            break;
        }
        let loc_a = doc_list[i].0;
        let loc_b = doc_list[j].0;

        let dist = shortest_undirected_distance(graph, loc_a, loc_b, 3);
        if dist.is_some_and(|d| d <= 2) {
            continue;
        }

        let qid = question_id(loc_a, loc_b);
        if suppressed_question_ids.contains(&qid) {
            suppressed_by_negative_memory += 1;
            continue;
        }

        // Anti-Tautology: if a third existing note already scores top-3 on all shared signature
        // terms via exact/conjunctive search, the bridge already exists in the vault.
        let probe_query = shared_terms
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        let existing_bridges = search.query(&probe_query, true, None, None, 5);
        let already_bridged = existing_bridges
            .iter()
            .take(3)
            .any(|hit| &hit.locator != loc_a && &hit.locator != loc_b);
        if already_bridged {
            filtered_by_anti_tautology += 1;
            continue;
        }

        gaps.push(StructuralGap {
            question_id: qid,
            from: loc_a.clone(),
            to: loc_b.clone(),
            graph_distance: dist,
            lexical_similarity: sim,
            shared_terms: shared_terms
                .into_iter()
                .take(6)
                .map(str::to_owned)
                .collect(),
            tautology_filtered: false,
        });
    }

    GapReport {
        candidate_pairs_evaluated,
        suppressed_by_negative_memory,
        filtered_by_anti_tautology,
        template_terms_filtered,
        gaps,
    }
}

/// Deterministic LLM Bias Audit: measure how many source context notes' rare signature terms
/// are actually represented in a draft answer vs omitted as blind spots.
#[must_use]
pub fn audit_response_bias<'a>(
    sources: impl IntoIterator<Item = (&'a Locator, &'a str)>,
    response_text: &str,
) -> BiasAuditReport {
    let source_list: Vec<(&Locator, &str)> = sources.into_iter().collect();
    let answer_terms: HashSet<String> = tokenize(response_text)
        .into_iter()
        .map(|t| t.term)
        .filter(|t| t.chars().count() >= 4 && !is_template_token(t))
        .collect();

    let mut df: HashMap<String, usize> = HashMap::new();
    let mut per_source_terms: Vec<HashMap<String, u32>> = Vec::with_capacity(source_list.len());
    for &(_, body) in &source_list {
        let mut counts = HashMap::new();
        for tok in tokenize(body) {
            if tok.term.chars().count() < 4 || is_template_token(&tok.term) {
                continue;
            }
            *counts.entry(tok.term).or_insert(0) += 1;
        }
        for k in counts.keys() {
            *df.entry(k.clone()).or_insert(0) += 1;
        }
        per_source_terms.push(counts);
    }

    let n = source_list.len().max(1) as f64;
    let mut covered = Vec::new();
    let mut blind_spots = Vec::new();

    for ((loc, _), counts) in source_list.iter().zip(per_source_terms) {
        let mut sig: Vec<(String, f64)> = counts
            .into_iter()
            .map(|(t, tf)| {
                let f = df.get(&t).copied().unwrap_or(1) as f64;
                let idf = ((n + 1.0) / (f + 1.0)).ln() + 1.0;
                (t, (tf as f64) * idf)
            })
            .collect();
        sig.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let top_sig: Vec<String> = sig.into_iter().take(8).map(|(t, _)| t).collect();
        let matched = top_sig.iter().filter(|t| answer_terms.contains(*t)).count();
        let ratio = if top_sig.is_empty() {
            1.0
        } else {
            matched as f64 / top_sig.len() as f64
        };
        let row = SourceCoverage {
            locator: (*loc).clone(),
            matched_terms: matched,
            signature_terms: top_sig,
            overlap_ratio: ratio,
        };
        if ratio >= 0.25 {
            covered.push(row);
        } else {
            blind_spots.push(row);
        }
    }

    let total = covered.len() + blind_spots.len();
    let coverage_ratio = if total == 0 {
        1.0
    } else {
        covered.len() as f64 / total as f64
    };
    BiasAuditReport {
        sources_total: total,
        sources_covered: covered.len(),
        coverage_ratio,
        covered,
        blind_spots,
    }
}

/// Build the compact ~400-token Structural Contextual Hint (`tb_orient`).
#[must_use]
pub fn build_orient_hint<'a>(
    graph: &TypedLinkGraph,
    search: &SearchIndex,
    docs: impl IntoIterator<Item = (&'a Locator, &'a str)>,
    suppressed_question_ids: &HashSet<String>,
) -> OrientHint {
    let topo = analyze_topology(graph, 5, 5);
    let gaps = detect_structural_gaps(graph, search, docs, suppressed_question_ids, 5);
    OrientHint {
        node_count: topo.node_count,
        edge_count: topo.edge_count,
        community_count: topo.communities.community_count,
        modularity: topo.communities.modularity,
        modularity_z_score: topo.communities.modularity_z_score,
        mean_nmi: topo.communities.mean_nmi,
        top_communities: topo.communities.communities,
        top_gateways: topo.top_gateways,
        peel_hidden_bridges: topo.peel.residual_gateways,
        top_gaps: gaps.gaps,
        fragile_hubs: topo.fragile_hubs,
    }
}

/// Stable, collision-free 16-hex `QuestionId` for an unordered pair of locators (negative memory key `D11`).
///
/// Uses explicit length-prefixed encoding of every field so paths containing arbitrary
/// delimiters (such as `<->` or `/`) can never collide across distinct locator pairs.
#[must_use]
pub fn question_id(a: &Locator, b: &Locator) -> String {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let mut hasher = Sha256::new();
    hasher.update(b"2brain.question-id/v1\0");
    for field in [
        first.repo.as_bytes(),
        first.path.as_bytes(),
        second.repo.as_bytes(),
        second.path.as_bytes(),
    ] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for byte in &digest[..8] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn is_template_token(term: &str) -> bool {
    matches!(
        term,
        "http"
            | "https"
            | "arxiv"
            | "github"
            | "www"
            | "html"
            | "true"
            | "false"
            | "null"
            | "none"
            | "this"
            | "that"
            | "with"
            | "from"
            | "have"
            | "will"
            | "into"
            | "also"
            | "note"
            | "notes"
            | "vault"
            | "type"
            | "status"
            | "description"
    ) || term.chars().all(|c| c.is_ascii_digit())
}

fn shortest_undirected_distance(
    graph: &TypedLinkGraph,
    from: &Locator,
    to: &Locator,
    max_hops: usize,
) -> Option<usize> {
    if from == to {
        return Some(0);
    }
    let start = graph.node_id(from)?;
    let goal = graph.node_id(to)?;
    let mut visited = vec![false; graph.node_count()];
    let mut queue = VecDeque::new();
    visited[start.0 as usize] = true;
    queue.push_back((start, 0usize));

    while let Some((u, d)) = queue.pop_front() {
        if d >= max_hops {
            continue;
        }
        for &v in graph.undirected_neighbors(u) {
            if v == goal {
                return Some(d + 1);
            }
            let vi = v.0 as usize;
            if !visited[vi] {
                visited[vi] = true;
                queue.push_back((v, d + 1));
            }
        }
    }
    None
}

// === Internal CSR Adjacency, Leiden-Connected Modularity, NMI & Null Model ===

struct SimpleAdj {
    neighbors: Vec<Vec<usize>>,
    total_edges: usize,
}

fn build_masked_adj(graph: &TypedLinkGraph, masked: Option<&[bool]>) -> SimpleAdj {
    let n = graph.node_count();
    let is_masked = |u: usize| masked.is_some_and(|m| m.get(u).copied().unwrap_or(false));
    let mut neighbors = vec![Vec::new(); n];
    let mut doubled_edges = 0usize;
    for (u, slot) in neighbors.iter_mut().enumerate() {
        if is_masked(u) {
            continue;
        }
        for &v_id in graph.undirected_neighbors(NodeId(u as u32)) {
            let v = v_id.0 as usize;
            if !is_masked(v) && v != u {
                slot.push(v);
                doubled_edges += 1;
            }
        }
    }
    SimpleAdj {
        neighbors,
        total_edges: doubled_edges / 2,
    }
}

#[derive(Clone)]
struct XorShift64(u64);

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9e37_79b9_7f4a_7c15
        } else {
            seed
        })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn shuffle(&mut self, slice: &mut [usize]) {
        for i in (1..slice.len()).rev() {
            let j = (self.next_u64() as usize) % (i + 1);
            slice.swap(i, j);
        }
    }
}

/// Greedy modularity optimization + Leiden connectivity refinement step (every community is
/// guaranteed to be a connected subgraph). Returns `(labels, modularity)`.
fn detect_connected_communities(adj: &SimpleAdj, seed: u64) -> (Vec<usize>, f64) {
    let n = adj.neighbors.len();
    if n == 0 || adj.total_edges == 0 {
        return ((0..n).collect(), 0.0);
    }

    let two_m = (2 * adj.total_edges) as f64;
    let deg: Vec<usize> = adj.neighbors.iter().map(Vec::len).collect();
    let mut labels: Vec<usize> = (0..n).collect();
    let mut comm_vol: Vec<usize> = deg.clone();
    let mut order: Vec<usize> = (0..n).collect();
    let mut rng = XorShift64::new(seed);
    let mut neighbor_comm_edges: HashMap<usize, usize> = HashMap::new();

    for _pass in 0..12 {
        rng.shuffle(&mut order);
        let mut moved = false;
        for &u in &order {
            let k_u = deg[u];
            if k_u == 0 {
                continue;
            }
            let cur = labels[u];
            neighbor_comm_edges.clear();
            for &v in &adj.neighbors[u] {
                *neighbor_comm_edges.entry(labels[v]).or_insert(0) += 1;
            }

            comm_vol[cur] = comm_vol[cur].saturating_sub(k_u);
            let k_u_in_cur = neighbor_comm_edges.get(&cur).copied().unwrap_or(0);
            let base_gain = (k_u_in_cur as f64) - (k_u as f64 * comm_vol[cur] as f64) / two_m;

            let mut best_comm = cur;
            let mut best_gain = base_gain;
            for (&c, &k_u_in_c) in &neighbor_comm_edges {
                let gain = (k_u_in_c as f64) - (k_u as f64 * comm_vol[c] as f64) / two_m;
                if gain > best_gain + 1e-12 || ((gain - best_gain).abs() <= 1e-12 && c < best_comm)
                {
                    best_gain = gain;
                    best_comm = c;
                }
            }
            labels[u] = best_comm;
            comm_vol[best_comm] += k_u;
            if best_comm != cur {
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }

    // Leiden connectivity refinement: split any disconnected label into connected components.
    let refined = refine_connected(adj, &labels);
    let q = compute_modularity(adj, &refined);
    (refined, q)
}

fn refine_connected(adj: &SimpleAdj, labels: &[usize]) -> Vec<usize> {
    let n = labels.len();
    let mut next_label = 0usize;
    let mut out = vec![usize::MAX; n];
    let mut queue = VecDeque::new();

    for start in 0..n {
        if out[start] != usize::MAX {
            continue;
        }
        let target_comm = labels[start];
        out[start] = next_label;
        queue.push_back(start);
        while let Some(u) = queue.pop_front() {
            for &v in &adj.neighbors[u] {
                if out[v] == usize::MAX && labels[v] == target_comm {
                    out[v] = next_label;
                    queue.push_back(v);
                }
            }
        }
        next_label += 1;
    }
    out
}

fn compute_modularity(adj: &SimpleAdj, labels: &[usize]) -> f64 {
    if adj.total_edges == 0 {
        return 0.0;
    }
    let two_m = (2 * adj.total_edges) as f64;
    let mut comm_in: HashMap<usize, usize> = HashMap::new();
    let mut comm_tot: HashMap<usize, usize> = HashMap::new();
    for (u, nbrs) in adj.neighbors.iter().enumerate() {
        let cu = labels[u];
        *comm_tot.entry(cu).or_insert(0) += nbrs.len();
        for &v in nbrs {
            if labels[v] == cu {
                *comm_in.entry(cu).or_insert(0) += 1;
            }
        }
    }
    let mut q = 0.0f64;
    for (c, tot) in comm_tot {
        let e_in = comm_in.get(&c).copied().unwrap_or(0) as f64;
        let a_c = (tot as f64) / two_m;
        q += (e_in / two_m) - a_c * a_c;
    }
    q
}

/// Exact Normalized Mutual Information (NMI) in `[0, 1]` between two partitions.
fn normalized_mutual_information(a: &[usize], b: &[usize]) -> f64 {
    let n = a.len();
    if n == 0 {
        return 1.0;
    }
    let n_f = n as f64;
    let mut ca: HashMap<usize, usize> = HashMap::new();
    let mut cb: HashMap<usize, usize> = HashMap::new();
    let mut joint: HashMap<(usize, usize), usize> = HashMap::new();

    for (&la, &lb) in a.iter().zip(b.iter()) {
        *ca.entry(la).or_insert(0) += 1;
        *cb.entry(lb).or_insert(0) += 1;
        *joint.entry((la, lb)).or_insert(0) += 1;
    }

    let ha: f64 = ca
        .values()
        .map(|&c| {
            let p = c as f64 / n_f;
            -p * p.ln()
        })
        .sum();
    let hb: f64 = cb
        .values()
        .map(|&c| {
            let p = c as f64 / n_f;
            -p * p.ln()
        })
        .sum();
    if ha + hb <= 1e-12 {
        return 1.0;
    }

    let mut mi = 0.0f64;
    for ((la, lb), count) in joint {
        let p_ab = count as f64 / n_f;
        let p_a = ca[&la] as f64 / n_f;
        let p_b = cb[&lb] as f64 / n_f;
        if p_ab > 0.0 {
            mi += p_ab * (p_ab / (p_a * p_b)).ln();
        }
    }
    (2.0 * mi / (ha + hb)).clamp(0.0, 1.0)
}

/// Maslov-Sneppen degree-preserving double-edge-swap configuration null model.
fn randomize_degree_preserving(adj: &SimpleAdj, seed: u64) -> SimpleAdj {
    let n = adj.neighbors.len();
    let mut edges: Vec<(usize, usize)> = Vec::with_capacity(adj.total_edges);
    let mut edge_set: HashSet<(usize, usize)> = HashSet::with_capacity(adj.total_edges * 2);
    for (u, nbrs) in adj.neighbors.iter().enumerate() {
        for &v in nbrs {
            if u < v {
                edges.push((u, v));
                edge_set.insert((u, v));
            }
        }
    }
    let m = edges.len();
    if m < 2 {
        return SimpleAdj {
            neighbors: adj.neighbors.clone(),
            total_edges: adj.total_edges,
        };
    }

    let mut rng = XorShift64::new(seed);
    let attempts = (m * 4).min(20_000);
    for _ in 0..attempts {
        let i = (rng.next_u64() as usize) % m;
        let j = (rng.next_u64() as usize) % m;
        if i == j {
            continue;
        }
        let (a, b) = edges[i];
        let (c, d) = edges[j];
        if a == c || a == d || b == c || b == d {
            continue;
        }
        let e1 = if a < d { (a, d) } else { (d, a) };
        let e2 = if c < b { (c, b) } else { (b, c) };
        if edge_set.contains(&e1) || edge_set.contains(&e2) {
            continue;
        }
        edge_set.remove(&(a, b));
        edge_set.remove(&(c, d));
        edge_set.insert(e1);
        edge_set.insert(e2);
        edges[i] = e1;
        edges[j] = e2;
    }

    let mut neighbors = vec![Vec::new(); n];
    for (u, v) in edges {
        neighbors[u].push(v);
        neighbors[v].push(u);
    }
    SimpleAdj {
        neighbors,
        total_edges: m,
    }
}

fn analyze_communities(
    graph: &TypedLinkGraph,
    adj: &SimpleAdj,
    betweenness: &[f64],
    seeds: usize,
    null_trials: usize,
    top_limit: usize,
) -> CommunityReport {
    let seeds = seeds.max(1);
    let mut runs: Vec<(Vec<usize>, f64)> = Vec::with_capacity(seeds);
    for s in 0..seeds {
        let seed = 0x2b2a_1a00_u64.wrapping_add((s as u64 + 1) * 0x9e37_79b9);
        runs.push(detect_connected_communities(adj, seed));
    }

    // Best partition by modularity.
    let best_idx = runs
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
        .map_or(0, |(i, _)| i);
    let (best_labels, observed_q) = (&runs[best_idx].0, runs[best_idx].1);

    // Pairwise NMI across all seed runs.
    let mut nmi_vals = Vec::new();
    for i in 0..runs.len() {
        for j in (i + 1)..runs.len() {
            nmi_vals.push(normalized_mutual_information(&runs[i].0, &runs[j].0));
        }
    }
    let mean_nmi = if nmi_vals.is_empty() {
        1.0
    } else {
        nmi_vals.iter().sum::<f64>() / nmi_vals.len() as f64
    };
    let min_nmi = nmi_vals.iter().copied().fold(1.0f64, f64::min);

    // Configuration null-model Z-score.
    let mut null_qs = Vec::with_capacity(null_trials);
    for t in 0..null_trials {
        let seed = 0xdead_beef_0000_u64.wrapping_add((t as u64 + 1) * 7919);
        let null_adj = randomize_degree_preserving(adj, seed);
        let (_, nq) = detect_connected_communities(&null_adj, seed ^ 0x55aa);
        null_qs.push(nq);
    }
    let null_mean = if null_qs.is_empty() {
        0.0
    } else {
        null_qs.iter().sum::<f64>() / null_qs.len() as f64
    };
    let null_var = if null_qs.len() > 1 {
        null_qs
            .iter()
            .map(|&q| (q - null_mean) * (q - null_mean))
            .sum::<f64>()
            / (null_qs.len() - 1) as f64
    } else {
        0.0
    };
    let null_std = null_var.sqrt();
    let z_score = if null_std > 1e-9 {
        (observed_q - null_mean) / null_std
    } else if observed_q > null_mean + 1e-6 {
        10.0
    } else {
        0.0
    };

    // Group nodes into communities (only non-trivial size >= 2 for strategic summary).
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (u, &c) in best_labels.iter().enumerate() {
        members.entry(c).or_default().push(u);
    }
    let community_count = members.values().filter(|m| m.len() >= 2).count();

    struct RawComm {
        nodes: Vec<usize>,
        internal_edges: usize,
        external_edges: usize,
        density: f64,
        centrality: f64,
    }
    let mut raws: Vec<RawComm> = Vec::new();
    for nodes in members.into_values() {
        if nodes.len() < 2 {
            continue;
        }
        let comm_label = best_labels[nodes[0]];
        let mut int_deg = 0usize;
        let mut ext_deg = 0usize;
        for &u in &nodes {
            for &v in &adj.neighbors[u] {
                if best_labels[v] == comm_label {
                    int_deg += 1;
                } else {
                    ext_deg += 1;
                }
            }
        }
        let internal_edges = int_deg / 2;
        let k = nodes.len();
        let max_edges = k * (k - 1) / 2;
        let density = if max_edges > 0 {
            internal_edges as f64 / max_edges as f64
        } else {
            0.0
        };
        let centrality = ext_deg as f64;
        raws.push(RawComm {
            nodes,
            internal_edges,
            external_edges: ext_deg,
            density,
            centrality,
        });
    }

    let med_c = median(raws.iter().map(|r| r.centrality).collect());
    let med_d = median(raws.iter().map(|r| r.density).collect());

    raws.sort_by(|a, b| {
        b.nodes
            .len()
            .cmp(&a.nodes.len())
            .then_with(|| b.centrality.total_cmp(&a.centrality))
    });

    let communities: Vec<CommunitySummary> = raws
        .into_iter()
        .take(top_limit)
        .enumerate()
        .map(|(id, mut raw)| {
            let quadrant = match (raw.centrality >= med_c, raw.density >= med_d) {
                (true, true) => CallonQuadrant::Motor,
                (true, false) => CallonQuadrant::BasicTransversal,
                (false, true) => CallonQuadrant::Niche,
                (false, false) => CallonQuadrant::EmergingDeclining,
            };
            raw.nodes.sort_by(|&a, &b| {
                betweenness[b]
                    .total_cmp(&betweenness[a])
                    .then_with(|| adj.neighbors[b].len().cmp(&adj.neighbors[a].len()))
            });
            let representatives: Vec<Locator> = raw
                .nodes
                .iter()
                .take(3)
                .filter_map(|&u| graph.locator(NodeId(u as u32)).cloned())
                .collect();
            CommunitySummary {
                id,
                size: raw.nodes.len(),
                internal_edges: raw.internal_edges,
                external_edges: raw.external_edges,
                density: raw.density,
                centrality: raw.centrality,
                quadrant,
                representatives,
            }
        })
        .collect();

    CommunityReport {
        modularity: observed_q,
        null_mean_modularity: null_mean,
        null_std_modularity: null_std,
        modularity_z_score: z_score,
        mean_nmi,
        min_nmi,
        community_count,
        communities,
    }
}

fn count_active_communities(labels: &[usize], masked: Option<&[bool]>) -> usize {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for (u, &c) in labels.iter().enumerate() {
        if masked.is_some_and(|m| m.get(u).copied().unwrap_or(false)) {
            continue;
        }
        *counts.entry(c).or_insert(0) += 1;
    }
    counts.values().filter(|&&cnt| cnt >= 2).count()
}

fn median(mut vals: Vec<f64>) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.sort_by(f64::total_cmp);
    let mid = vals.len() / 2;
    if vals.len().is_multiple_of(2) {
        0.5 * (vals[mid - 1] + vals[mid])
    } else {
        vals[mid]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tb_syntax::EdgeKind;

    fn loc(name: &str) -> Locator {
        Locator::new("vault", format!("{name}.md")).unwrap()
    }

    #[test]
    fn planted_barbell_identifies_bridge_gateway_communities_and_peel() {
        // Two 4-node cliques connected via a single conceptual gateway `bridge`.
        let nodes = vec![
            loc("a1"),
            loc("a2"),
            loc("a3"),
            loc("a4"),
            loc("bridge"),
            loc("b1"),
            loc("b2"),
            loc("b3"),
            loc("b4"),
        ];
        let mut edges = Vec::new();
        for &(u, v) in &[
            ("a1", "a2"),
            ("a1", "a3"),
            ("a1", "a4"),
            ("a2", "a3"),
            ("a2", "a4"),
            ("a3", "a4"),
            ("b1", "b2"),
            ("b1", "b3"),
            ("b1", "b4"),
            ("b2", "b3"),
            ("b2", "b4"),
            ("b3", "b4"),
            ("a1", "bridge"),
            ("bridge", "b1"),
        ] {
            edges.push((loc(u), loc(v), EdgeKind::Navigational));
        }

        let graph = TypedLinkGraph::build(nodes, &edges);
        let report = analyze_topology(&graph, 1, 5);

        assert_eq!(report.top_betweenness[0].locator, loc("bridge"));
        assert_eq!(report.top_gateways[0].locator, loc("bridge"));
        assert!(report.communities.modularity > 0.35);
        assert!(report.communities.mean_nmi > 0.7);
        assert!(report.communities.community_count >= 2);
        assert_eq!(report.peel.masked_hubs, vec![loc("bridge")]);
        assert!(
            report
                .fragile_hubs
                .iter()
                .any(|h| h.locator == loc("bridge"))
        );
    }

    #[test]
    fn dual_layer_gap_detector_finds_disconnected_lexical_twins_and_respects_negative_memory() {
        let l1 = loc("cluster-a");
        let l2 = loc("cluster-b");
        let l3 = loc("unrelated-1");
        let l4 = loc("unrelated-2");
        let l5 = loc("unrelated-3");
        let l6 = loc("unrelated-4");

        let graph = TypedLinkGraph::build(
            vec![
                l1.clone(),
                l2.clone(),
                l3.clone(),
                l4.clone(),
                l5.clone(),
                l6.clone(),
            ],
            &[
                (l3.clone(), l4.clone(), EdgeKind::Navigational),
                (l5.clone(), l6.clone(), EdgeKind::Navigational),
            ],
        );

        let d1 = "Topological cohomology invariants govern spectral clustering stability.";
        let d2 = "Spectral clustering stability improves via topological cohomology checks.";
        let d3 = "Gardening soil compost nitrogen moisture schedule.";
        let d4 = "Tomato watering greenhouse temperature calibration.";
        let d5 = "Bicycle chain lubricant cassette drivetrain maintenance.";
        let d6 = "Espresso grinder burr alignment extraction pressure.";

        let docs = [
            (&l1, d1),
            (&l2, d2),
            (&l3, d3),
            (&l4, d4),
            (&l5, d5),
            (&l6, d6),
        ];
        let search = SearchIndex::build(docs.iter().map(|&(l, b)| (l.clone(), b, "")));

        let report = detect_structural_gaps(&graph, &search, docs, &HashSet::new(), 5);
        assert_eq!(report.gaps.len(), 1);
        assert_eq!(report.gaps[0].from, l1);
        assert_eq!(report.gaps[0].to, l2);
        assert!(report.gaps[0].graph_distance.is_none());

        // Suppressing that QuestionId via D11 negative memory removes it cleanly.
        let mut suppressed = HashSet::new();
        suppressed.insert(report.gaps[0].question_id.clone());
        let report2 = detect_structural_gaps(&graph, &search, docs, &suppressed, 5);
        assert!(report2.gaps.is_empty());
        assert_eq!(report2.suppressed_by_negative_memory, 1);
    }

    #[test]
    fn bias_audit_separates_covered_sources_from_blind_spots() {
        let s1 = loc("evidence-primary");
        let s2 = loc("evidence-exception");
        let sources = [
            (
                &s1,
                "CRDT convergence guarantees eventual consistency across partitions.",
            ),
            (
                &s2,
                "Byzantine equivocation breaks naive CRDT merge without cryptographic signatures.",
            ),
        ];
        let draft = "CRDT convergence guarantees eventual consistency across network partitions.";
        let audit = audit_response_bias(sources, draft);
        assert_eq!(audit.sources_total, 2);
        assert_eq!(audit.sources_covered, 1);
        assert_eq!(audit.blind_spots.len(), 1);
        assert_eq!(audit.blind_spots[0].locator, s2);
    }

    #[test]
    fn question_id_is_order_independent_and_collision_free_with_delimiters() {
        let a = Locator::new("vault", "a").unwrap();
        let b_delim = Locator::new("vault", "b<->vault/c").unwrap();
        let a_delim = Locator::new("vault", "a<->vault/b").unwrap();
        let c = Locator::new("vault", "c").unwrap();

        assert_eq!(question_id(&a, &b_delim), question_id(&b_delim, &a));
        assert_ne!(
            question_id(&a, &b_delim),
            question_id(&a_delim, &c),
            "length-prefixed encoding must prevent delimiter-ambiguity collisions"
        );
    }
}
