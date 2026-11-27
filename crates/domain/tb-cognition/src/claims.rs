//! Pure-domain Claim Layer (`D03`), Temporal Validity (`R14`), and Polysingularity
//! Contradiction Audit (`0` I/O, Miri-clean).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tb_contracts::Locator;

/// Epistemic polarity of a claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    Positive,
    Negative,
    Neutral,
    Conditional,
}

impl Polarity {
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("positive" | "supports" | "affirm" | "true" | "for") => Self::Positive,
            Some("negative" | "refutes" | "deny" | "false" | "against") => Self::Negative,
            Some("conditional" | "scoped" | "depends") => Self::Conditional,
            _ => Self::Neutral,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
            Self::Neutral => "neutral",
            Self::Conditional => "conditional",
        }
    }
}

/// Epistemic modality of a claim (`D03`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Fact,
    Hypothesis,
    Decision,
    Norm,
    Open,
}

impl Modality {
    #[must_use]
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("fact" | "observation" | "measurement" | "evidence") => Self::Fact,
            Some("hypothesis" | "conjecture" | "speculation") => Self::Hypothesis,
            Some("decision" | "adr" | "commitment") => Self::Decision,
            Some("norm" | "rule" | "policy" | "principle" | "invariant") => Self::Norm,
            _ => Self::Open,
        }
    }
}

/// Temporal validity classification of a claim at an `as_of` date (`R14`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalStatus {
    Active,
    Expired,
    NotYetValid,
    Superseded,
    UnknownWindow,
}

/// One ambiguous frontmatter reference (`supersedes` or `conflicts_with`) that matched
/// multiple notes by stem and therefore cannot be resolved unambiguously.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AmbiguousClaimRef {
    pub field: String,
    pub raw_target: String,
    pub candidates: Vec<Locator>,
}

/// Normalized claim record extracted from a note's frontmatter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimRecord {
    pub locator: Locator,
    pub sha256: String,
    pub kind: Option<String>,
    pub status: Option<String>,
    /// Optional normalized subject/key used to detect implicit polarity clashes.
    pub claim_key: Option<String>,
    pub polarity: Polarity,
    pub modality: Modality,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub observed_at: Option<String>,
    pub supersedes: Vec<Locator>,
    pub conflicts_with: Vec<Locator>,
    #[serde(default)]
    pub ambiguous_references: Vec<AmbiguousClaimRef>,
    pub summary: Option<String>,
}

/// Kind of epistemic or temporal anomaly surfaced by [`audit_contradictions`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContradictionKind {
    ExplicitConflict,
    ImplicitPolarityClash,
    SupersessionAnomaly,
    ExpiredActiveClaim,
    AmbiguousReference,
}

impl ContradictionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitConflict => "explicit_conflict",
            Self::ImplicitPolarityClash => "implicit_polarity_clash",
            Self::SupersessionAnomaly => "supersession_anomaly",
            Self::ExpiredActiveClaim => "expired_active_claim",
            Self::AmbiguousReference => "ambiguous_reference",
        }
    }
}

/// One preserved pole of a contradiction (Polysingularity — never collapsed silently).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContradictionPole {
    pub locator: Locator,
    pub sha256: String,
    pub polarity: Polarity,
    pub modality: Modality,
    pub temporal_status: TemporalStatus,
    pub summary: Option<String>,
}

/// One surfaced epistemic contradiction or temporal anomaly with stable [`conflict_id`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContradictionItem {
    pub conflict_id: String,
    pub kind: ContradictionKind,
    pub severity: String,
    pub claim_key: Option<String>,
    pub poles: Vec<ContradictionPole>,
    pub explanation: String,
}

/// Aggregate temporal distribution across inspected claims.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TemporalSummary {
    pub total_claims: usize,
    pub active: usize,
    pub expired: usize,
    pub not_yet_valid: usize,
    pub superseded: usize,
    pub unknown_window: usize,
    pub as_of: Option<String>,
}

/// Full deterministic contradiction and temporal validity audit report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContradictionAuditReport {
    pub temporal_summary: TemporalSummary,
    pub total_conflicts: usize,
    pub conflicts: Vec<ContradictionItem>,
}

/// Validate and extract a canonical `YYYY-MM-DD` prefix from an ISO-8601 date/timestamp.
#[must_use]
pub fn parse_iso_date_prefix(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.len() < 10 {
        return None;
    }
    let prefix = trimmed.get(..10)?;
    let bytes = prefix.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: u16 = prefix.get(0..4)?.parse().ok()?;
    let month: u8 = prefix.get(5..7)?.parse().ok()?;
    let day: u8 = prefix.get(8..10)?.parse().ok()?;
    if year < 1900 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Verify day-of-month upper bound (conservative calendar check).
    let max_day = match month {
        2 => {
            let leap =
                (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
            if leap { 29 } else { 28 }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day > max_day {
        return None;
    }
    Some(prefix)
}

/// Deterministic, order-independent, length-prefixed SHA-256 identifier for a contradiction.
#[must_use]
pub fn conflict_id(kind: ContradictionKind, poles: &[Locator], extra_key: Option<&str>) -> String {
    let mut sorted: Vec<String> = poles
        .iter()
        .map(|l| format!("{}/{}", l.repo, l.path))
        .collect();
    sorted.sort();
    sorted.dedup();

    let mut hasher = Sha256::new();
    hasher.update(b"2brain.conflict-id/v1\0");
    let kind_s = kind.as_str();
    hasher.update((kind_s.len() as u64).to_le_bytes());
    hasher.update(kind_s.as_bytes());
    if let Some(k) = extra_key {
        hasher.update([1u8]);
        hasher.update((k.len() as u64).to_le_bytes());
        hasher.update(k.as_bytes());
    } else {
        hasher.update([0u8]);
    }
    hasher.update((sorted.len() as u64).to_le_bytes());
    for p in &sorted {
        hasher.update((p.len() as u64).to_le_bytes());
        hasher.update(p.as_bytes());
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(20);
    for byte in &digest[..8] {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    format!("c_{hex}")
}

/// Run the pure-domain Polysingularity Contradiction & Temporal Validity Audit (`D03` + `R14`).
#[must_use]
pub fn audit_contradictions(
    claims: &[ClaimRecord],
    as_of: Option<&str>,
    limit: usize,
) -> ContradictionAuditReport {
    let valid_as_of = as_of.and_then(parse_iso_date_prefix);
    let by_loc: BTreeMap<&Locator, usize> = claims
        .iter()
        .enumerate()
        .map(|(i, c)| (&c.locator, i))
        .collect();

    // First pass: temporal window evaluation (before supersession).
    let mut base_status = vec![TemporalStatus::Active; claims.len()];
    for (idx, c) in claims.iter().enumerate() {
        let from_parsed = match c.valid_from.as_deref() {
            Some(s) => match parse_iso_date_prefix(s) {
                Some(d) => Some(d),
                None => {
                    base_status[idx] = TemporalStatus::UnknownWindow;
                    continue;
                }
            },
            None => None,
        };
        let to_parsed = match c.valid_to.as_deref() {
            Some(s) => match parse_iso_date_prefix(s) {
                Some(d) => Some(d),
                None => {
                    base_status[idx] = TemporalStatus::UnknownWindow;
                    continue;
                }
            },
            None => None,
        };
        if let (Some(f), Some(t)) = (from_parsed, to_parsed)
            && f > t
        {
            base_status[idx] = TemporalStatus::UnknownWindow;
            continue;
        }
        if let Some(now) = valid_as_of {
            if let Some(t) = to_parsed
                && t < now
            {
                base_status[idx] = TemporalStatus::Expired;
                continue;
            }
            if let Some(f) = from_parsed
                && f > now
            {
                base_status[idx] = TemporalStatus::NotYetValid;
                continue;
            }
        }
        if c.status
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("superseded"))
        {
            base_status[idx] = TemporalStatus::Superseded;
        }
    }

    // Second pass: active supersession edges mark targets as Superseded, while tracking anomalies.
    let mut effective_status = base_status.clone();
    let mut supersedes_pairs: HashSet<(usize, usize)> = HashSet::new();
    let mut conflicts: Vec<ContradictionItem> = Vec::new();
    let mut seen_conflict_ids: HashSet<String> = HashSet::new();

    for (src_idx, c) in claims.iter().enumerate() {
        for target in &c.supersedes {
            match by_loc.get(target).copied() {
                Some(dst_idx) => {
                    supersedes_pairs.insert((src_idx, dst_idx));
                    if src_idx == dst_idx {
                        let cid = conflict_id(
                            ContradictionKind::SupersessionAnomaly,
                            std::slice::from_ref(&c.locator),
                            Some("self_supersede"),
                        );
                        if seen_conflict_ids.insert(cid.clone()) {
                            conflicts.push(ContradictionItem {
                                conflict_id: cid,
                                kind: ContradictionKind::SupersessionAnomaly,
                                severity: "warning".to_owned(),
                                claim_key: c.claim_key.clone(),
                                poles: vec![pole_of(c, effective_status[src_idx])],
                                explanation: format!(
                                    "Claim '{}' declares self-supersession",
                                    c.locator.path
                                ),
                            });
                        }
                    } else if base_status[src_idx] == TemporalStatus::Active {
                        effective_status[dst_idx] = TemporalStatus::Superseded;
                    }
                }
                None => {
                    if base_status[src_idx] == TemporalStatus::Active {
                        let cid = conflict_id(
                            ContradictionKind::SupersessionAnomaly,
                            &[c.locator.clone(), target.clone()],
                            Some("missing_target"),
                        );
                        if seen_conflict_ids.insert(cid.clone()) {
                            conflicts.push(ContradictionItem {
                                conflict_id: cid,
                                kind: ContradictionKind::SupersessionAnomaly,
                                severity: "warning".to_owned(),
                                claim_key: c.claim_key.clone(),
                                poles: vec![pole_of(c, effective_status[src_idx])],
                                explanation: format!(
                                    "Active claim '{}' supersedes missing target '{}/{}'",
                                    c.locator.path, target.repo, target.path
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    // Detect supersession cycles (A supersedes B and B supersedes A, or longer cycles).
    let cycle_nodes = find_supersession_cycles(claims.len(), &supersedes_pairs);
    for cyc in cycle_nodes {
        let locs: Vec<Locator> = cyc.iter().map(|&i| claims[i].locator.clone()).collect();
        let cid = conflict_id(
            ContradictionKind::SupersessionAnomaly,
            &locs,
            Some("supersession_cycle"),
        );
        if seen_conflict_ids.insert(cid.clone()) {
            let poles = cyc
                .iter()
                .map(|&i| pole_of(&claims[i], effective_status[i]))
                .collect();
            conflicts.push(ContradictionItem {
                conflict_id: cid,
                kind: ContradictionKind::SupersessionAnomaly,
                severity: "error".to_owned(),
                claim_key: None,
                poles,
                explanation: format!(
                    "Cyclic supersedes chain across {} claims; cannot determine canonical successor",
                    cyc.len()
                ),
            });
        }
    }

    // Third pass: explicit `conflicts_with` edges (Polysingularity — preserve both poles).
    for (src_idx, c) in claims.iter().enumerate() {
        if effective_status[src_idx] != TemporalStatus::Active {
            continue;
        }
        for target in &c.conflicts_with {
            let Some(&dst_idx) = by_loc.get(target) else {
                continue;
            };
            if dst_idx == src_idx || effective_status[dst_idx] != TemporalStatus::Active {
                continue;
            }
            let (a, b) = if claims[src_idx].locator <= claims[dst_idx].locator {
                (src_idx, dst_idx)
            } else {
                (dst_idx, src_idx)
            };
            let cid = conflict_id(
                ContradictionKind::ExplicitConflict,
                &[claims[a].locator.clone(), claims[b].locator.clone()],
                None,
            );
            if seen_conflict_ids.insert(cid.clone()) {
                conflicts.push(ContradictionItem {
                    conflict_id: cid,
                    kind: ContradictionKind::ExplicitConflict,
                    severity: "error".to_owned(),
                    claim_key: claims[a]
                        .claim_key
                        .clone()
                        .or_else(|| claims[b].claim_key.clone()),
                    poles: vec![
                        pole_of(&claims[a], effective_status[a]),
                        pole_of(&claims[b], effective_status[b]),
                    ],
                    explanation: format!(
                        "Active conflict between '{}' ({}) and '{}' ({}); both poles preserved",
                        claims[a].locator.path,
                        claims[a].polarity.as_str(),
                        claims[b].locator.path,
                        claims[b].polarity.as_str()
                    ),
                });
            }
        }
    }

    // Fourth pass: implicit polarity clashes on the same normalized `claim_key`.
    let mut by_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, c) in claims.iter().enumerate() {
        if effective_status[idx] != TemporalStatus::Active {
            continue;
        }
        if let Some(key) = c.claim_key.as_deref() {
            let norm = key.trim().to_ascii_lowercase();
            if !norm.is_empty() {
                by_key.entry(norm).or_default().push(idx);
            }
        }
    }
    for (key, group) in &by_key {
        let positives: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&i| claims[i].polarity == Polarity::Positive)
            .collect();
        let negatives: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&i| claims[i].polarity == Polarity::Negative)
            .collect();
        for &pos_i in &positives {
            for &neg_i in &negatives {
                if supersedes_pairs.contains(&(pos_i, neg_i))
                    || supersedes_pairs.contains(&(neg_i, pos_i))
                {
                    continue;
                }
                let (a, b) = if claims[pos_i].locator <= claims[neg_i].locator {
                    (pos_i, neg_i)
                } else {
                    (neg_i, pos_i)
                };
                let cid = conflict_id(
                    ContradictionKind::ImplicitPolarityClash,
                    &[claims[a].locator.clone(), claims[b].locator.clone()],
                    Some(key),
                );
                if seen_conflict_ids.insert(cid.clone()) {
                    conflicts.push(ContradictionItem {
                        conflict_id: cid,
                        kind: ContradictionKind::ImplicitPolarityClash,
                        severity: "error".to_owned(),
                        claim_key: Some(key.clone()),
                        poles: vec![
                            pole_of(&claims[a], effective_status[a]),
                            pole_of(&claims[b], effective_status[b]),
                        ],
                        explanation: format!(
                            "Opposing active claims on '{key}': '{}' ({}) vs '{}' ({}) without supersession",
                            claims[a].locator.path,
                            claims[a].polarity.as_str(),
                            claims[b].locator.path,
                            claims[b].polarity.as_str()
                        ),
                    });
                }
            }
        }
    }

    // Fifth pass: expired active claims (status explicitly says active, but valid_to < as_of).
    for (idx, c) in claims.iter().enumerate() {
        if effective_status[idx] == TemporalStatus::Expired
            && c.status
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("active"))
        {
            let cid = conflict_id(
                ContradictionKind::ExpiredActiveClaim,
                std::slice::from_ref(&c.locator),
                c.valid_to.as_deref(),
            );
            if seen_conflict_ids.insert(cid.clone()) {
                conflicts.push(ContradictionItem {
                    conflict_id: cid,
                    kind: ContradictionKind::ExpiredActiveClaim,
                    severity: "warning".to_owned(),
                    claim_key: c.claim_key.clone(),
                    poles: vec![pole_of(c, effective_status[idx])],
                    explanation: format!(
                        "Claim '{}' is still marked status=active past valid_to={} (as_of={})",
                        c.locator.path,
                        c.valid_to.as_deref().unwrap_or("?"),
                        valid_as_of.unwrap_or("?")
                    ),
                });
            }
        }
    }

    // Sixth pass: surface ambiguous `conflicts_with` / `supersedes` stem references explicitly
    // so contradiction poles are never silently lost.
    for (idx, c) in claims.iter().enumerate() {
        for amb in &c.ambiguous_references {
            let mut key_locs = vec![c.locator.clone()];
            key_locs.extend(amb.candidates.iter().cloned());
            let extra = format!("{}:{}", amb.field, amb.raw_target);
            let cid = conflict_id(
                ContradictionKind::AmbiguousReference,
                &key_locs,
                Some(&extra),
            );
            if seen_conflict_ids.insert(cid.clone()) {
                let mut poles = vec![pole_of(c, effective_status[idx])];
                for cand in &amb.candidates {
                    if let Some(&cand_idx) = by_loc.get(cand) {
                        poles.push(pole_of(&claims[cand_idx], effective_status[cand_idx]));
                    }
                }
                let cand_list: Vec<String> = amb
                    .candidates
                    .iter()
                    .map(|l| format!("{}/{}", l.repo, l.path))
                    .collect();
                conflicts.push(ContradictionItem {
                    conflict_id: cid,
                    kind: ContradictionKind::AmbiguousReference,
                    severity: "warning".to_owned(),
                    claim_key: c.claim_key.clone(),
                    poles,
                    explanation: format!(
                        "Ambiguous {} target '{}' in '{}' matches {} notes: [{}]; qualify by path to preserve contradiction poles",
                        amb.field,
                        amb.raw_target,
                        c.locator.path,
                        amb.candidates.len(),
                        cand_list.join(", ")
                    ),
                });
            }
        }
    }

    // Deterministic ordering: errors before warnings, then kind, then conflict_id.
    conflicts.sort_by(|a, b| {
        let rank = |sev: &str| if sev == "error" { 0u8 } else { 1u8 };
        rank(&a.severity)
            .cmp(&rank(&b.severity))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.conflict_id.cmp(&b.conflict_id))
    });

    let mut summary = TemporalSummary {
        total_claims: claims.len(),
        active: 0,
        expired: 0,
        not_yet_valid: 0,
        superseded: 0,
        unknown_window: 0,
        as_of: valid_as_of.map(str::to_owned),
    };
    for st in &effective_status {
        match st {
            TemporalStatus::Active => summary.active += 1,
            TemporalStatus::Expired => summary.expired += 1,
            TemporalStatus::NotYetValid => summary.not_yet_valid += 1,
            TemporalStatus::Superseded => summary.superseded += 1,
            TemporalStatus::UnknownWindow => summary.unknown_window += 1,
        }
    }

    let total_conflicts = conflicts.len();
    conflicts.truncate(limit.clamp(1, 200));

    ContradictionAuditReport {
        temporal_summary: summary,
        total_conflicts,
        conflicts,
    }
}

fn pole_of(c: &ClaimRecord, temporal_status: TemporalStatus) -> ContradictionPole {
    ContradictionPole {
        locator: c.locator.clone(),
        sha256: c.sha256.clone(),
        polarity: c.polarity,
        modality: c.modality,
        temporal_status,
        summary: c.summary.clone(),
    }
}

/// Deterministic iterative cycle detector over `supersedes` edges (SCCs of size >= 2).
fn find_supersession_cycles(n: usize, edges: &HashSet<(usize, usize)>) -> Vec<Vec<usize>> {
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(u, v) in edges {
        if u < n && v < n && u != v {
            adj[u].push(v);
        }
    }
    for list in &mut adj {
        list.sort_unstable();
        list.dedup();
    }

    // Iterative Tarjan SCC.
    let mut index = 0usize;
    let mut indices = vec![usize::MAX; n];
    let mut lowlink = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut tarjan_stack: Vec<usize> = Vec::new();
    let mut call_stack: Vec<(usize, usize)> = Vec::new();
    let mut cycles: Vec<Vec<usize>> = Vec::new();

    for start in 0..n {
        if indices[start] != usize::MAX || adj[start].is_empty() {
            continue;
        }
        indices[start] = index;
        lowlink[start] = index;
        index += 1;
        tarjan_stack.push(start);
        on_stack[start] = true;
        call_stack.push((start, 0));

        while let Some((v, next_i)) = call_stack.last_mut() {
            let v_node = *v;
            if *next_i < adj[v_node].len() {
                let w = adj[v_node][*next_i];
                *next_i += 1;
                if indices[w] == usize::MAX {
                    indices[w] = index;
                    lowlink[w] = index;
                    index += 1;
                    tarjan_stack.push(w);
                    on_stack[w] = true;
                    call_stack.push((w, 0));
                } else if on_stack[w] {
                    lowlink[v_node] = lowlink[v_node].min(indices[w]);
                }
            } else {
                call_stack.pop();
                if let Some(&(parent, _)) = call_stack.last() {
                    lowlink[parent] = lowlink[parent].min(lowlink[v_node]);
                }
                if lowlink[v_node] == indices[v_node] {
                    let mut comp = Vec::new();
                    while let Some(w) = tarjan_stack.pop() {
                        on_stack[w] = false;
                        comp.push(w);
                        if w == v_node {
                            break;
                        }
                    }
                    if comp.len() >= 2 {
                        comp.sort_unstable();
                        cycles.push(comp);
                    }
                }
            }
        }
    }

    let mut unique: BTreeSet<Vec<usize>> = BTreeSet::new();
    for c in cycles {
        unique.insert(c);
    }
    unique.into_iter().collect()
}

/// Extract a [`ClaimRecord`] from generic key-value frontmatter fields (used by `tb-app`).
#[must_use]
pub fn is_claim_relevant(
    kind: Option<&str>,
    has_polarity: bool,
    has_valid_window: bool,
    has_supersedes_or_conflicts: bool,
) -> bool {
    let kind_is_claim = kind.is_some_and(|k| {
        matches!(
            k.to_ascii_lowercase().as_str(),
            "claim" | "decision" | "adr" | "hypothesis" | "fact" | "principle" | "rule"
        )
    });
    kind_is_claim || has_polarity || has_valid_window || has_supersedes_or_conflicts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(path: &str) -> Locator {
        Locator::new("vault", path).unwrap()
    }

    #[test]
    fn polysingularity_preserves_both_poles_for_explicit_and_implicit_clashes() {
        let claims = vec![
            ClaimRecord {
                locator: loc("claims/a.md"),
                sha256: "aaa".into(),
                kind: Some("claim".into()),
                status: Some("active".into()),
                claim_key: Some("use-wal-mode".into()),
                polarity: Polarity::Positive,
                modality: Modality::Decision,
                valid_from: Some("2026-01-01".into()),
                valid_to: Some("2026-12-31".into()),
                observed_at: None,
                supersedes: vec![],
                conflicts_with: vec![],
                ambiguous_references: vec![],
                summary: Some("WAL improves read concurrency".into()),
            },
            ClaimRecord {
                locator: loc("claims/b.md"),
                sha256: "bbb".into(),
                kind: Some("claim".into()),
                status: Some("active".into()),
                claim_key: Some("use-wal-mode".into()),
                polarity: Polarity::Negative,
                modality: Modality::Decision,
                valid_from: Some("2026-02-01".into()),
                valid_to: None,
                observed_at: None,
                supersedes: vec![],
                conflicts_with: vec![],
                ambiguous_references: vec![],
                summary: Some("WAL fails on NFS mounts".into()),
            },
            ClaimRecord {
                locator: loc("claims/c.md"),
                sha256: "ccc".into(),
                kind: Some("claim".into()),
                status: Some("active".into()),
                claim_key: Some("old-claim".into()),
                polarity: Polarity::Positive,
                modality: Modality::Fact,
                valid_from: Some("2025-01-01".into()),
                valid_to: Some("2025-12-31".into()),
                observed_at: None,
                supersedes: vec![],
                conflicts_with: vec![],
                ambiguous_references: vec![],
                summary: Some("Expired fact still marked active".into()),
            },
        ];

        let report = audit_contradictions(&claims, Some("2026-06-01"), 50);
        assert_eq!(report.temporal_summary.active, 2);
        assert_eq!(report.temporal_summary.expired, 1);
        assert!(
            report.conflicts.iter().any(|c| {
                c.kind == ContradictionKind::ImplicitPolarityClash && c.poles.len() == 2
            })
        );
        assert!(report.conflicts.iter().any(|c| {
            c.kind == ContradictionKind::ExpiredActiveClaim
                && c.poles[0].locator.path == "claims/c.md"
        }));
    }

    #[test]
    fn supersession_resolves_polarity_clash_and_detects_cycles() {
        let resolved = vec![
            ClaimRecord {
                locator: loc("claims/v1.md"),
                sha256: "111".into(),
                kind: Some("decision".into()),
                status: Some("active".into()),
                claim_key: Some("storage".into()),
                polarity: Polarity::Positive,
                modality: Modality::Decision,
                valid_from: None,
                valid_to: None,
                observed_at: None,
                supersedes: vec![],
                conflicts_with: vec![],
                ambiguous_references: vec![],
                summary: None,
            },
            ClaimRecord {
                locator: loc("claims/v2.md"),
                sha256: "222".into(),
                kind: Some("decision".into()),
                status: Some("active".into()),
                claim_key: Some("storage".into()),
                polarity: Polarity::Negative,
                modality: Modality::Decision,
                valid_from: None,
                valid_to: None,
                observed_at: None,
                supersedes: vec![loc("claims/v1.md")],
                conflicts_with: vec![],
                ambiguous_references: vec![],
                summary: None,
            },
        ];
        let rep = audit_contradictions(&resolved, Some("2026-11-28"), 50);
        assert_eq!(rep.temporal_summary.superseded, 1);
        assert_eq!(rep.temporal_summary.active, 1);
        assert!(rep.conflicts.is_empty());

        // Cyclic supersession must be flagged.
        let mut cyclic = resolved;
        cyclic[0].supersedes.push(loc("claims/v2.md"));
        let cyc_rep = audit_contradictions(&cyclic, Some("2026-11-28"), 50);
        assert!(
            cyc_rep
                .conflicts
                .iter()
                .any(|c| c.kind == ContradictionKind::SupersessionAnomaly)
        );
    }

    #[test]
    fn conflict_id_is_order_independent_and_collision_free() {
        let a = Locator::new("r1", "a/b.md").unwrap();
        let b = Locator::new("r2", "c.md").unwrap();
        let id1 = conflict_id(
            ContradictionKind::ExplicitConflict,
            &[a.clone(), b.clone()],
            None,
        );
        let id2 = conflict_id(ContradictionKind::ExplicitConflict, &[b, a], None);
        assert_eq!(id1, id2);
        assert!(id1.starts_with("c_"));
    }
}
