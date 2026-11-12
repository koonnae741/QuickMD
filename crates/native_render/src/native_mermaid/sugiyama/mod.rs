//! Sugiyama 4-pass layered graph layout pipeline.

pub mod coordinates;
pub mod crossing_reduction;
pub mod cycle_breaking;
pub mod layering;

pub use coordinates::{assign_coordinates, LayoutOptions};
pub use crossing_reduction::reduce_crossings;
pub use cycle_breaking::break_cycles;
pub use layering::{assign_layers, EdgeChain, LayeredGraph, LayoutNodeData};

use crate::native_mermaid::ast::DiagramGraph;

/// Runs the complete Sugiyama 4-pass layout pipeline on a DiagramGraph.
pub fn layout_graph(
    graph: &DiagramGraph,
    node_sizes: &std::collections::HashMap<String, (f64, f64)>,
    options: &LayoutOptions,
) -> LayeredGraph {
    let mut initial_nodes = Vec::with_capacity(graph.node_order.len());
    let mut id_to_index = std::collections::HashMap::with_capacity(graph.node_order.len());

    for (idx, id) in graph.node_order.iter().enumerate() {
        let &(w, h) = node_sizes.get(id).unwrap_or(&(100.0, 40.0));
        initial_nodes.push((id.clone(), w, h));
        id_to_index.insert(id.clone(), idx);
    }

    let mut edges = Vec::with_capacity(graph.edges.len());
    for (orig_idx, edge) in graph.edges.iter().enumerate() {
        if let (Some(&u), Some(&v)) = (id_to_index.get(&edge.from), id_to_index.get(&edge.to)) {
            edges.push((u, v, orig_idx));
        }
    }

    // Pass 1: Cycle Breaking (3-color DFS)
    let dag_edges = break_cycles(initial_nodes.len(), &edges);

    // Pass 2: Layer Assignment & Dummy Node Injection
    let mut layered_graph = assign_layers(initial_nodes, &dag_edges, &graph.subgraphs);

    // Pass 3: Crossing Reduction (Barycenter Sweep)
    reduce_crossings(&mut layered_graph, 10);

    // Pass 4: Symmetrical Coordinate Assignment
    assign_coordinates(&mut layered_graph, graph.direction, options);

    layered_graph
}
