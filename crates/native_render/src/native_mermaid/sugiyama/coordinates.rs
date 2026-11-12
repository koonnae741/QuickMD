//! Pass 4 of Sugiyama layout: Symmetrical Coordinate Assignment.
//!
//! Assigns 2D (X, Y) pixel coordinates with overlap prevention,
//! center-aligned hierarchies, and support for all 4 diagram directions.

use super::layering::LayeredGraph;
use crate::native_mermaid::ast::Direction;

pub struct LayoutOptions {
    pub rank_separation: f64,
    pub node_separation: f64,
    pub margin: f64,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            rank_separation: 80.0,
            node_separation: 40.0,
            margin: 40.0,
        }
    }
}

pub fn assign_coordinates(
    graph: &mut LayeredGraph,
    direction: Direction,
    options: &LayoutOptions,
) {
    if graph.nodes.is_empty() {
        return;
    }

    let is_vert = direction.is_vertical();

    // 1. Calculate rank positions along the primary flow axis
    // For vertical (TB/BT): primary flow is Y, layer thickness is max_height
    // For horizontal (LR/RL): primary flow is X, layer thickness is max_width
    let mut current_rank = options.margin;
    for (l_idx, layer) in graph.layers.iter().enumerate() {
        let max_thickness = layer
            .iter()
            .map(|&idx| {
                let n = &graph.nodes[idx];
                if is_vert {
                    if n.is_dummy { 10.0 } else { n.height }
                } else {
                    if n.is_dummy { 10.0 } else { n.width }
                }
            })
            .fold(30.0f64, f64::max);

        for &idx in layer {
            let center = current_rank + max_thickness / 2.0;
            if is_vert {
                graph.nodes[idx].y = center;
            } else {
                graph.nodes[idx].x = center;
            }
        }

        let mut extra_sep = 0.0;
        if l_idx + 1 < graph.layers.len() {
            let next_layer = &graph.layers[l_idx + 1];
            let curr_clusters: std::collections::HashSet<usize> = layer.iter().filter_map(|&i| graph.nodes[i].cluster_id).collect();
            let next_clusters: std::collections::HashSet<usize> = next_layer.iter().filter_map(|&i| graph.nodes[i].cluster_id).collect();

            // Provide rank separation if a new cluster begins in next_layer, or if clusters change
            let has_new_cluster = next_clusters.iter().any(|c| !curr_clusters.contains(c));
            let clusters_disjoint = !curr_clusters.is_empty() && !next_clusters.is_empty() && curr_clusters.is_disjoint(&next_clusters);
            if has_new_cluster || clusters_disjoint {
                extra_sep += 68.0;
            }

            // Cross-rank edges benefit from additional breathing room when labels are present
            let has_cross_rank_labels = graph.edge_chains.iter().any(|chain| {
                if chain.path_node_indices.len() >= 2 {
                    let u = chain.path_node_indices[0];
                    let v = chain.path_node_indices[chain.path_node_indices.len() - 1];
                    (graph.nodes[u].layer == l_idx && graph.nodes[v].layer > l_idx)
                        || (graph.nodes[v].layer == l_idx && graph.nodes[u].layer > l_idx)
                } else {
                    false
                }
            });
            if has_cross_rank_labels && extra_sep < 30.0 {
                extra_sep += 28.0;
            }
        }

        current_rank += max_thickness + options.rank_separation + extra_sep;
    }

    // Helper to get half-extent along the layer (secondary) axis
    let half_layer_dim = |u: usize, nodes: &[super::layering::LayoutNodeData]| -> f64 {
        let n = &nodes[u];
        if n.is_dummy {
            5.0
        } else if is_vert {
            n.width / 2.0
        } else {
            n.height / 2.0
        }
    };

    // 2. Calculate positions within each layer (X for vertical, Y for horizontal)
    for layer in &graph.layers {
        if layer.is_empty() {
            continue;
        }

        let n = layer.len();
        let mut fwd = vec![0.0f64; n];
        let mut bwd = vec![0.0f64; n];

        let mut cur_pos = options.margin;
        for i in 0..n {
            let u = layer[i];
            let hd = half_layer_dim(u, &graph.nodes);
            cur_pos += hd;
            fwd[i] = cur_pos;
            cur_pos += hd + options.node_separation;
        }

        // Forward separation guard
        for i in 1..n {
            let prev_u = layer[i - 1];
            let curr_u = layer[i];
            let prev_hd = half_layer_dim(prev_u, &graph.nodes);
            let curr_hd = half_layer_dim(curr_u, &graph.nodes);
            let extra = if (graph.nodes[prev_u].cluster_id.is_some() && graph.nodes[curr_u].is_dummy)
                || (graph.nodes[prev_u].is_dummy && graph.nodes[curr_u].cluster_id.is_some())
                || (graph.nodes[prev_u].cluster_id.is_some()
                    && graph.nodes[curr_u].cluster_id.is_some()
                    && graph.nodes[prev_u].cluster_id != graph.nodes[curr_u].cluster_id)
            {
                54.0
            } else {
                0.0
            };
            let min_dist = prev_hd + options.node_separation + curr_hd + extra;
            if fwd[i] < fwd[i - 1] + min_dist {
                fwd[i] = fwd[i - 1] + min_dist;
            }
        }

        // Backward separation guard
        bwd.copy_from_slice(&fwd);
        for i in (0..n - 1).rev() {
            let curr_u = layer[i];
            let next_u = layer[i + 1];
            let curr_hd = half_layer_dim(curr_u, &graph.nodes);
            let next_hd = half_layer_dim(next_u, &graph.nodes);
            let extra = if (graph.nodes[curr_u].cluster_id.is_some() && graph.nodes[next_u].is_dummy)
                || (graph.nodes[curr_u].is_dummy && graph.nodes[next_u].cluster_id.is_some())
                || (graph.nodes[curr_u].cluster_id.is_some()
                    && graph.nodes[next_u].cluster_id.is_some()
                    && graph.nodes[curr_u].cluster_id != graph.nodes[next_u].cluster_id)
            {
                54.0
            } else {
                0.0
            };
            let min_dist = curr_hd + options.node_separation + next_hd + extra;
            if bwd[i] > bwd[i + 1] - min_dist {
                bwd[i] = bwd[i + 1] - min_dist;
            }
        }

        // Average for symmetry
        for i in 0..n {
            let u = layer[i];
            let center = (fwd[i] + bwd[i]) / 2.0;
            if is_vert {
                graph.nodes[u].x = center;
            } else {
                graph.nodes[u].y = center;
            }
        }
    }

    // 3. Center layers relative to the widest/tallest layer span
    let max_span = graph
        .layers
        .iter()
        .map(|layer| {
            if layer.is_empty() {
                0.0
            } else {
                let first = layer[0];
                let last = layer[layer.len() - 1];
                let (f_pos, l_pos) = if is_vert {
                    (graph.nodes[first].x, graph.nodes[last].x)
                } else {
                    (graph.nodes[first].y, graph.nodes[last].y)
                };
                let f_hd = half_layer_dim(first, &graph.nodes);
                let l_hd = half_layer_dim(last, &graph.nodes);
                (l_pos + l_hd) - (f_pos - f_hd)
            }
        })
        .fold(0.0f64, f64::max);

    for layer in &graph.layers {
        if layer.is_empty() {
            continue;
        }
        let first = layer[0];
        let last = layer[layer.len() - 1];
        let (f_pos, l_pos) = if is_vert {
            (graph.nodes[first].x, graph.nodes[last].x)
        } else {
            (graph.nodes[first].y, graph.nodes[last].y)
        };
        let f_hd = half_layer_dim(first, &graph.nodes);
        let l_hd = half_layer_dim(last, &graph.nodes);
        let layer_span = (l_pos + l_hd) - (f_pos - f_hd);
        let offset = (max_span - layer_span) / 2.0;

        for &u in layer {
            if is_vert {
                graph.nodes[u].x += offset;
            } else {
                graph.nodes[u].y += offset;
            }
        }
    }

    // 4. Direction reversal transforms (BT, RL)
    if direction == Direction::BottomTop {
        let max_y = graph.nodes.iter().map(|n| n.y).fold(0.0f64, f64::max);
        for node in &mut graph.nodes {
            node.y = max_y - node.y + options.margin;
        }
    } else if direction == Direction::RightLeft {
        let max_x = graph.nodes.iter().map(|n| n.x).fold(0.0f64, f64::max);
        for node in &mut graph.nodes {
            node.x = max_x - node.x + options.margin;
        }
    }

    // 5. Margin normalization: Ensure all non-dummy nodes have at least `options.margin` clearance
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    for node in &graph.nodes {
        if !node.is_dummy {
            min_x = min_x.min(node.x - node.width / 2.0);
            min_y = min_y.min(node.y - node.height / 2.0);
        }
    }
    if min_x.is_finite() && min_y.is_finite() {
        let shift_x = if min_x < options.margin { options.margin - min_x } else { 0.0 };
        let shift_y = if min_y < options.margin { options.margin - min_y } else { 0.0 };
        if shift_x > 0.0 || shift_y > 0.0 {
            for node in &mut graph.nodes {
                node.x += shift_x;
                node.y += shift_y;
            }
        }
    }
}
