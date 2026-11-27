//! Data-oriented `u32` Compressed Sparse Row (CSR) knowledge graph (0 I/O, Miri-clean).
//!
//! Keeps graph memory minimal (< 2 MB for 10k nodes / 100k edges) and traversal
//! cache-local for `tb_overview`, `tb_hubs`, `tb_graph_health`, `tb_neighborhood`,
//! and downstream `tb-cognition` algorithms (Brandes, Leiden, Peel-the-Onion).

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use tb_contracts::{Locator, NodeId};
use tb_syntax::EdgeKind;

/// Forward and reverse CSR adjacency arrays over dense `NodeId(0..N)`.
#[derive(Clone, Debug, Default)]
pub struct CsrAdjacency {
    offsets: Vec<u32>,
    targets: Vec<NodeId>,
    kinds: Vec<EdgeKind>,
}

impl CsrAdjacency {
    #[must_use]
    pub fn build(node_count: usize, mut edges: Vec<(NodeId, NodeId, EdgeKind)>) -> Self {
        edges.sort_unstable_by_key(|&(u, v, k)| (u.0, v.0, k));
        let mut offsets = vec![0u32; node_count + 1];
        for &(u, _, _) in &edges {
            let idx = u.0 as usize;
            if idx < node_count {
                offsets[idx + 1] += 1;
            }
        }
        for i in 1..=node_count {
            offsets[i] += offsets[i - 1];
        }
        let mut targets = Vec::with_capacity(edges.len());
        let mut kinds = Vec::with_capacity(edges.len());
        for &(_, v, k) in &edges {
            targets.push(v);
            kinds.push(k);
        }
        Self {
            offsets,
            targets,
            kinds,
        }
    }

    #[must_use]
    pub fn neighbors(&self, node: NodeId) -> (&[NodeId], &[EdgeKind]) {
        let idx = node.0 as usize;
        if idx + 1 >= self.offsets.len() {
            return (&[], &[]);
        }
        let start = self.offsets[idx] as usize;
        let end = self.offsets[idx + 1] as usize;
        (&self.targets[start..end], &self.kinds[start..end])
    }

    #[must_use]
    pub fn degree(&self, node: NodeId) -> usize {
        self.neighbors(node).0.len()
    }

    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.targets.len()
    }
}

/// Immutable, lock-free-shareable typed link graph (`KTD6`: structural vs navigational).
#[derive(Clone, Debug)]
pub struct TypedLinkGraph {
    nodes: Vec<Locator>,
    index: HashMap<Locator, NodeId>,
    forward: CsrAdjacency,
    reverse: CsrAdjacency,
    /// Deduplicated, sorted undirected adjacency for fast triangle / Tarjan / Leiden passes.
    undirected_offsets: Vec<u32>,
    undirected_targets: Vec<NodeId>,
    edges_structural: usize,
    edges_navigational: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HubRow {
    pub locator: Locator,
    pub inbound_total: usize,
    pub inbound_structural: usize,
    pub inbound_navigational: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphShape {
    pub node_count: usize,
    pub edge_count: usize,
    pub edges_structural: usize,
    pub edges_navigational: usize,
    pub components: usize,
    pub largest_component: usize,
    pub isolated: usize,
    pub no_inbound: usize,
    pub density: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphHealth {
    pub directed_cycles: usize,
    pub dead_ends: usize,
    pub sources: usize,
    pub bridges: Vec<(Locator, Locator)>,
    pub articulation_points: Vec<Locator>,
    pub triangles: u64,
    pub transitivity: f64,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Direction {
    Out,
    In,
    Both,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NeighborhoodResult {
    pub nodes: Vec<Locator>,
    pub truncated: bool,
}

impl TypedLinkGraph {
    /// Build a canonical CSR graph from a list of node locators and directed typed edges.
    #[must_use]
    pub fn build(mut locators: Vec<Locator>, raw_edges: &[(Locator, Locator, EdgeKind)]) -> Self {
        locators.sort();
        locators.dedup();
        let mut index = HashMap::with_capacity(locators.len());
        for (i, loc) in locators.iter().enumerate() {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "vault node count fits in u32"
            )]
            index.insert(loc.clone(), NodeId(i as u32));
        }

        let mut fwd_edges = Vec::with_capacity(raw_edges.len());
        let mut rev_edges = Vec::with_capacity(raw_edges.len());
        let mut undir_pairs = Vec::with_capacity(raw_edges.len() * 2);
        let mut edges_structural = 0usize;
        let mut edges_navigational = 0usize;

        for (src, dst, kind) in raw_edges {
            if let (Some(&u), Some(&v)) = (index.get(src), index.get(dst)) {
                fwd_edges.push((u, v, *kind));
                rev_edges.push((v, u, *kind));
                if kind.is_structural() {
                    edges_structural += 1;
                } else {
                    edges_navigational += 1;
                }
                if u != v {
                    undir_pairs.push((u, v));
                    undir_pairs.push((v, u));
                }
            }
        }

        let n = locators.len();
        let forward = CsrAdjacency::build(n, fwd_edges);
        let reverse = CsrAdjacency::build(n, rev_edges);

        undir_pairs.sort_unstable_by_key(|&(u, v)| (u.0, v.0));
        undir_pairs.dedup();
        let mut undirected_offsets = vec![0u32; n + 1];
        for &(u, _) in &undir_pairs {
            undirected_offsets[u.0 as usize + 1] += 1;
        }
        for i in 1..=n {
            undirected_offsets[i] += undirected_offsets[i - 1];
        }
        let undirected_targets = undir_pairs.into_iter().map(|(_, v)| v).collect();

        Self {
            nodes: locators,
            index,
            forward,
            reverse,
            undirected_offsets,
            undirected_targets,
            edges_structural,
            edges_navigational,
        }
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.forward.edge_count()
    }

    #[must_use]
    pub fn node_id(&self, locator: &Locator) -> Option<NodeId> {
        self.index.get(locator).copied()
    }

    #[must_use]
    pub fn locator(&self, id: NodeId) -> Option<&Locator> {
        self.nodes.get(id.0 as usize)
    }

    #[must_use]
    pub fn out_neighbors(&self, id: NodeId) -> (&[NodeId], &[EdgeKind]) {
        self.forward.neighbors(id)
    }

    #[must_use]
    pub fn in_neighbors(&self, id: NodeId) -> (&[NodeId], &[EdgeKind]) {
        self.reverse.neighbors(id)
    }

    #[must_use]
    pub fn undirected_neighbors(&self, id: NodeId) -> &[NodeId] {
        let idx = id.0 as usize;
        if idx + 1 >= self.undirected_offsets.len() {
            return &[];
        }
        let start = self.undirected_offsets[idx] as usize;
        let end = self.undirected_offsets[idx + 1] as usize;
        &self.undirected_targets[start..end]
    }

    /// Compute `tb_overview` / `graph_shape` statistics in O(V + E).
    #[must_use]
    pub fn shape(&self) -> GraphShape {
        let n = self.nodes.len();
        let mut visited = vec![false; n];
        let mut queue = VecDeque::new();
        let mut components = 0usize;
        let mut largest_component = 0usize;

        for start in 0..n {
            if visited[start] {
                continue;
            }
            components += 1;
            visited[start] = true;
            #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
            queue.push_back(NodeId(start as u32));
            let mut size = 0usize;
            while let Some(u) = queue.pop_front() {
                size += 1;
                for &v in self.undirected_neighbors(u) {
                    let vi = v.0 as usize;
                    if !visited[vi] {
                        visited[vi] = true;
                        queue.push_back(v);
                    }
                }
            }
            largest_component = largest_component.max(size);
        }

        let mut isolated = 0usize;
        let mut no_inbound = 0usize;
        for i in 0..n {
            #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
            let id = NodeId(i as u32);
            let in_deg = self.reverse.degree(id);
            let out_deg = self.forward.degree(id);
            if in_deg == 0 {
                no_inbound += 1;
                if out_deg == 0 {
                    isolated += 1;
                }
            }
        }

        let density = if n == 0 {
            0.0
        } else {
            self.edge_count() as f64 / n as f64
        };

        GraphShape {
            node_count: n,
            edge_count: self.edge_count(),
            edges_structural: self.edges_structural,
            edges_navigational: self.edges_navigational,
            components,
            largest_component,
            isolated,
            no_inbound,
            density,
        }
    }

    /// Top `limit` hubs ranked by total inbound edges (split structural vs navigational).
    #[must_use]
    pub fn hubs(&self, limit: usize) -> Vec<HubRow> {
        let mut rows = Vec::with_capacity(self.nodes.len());
        for (i, loc) in self.nodes.iter().enumerate() {
            #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
            let id = NodeId(i as u32);
            let (_, kinds) = self.reverse.neighbors(id);
            if kinds.is_empty() {
                continue;
            }
            let structural = kinds.iter().filter(|k| k.is_structural()).count();
            let navigational = kinds.len() - structural;
            rows.push(HubRow {
                locator: loc.clone(),
                inbound_total: kinds.len(),
                inbound_structural: structural,
                inbound_navigational: navigational,
            });
        }
        rows.sort_by(|a, b| {
            b.inbound_total
                .cmp(&a.inbound_total)
                .then_with(|| a.locator.cmp(&b.locator))
        });
        rows.truncate(limit);
        rows
    }

    /// Compute directed cycles (non-trivial SCCs), dead ends, sources, bridges,
    /// articulation points, triangle count, and global transitivity.
    #[must_use]
    pub fn health(&self) -> GraphHealth {
        let n = self.nodes.len();
        let mut dead_ends = 0usize;
        let mut sources = 0usize;
        for i in 0..n {
            #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
            let id = NodeId(i as u32);
            let in_d = self.reverse.degree(id);
            let out_d = self.forward.degree(id);
            if in_d > 0 && out_d == 0 {
                dead_ends += 1;
            }
            if out_d > 0 && in_d == 0 {
                sources += 1;
            }
        }

        let directed_cycles = self.count_cyclic_sccs();
        let (bridges, articulation_points) = self.bridges_and_articulation_points();
        let (triangles, transitivity) = self.triangles_and_transitivity();

        GraphHealth {
            directed_cycles,
            dead_ends,
            sources,
            bridges,
            articulation_points,
            triangles,
            transitivity,
        }
    }

    /// Bounded N-hop BFS neighborhood traversal with deterministic order and truncation flag.
    #[must_use]
    pub fn neighborhood(
        &self,
        start: &Locator,
        max_depth: usize,
        direction: Direction,
        max_nodes: usize,
    ) -> Option<NeighborhoodResult> {
        let start_id = self.node_id(start)?;
        let mut visited = vec![false; self.nodes.len()];
        let mut queue = VecDeque::new();
        visited[start_id.0 as usize] = true;
        queue.push_back((start_id, 0usize));

        let mut out = Vec::new();
        let mut truncated = false;

        while let Some((u, depth)) = queue.pop_front() {
            if out.len() >= max_nodes {
                truncated = true;
                break;
            }
            out.push(self.nodes[u.0 as usize].clone());
            if depth >= max_depth {
                continue;
            }
            let mut next_ids: Vec<NodeId> = Vec::new();
            match direction {
                Direction::Out => next_ids.extend_from_slice(self.forward.neighbors(u).0),
                Direction::In => next_ids.extend_from_slice(self.reverse.neighbors(u).0),
                Direction::Both => next_ids.extend_from_slice(self.undirected_neighbors(u)),
            }
            next_ids.sort_unstable();
            next_ids.dedup();
            for v in next_ids {
                let vi = v.0 as usize;
                if !visited[vi] {
                    visited[vi] = true;
                    queue.push_back((v, depth + 1));
                }
            }
        }

        Some(NeighborhoodResult {
            nodes: out,
            truncated,
        })
    }

    /// Iterative Tarjan SCC count of non-trivial cycles (size >= 2 or self-loop).
    fn count_cyclic_sccs(&self) -> usize {
        let n = self.nodes.len();
        let mut index = vec![u32::MAX; n];
        let mut low = vec![0u32; n];
        let mut on_stack = vec![false; n];
        let mut stack = Vec::new();
        let mut next_idx = 0u32;
        let mut cyclic = 0usize;

        for start in 0..n {
            if index[start] != u32::MAX {
                continue;
            }
            let mut call_stack: Vec<(usize, usize)> = vec![(start, 0)];
            index[start] = next_idx;
            low[start] = next_idx;
            next_idx += 1;
            stack.push(start);
            on_stack[start] = true;

            while let Some((u, edge_pos)) = call_stack.last_mut() {
                let u_val = *u;
                #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
                let nbrs = self.forward.neighbors(NodeId(u_val as u32)).0;
                if *edge_pos < nbrs.len() {
                    let v = nbrs[*edge_pos].0 as usize;
                    *edge_pos += 1;
                    if index[v] == u32::MAX {
                        index[v] = next_idx;
                        low[v] = next_idx;
                        next_idx += 1;
                        stack.push(v);
                        on_stack[v] = true;
                        call_stack.push((v, 0));
                    } else if on_stack[v] {
                        low[u_val] = low[u_val].min(index[v]);
                    }
                } else {
                    call_stack.pop();
                    if let Some(&(parent, _)) = call_stack.last() {
                        low[parent] = low[parent].min(low[u_val]);
                    }
                    if low[u_val] == index[u_val] {
                        let mut comp_size = 0usize;
                        let mut has_self_loop = false;
                        while let Some(w) = stack.pop() {
                            on_stack[w] = false;
                            comp_size += 1;
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "node index fits u32"
                            )]
                            if self
                                .forward
                                .neighbors(NodeId(w as u32))
                                .0
                                .contains(&NodeId(w as u32))
                            {
                                has_self_loop = true;
                            }
                            if w == u_val {
                                break;
                            }
                        }
                        if comp_size > 1 || has_self_loop {
                            cyclic += 1;
                        }
                    }
                }
            }
        }
        cyclic
    }

    /// Iterative Tarjan bridges and articulation points on the undirected CSR graph.
    fn bridges_and_articulation_points(&self) -> (Vec<(Locator, Locator)>, Vec<Locator>) {
        let n = self.nodes.len();
        let mut disc = vec![u32::MAX; n];
        let mut low = vec![0u32; n];
        let mut parent = vec![usize::MAX; n];
        let mut is_ap = vec![false; n];
        let mut root_children = vec![0usize; n];
        let mut bridges = Vec::new();
        let mut timer = 0u32;

        for start in 0..n {
            if disc[start] != u32::MAX {
                continue;
            }
            disc[start] = timer;
            low[start] = timer;
            timer += 1;
            let mut dfs: Vec<(usize, usize)> = vec![(start, 0)];

            while let Some((u, pos)) = dfs.last_mut() {
                let u_val = *u;
                #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
                let nbrs = self.undirected_neighbors(NodeId(u_val as u32));
                if *pos < nbrs.len() {
                    let v = nbrs[*pos].0 as usize;
                    *pos += 1;
                    if disc[v] == u32::MAX {
                        parent[v] = u_val;
                        if parent[u_val] == usize::MAX {
                            root_children[u_val] += 1;
                        }
                        disc[v] = timer;
                        low[v] = timer;
                        timer += 1;
                        dfs.push((v, 0));
                    } else if v != parent[u_val] {
                        low[u_val] = low[u_val].min(disc[v]);
                    }
                } else {
                    dfs.pop();
                    let p = parent[u_val];
                    if p != usize::MAX {
                        low[p] = low[p].min(low[u_val]);
                        if low[u_val] > disc[p] {
                            let (a, b) = if p < u_val { (p, u_val) } else { (u_val, p) };
                            bridges.push((self.nodes[a].clone(), self.nodes[b].clone()));
                        }
                        if parent[p] != usize::MAX && low[u_val] >= disc[p] {
                            is_ap[p] = true;
                        }
                    } else if root_children[u_val] > 1 {
                        is_ap[u_val] = true;
                    }
                }
            }
        }

        bridges.sort();
        let aps = is_ap
            .into_iter()
            .enumerate()
            .filter(|&(_, flag)| flag)
            .map(|(i, _)| self.nodes[i].clone())
            .collect();
        (bridges, aps)
    }

    /// Exact triangle count via sorted-CSR intersection (counts each triangle once for u < v < w).
    fn triangles_and_transitivity(&self) -> (u64, f64) {
        let n = self.nodes.len();
        let mut triangles = 0u64;
        let mut wedges = 0u64;

        for u in 0..n {
            #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
            let u_id = NodeId(u as u32);
            let u_nbrs = self.undirected_neighbors(u_id);
            let deg = u_nbrs.len() as u64;
            if deg >= 2 {
                wedges = wedges.saturating_add(deg * (deg - 1) / 2);
            }
            for &v_id in u_nbrs {
                if v_id.0 <= u_id.0 {
                    continue;
                }
                let v_nbrs = self.undirected_neighbors(v_id);
                // Two-pointer sorted intersection counting w > v_id.
                let mut i = u_nbrs.partition_point(|x| x.0 <= v_id.0);
                let mut j = v_nbrs.partition_point(|x| x.0 <= v_id.0);
                while i < u_nbrs.len() && j < v_nbrs.len() {
                    match u_nbrs[i].0.cmp(&v_nbrs[j].0) {
                        std::cmp::Ordering::Equal => {
                            triangles += 1;
                            i += 1;
                            j += 1;
                        }
                        std::cmp::Ordering::Less => i += 1,
                        std::cmp::Ordering::Greater => j += 1,
                    }
                }
            }
        }

        let transitivity = if wedges == 0 {
            0.0
        } else {
            (3.0 * triangles as f64) / (wedges as f64)
        };
        (triangles, transitivity)
    }

    /// Degree histogram `(degree -> count)` for inbound or outbound directed edges.
    #[must_use]
    pub fn degree_histogram(&self, inbound: bool) -> BTreeMap<usize, usize> {
        let mut hist = BTreeMap::new();
        let adj = if inbound {
            &self.reverse
        } else {
            &self.forward
        };
        for i in 0..self.nodes.len() {
            #[expect(clippy::cast_possible_truncation, reason = "node index fits u32")]
            let d = adj.degree(NodeId(i as u32));
            *hist.entry(d).or_insert(0) += 1;
        }
        hist
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(name: &str) -> Locator {
        Locator::new("vault", name).unwrap()
    }

    #[test]
    fn planted_two_triangles_bridge_and_orphan_metrics_are_exact() {
        // Two triangles {a,b,c} and {d,e,f} joined by bridge c->d, plus isolated node z.
        let nodes = ["a", "b", "c", "d", "e", "f", "z"]
            .into_iter()
            .map(loc)
            .collect();
        let edges = vec![
            (loc("a"), loc("b"), EdgeKind::Navigational),
            (loc("b"), loc("c"), EdgeKind::Navigational),
            (loc("c"), loc("a"), EdgeKind::Navigational),
            (loc("c"), loc("d"), EdgeKind::Field),
            (loc("d"), loc("e"), EdgeKind::Navigational),
            (loc("e"), loc("f"), EdgeKind::Navigational),
            (loc("f"), loc("d"), EdgeKind::Navigational),
        ];
        let g = TypedLinkGraph::build(nodes, &edges);

        let shape = g.shape();
        assert_eq!(shape.node_count, 7);
        assert_eq!(shape.edge_count, 7);
        assert_eq!(shape.edges_structural, 1);
        assert_eq!(shape.edges_navigational, 6);
        assert_eq!(shape.components, 2);
        assert_eq!(shape.largest_component, 6);
        assert_eq!(shape.isolated, 1);

        let health = g.health();
        assert_eq!(health.directed_cycles, 2);
        assert_eq!(health.triangles, 2);
        assert_eq!(health.bridges, vec![(loc("c"), loc("d"))]);
        assert_eq!(health.articulation_points, vec![loc("c"), loc("d")]);

        let hubs = g.hubs(2);
        assert_eq!(hubs[0].locator, loc("d"));
        assert_eq!(hubs[0].inbound_total, 2);
        assert_eq!(hubs[0].inbound_structural, 1);
        assert_eq!(hubs[0].inbound_navigational, 1);

        let nb = g.neighborhood(&loc("c"), 1, Direction::Out, 10).unwrap();
        assert!(!nb.truncated);
        assert_eq!(nb.nodes, vec![loc("c"), loc("a"), loc("d")]);
    }
}
