//! Recursive-descent parser for Mermaid diagrams.

use std::ops::Range;

use super::ast::{DiagramEdge, DiagramGraph, DiagramNode, DiagramSubgraph, Direction, EdgeType, NodeShape};
use super::error::MermaidError;
use super::tokens::{tokenize, Spanned, Token};

pub fn parse(source: &str) -> Result<DiagramGraph, MermaidError> {
    let tokens = tokenize(source);
    let mut parser = Parser::new(source, tokens);
    parser.parse_graph()
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Spanned<'a>>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, tokens: Vec<Spanned<'a>>) -> Self {
        Self {
            source,
            tokens,
            pos: 0,
        }
    }

    fn peek(&self) -> Option<Token<'a>> {
        self.tokens.get(self.pos).map(|(t, _)| *t)
    }

    fn peek_span(&self) -> Range<usize> {
        self.tokens.get(self.pos).map(|(_, s)| s.clone()).unwrap_or(self.source.len()..self.source.len())
    }

    fn bump(&mut self) -> Option<(Token<'a>, Range<usize>)> {
        if self.pos < self.tokens.len() {
            let item = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(item)
        } else {
            None
        }
    }

    fn skip_separators(&mut self) {
        while matches!(self.peek(), Some(Token::Separator)) {
            self.pos += 1;
        }
    }

    fn parse_graph(&mut self) -> Result<DiagramGraph, MermaidError> {
        self.skip_separators();
        let mut direction = Direction::TopBottom;

        if matches!(self.peek(), Some(Token::Graph)) {
            self.bump();
            direction = match self.peek() {
                Some(Token::DirTB) => { self.bump(); Direction::TopBottom }
                Some(Token::DirBT) => { self.bump(); Direction::BottomTop }
                Some(Token::DirLR) => { self.bump(); Direction::LeftRight }
                Some(Token::DirRL) => { self.bump(); Direction::RightLeft }
                _ => Direction::TopBottom,
            };
        }

        let mut graph = DiagramGraph::new(direction);
        let mut current_subgraph: Option<DiagramSubgraph> = None;

        while self.pos < self.tokens.len() {
            self.skip_separators();
            if self.pos >= self.tokens.len() {
                break;
            }

            match self.peek() {
                Some(Token::Subgraph) => {
                    self.bump();
                    let (id, title) = self.parse_subgraph_header()?;
                    if let Some(prev) = current_subgraph.take() {
                        graph.subgraphs.push(prev);
                    }
                    current_subgraph = Some(DiagramSubgraph {
                        id,
                        title,
                        node_ids: Vec::new(),
                    });
                }
                Some(Token::End) => {
                    self.bump();
                    if let Some(sub) = current_subgraph.take() {
                        graph.subgraphs.push(sub);
                    }
                }
                _ => {
                    let created_nodes = self.parse_statement(&mut graph)?;
                    if let Some(ref mut sub) = current_subgraph {
                        for node_id in created_nodes {
                            if !sub.node_ids.contains(&node_id) {
                                sub.node_ids.push(node_id);
                            }
                        }
                    }
                }
            }
        }

        if let Some(sub) = current_subgraph {
            graph.subgraphs.push(sub);
        }

        // Subgraph Edge Target Resolution:
        // When an edge connects to/from a Subgraph ID (e.g. `File --> Subgraph` or `Subgraph --> Node`),
        // resolve it to the subgraph's entry/exit child nodes and eliminate phantom nodes.
        for edge in &mut graph.edges {
            if let Some(sub) = graph.subgraphs.iter().find(|s| s.id == edge.from) {
                if let Some(exit_node) = sub.node_ids.iter().rev().find(|&id| id != &edge.to) {
                    edge.from = exit_node.clone();
                } else if let Some(first) = sub.node_ids.first() {
                    edge.from = first.clone();
                }
            }
            if let Some(sub) = graph.subgraphs.iter().find(|s| s.id == edge.to) {
                if let Some(entry_node) = sub.node_ids.iter().find(|&id| id != &edge.from) {
                    edge.to = entry_node.clone();
                } else if let Some(first) = sub.node_ids.first() {
                    edge.to = first.clone();
                }
            }
        }

        // Eliminate any phantom nodes that were created with subgraph IDs
        for sub in &graph.subgraphs {
            graph.nodes.remove(&sub.id);
            graph.node_order.retain(|id| id != &sub.id);
        }

        Ok(graph)
    }

    fn parse_subgraph_header(&mut self) -> Result<(String, String), MermaidError> {
        let span = self.peek_span();
        let id = match self.bump() {
            Some((Token::Ident(s) | Token::QuotedString(s), _)) => s.to_string(),
            _ => return Err(MermaidError::Parse {
                message: "expected subgraph identifier".to_string(),
                span,
            }),
        };

        let title = match self.peek() {
            Some(Token::OpenBracket) => {
                let open_span = self.peek_span();
                self.bump();
                let close_span = self.find_close_delimiter(Token::CloseBracket)?;
                self.extract_raw_label(open_span.end, close_span.start)
            }
            Some(Token::QuotedString(s)) => {
                self.bump();
                s.to_string()
            }
            _ => id.clone(),
        };

        Ok((id, title))
    }

    fn parse_statement(&mut self, graph: &mut DiagramGraph) -> Result<Vec<String>, MermaidError> {
        let mut all_affected_nodes = Vec::new();
        let mut prev_group = self.parse_node_group(graph)?;
        all_affected_nodes.extend(prev_group.clone());

        while let Some((edge_type, label)) = self.parse_edge_operator()? {
            let next_group = self.parse_node_group(graph)?;
            for from_id in &prev_group {
                for to_id in &next_group {
                    graph.edges.push(DiagramEdge::new(
                        from_id.clone(),
                        to_id.clone(),
                        edge_type,
                        label.clone(),
                    ));
                }
            }
            all_affected_nodes.extend(next_group.clone());
            prev_group = next_group;
        }

        Ok(all_affected_nodes)
    }

    fn parse_node_group(&mut self, graph: &mut DiagramGraph) -> Result<Vec<String>, MermaidError> {
        let mut nodes = Vec::new();
        let first = self.parse_single_node(graph)?;
        nodes.push(first);

        while matches!(self.peek(), Some(Token::Ampersand)) {
            self.bump();
            let next = self.parse_single_node(graph)?;
            nodes.push(next);
        }

        Ok(nodes)
    }

    fn parse_single_node(&mut self, graph: &mut DiagramGraph) -> Result<String, MermaidError> {
        let span = self.peek_span();
        let (raw_id, _is_quoted) = match self.bump() {
            Some((Token::Ident(s), _)) => (s.to_string(), false),
            Some((Token::QuotedString(s), _)) => (s.to_string(), true),
            _ => return Err(MermaidError::Parse {
                message: "expected node identifier".to_string(),
                span,
            }),
        };

        let node_id = raw_id.clone();
        let shape_info = self.parse_node_shape()?;

        if let Some((shape, label)) = shape_info {
            let final_label = if label.is_empty() { node_id.clone() } else { label };
            if let Some(existing) = graph.nodes.get_mut(&node_id) {
                existing.shape = shape;
                existing.label = final_label;
            } else {
                let node = DiagramNode::new(node_id.clone(), final_label, shape);
                graph.nodes.insert(node_id.clone(), node);
                graph.node_order.push(node_id.clone());
            }
        } else if !graph.nodes.contains_key(&node_id) {
            let label = node_id.clone();
            let node = DiagramNode::new(node_id.clone(), label, NodeShape::Rectangle);
            graph.nodes.insert(node_id.clone(), node);
            graph.node_order.push(node_id.clone());
        }

        Ok(node_id)
    }

    fn parse_node_shape(&mut self) -> Result<Option<(NodeShape, String)>, MermaidError> {
        let (shape, close_token) = match self.peek() {
            Some(Token::OpenStadium) => (NodeShape::Stadium, Token::CloseStadium),
            Some(Token::OpenSubroutine) => (NodeShape::Subroutine, Token::CloseSubroutine),
            Some(Token::OpenCylinder) => (NodeShape::Cylinder, Token::CloseCylinder),
            Some(Token::OpenCircle) => (NodeShape::Circle, Token::CloseCircle),
            Some(Token::OpenHexagon) => (NodeShape::Hexagon, Token::CloseHexagon),
            Some(Token::OpenBracket) => (NodeShape::Rectangle, Token::CloseBracket),
            Some(Token::OpenParen) => (NodeShape::RoundedRect, Token::CloseParen),
            Some(Token::OpenBrace) => (NodeShape::Diamond, Token::CloseBrace),
            _ => return Ok(None),
        };

        let open_span = self.peek_span();
        self.bump();
        let close_span = self.find_close_delimiter(close_token)?;
        let label = self.extract_raw_label(open_span.end, close_span.start);
        Ok(Some((shape, label)))
    }

    fn find_close_delimiter(&mut self, close_token: Token<'a>) -> Result<Range<usize>, MermaidError> {
        let start_pos = self.pos;
        while let Some((token, span)) = self.bump() {
            if token == close_token {
                return Ok(span);
            }
            if token == Token::Separator && !span.is_empty() && self.source.as_bytes()[span.start] == b'\n' {
                break;
            }
        }
        Err(MermaidError::Parse {
            message: format!("unclosed shape delimiter, expected {:?}", close_token),
            span: self.tokens.get(start_pos).map(|(_, s)| s.clone()).unwrap_or(0..0),
        })
    }

    fn extract_raw_label(&self, start: usize, end: usize) -> String {
        if start <= end && end <= self.source.len() {
            let mut s = self.source[start..end].trim();
            if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                s = &s[1..s.len() - 1];
            }
            s.to_string()
        } else {
            String::new()
        }
    }

    fn parse_edge_operator(&mut self) -> Result<Option<(EdgeType, Option<String>)>, MermaidError> {
        let edge_type = match self.peek() {
            Some(Token::ArrowSolid) => EdgeType::SolidArrow,
            Some(Token::LineSolid) => EdgeType::Solid,
            Some(Token::ArrowDotted) => EdgeType::DottedArrow,
            Some(Token::LineDotted) => EdgeType::Dotted,
            Some(Token::ArrowThick) => EdgeType::ThickArrow,
            Some(Token::LineThick) => EdgeType::Thick,
            Some(Token::EdgeTextOpenSolid) => {
                self.bump();
                return self.parse_inline_text_edge(EdgeType::SolidArrow, EdgeType::Solid);
            }
            Some(Token::EdgeTextOpenDotted) => {
                self.bump();
                return self.parse_inline_text_edge(EdgeType::DottedArrow, EdgeType::Dotted);
            }
            Some(Token::EdgeTextOpenThick) => {
                self.bump();
                return self.parse_inline_text_edge(EdgeType::ThickArrow, EdgeType::Thick);
            }
            _ => return Ok(None),
        };

        self.bump();
        let label = self.parse_pipe_label()?;
        Ok(Some((edge_type, label)))
    }

    fn parse_pipe_label(&mut self) -> Result<Option<String>, MermaidError> {
        if matches!(self.peek(), Some(Token::Pipe)) {
            let open_span = self.peek_span();
            self.bump();
            let close_span = self.find_close_delimiter(Token::Pipe)?;
            let text = self.extract_raw_label(open_span.end, close_span.start);
            Ok(Some(text))
        } else {
            Ok(None)
        }
    }

    fn parse_inline_text_edge(
        &mut self,
        arrow_kind: EdgeType,
        line_kind: EdgeType,
    ) -> Result<Option<(EdgeType, Option<String>)>, MermaidError> {
        let open_span = self.peek_span();

        while let Some((token, span)) = self.bump() {
            match token {
                Token::ArrowSolid | Token::ArrowDotted | Token::ArrowThick => {
                    let label_text = self.extract_raw_label(open_span.start, span.start);
                    return Ok(Some((arrow_kind, Some(label_text))));
                }
                Token::EdgeTextOpenSolid | Token::LineSolid | Token::LineDotted | Token::LineThick => {
                    let label_text = self.extract_raw_label(open_span.start, span.start);
                    return Ok(Some((line_kind, Some(label_text))));
                }
                _ => {}
            }
        }

        Err(MermaidError::Parse {
            message: "unterminated inline edge text".to_string(),
            span: open_span,
        })
    }
}
