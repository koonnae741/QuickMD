//! Pass 2 of Sugiyama layout: Layer Assignment & Dummy Node Injection.
//!
//! Uses Longest-Path Layering on the DAG and injects dummy nodes for
//! long edges spanning multiple layers to ensure all edges connect adjacent layers.

use super::cycle_breaking::InternalEdge;
use crate::native_mermaid::ast::DiagramSubgraph;

#[derive(Debug, Clone)]
pub struct LayoutNodeData {
    pub original_id: Option<String>,
    pub is_dummy: bool,
    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
    pub layer: usize,
    pub pos_in_layer: usize,
    pub cluster_id: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct LayeredGraph {
    pub nodes: Vec<LayoutNodeData>,
    pub layers: Vec<Vec<usize>>,
    pub edge_chains: Vec<EdgeChain>,
}

#[derive(Debug, Clone)]
pub struct EdgeChain {
    pub original_index: usize,
    pub reversed: bool,
    pub path_node_indices: Vec<usize>,
}

pub fn assign_layers(
    initial_nodes: Vec<(String, f64, f64)>,
    dag_edges: &[InternalEdge],
    subgraphs: &[DiagramSubgraph],
) -> LayeredGraph {
    let num_initial = initial_nodes.len();
    if num_initial == 0 {
        return LayeredGraph {
            nodes: Vec::new(),
            layers: Vec::new(),
            edge_chains: Vec::new(),
        };
    }

    // 1. Longest-Path Layering
    let mut in_degrees = vec![0usize; num_initial];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); num_initial];
    for edge in dag_edges {
        if edge.from < num_initial && edge.to < num_initial && edge.from != edge.to {
            adj[edge.from].push(edge.to);
            in_degrees[edge.to] += 1;
        }
    }

    let mut layers = vec![0usize; num_initial];
    let mut queue = std::collections::VecDeque::new();
    for (i, &deg) in in_degrees.iter().enumerate().take(num_initial) {
        if deg == 0 {
            queue.push_back(i);
        }
    }

    let mut visited_count = 0;
    while let Some(u) = queue.pop_front() {
        visited_count += 1;
        let u_layer = layers[u];
        for &v in &adj[u] {
            if u_layer + 1 > layers[v] {
                layers[v] = u_layer + 1;
            }
            in_degrees[v] -= 1;
            if in_degrees[v] == 0 {
                queue.push_back(v);
            }
        }
    }

    // Handle any unreachable cycle leftovers safely
    if visited_count < num_initial {
        for (i, &deg) in in_degrees.iter().enumerate().take(num_initial) {
            if deg > 0 {
                layers[i] = layers[i].max(1);
            }
        }
    }

    // 1.5 Cluster-Aware Topological Rank Separation
    let mut node_to_cluster = vec![None; num_initial];
    if !subgraphs.is_empty() {
        let mut id_to_idx = std::collections::HashMap::with_capacity(num_initial);
        for (i, (id, _, _)) in initial_nodes.iter().enumerate() {
            id_to_idx.insert(id.as_str(), i);
        }

        let mut cluster_members: Vec<Vec<usize>> = vec![Vec::new(); subgraphs.len()];
        for (s_idx, sub) in subgraphs.iter().enumerate() {
            for nid in &sub.node_ids {
                if let Some(&idx) = id_to_idx.get(nid.as_str()) {
                    node_to_cluster[idx] = Some(s_idx);
                    cluster_members[s_idx].push(idx);
                }
            }
        }

        // Build Cluster Meta-DAG
        let mut cluster_adj: Vec<std::collections::HashSet<usize>> = vec![std::collections::HashSet::new(); subgraphs.len()];
        let mut cluster_in_deg = vec![0usize; subgraphs.len()];
        for edge in dag_edges {
            if edge.from < num_initial && edge.to < num_initial {
                if let (Some(cf), Some(ct)) = (node_to_cluster[edge.from], node_to_cluster[edge.to]) {
                    if cf != ct && cluster_adj[cf].insert(ct) {
                        cluster_in_deg[ct] += 1;
                    }
                }
            }
        }

        // Topological order of clusters
        let mut cluster_q = std::collections::VecDeque::new();
        for (c, &deg) in cluster_in_deg.iter().enumerate() {
            if deg == 0 {
                cluster_q.push_back(c);
            }
        }
        let mut cluster_order = Vec::with_capacity(subgraphs.len());
        while let Some(c) = cluster_q.pop_front() {
            cluster_order.push(c);
            for &next_c in &cluster_adj[c] {
                cluster_in_deg[next_c] -= 1;
                if cluster_in_deg[next_c] == 0 {
                    cluster_q.push_back(next_c);
                }
            }
        }
        for c in 0..subgraphs.len() {
            if !cluster_order.contains(&c) {
                cluster_order.push(c);
            }
        }

        // Enforce: ∀ v ∈ cluster C, layer(v) >= min_req_layer
        for &c in &cluster_order {
            let members = &cluster_members[c];
            if members.is_empty() {
                continue;
            }

            let mut min_req_layer = 0usize;
            // From predecessor clusters:
            for (pred_c, succs) in cluster_adj.iter().enumerate() {
                if succs.contains(&c) {
                    for &p_node in &cluster_members[pred_c] {
                        min_req_layer = min_req_layer.max(layers[p_node] + 1);
                    }
                }
            }
            // From external edges into this cluster:
            for edge in dag_edges {
                if edge.from < num_initial && edge.to < num_initial {
                    if node_to_cluster[edge.to] == Some(c) && node_to_cluster[edge.from] != Some(c) {
                        min_req_layer = min_req_layer.max(layers[edge.from] + 1);
                    }
                }
            }

            let mut changed = false;
            for &u in members {
                if layers[u] < min_req_layer {
                    layers[u] = min_req_layer;
                    changed = true;
                }
            }

            // Propagate forward through BFS within/beyond cluster
            if changed {
                let mut prop_q = std::collections::VecDeque::from(members.clone());
                while let Some(curr) = prop_q.pop_front() {
                    let curr_l = layers[curr];
                    for &next_node in &adj[curr] {
                        if layers[next_node] < curr_l + 1 {
                            layers[next_node] = curr_l + 1;
                            prop_q.push_back(next_node);
                        }
                    }
                }
            }
        }
    }

    // 2. Prepare nodes list and inject dummy nodes for edges spanning multiple layers
    let mut nodes: Vec<LayoutNodeData> = initial_nodes
        .into_iter()
        .enumerate()
        .map(|(idx, (id, w, h))| LayoutNodeData {
            original_id: Some(id),
            is_dummy: false,
            width: w,
            height: h,
            x: 0.0,
            y: 0.0,
            layer: layers[idx],
            pos_in_layer: 0,
            cluster_id: node_to_cluster[idx],
        })
        .collect();

    let mut edge_chains = Vec::with_capacity(dag_edges.len());

    for edge in dag_edges {
        let u = edge.from;
        let v = edge.to;
        if u >= num_initial || v >= num_initial {
            continue;
        }

        let u_layer = nodes[u].layer;
        let v_layer = nodes[v].layer;

        if u_layer < v_layer {
            let mut chain = vec![u];
            // SEC-03: Cap maximum total layout nodes (including dummies) to prevent DoS explosion
            const MAX_TOTAL_NODES: usize = 1024;
            for l in (u_layer + 1)..v_layer {
                if nodes.len() >= MAX_TOTAL_NODES {
                    break;
                }
                let dummy_idx = nodes.len();
                nodes.push(LayoutNodeData {
                    original_id: None,
                    is_dummy: true,
                    width: 0.0,
                    height: 0.0,
                    x: 0.0,
                    y: 0.0,
                    layer: l,
                    pos_in_layer: 0,
                    cluster_id: None,
                });
                chain.push(dummy_idx);
            }
            chain.push(v);

            edge_chains.push(EdgeChain {
                original_index: edge.original_index,
                reversed: edge.reversed,
                path_node_indices: chain,
            });
        } else {
            edge_chains.push(EdgeChain {
                original_index: edge.original_index,
                reversed: edge.reversed,
                path_node_indices: vec![u, v],
            });
        }
    }

    // 3. Assemble layer buckets
    let max_layer = nodes.iter().map(|n| n.layer).max().unwrap_or(0);
    let mut layer_buckets = vec![Vec::new(); max_layer + 1];
    for (idx, node) in nodes.iter().enumerate() {
        layer_buckets[node.layer].push(idx);
    }

    LayeredGraph {
        nodes,
        layers: layer_buckets,
        edge_chains,
    }
}
