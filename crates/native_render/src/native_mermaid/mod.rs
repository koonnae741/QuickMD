//! `native_mermaid`: 100% pure-Rust Mermaid diagram parser, Sugiyama layered layout, and tiny-skia rasterizer.

pub mod ast;
pub mod error;
pub mod parser;
pub mod rasterizer;
pub mod router;
pub mod sugiyama;
pub mod tokens;

pub use ast::{DiagramEdge, DiagramGraph, DiagramNode, DiagramSubgraph, Direction, EdgeType, NodeShape};
pub use error::MermaidError;
pub use rasterizer::{rasterize_diagram, DiagramTheme};
pub use router::{deconflict_edge_labels, route_edge, RoutedEdge};
pub use sugiyama::{layout_graph, LayoutOptions, LayeredGraph};

use std::collections::HashMap;
use tiny_skia::Pixmap;
use crate::native_math::{FontVariant, MathFontContext};

/// High-level façade for parsing and rendering Mermaid diagrams.
pub struct MermaidRenderer {
    fonts: MathFontContext,
}

impl MermaidRenderer {
    pub fn new(fonts: MathFontContext) -> Self {
        Self { fonts }
    }

    pub fn with_system_fonts() -> Self {
        Self::new(MathFontContext::from_system(None))
    }

    pub fn fonts_mut(&mut self) -> &mut MathFontContext {
        &mut self.fonts
    }

    /// Parses and renders a Mermaid flowchart source into a rasterized Pixmap.
    pub fn render(
        &mut self,
        source: &str,
        theme: &DiagramTheme,
    ) -> Result<Pixmap, MermaidError> {
        // 1. Parse AST
        let graph = parser::parse(source)?;

        if graph.nodes.is_empty() {
            return Err(MermaidError::Layout {
                message: "diagram contains no nodes to render".to_string(),
            });
        }

        // 2. Measure text bounds for each node
        let mut node_sizes = HashMap::with_capacity(graph.nodes.len());
        for (id, node) in &graph.nodes {
            let shaped = self.fonts.shape(&node.label, FontVariant::Roman, theme.font_size);
            let h_pad = match node.shape {
                NodeShape::Stadium | NodeShape::Hexagon => 36.0,
                NodeShape::Diamond => 44.0,
                NodeShape::Circle => 24.0,
                _ => 28.0,
            };
            let v_pad = match node.shape {
                NodeShape::Diamond => 32.0,
                NodeShape::Circle => 24.0,
                _ => 20.0,
            };

            let text_w = shaped.advance;
            let text_h = shaped.height + shaped.depth;

            let final_w = (text_w + h_pad).max(60.0);
            let final_h = (text_h + v_pad).max(36.0);

            let (w, h) = if node.shape == NodeShape::Circle {
                let d = final_w.max(final_h);
                (d, d)
            } else {
                (final_w, final_h)
            };

            node_sizes.insert(id.clone(), (w, h));
        }

        // 3. Run Sugiyama 4-Pass Layout
        let layout_opts = LayoutOptions::default();
        let layered = layout_graph(&graph, &node_sizes, &layout_opts);

        // 4. Route Spline Curves for each Edge
        let mut routed_edges = Vec::with_capacity(layered.edge_chains.len());
        for chain in &layered.edge_chains {
            let orig_edge = &graph.edges[chain.original_index];
            let shape_from = graph.nodes.get(&orig_edge.from).map(|n| n.shape).unwrap_or(NodeShape::Rectangle);
            let shape_to = graph.nodes.get(&orig_edge.to).map(|n| n.shape).unwrap_or(NodeShape::Rectangle);

            let routed = route_edge(
                &chain.path_node_indices,
                &layered.nodes,
                shape_from,
                shape_to,
                orig_edge.edge_type,
                orig_edge.label.clone(),
                graph.direction,
                chain.reversed,
                0.5,
            );
            routed_edges.push(routed);
        }

        // 4.5 Deconflict and Stagger Edge Labels to Prevent All Collisions
        deconflict_edge_labels(
            &mut routed_edges,
            &layered.nodes,
            &graph.subgraphs,
            &mut self.fonts,
            theme.font_size,
            graph.direction,
        );

        // 5. Rasterize Pixmap
        rasterize_diagram(&graph, &layered, &routed_edges, &mut self.fonts, theme)
            .ok_or_else(|| MermaidError::Render {
                message: "failed to allocate or rasterize pixmap".to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flowchart_header_and_nodes() {
        let src = r#"
        flowchart TD
            A[Start] --> B(Process)
            B --> C{Decision}
            C -->|Yes| D([Done])
        "#;
        let g = parser::parse(src).unwrap();
        assert_eq!(g.direction, Direction::TopBottom);
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.edges.len(), 3);
        assert_eq!(g.nodes["A"].shape, NodeShape::Rectangle);
        assert_eq!(g.nodes["B"].shape, NodeShape::RoundedRect);
        assert_eq!(g.nodes["C"].shape, NodeShape::Diamond);
        assert_eq!(g.nodes["D"].shape, NodeShape::Stadium);
    }

    #[test]
    fn handles_multi_node_chaining() {
        let src = r#"
        graph LR
            A & B --> C & D
        "#;
        let g = parser::parse(src).unwrap();
        assert_eq!(g.direction, Direction::LeftRight);
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.edges.len(), 4); // A->C, A->D, B->C, B->D
    }

    #[test]
    fn breaks_cycles_without_deadlock() {
        let src = r#"
        graph TD
            A --> B
            B --> C
            C --> A
        "#;
        let g = parser::parse(src).unwrap();
        let mut sizes = HashMap::new();
        sizes.insert("A".to_string(), (80.0, 36.0));
        sizes.insert("B".to_string(), (80.0, 36.0));
        sizes.insert("C".to_string(), (80.0, 36.0));

        let layered = layout_graph(&g, &sizes, &LayoutOptions::default());
        assert!(!layered.nodes.is_empty());
        assert!(layered.layers.len() >= 2);
    }

    #[test]
    fn parses_all_node_shapes() {
        let src = r#"
        graph TD
            R[Rect]
            RR(Rounded)
            ST([Stadium])
            SB[[Subroutine]]
            CY[(Cylinder)]
            CI((Circle))
            DI{Diamond}
            HX{{Hexagon}}
        "#;
        let g = parser::parse(src).unwrap();
        assert_eq!(g.nodes["R"].shape, NodeShape::Rectangle);
        assert_eq!(g.nodes["RR"].shape, NodeShape::RoundedRect);
        assert_eq!(g.nodes["ST"].shape, NodeShape::Stadium);
        assert_eq!(g.nodes["SB"].shape, NodeShape::Subroutine);
        assert_eq!(g.nodes["CY"].shape, NodeShape::Cylinder);
        assert_eq!(g.nodes["CI"].shape, NodeShape::Circle);
        assert_eq!(g.nodes["DI"].shape, NodeShape::Diamond);
        assert_eq!(g.nodes["HX"].shape, NodeShape::Hexagon);
    }

    #[test]
    fn renders_visible_diagram_pixels() {
        let mut r = MermaidRenderer::with_system_fonts();
        let src = r#"
        flowchart TD
            Start([เริ่มต้นระบบ]) --> Calc[ประมวลผล]
            Calc --> Decision{ถูกต้องไหม?}
            Decision -->|ใช่| Success[สำเร็จ]
            Decision -->|ไม่ใช่| Fail[แก้ไขใหม่]
            Fail --> Calc
        "#;
        let theme = DiagramTheme::default();
        let pixmap = r.render(src, &theme).unwrap();
        assert!(pixmap.width() > 100);
        assert!(pixmap.height() > 100);
        assert!(pixmap.pixels().iter().any(|p| p.alpha() > 0));
    }
}
