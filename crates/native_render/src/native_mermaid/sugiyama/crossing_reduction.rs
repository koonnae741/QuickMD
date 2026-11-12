//! Pass 3 of Sugiyama layout: Crossing Reduction via Barycenter Sweep.
//!
//! Iteratively sorts nodes in each layer based on the average positions
//! of their connected neighbors to minimize edge crossings.

use super::layering::LayeredGraph;

pub fn reduce_crossings(graph: &mut LayeredGraph, iterations: usize) {
    if graph.layers.len() <= 1 {
        return;
    }

    // Build adjacent layer connections from edge chains
    let num_nodes = graph.nodes.len();
    let mut up_adj: Vec<Vec<usize>> = vec![Vec::new(); num_nodes];
    let mut down_adj: Vec<Vec<usize>> = vec![Vec::new(); num_nodes];

    for chain in &graph.edge_chains {
        for window in chain.path_node_indices.windows(2) {
            let u = window[0];
            let v = window[1];
            if graph.nodes[u].layer + 1 == graph.nodes[v].layer {
                down_adj[u].push(v);
                up_adj[v].push(u);
            }
        }
    }

    // Initialize pos_in_layer
    for layer in &mut graph.layers {
        for (pos, &node_idx) in layer.iter().enumerate() {
            graph.nodes[node_idx].pos_in_layer = pos;
        }
    }

    // Sweep iterations
    for it in 0..iterations {
        if it % 2 == 0 {
            // Top-down sweep (Layer 1 -> Max)
            for l in 1..graph.layers.len() {
                sort_layer_by_barycenter(&mut graph.layers[l], &graph.nodes, &up_adj);
                for (pos, &node_idx) in graph.layers[l].iter().enumerate() {
                    graph.nodes[node_idx].pos_in_layer = pos;
                }
            }
        } else {
            // Bottom-up sweep (Layer Max-1 -> 0)
            for l in (0..graph.layers.len() - 1).rev() {
                sort_layer_by_barycenter(&mut graph.layers[l], &graph.nodes, &down_adj);
                for (pos, &node_idx) in graph.layers[l].iter().enumerate() {
                    graph.nodes[node_idx].pos_in_layer = pos;
                }
            }
        }
    }
}

fn sort_layer_by_barycenter(
    layer: &mut [usize],
    nodes: &[super::layering::LayoutNodeData],
    adj: &[Vec<usize>],
) {
    let node_scores: Vec<(usize, f64, Option<usize>)> = layer
        .iter()
        .map(|&u| {
            let neighbors = &adj[u];
            let score = if neighbors.is_empty() {
                nodes[u].pos_in_layer as f64
            } else {
                let sum: usize = neighbors.iter().map(|&v| nodes[v].pos_in_layer).sum();
                sum as f64 / neighbors.len() as f64
            };
            (u, score, nodes[u].cluster_id)
        })
        .collect();

    // Calculate average barycenter for each cluster in this layer
    let mut cluster_sums: std::collections::HashMap<usize, (f64, usize)> = std::collections::HashMap::new();
    for &(_, score, cluster_opt) in &node_scores {
        if let Some(cid) = cluster_opt {
            let entry = cluster_sums.entry(cid).or_insert((0.0, 0));
            entry.0 += score;
            entry.1 += 1;
        }
    }
    let cluster_avgs: std::collections::HashMap<usize, f64> = cluster_sums
        .into_iter()
        .map(|(cid, (sum, count))| (cid, sum / count as f64))
        .collect();

    let mut scored = node_scores;
    scored.sort_by(|a, b| {
        let cluster_a_score = a.2.and_then(|c| cluster_avgs.get(&c).copied()).unwrap_or(a.1);
        let cluster_b_score = b.2.and_then(|c| cluster_avgs.get(&c).copied()).unwrap_or(b.1);

        match cluster_a_score.total_cmp(&cluster_b_score) {
            std::cmp::Ordering::Equal => match a.2.cmp(&b.2) {
                std::cmp::Ordering::Equal => a.1.total_cmp(&b.1),
                other => other,
            },
            other => other,
        }
    });

    for (i, (u, _, _)) in scored.into_iter().enumerate() {
        layer[i] = u;
    }
}
