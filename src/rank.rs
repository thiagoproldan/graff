//! PageRank over the graph of definitions and the uses that tie them, as
//! Aider's repository map ranks a repository's files (repomap.py, after
//! networkx's `pagerank`): a walker follows a use from the definition it is
//! in to the one it reaches, or jumps, about one step in seven, to where the
//! question's focus is -- anywhere, without one. What it visits most is
//! what the focus leans on most.

use std::collections::HashMap;

/// The share of steps that follow a use: networkx's and Aider's.
const DAMPING: f64 = 0.85;

/// When the walk has settled: the shares move by less than this in all
/// from one step to the next. networkx allows a millionth for each node,
/// which on a worktree of 100,000 definitions stops at a tenth.
const TOLERANCE: f64 = 1e-6;

/// The most steps the walk takes before it stops, settled or not.
const STEPS: usize = 100;

/// The bytes of a kept edge: its two nodes and its weight.
const KEPT: usize = 16;

/// A weighted graph of a fixed number of nodes.
pub struct Graph {
    nodes: usize,
    /// Each edge's weight, by the nodes it goes from and to.
    edges: HashMap<(usize, usize), f64>,
}

impl Graph {
    pub fn new(nodes: usize) -> Graph {
        Graph {
            nodes,
            edges: HashMap::new(),
        }
    }

    pub fn nodes(&self) -> usize {
        self.nodes
    }

    /// An edge from one node to another, which adds its weight to that of an
    /// edge already there. A node's edge to itself, and one of no weight, is
    /// none: what a definition does within itself says nothing of its rank.
    pub fn add(&mut self, from: usize, to: usize, weight: f64) {
        if from != to && weight > 0.0 {
            *self.edges.entry((from, to)).or_default() += weight;
        }
    }

    /// The edges as the index keeps them: each its two nodes and its weight,
    /// little-endian, in the nodes' order.
    pub fn kept(&self) -> Vec<u8> {
        let mut edges: Vec<(&(usize, usize), &f64)> = self.edges.iter().collect();
        edges.sort_by_key(|&(&nodes, _)| nodes);
        let mut bytes = Vec::with_capacity(edges.len() * KEPT);
        for (&(from, to), &w) in edges {
            bytes.extend_from_slice(&(from as u32).to_le_bytes());
            bytes.extend_from_slice(&(to as u32).to_le_bytes());
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        bytes
    }

    /// The graph of `nodes` nodes whose edges `kept` gave; none when they
    /// are not whole or reach past its nodes.
    pub fn from_kept(nodes: usize, bytes: &[u8]) -> Option<Graph> {
        let (edges, rest) = bytes.as_chunks::<KEPT>();
        if !rest.is_empty() {
            return None;
        }
        let mut graph = Graph::new(nodes);
        graph.edges.reserve(edges.len());
        for edge in edges {
            let from = u32::from_le_bytes(edge[0..4].try_into().ok()?) as usize;
            let to = u32::from_le_bytes(edge[4..8].try_into().ok()?) as usize;
            let w = f64::from_le_bytes(edge[8..16].try_into().ok()?);
            if from >= nodes || to >= nodes {
                return None;
            }
            graph.edges.insert((from, to), w);
        }
        Some(graph)
    }

    /// Each node's edges, by the node they reach, with what a weight holds
    /// past one tempered to its square root, as Aider tempers how many
    /// times one file names a definition of another: a function called ten
    /// times from one place is not ten callers. In the nodes' order, so that
    /// a rank is the same from one run to the next.
    fn out(&self) -> Vec<Vec<(usize, f64)>> {
        let mut out = vec![Vec::new(); self.nodes];
        for (&(from, to), &w) in &self.edges {
            out[from].push((to, if w > 1.0 { w.sqrt() } else { w }));
        }
        for edges in &mut out {
            edges.sort_by_key(|&(to, _)| to);
        }
        out
    }

    /// Each node's share of a walk that follows the edges by their weights
    /// and, about one step in seven, jumps to a node by `focus`: its weights,
    /// which need not sum to one; uniform when it is none or holds no
    /// weight. A node whose edges weigh less than one in all, a use that
    /// may reach one of several definitions alone, follows them that much
    /// and jumps the rest, rather than passing all it holds through so
    /// weak a tie; one with none jumps. The shares sum to one.
    pub fn rank(&self, focus: Option<&[f64]>) -> Vec<f64> {
        let n = self.nodes;
        if n == 0 {
            return Vec::new();
        }
        let out = self.out();
        let total: f64 = focus.map_or(0.0, |f| f.iter().sum());
        let jump: Vec<f64> = match focus {
            Some(focus) if total > 0.0 => focus.iter().map(|w| w / total).collect(),
            _ => vec![1.0 / n as f64; n],
        };
        let sums: Vec<f64> = out
            .iter()
            .map(|edges| edges.iter().map(|(_, w)| w).sum::<f64>().max(1.0))
            .collect();
        let mut shares = jump.clone();
        let mut next = vec![0.0; n];
        for _ in 0..STEPS {
            next.iter_mut().for_each(|slot| *slot = 0.0);
            for (v, edges) in out.iter().enumerate() {
                let flow = DAMPING * shares[v] / sums[v];
                for &(to, w) in edges {
                    next[to] += flow * w;
                }
            }
            // What did not follow an edge jumps.
            let jumps = 1.0 - next.iter().sum::<f64>();
            for (slot, j) in next.iter_mut().zip(&jump) {
                *slot += jumps * j;
            }
            let moved: f64 = next.iter().zip(&shares).map(|(a, b)| (a - b).abs()).sum();
            std::mem::swap(&mut shares, &mut next);
            if moved < TOLERANCE {
                break;
            }
        }
        shares
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weak_edge_passes_on_only_its_weight() {
        // 1 and 2 each reach 0, 1 surely, 2 by a weak tie: 0 gets more of
        // what 1 holds than of what 2 does.
        let mut strong = Graph::new(3);
        strong.add(1, 0, 1.0);
        let mut weak = Graph::new(3);
        weak.add(2, 0, 0.1);
        let (strong, weak) = (strong.rank(None), weak.rank(None));
        assert!(strong[0] > weak[0], "{strong:?} {weak:?}");
        assert!((weak.iter().sum::<f64>() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn what_many_reach_outranks_what_reaches_them() {
        let mut graph = Graph::new(4);
        for from in 1..4 {
            graph.add(from, 0, 1.0);
        }
        let shares = graph.rank(None);
        assert!((shares.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((1..4).all(|v| shares[0] > shares[v]), "{shares:?}");
    }

    #[test]
    fn a_focus_draws_the_walk_to_its_neighbours() {
        // 0 reaches 1, 2 reaches 3: a focus on 0 ranks 1 above 3; on 2,
        // the reverse.
        let mut graph = Graph::new(4);
        graph.add(0, 1, 1.0);
        graph.add(2, 3, 1.0);
        let shares = graph.rank(Some(&[1.0, 0.0, 0.0, 0.0]));
        assert!(shares[1] > shares[3], "{shares:?}");
        let shares = graph.rank(Some(&[0.0, 0.0, 2.0, 0.0]));
        assert!(shares[3] > shares[1], "{shares:?}");
        // A focus of no weight is none.
        let none = graph.rank(Some(&[0.0; 4]));
        assert_eq!(none, graph.rank(None));
    }

    #[test]
    fn a_heavier_edge_carries_more() {
        let mut graph = Graph::new(3);
        graph.add(0, 1, 3.0);
        graph.add(0, 2, 1.0);
        let shares = graph.rank(Some(&[1.0, 0.0, 0.0]));
        assert!(shares[1] > shares[2], "{shares:?}");
    }

    #[test]
    fn an_edge_to_itself_or_of_no_weight_is_none_and_weights_add() {
        let mut graph = Graph::new(2);
        graph.add(0, 0, 1.0);
        graph.add(0, 1, 0.0);
        assert!(graph.out()[0].is_empty());
        graph.add(0, 1, 2.0);
        graph.add(0, 1, 2.0);
        assert_eq!(graph.out()[0], [(1, 2.0)]);
        // What is less than one stays as it is.
        graph.add(1, 0, 0.25);
        assert_eq!(graph.out()[1], [(0, 0.25)]);
    }

    #[test]
    fn a_kept_graph_reads_back_as_it_was() {
        let mut graph = Graph::new(4);
        graph.add(3, 0, 1.0);
        graph.add(0, 1, 0.1 / 3.0);
        graph.add(0, 1, 2.0);
        graph.add(2, 3, 0.25);
        let kept = graph.kept();
        let back = Graph::from_kept(4, &kept).unwrap();
        assert_eq!(back.edges, graph.edges);
        assert_eq!(back.kept(), kept);
        assert_eq!(back.rank(None), graph.rank(None));
        // Cut short, or reaching past its nodes, it is none.
        assert!(Graph::from_kept(4, &kept[..kept.len() - 1]).is_none());
        assert!(Graph::from_kept(3, &kept).is_none());
        assert!(Graph::from_kept(4, &[]).unwrap().edges.is_empty());
    }
}
