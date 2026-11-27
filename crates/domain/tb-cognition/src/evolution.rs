//! Pure-domain Difference Graph (`G(T1) -> G(T2)`), Change-Driven Decay,
//! and Hub Fragility / Overloaded MOC Split Analysis (`0` I/O, Miri-clean).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use tb_contracts::{Locator, NodeId};
use tb_graph::TypedLinkGraph;

use crate::{analyze_topology, brandes_betweenness};

/// Per-node metadata + content digest needed for snapshot comparison and decay analysis.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotNodeMeta {
    pub locator: Locator,
    pub sha256: String,
    pub kind: Option<String>,
    pub status: Option<String>,
}

/// Quantitative topological shift between two graph snapshots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TopologyDelta {
    pub nodes_before: usize,
    pub nodes_after: usize,
    pub delta_nodes: isize,
    pub edges_before: usize,
    pub edges_after: usize,
    pub delta_edges: isize,
    pub density_before: f64,
    pub density_after: f64,
    pub delta_density: f64,
    pub bridges_before: usize,
    pub bridges_after: usize,
    pub delta_bridges: isize,
    pub articulations_before: usize,
    pub articulations_after: usize,
    pub delta_articulations: isize,
    pub modularity_before: f64,
    pub modularity_after: f64,
    pub delta_modularity: f64,
}

/// Note whose Brandes betweenness shifted meaningfully between snapshots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BetweennessMover {
    pub locator: Locator,
    pub kind: Option<String>,
    pub bc_before: f64,
    pub bc_after: f64,
    pub delta_bc: f64,
    pub role: String,
}

/// Note flagged for review by Change-Driven Decay (never triggered by age alone).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecayCandidate {
    pub locator: Locator,
    pub kind: Option<String>,
    pub durable_kind: bool,
    pub reason: String,
    pub changed_sources: Vec<Locator>,
    pub removed_sources: Vec<Locator>,
}

/// Edge diff sample pair.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EdgeDiffEntry {
    pub from: Locator,
    pub to: Locator,
}

/// Full structural & topological diff between two snapshots `G(T1) -> G(T2)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SnapshotDiffReport {
    pub added_nodes_count: usize,
    pub removed_nodes_count: usize,
    pub modified_nodes_count: usize,
    pub added_nodes: Vec<Locator>,
    pub removed_nodes: Vec<Locator>,
    pub modified_nodes: Vec<Locator>,
    pub added_edges_count: usize,
    pub removed_edges_count: usize,
    pub sample_added_edges: Vec<EdgeDiffEntry>,
    pub sample_removed_edges: Vec<EdgeDiffEntry>,
    pub topology_delta: TopologyDelta,
    pub top_movers: Vec<BetweennessMover>,
    pub decay_candidates: Vec<DecayCandidate>,
    pub durable_protected_count: usize,
}

/// Durable knowledge types that must NEVER decay on calendar age alone.
fn is_durable_kind(kind: Option<&str>) -> bool {
    kind.is_some_and(|k| {
        matches!(
            k.to_ascii_lowercase().as_str(),
            "principle" | "pattern" | "moc" | "guide" | "rule" | "mental-model" | "invariant"
        )
    })
}

/// Compute a pure-domain structural + topological diff between two graph snapshots,
/// including emerging/fading gateways and change-driven decay candidates.
#[must_use]
pub fn diff_snapshots(
    before_graph: &TypedLinkGraph,
    before_meta: &[SnapshotNodeMeta],
    after_graph: &TypedLinkGraph,
    after_meta: &[SnapshotNodeMeta],
    limit: usize,
) -> SnapshotDiffReport {
    let limit = limit.clamp(1, 200);

    let before_map: BTreeMap<&Locator, &SnapshotNodeMeta> =
        before_meta.iter().map(|m| (&m.locator, m)).collect();
    let after_map: BTreeMap<&Locator, &SnapshotNodeMeta> =
        after_meta.iter().map(|m| (&m.locator, m)).collect();

    let mut added_nodes = Vec::new();
    let mut removed_nodes = Vec::new();
    let mut modified_nodes = Vec::new();
    let mut modified_set: BTreeSet<Locator> = BTreeSet::new();
    let mut removed_set: BTreeSet<Locator> = BTreeSet::new();

    for (&loc, meta) in &after_map {
        match before_map.get(loc) {
            None => added_nodes.push(loc.clone()),
            Some(prev) => {
                if prev.sha256 != meta.sha256 {
                    modified_nodes.push(loc.clone());
                    modified_set.insert(loc.clone());
                }
            }
        }
    }
    for &loc in before_map.keys() {
        if !after_map.contains_key(loc) {
            removed_nodes.push(loc.clone());
            removed_set.insert(loc.clone());
        }
    }

    // Collect canonical directed edge sets `(from_locator, to_locator)`.
    let before_edges = collect_edges(before_graph);
    let after_edges = collect_edges(after_graph);

    let mut added_edges: Vec<EdgeDiffEntry> = after_edges
        .difference(&before_edges)
        .map(|(f, t)| EdgeDiffEntry {
            from: f.clone(),
            to: t.clone(),
        })
        .collect();
    let mut removed_edges: Vec<EdgeDiffEntry> = before_edges
        .difference(&after_edges)
        .map(|(f, t)| EdgeDiffEntry {
            from: f.clone(),
            to: t.clone(),
        })
        .collect();
    let added_edges_count = added_edges.len();
    let removed_edges_count = removed_edges.len();
    added_edges.truncate(limit);
    removed_edges.truncate(limit);

    // Topology metrics + modularity delta (lightweight: peel_top_k=0).
    let before_shape = before_graph.shape();
    let after_shape = after_graph.shape();
    let before_health = before_graph.health();
    let after_health = after_graph.health();
    let before_topo = analyze_topology(before_graph, 0, 5);
    let after_topo = analyze_topology(after_graph, 0, 5);

    let topology_delta = TopologyDelta {
        nodes_before: before_shape.node_count,
        nodes_after: after_shape.node_count,
        delta_nodes: after_shape.node_count as isize - before_shape.node_count as isize,
        edges_before: before_shape.edge_count,
        edges_after: after_shape.edge_count,
        delta_edges: after_shape.edge_count as isize - before_shape.edge_count as isize,
        density_before: before_shape.density,
        density_after: after_shape.density,
        delta_density: after_shape.density - before_shape.density,
        bridges_before: before_health.bridges.len(),
        bridges_after: after_health.bridges.len(),
        delta_bridges: after_health.bridges.len() as isize - before_health.bridges.len() as isize,
        articulations_before: before_health.articulation_points.len(),
        articulations_after: after_health.articulation_points.len(),
        delta_articulations: after_health.articulation_points.len() as isize
            - before_health.articulation_points.len() as isize,
        modularity_before: before_topo.communities.modularity,
        modularity_after: after_topo.communities.modularity,
        delta_modularity: after_topo.communities.modularity - before_topo.communities.modularity,
    };

    // Betweenness movers across shared nodes.
    let bc_before = brandes_betweenness(before_graph, None);
    let bc_after = brandes_betweenness(after_graph, None);
    let mut bc_before_by_loc: HashMap<&Locator, f64> = HashMap::new();
    for (i, &val) in bc_before.iter().enumerate() {
        #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
        if let Some(loc) = before_graph.locator(NodeId(i as u32)) {
            bc_before_by_loc.insert(loc, val);
        }
    }
    let mut movers: Vec<BetweennessMover> = Vec::new();
    for (i, &cur) in bc_after.iter().enumerate() {
        #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
        let Some(loc) = after_graph.locator(NodeId(i as u32)) else {
            continue;
        };
        let prev = bc_before_by_loc.get(loc).copied().unwrap_or(0.0);
        let delta = cur - prev;
        if delta.abs() > 1e-6 {
            let role = if delta > 0.0 {
                "emerging_gateway"
            } else {
                "fading_hub"
            };
            movers.push(BetweennessMover {
                locator: loc.clone(),
                kind: after_map.get(loc).and_then(|m| m.kind.clone()),
                bc_before: prev,
                bc_after: cur,
                delta_bc: delta,
                role: role.to_owned(),
            });
        }
    }
    movers.sort_by(|a, b| {
        b.delta_bc
            .abs()
            .total_cmp(&a.delta_bc.abs())
            .then_with(|| a.locator.cmp(&b.locator))
    });
    movers.truncate(limit);

    // Change-Driven Decay: inspect unchanged notes in `after` whose outbound sources
    // (links present in `before` or `after`) were modified or removed.
    let mut decay_candidates: Vec<DecayCandidate> = Vec::new();
    let mut durable_protected_count = 0usize;

    for (loc, meta) in &after_map {
        let durable = is_durable_kind(meta.kind.as_deref());
        // Skip notes that were themselves just updated or newly created.
        if modified_set.contains(*loc) || !before_map.contains_key(*loc) {
            continue;
        }
        let mut changed_sources = BTreeSet::new();
        let mut removed_sources = BTreeSet::new();

        // Check outbound links in `after` that point to modified targets.
        if let Some(id) = after_graph.node_id(loc) {
            for &dst in after_graph.out_neighbors(id).0 {
                if let Some(dst_loc) = after_graph.locator(dst)
                    && modified_set.contains(dst_loc)
                {
                    changed_sources.insert(dst_loc.clone());
                }
            }
        }
        // Check outbound links that existed in `before` and pointed to now-removed targets.
        if let Some(prev_id) = before_graph.node_id(loc) {
            for &dst in before_graph.out_neighbors(prev_id).0 {
                if let Some(dst_loc) = before_graph.locator(dst)
                    && removed_set.contains(dst_loc)
                {
                    removed_sources.insert(dst_loc.clone());
                }
            }
        }

        if changed_sources.is_empty() && removed_sources.is_empty() {
            if durable {
                durable_protected_count += 1;
            }
            continue;
        }

        let reason = match (changed_sources.len(), removed_sources.len()) {
            (c, 0) => format!("{c} referenced source note(s) changed since baseline"),
            (0, r) => format!("{r} referenced source note(s) were removed since baseline"),
            (c, r) => {
                format!("{c} referenced source(s) changed and {r} were removed since baseline")
            }
        };
        decay_candidates.push(DecayCandidate {
            locator: (*loc).clone(),
            kind: meta.kind.clone(),
            durable_kind: durable,
            reason,
            changed_sources: changed_sources.into_iter().collect(),
            removed_sources: removed_sources.into_iter().collect(),
        });
    }
    decay_candidates.sort_by(|a, b| {
        let score_a = a.removed_sources.len() * 2 + a.changed_sources.len();
        let score_b = b.removed_sources.len() * 2 + b.changed_sources.len();
        score_b
            .cmp(&score_a)
            .then_with(|| a.locator.cmp(&b.locator))
    });
    decay_candidates.truncate(limit);

    let added_nodes_count = added_nodes.len();
    let removed_nodes_count = removed_nodes.len();
    let modified_nodes_count = modified_nodes.len();
    added_nodes.truncate(limit);
    removed_nodes.truncate(limit);
    modified_nodes.truncate(limit);

    SnapshotDiffReport {
        added_nodes_count,
        removed_nodes_count,
        modified_nodes_count,
        added_nodes,
        removed_nodes,
        modified_nodes,
        added_edges_count,
        removed_edges_count,
        sample_added_edges: added_edges,
        sample_removed_edges: removed_edges,
        topology_delta,
        top_movers: movers,
        decay_candidates,
        durable_protected_count,
    }
}

fn collect_edges(graph: &TypedLinkGraph) -> BTreeSet<(Locator, Locator)> {
    let mut set = BTreeSet::new();
    for i in 0..graph.node_count() {
        #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
        let src_id = NodeId(i as u32);
        let Some(src_loc) = graph.locator(src_id) else {
            continue;
        };
        for &dst_id in graph.out_neighbors(src_id).0 {
            if let Some(dst_loc) = graph.locator(dst_id) {
                set.insert((src_loc.clone(), dst_loc.clone()));
            }
        }
    }
    set
}

// === Hub Fragility & Overloaded MOC Split Analysis ===

/// One fragile articulation hub and its exact disconnection impact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HubFragilityEntry {
    pub locator: Locator,
    pub kind: Option<String>,
    pub degree: usize,
    pub betweenness: f64,
    /// Number of connected sub-fragments created within this hub's component if removed.
    pub fragmentation_components: usize,
    /// Total nodes separated from the main branch if this hub is removed.
    pub detached_nodes: usize,
    /// Size of the largest detached island (excluding the largest remaining main branch).
    pub largest_detached_island: usize,
    pub recommended_action: String,
    /// Representative anchor notes from detached islands (candidate satellites).
    pub satellite_hints: Vec<Locator>,
}

/// Complete Hub Fragility report over articulation points and bridge edges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FragilityReport {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub articulation_point_count: usize,
    pub bridge_edge_count: usize,
    pub overloaded_hub_count: usize,
    pub fragile_hubs: Vec<HubFragilityEntry>,
}

/// Analyze structural hub fragility: find articulation points, measure exact
/// fragmentation impact upon removal, and recommend `map + satellite` splits
/// for overloaded MOCs/hubs.
#[must_use]
pub fn analyze_fragility(
    graph: &TypedLinkGraph,
    kind_by_locator: &BTreeMap<Locator, Option<String>>,
    limit: usize,
) -> FragilityReport {
    let limit = limit.clamp(1, 200);
    let shape = graph.shape();
    let health = graph.health();
    let bc = brandes_betweenness(graph, None);
    let n = graph.node_count();

    let mut hubs: Vec<HubFragilityEntry> = Vec::new();
    let mut visited = vec![0u32; n];
    let mut epoch = 0u32;
    let mut queue = VecDeque::new();

    for art_loc in &health.articulation_points {
        let Some(art_id) = graph.node_id(art_loc) else {
            continue;
        };
        let u = art_id.0 as usize;
        let neighbors = graph.undirected_neighbors(art_id);
        let degree = neighbors.len();
        if degree < 2 {
            continue;
        }

        epoch = epoch.wrapping_add(1);
        if epoch == 0 {
            visited.fill(0);
            epoch = 1;
        }
        visited[u] = epoch;

        // Explore connected components reachable from u's neighbors without passing through u.
        let mut fragments: Vec<(usize, usize)> = Vec::new(); // (size, representative_node)
        for &nb in neighbors {
            let nb_i = nb.0 as usize;
            if visited[nb_i] == epoch {
                continue;
            }
            visited[nb_i] = epoch;
            queue.clear();
            queue.push_back(nb_i);
            let mut size = 0usize;
            let mut best_rep = nb_i;
            let mut best_deg = graph.undirected_neighbors(nb).len();
            while let Some(cur) = queue.pop_front() {
                size += 1;
                #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
                let cur_id = NodeId(cur as u32);
                let cur_deg = graph.undirected_neighbors(cur_id).len();
                if cur_deg > best_deg || (cur_deg == best_deg && cur < best_rep) {
                    best_deg = cur_deg;
                    best_rep = cur;
                }
                for &nxt in graph.undirected_neighbors(cur_id) {
                    let nxt_i = nxt.0 as usize;
                    if visited[nxt_i] != epoch {
                        visited[nxt_i] = epoch;
                        queue.push_back(nxt_i);
                    }
                }
            }
            fragments.push((size, best_rep));
        }

        if fragments.len() < 2 {
            continue;
        }
        fragments.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let detached_fragments = &fragments[1..];
        let detached_nodes: usize = detached_fragments.iter().map(|(s, _)| *s).sum();
        let largest_detached_island = detached_fragments.first().map_or(0, |(s, _)| *s);
        let satellite_hints: Vec<Locator> = detached_fragments
            .iter()
            .take(4)
            .filter_map(|&(_, rep)| {
                #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
                graph.locator(NodeId(rep as u32)).cloned()
            })
            .collect();

        let recommended_action = if degree >= 6 && detached_nodes >= 2 {
            "split_into_map_and_satellites"
        } else if detached_nodes >= 2 {
            "add_redundant_cross_links"
        } else {
            "monitor_single_point_of_failure"
        };

        hubs.push(HubFragilityEntry {
            locator: art_loc.clone(),
            kind: kind_by_locator.get(art_loc).cloned().flatten(),
            degree,
            betweenness: bc.get(u).copied().unwrap_or(0.0),
            fragmentation_components: fragments.len(),
            detached_nodes,
            largest_detached_island,
            recommended_action: recommended_action.to_owned(),
            satellite_hints,
        });
    }

    hubs.sort_by(|a, b| {
        b.detached_nodes
            .cmp(&a.detached_nodes)
            .then_with(|| b.fragmentation_components.cmp(&a.fragmentation_components))
            .then_with(|| b.betweenness.total_cmp(&a.betweenness))
            .then_with(|| a.locator.cmp(&b.locator))
    });

    let overloaded_hub_count = hubs
        .iter()
        .filter(|h| h.recommended_action == "split_into_map_and_satellites")
        .count();
    hubs.truncate(limit);

    FragilityReport {
        total_nodes: shape.node_count,
        total_edges: shape.edge_count,
        articulation_point_count: health.articulation_points.len(),
        bridge_edge_count: health.bridges.len(),
        overloaded_hub_count,
        fragile_hubs: hubs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tb_syntax::EdgeKind;

    fn loc(path: &str) -> Locator {
        Locator::new("vault", path).unwrap()
    }

    #[test]
    fn diff_snapshots_detects_change_driven_decay_and_protects_unchanged_durable_notes() {
        let g1 = TypedLinkGraph::build(
            vec![loc("source.md"), loc("derived.md"), loc("principle.md")],
            &[(loc("derived.md"), loc("source.md"), EdgeKind::Navigational)],
        );
        let m1 = vec![
            SnapshotNodeMeta {
                locator: loc("source.md"),
                sha256: "v1".into(),
                kind: Some("note".into()),
                status: None,
            },
            SnapshotNodeMeta {
                locator: loc("derived.md"),
                sha256: "d1".into(),
                kind: Some("note".into()),
                status: None,
            },
            SnapshotNodeMeta {
                locator: loc("principle.md"),
                sha256: "p1".into(),
                kind: Some("principle".into()),
                status: None,
            },
        ];

        // Now source.md changes from v1 -> v2; derived.md and principle.md are unchanged.
        let g2 = TypedLinkGraph::build(
            vec![
                loc("source.md"),
                loc("derived.md"),
                loc("principle.md"),
                loc("new.md"),
            ],
            &[
                (loc("derived.md"), loc("source.md"), EdgeKind::Navigational),
                (loc("new.md"), loc("source.md"), EdgeKind::Navigational),
            ],
        );
        let m2 = vec![
            SnapshotNodeMeta {
                locator: loc("source.md"),
                sha256: "v2".into(),
                kind: Some("note".into()),
                status: None,
            },
            SnapshotNodeMeta {
                locator: loc("derived.md"),
                sha256: "d1".into(),
                kind: Some("note".into()),
                status: None,
            },
            SnapshotNodeMeta {
                locator: loc("principle.md"),
                sha256: "p1".into(),
                kind: Some("principle".into()),
                status: None,
            },
            SnapshotNodeMeta {
                locator: loc("new.md"),
                sha256: "n1".into(),
                kind: Some("note".into()),
                status: None,
            },
        ];

        let diff = diff_snapshots(&g1, &m1, &g2, &m2, 20);
        assert_eq!(diff.added_nodes_count, 1);
        assert_eq!(diff.modified_nodes_count, 1);
        assert_eq!(diff.durable_protected_count, 1);
        assert_eq!(diff.decay_candidates.len(), 1);
        assert_eq!(diff.decay_candidates[0].locator.path, "derived.md");
        assert_eq!(diff.decay_candidates[0].changed_sources.len(), 1);
    }

    #[test]
    fn fragility_flags_overloaded_star_hub_for_map_and_satellite_split() {
        let hub = loc("moc/hub.md");
        let mut nodes = vec![hub.clone()];
        let mut edges = Vec::new();
        // 3 separate triangles joined only at `hub` -> degree 6, 3 fragments, 4 detached nodes!
        for cluster in 0..3 {
            let a = loc(&format!("c{cluster}/a.md"));
            let c = loc(&format!("c{cluster}/b.md"));
            nodes.push(a.clone());
            nodes.push(c.clone());
            edges.push((hub.clone(), a.clone(), EdgeKind::Navigational));
            edges.push((hub.clone(), c.clone(), EdgeKind::Navigational));
            edges.push((a, c, EdgeKind::Navigational));
        }
        let g = TypedLinkGraph::build(nodes, &edges);
        let mut kinds = BTreeMap::new();
        kinds.insert(hub, Some("moc".to_owned()));

        let rep = analyze_fragility(&g, &kinds, 10);
        assert_eq!(rep.overloaded_hub_count, 1);
        assert_eq!(rep.fragile_hubs[0].locator.path, "moc/hub.md");
        assert_eq!(rep.fragile_hubs[0].fragmentation_components, 3);
        assert_eq!(rep.fragile_hubs[0].detached_nodes, 4);
        assert_eq!(
            rep.fragile_hubs[0].recommended_action,
            "split_into_map_and_satellites"
        );
        assert_eq!(rep.fragile_hubs[0].satellite_hints.len(), 2);
    }
}
