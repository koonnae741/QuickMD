//! Pass 1 of Sugiyama layout: Cycle Breaking via 3-color DFS.
//!
//! Transforms any directed graph into a Directed Acyclic Graph (DAG) by
//! identifying back-edges and temporarily reversing their direction.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    White, // Unvisited
    Gray,  // Visiting (ancestor in current DFS recursion stack)
    Black, // Visited (fully explored)
}

#[derive(Debug, Clone)]
pub struct InternalEdge {
    pub from: usize,
    pub to: usize,
    pub reversed: bool,
    pub original_index: usize,
}

/// Identifies all back-edges in the graph and reverses them.
/// Returns a new set of edges forming a valid DAG.
pub fn break_cycles(num_nodes: usize, edges: &[(usize, usize, usize)]) -> Vec<InternalEdge> {
    let mut adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); num_nodes];
    for &(from, to, orig_idx) in edges {
        if from < num_nodes && to < num_nodes {
            adj[from].push((to, orig_idx));
        }
    }

    let mut colors = vec![Color::White; num_nodes];
    let mut dag_edges = Vec::with_capacity(edges.len());

    for start in 0..num_nodes {
        if colors[start] == Color::White {
            dfs(start, &adj, &mut colors, &mut dag_edges);
        }
    }

    dag_edges
}

fn dfs(
    u: usize,
    adj: &[Vec<(usize, usize)>],
    colors: &mut [Color],
    dag_edges: &mut Vec<InternalEdge>,
) {
    colors[u] = Color::Gray;

    for &(v, orig_idx) in &adj[u] {
        if u == v {
            // Self-loop: treat as back-edge or separate handling
            dag_edges.push(InternalEdge {
                from: u,
                to: v,
                reversed: false,
                original_index: orig_idx,
            });
            continue;
        }

        match colors[v] {
            Color::White => {
                dag_edges.push(InternalEdge {
                    from: u,
                    to: v,
                    reversed: false,
                    original_index: orig_idx,
                });
                dfs(v, adj, colors, dag_edges);
            }
            Color::Gray => {
                // Back-edge detected! Reverse direction (v -> u) to break cycle
                dag_edges.push(InternalEdge {
                    from: v,
                    to: u,
                    reversed: true,
                    original_index: orig_idx,
                });
            }
            Color::Black => {
                // Forward-edge or cross-edge: maintain normal direction
                dag_edges.push(InternalEdge {
                    from: u,
                    to: v,
                    reversed: false,
                    original_index: orig_idx,
                });
            }
        }
    }

    colors[u] = Color::Black;
}
