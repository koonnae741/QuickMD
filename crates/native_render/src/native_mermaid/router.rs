//! Spline curve routing and ray-shape analytical boundary clipping.

use kurbo::{Affine, BezPath, Point, Rect, Vec2};

use crate::native_mermaid::ast::{DiagramSubgraph, Direction, EdgeType, NodeShape};
use crate::native_mermaid::sugiyama::LayoutNodeData;
use crate::native_math::{FontVariant, MathFontContext};

#[derive(Debug, Clone)]
pub struct RoutedEdge {
    pub original_index: usize,
    pub path: BezPath,
    pub arrowhead: Option<BezPath>,
    pub edge_type: EdgeType,
    pub label: Option<String>,
    pub label_pos: Option<Point>,
    pub start_pt: Point,
    pub end_pt: Point,
    pub ctrl1: Point,
    pub ctrl2: Point,
    pub is_simple_curve: bool,
    pub src_node_idx: usize,
    pub dst_node_idx: usize,
}

/// Evaluates a point on a cubic Bézier curve at parameter t in [0.0, 1.0].
pub fn eval_cubic_bezier(p0: Point, c1: Point, c2: Point, p3: Point, t: f64) -> Point {
    let t = t.clamp(0.0, 1.0);
    let mt = 1.0 - t;
    let mt2 = mt * mt;
    let mt3 = mt2 * mt;
    let t2 = t * t;
    let t3 = t2 * t;

    Point::new(
        mt3 * p0.x + 3.0 * mt2 * t * c1.x + 3.0 * mt * t2 * c2.x + t3 * p3.x,
        mt3 * p0.y + 3.0 * mt2 * t * c1.y + 3.0 * mt * t2 * c2.y + t3 * p3.y,
    )
}

/// Routes an edge chain into a smooth Bézier curve clipped to node boundaries.
#[allow(clippy::too_many_arguments)]
pub fn route_edge(
    chain_nodes: &[usize],
    all_nodes: &[LayoutNodeData],
    shape_from: NodeShape,
    shape_to: NodeShape,
    edge_type: EdgeType,
    label: Option<String>,
    direction: Direction,
    reversed: bool,
    param_t: f64,
) -> RoutedEdge {
    if chain_nodes.len() < 2 {
        return RoutedEdge {
            original_index: 0,
            path: BezPath::new(),
            arrowhead: None,
            edge_type,
            label: None,
            label_pos: None,
            start_pt: Point::ZERO,
            end_pt: Point::ZERO,
            ctrl1: Point::ZERO,
            ctrl2: Point::ZERO,
            is_simple_curve: false,
            src_node_idx: 0,
            dst_node_idx: 0,
        };
    }

    let u_idx = chain_nodes[0];
    let v_idx = chain_nodes[chain_nodes.len() - 1];
    let u_node = &all_nodes[u_idx];
    let v_node = &all_nodes[v_idx];

    let (src_node, dst_node, src_shape, dst_shape, src_actual_idx, dst_actual_idx) = if reversed {
        (v_node, u_node, shape_to, shape_from, v_idx, u_idx)
    } else {
        (u_node, v_node, shape_from, shape_to, u_idx, v_idx)
    };

    let p_src = Point::new(src_node.x, src_node.y);
    let p_dst = Point::new(dst_node.x, dst_node.y);

    // Collect all points along the chain
    let mut raw_points = Vec::with_capacity(chain_nodes.len());
    if reversed {
        for &idx in chain_nodes.iter().rev() {
            raw_points.push(Point::new(all_nodes[idx].x, all_nodes[idx].y));
        }
    } else {
        for &idx in chain_nodes {
            raw_points.push(Point::new(all_nodes[idx].x, all_nodes[idx].y));
        }
    }

    let p_after_src = raw_points.get(1).copied().unwrap_or(p_dst);
    let p_before_dst = raw_points.get(raw_points.len() - 2).copied().unwrap_or(p_src);

    // Analytical shape clipping
    let start_pt = clip_shape_boundary(p_src, p_after_src, src_node.width, src_node.height, src_shape);
    let end_pt = clip_shape_boundary(p_dst, p_before_dst, dst_node.width, dst_node.height, dst_shape);

    let mut path = BezPath::new();
    path.move_to(start_pt);

    let mut end_tangent = Vec2::ZERO;
    let mut ctrl1 = Point::ZERO;
    let mut ctrl2 = Point::ZERO;
    let is_simple = raw_points.len() == 2;

    let label_pos = if is_simple {
        // Direct single-segment curve
        let (c1, c2) = if direction.is_vertical() {
            let dy = end_pt.y - start_pt.y;
            (
                Point::new(start_pt.x, start_pt.y + dy * 0.5),
                Point::new(end_pt.x, end_pt.y - dy * 0.5),
            )
        } else {
            let dx = end_pt.x - start_pt.x;
            (
                Point::new(start_pt.x + dx * 0.5, start_pt.y),
                Point::new(end_pt.x - dx * 0.5, end_pt.y),
            )
        };
        ctrl1 = c1;
        ctrl2 = c2;
        path.curve_to(c1, c2, end_pt);
        end_tangent = end_pt - c2;
        eval_cubic_bezier(start_pt, c1, c2, end_pt, param_t)
    } else {
        // Multi-segment curve through dummy nodes
        let n = raw_points.len();
        let mut pts = raw_points;
        pts[0] = start_pt;
        pts[n - 1] = end_pt;

        for i in 0..(n - 1) {
            let p0 = pts[i];
            let p1 = pts[i + 1];
            if direction.is_vertical() {
                let dy = p1.y - p0.y;
                let c1 = Point::new(p0.x, p0.y + dy * 0.45);
                let c2 = Point::new(p1.x, p1.y - dy * 0.45);
                path.curve_to(c1, c2, p1);
                if i + 1 == n - 1 {
                    end_tangent = p1 - c2;
                }
            } else {
                let dx = p1.x - p0.x;
                let c1 = Point::new(p0.x + dx * 0.45, p0.y);
                let c2 = Point::new(p1.x - dx * 0.45, p1.y);
                path.curve_to(c1, c2, p1);
                if i + 1 == n - 1 {
                    end_tangent = p1 - c2;
                }
            }
        }
        let mid_idx = n / 2;
        let p_mid = pts[mid_idx];
        let p_prev = pts[mid_idx - 1];
        Point::new(
            (p_prev.x + p_mid.x) / 2.0,
            (p_prev.y + p_mid.y) / 2.0,
        )
    };

    // Build arrowhead if edge requires it
    let arrowhead = if edge_type.has_arrow() {
        Some(build_arrowhead(end_pt, end_tangent))
    } else {
        None
    };

    RoutedEdge {
        original_index: 0,
        path,
        arrowhead,
        edge_type,
        label,
        label_pos: Some(label_pos),
        start_pt,
        end_pt,
        ctrl1,
        ctrl2,
        is_simple_curve: is_simple,
        src_node_idx: src_actual_idx,
        dst_node_idx: dst_actual_idx,
    }
}

/// Staggers outgoing fan-out edge labels and relaxes AABB overlaps to ensure zero collisions.
pub fn deconflict_edge_labels(
    routed_edges: &mut [RoutedEdge],
    all_nodes: &[LayoutNodeData],
    subgraphs: &[DiagramSubgraph],
    fonts: &mut MathFontContext,
    font_size: f64,
    direction: Direction,
) {
    if routed_edges.is_empty() {
        return;
    }

    let is_vert = direction.is_vertical();

    // 1. Group labeled edges by source node to assign staggered curve parameter t
    let mut src_to_edges: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (i, edge) in routed_edges.iter().enumerate() {
        if edge.label.is_some() && edge.is_simple_curve {
            src_to_edges.entry(edge.src_node_idx).or_default().push(i);
        }
    }

    for edges in src_to_edges.values() {
        let count = edges.len();
        if count <= 1 {
            continue;
        }

        // Sort edges by destination coordinate (left-to-right for vertical, top-to-bottom for horizontal)
        let mut sorted = edges.clone();
        sorted.sort_by(|&a, &b| {
            let n_a = &all_nodes[routed_edges[a].dst_node_idx];
            let n_b = &all_nodes[routed_edges[b].dst_node_idx];
            if is_vert {
                n_a.x.partial_cmp(&n_b.x).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                n_a.y.partial_cmp(&n_b.y).unwrap_or(std::cmp::Ordering::Equal)
            }
        });

        // Assign alternating high/low t values so adjacent edges are never at the same rank coordinate
        for (rank_idx, &edge_idx) in sorted.iter().enumerate() {
            let t = if count == 2 {
                if rank_idx == 0 { 0.36 } else { 0.64 }
            } else if rank_idx % 2 == 0 {
                0.34 + 0.04 * (rank_idx as f64 / count as f64)
            } else {
                0.66 - 0.04 * (rank_idx as f64 / count as f64)
            };

            let edge = &mut routed_edges[edge_idx];
            edge.label_pos = Some(eval_cubic_bezier(edge.start_pt, edge.ctrl1, edge.ctrl2, edge.end_pt, t));
        }
    }

    // 2. Build AABB boxes for all labels
    let pad = 6.0;
    let mut label_boxes: Vec<(usize, Rect)> = Vec::with_capacity(routed_edges.len());
    for (i, edge) in routed_edges.iter().enumerate() {
        if let (Some(label), Some(pos)) = (&edge.label, edge.label_pos) {
            let run = fonts.shape(label, FontVariant::Roman, font_size * 0.85);
            let w = run.advance + pad * 2.0;
            let h = run.height + run.depth + pad * 2.0;
            label_boxes.push((
                i,
                Rect::new(pos.x - w / 2.0, pos.y - h / 2.0, pos.x + w / 2.0, pos.y + h / 2.0),
            ));
        }
    }

    // 3. Node and Subgraph Title obstacle boxes
    let mut obstacle_boxes: Vec<Rect> = all_nodes
        .iter()
        .filter(|n| !n.is_dummy)
        .map(|n| {
            let m = 6.0;
            Rect::new(
                n.x - n.width / 2.0 - m,
                n.y - n.height / 2.0 - m,
                n.x + n.width / 2.0 + m,
                n.y + n.height / 2.0 + m,
            )
        })
        .collect();

    for sub in subgraphs {
        if sub.title.is_empty() {
            continue;
        }
        let mut sub_min_x = f64::MAX;
        let mut sub_min_y = f64::MAX;
        let mut sub_max_x = f64::MIN;
        for node_id in &sub.node_ids {
            if let Some(node) = all_nodes.iter().find(|n| n.original_id.as_deref() == Some(node_id)) {
                sub_min_x = sub_min_x.min(node.x - node.width / 2.0);
                sub_min_y = sub_min_y.min(node.y - node.height / 2.0);
                sub_max_x = sub_max_x.max(node.x + node.width / 2.0);
            }
        }
        if sub_min_x < sub_max_x {
            let pad_x = 22.0;
            let pad_top = 46.0;
            let title_run = fonts.shape(&sub.title, FontVariant::Bold, font_size * 0.90);
            let pad_left = (title_run.advance + 24.0).max(pad_x);
            let title_x = sub_min_x - pad_left + 14.0;
            let title_y = sub_min_y - pad_top + 22.0;
            let m = 8.0;
            obstacle_boxes.push(Rect::new(
                title_x - m,
                title_y - title_run.height - m,
                title_x + title_run.advance + m,
                title_y + title_run.depth + m,
            ));
        }
    }

    // 4. Iterative AABB Relaxation (up to 12 iterations)
    for _ in 0..12 {
        let mut moved = false;
        let count = label_boxes.len();

        // Label vs Label deconfliction
        for a in 0..count {
            for b in (a + 1)..count {
                let rect_a = label_boxes[a].1;
                let rect_b = label_boxes[b].1;
                let inter = rect_a.intersect(rect_b);
                if inter.width() > 0.0 && inter.height() > 0.0 {
                    moved = true;
                    let ov_x = inter.width();
                    let ov_y = inter.height();

                    if ov_y <= ov_x {
                        let shift = (ov_y / 2.0) + 4.0;
                        if rect_a.center().y < rect_b.center().y {
                            label_boxes[a].1 = rect_a + Vec2::new(0.0, -shift);
                            label_boxes[b].1 = rect_b + Vec2::new(0.0, shift);
                        } else {
                            label_boxes[a].1 = rect_a + Vec2::new(0.0, shift);
                            label_boxes[b].1 = rect_b + Vec2::new(0.0, -shift);
                        }
                    } else {
                        let shift = (ov_x / 2.0) + 6.0;
                        if rect_a.center().x < rect_b.center().x {
                            label_boxes[a].1 = rect_a + Vec2::new(-shift, 0.0);
                            label_boxes[b].1 = rect_b + Vec2::new(shift, 0.0);
                        } else {
                            label_boxes[a].1 = rect_a + Vec2::new(shift, 0.0);
                            label_boxes[b].1 = rect_b + Vec2::new(-shift, 0.0);
                        }
                    }
                }
            }
        }

        // Label vs Obstacle (Node & Subgraph Title) deconfliction
        for item in &mut label_boxes {
            for obs in &obstacle_boxes {
                let inter = item.1.intersect(*obs);
                if inter.width() > 0.0 && inter.height() > 0.0 {
                    moved = true;
                    let ov_x = inter.width();
                    let ov_y = inter.height();
                    if is_vert {
                        let shift = ov_y + 8.0;
                        if item.1.center().y < obs.center().y {
                            item.1 = item.1 + Vec2::new(0.0, -shift);
                        } else {
                            item.1 = item.1 + Vec2::new(0.0, shift);
                        }
                    } else {
                        let shift = ov_x + 8.0;
                        if item.1.center().x < obs.center().x {
                            item.1 = item.1 + Vec2::new(-shift, 0.0);
                        } else {
                            item.1 = item.1 + Vec2::new(shift, 0.0);
                        }
                    }
                }
            }
        }

        if !moved {
            break;
        }
    }

    // 5. Commit deconflicted positions
    for (idx, rect) in label_boxes {
        routed_edges[idx].label_pos = Some(rect.center());
    }
}

/// Computes the intersection point where a ray from center exits the shape.
pub fn clip_shape_boundary(
    center: Point,
    target: Point,
    width: f64,
    height: f64,
    shape: NodeShape,
) -> Point {
    let dx = target.x - center.x;
    let dy = target.y - center.y;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist < 1e-4 {
        return center;
    }

    let half_w = (width / 2.0).max(10.0);
    let half_h = (height / 2.0).max(10.0);

    match shape {
        NodeShape::Circle => {
            let r = half_w.min(half_h);
            Point::new(center.x + (dx / dist) * r, center.y + (dy / dist) * r)
        }
        NodeShape::Diamond => {
            // Rhombus equation: |x|/half_w + |y|/half_h = 1
            let denom = dx.abs() / half_w + dy.abs() / half_h;
            let t = if denom > 1e-5 { 1.0 / denom } else { 1.0 };
            Point::new(center.x + dx * t, center.y + dy * t)
        }
        _ => {
            // AABB Ray Clipping for Rectangle, RoundedRect, Stadium, Cylinder, Hexagon
            let scale_x = if dx.abs() > 1e-5 { half_w / dx.abs() } else { f64::MAX };
            let scale_y = if dy.abs() > 1e-5 { half_h / dy.abs() } else { f64::MAX };
            let t = scale_x.min(scale_y);
            Point::new(center.x + dx * t, center.y + dy * t)
        }
    }
}

fn build_arrowhead(tip: Point, tangent: Vec2) -> BezPath {
    let angle = tangent.y.atan2(tangent.x);
    let head_len = 10.0;
    let head_width = 7.0;

    let p1 = Point::new(-head_len, -head_width / 2.0);
    let p2 = Point::new(-head_len, head_width / 2.0);

    let mut path = BezPath::new();
    path.move_to(Point::ORIGIN);
    path.line_to(p1);
    path.line_to(p2);
    path.close_path();

    let transform = Affine::translate(tip.to_vec2()) * Affine::rotate(angle);
    path.apply_affine(transform);
    path
}
