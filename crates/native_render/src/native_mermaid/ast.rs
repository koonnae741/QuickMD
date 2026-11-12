//! Abstract Syntax Tree for Mermaid diagrams.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Direction {
    #[default]
    TopBottom,
    BottomTop,
    LeftRight,
    RightLeft,
}

impl Direction {
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::TopBottom | Self::BottomTop)
    }

    pub fn is_reversed(self) -> bool {
        matches!(self, Self::BottomTop | Self::RightLeft)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NodeShape {
    #[default]
    Rectangle,   // [Text]
    RoundedRect, // (Text)
    Stadium,     // ([Text])
    Subroutine,  // [[Text]]
    Cylinder,    // [(Text)]
    Circle,      // ((Text))
    Diamond,     // {Text}
    Hexagon,     // {{Text}}
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagramNode {
    pub id: String,
    pub label: String,
    pub shape: NodeShape,
    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
    pub layer: usize,
    pub rank: usize,
}

impl DiagramNode {
    pub fn new(id: String, label: String, shape: NodeShape) -> Self {
        Self {
            id,
            label,
            shape,
            width: 0.0,
            height: 0.0,
            x: 0.0,
            y: 0.0,
            layer: 0,
            rank: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EdgeType {
    Solid,
    #[default]
    SolidArrow,
    Dotted,
    DottedArrow,
    Thick,
    ThickArrow,
}

impl EdgeType {
    pub fn has_arrow(self) -> bool {
        matches!(self, Self::SolidArrow | Self::DottedArrow | Self::ThickArrow)
    }

    pub fn is_thick(self) -> bool {
        matches!(self, Self::Thick | Self::ThickArrow)
    }

    pub fn is_dotted(self) -> bool {
        matches!(self, Self::Dotted | Self::DottedArrow)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagramEdge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub edge_type: EdgeType,
    pub control_points: Vec<kurbo::Point>,
}

impl DiagramEdge {
    pub fn new(from: String, to: String, edge_type: EdgeType, label: Option<String>) -> Self {
        Self {
            from,
            to,
            label,
            edge_type,
            control_points: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagramSubgraph {
    pub id: String,
    pub title: String,
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiagramGraph {
    pub direction: Direction,
    pub nodes: HashMap<String, DiagramNode>,
    pub node_order: Vec<String>,
    pub edges: Vec<DiagramEdge>,
    pub subgraphs: Vec<DiagramSubgraph>,
}

impl DiagramGraph {
    pub fn new(direction: Direction) -> Self {
        Self {
            direction,
            nodes: HashMap::new(),
            node_order: Vec::new(),
            edges: Vec::new(),
            subgraphs: Vec::new(),
        }
    }

    pub fn ensure_node(&mut self, id: &str) {
        if !self.nodes.contains_key(id) {
            let node = DiagramNode::new(id.to_string(), id.to_string(), NodeShape::Rectangle);
            self.nodes.insert(id.to_string(), node);
            self.node_order.push(id.to_string());
        }
    }
}
