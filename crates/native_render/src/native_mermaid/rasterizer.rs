//! Rasterization of Mermaid diagrams into a `tiny_skia::Pixmap` with `cosmic-text` typography.

use kurbo::{Affine, BezPath, Circle, Point, Rect, RoundedRect, Shape};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::native_math::rasterizer::draw_glyphs;
use crate::native_math::{FontVariant, MathFontContext};
use crate::native_mermaid::ast::{DiagramGraph, NodeShape};
use crate::native_mermaid::router::RoutedEdge;
use crate::native_mermaid::sugiyama::LayeredGraph;

#[derive(Debug, Clone)]
pub struct DiagramTheme {
    pub background: Option<[u8; 4]>,
    pub node_fill: [u8; 4],
    pub node_stroke: [u8; 4],
    pub node_stroke_width: f32,
    pub node_text_color: [u8; 4],
    pub edge_stroke: [u8; 4],
    pub edge_stroke_width: f32,
    pub font_size: f64,
    pub scale: f32,
}

impl Default for DiagramTheme {
    fn default() -> Self {
        Self {
            background: Some([255, 255, 255, 255]),
            node_fill: [238, 242, 255, 255],     // Soft indigo/lavender fill
            node_stroke: [99, 102, 241, 255],    // Indigo stroke
            node_stroke_width: 1.8,
            node_text_color: [30, 41, 59, 255],  // Slate dark text
            edge_stroke: [100, 116, 139, 255],   // Slate edge
            edge_stroke_width: 1.6,
            font_size: 14.0,
            scale: 2.0,
        }
    }
}

pub const MAX_MERMAID_DIMENSION: u32 = 8192;

pub fn rasterize_diagram(
    graph: &DiagramGraph,
    layered: &LayeredGraph,
    edges: &[RoutedEdge],
    fonts: &mut MathFontContext,
    theme: &DiagramTheme,
) -> Option<Pixmap> {
    // 1. Calculate overall diagram bounds including nodes, subgraphs, and edges
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = 100.0f64;
    let mut max_y = 100.0f64;

    for node in &layered.nodes {
        if !node.is_dummy {
            min_x = min_x.min(node.x - node.width / 2.0);
            min_y = min_y.min(node.y - node.height / 2.0);
            max_x = max_x.max(node.x + node.width / 2.0 + 32.0);
            max_y = max_y.max(node.y + node.height / 2.0 + 32.0);
        }
    }

    let sub_pad = 22.0f64;
    let sub_pad_top = 46.0f64;
    for sub in &graph.subgraphs {
        let title_run = fonts.shape(&sub.title, FontVariant::Bold, theme.font_size * 0.90);
        let pad_left = if sub.title.is_empty() { sub_pad } else { (title_run.advance + 24.0).max(sub_pad) };
        for node_id in &sub.node_ids {
            if let Some(node) = layered.nodes.iter().find(|n| n.original_id.as_deref() == Some(node_id)) {
                min_x = min_x.min(node.x - node.width / 2.0 - pad_left);
                min_y = min_y.min(node.y - node.height / 2.0 - sub_pad_top);
                max_x = max_x.max(node.x + node.width / 2.0 + sub_pad + 16.0);
                max_y = max_y.max(node.y + node.height / 2.0 + sub_pad + 16.0);
            }
        }
    }

    for edge in edges {
        for pt in edge.path.elements() {
            match pt {
                kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => {
                    min_x = min_x.min(p.x);
                    min_y = min_y.min(p.y);
                    max_x = max_x.max(p.x + 24.0);
                    max_y = max_y.max(p.y + 24.0);
                }
                kurbo::PathEl::CurveTo(p1, p2, p3) => {
                    min_x = min_x.min(p1.x).min(p2.x).min(p3.x);
                    min_y = min_y.min(p1.y).min(p2.y).min(p3.y);
                    max_x = max_x.max(p1.x.max(p2.x).max(p3.x) + 24.0);
                    max_y = max_y.max(p1.y.max(p2.y).max(p3.y) + 24.0);
                }
                _ => {}
            }
        }
    }

    let margin = 20.0f64;
    let offset_x = if min_x < margin { margin - min_x } else { 0.0 };
    let offset_y = if min_y < margin { margin - min_y } else { 0.0 };
    let total_w = max_x + offset_x + margin;
    let total_h = max_y + offset_y + margin;

    // Shift all nodes and edges so no content is ever clipped at < margin
    let mut shifted_nodes = layered.nodes.clone();
    for node in &mut shifted_nodes {
        node.x += offset_x;
        node.y += offset_y;
    }

    let mut shifted_edges = edges.to_vec();
    if offset_x != 0.0 || offset_y != 0.0 {
        let affine = Affine::translate((offset_x, offset_y));
        for edge in &mut shifted_edges {
            edge.path.apply_affine(affine);
            if let Some(ref mut arr) = edge.arrowhead {
                arr.apply_affine(affine);
            }
            if let Some(ref mut pos) = edge.label_pos {
                pos.x += offset_x;
                pos.y += offset_y;
            }
        }
    }

    let mut scale = (theme.scale as f64).max(1.0);
    let mut px_width = (total_w * scale).ceil();
    let mut px_height = (total_h * scale).ceil();

    if px_width > MAX_MERMAID_DIMENSION as f64 || px_height > MAX_MERMAID_DIMENSION as f64 {
        let max_dim = px_width.max(px_height);
        let factor = (MAX_MERMAID_DIMENSION as f64) / max_dim;
        scale *= factor;
        px_width = (total_w * scale).ceil();
        px_height = (total_h * scale).ceil();
    }

    let px_w = (px_width as u32).clamp(1, MAX_MERMAID_DIMENSION);
    let px_h = (px_height as u32).clamp(1, MAX_MERMAID_DIMENSION);
    let scale = scale as f32;

    let mut pixmap = Pixmap::new(px_w, px_h)?;

    if let Some(bg) = theme.background {
        pixmap.fill(Color::from_rgba8(bg[0], bg[1], bg[2], bg[3]));
    }

    let transform = Transform::from_scale(scale, scale);

    // 2. Draw Subgraph clusters (if any)
    for sub in &graph.subgraphs {
        let mut sub_min_x = f64::MAX;
        let mut sub_min_y = f64::MAX;
        let mut sub_max_x = f64::MIN;
        let mut sub_max_y = f64::MIN;

        for node_id in &sub.node_ids {
            if let Some(node) = shifted_nodes.iter().find(|n| n.original_id.as_deref() == Some(node_id)) {
                sub_min_x = sub_min_x.min(node.x - node.width / 2.0);
                sub_min_y = sub_min_y.min(node.y - node.height / 2.0);
                sub_max_x = sub_max_x.max(node.x + node.width / 2.0);
                sub_max_y = sub_max_y.max(node.y + node.height / 2.0);
            }
        }

        if sub_min_x < sub_max_x {
            let pad_x = 22.0;
            let pad_top = 46.0;
            let pad_bottom = 22.0;
            let title_run = fonts.shape(&sub.title, FontVariant::Bold, theme.font_size * 0.90);
            let pad_left = if sub.title.is_empty() { pad_x } else { (title_run.advance + 24.0).max(pad_x) };

            let sub_rect = Rect::new(sub_min_x - pad_left, sub_min_y - pad_top, sub_max_x + pad_x, sub_max_y + pad_bottom);
            let rrect = RoundedRect::from_rect(sub_rect, 8.0);
            if let Some(path) = kurbo_to_tiny_skia(&rrect.to_path(0.1)) {
                // Subtle card background
                let bg_paint = Paint {
                    shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                        theme.node_fill[0],
                        theme.node_fill[1],
                        theme.node_fill[2],
                        35,
                    )),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.fill_path(&path, &bg_paint, FillRule::Winding, transform, None);

                let stroke = Stroke { width: 1.2, ..Default::default() };
                let stroke_paint = Paint {
                    shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                        theme.edge_stroke[0],
                        theme.edge_stroke[1],
                        theme.edge_stroke[2],
                        120,
                    )),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.stroke_path(&path, &stroke_paint, &stroke, transform, None);
            }
        }
    }

    // 3. Draw Edges
    for edge in &shifted_edges {
        if let Some(path) = kurbo_to_tiny_skia(&edge.path) {
            let stroke_width = if edge.edge_type.is_thick() {
                theme.edge_stroke_width * 1.8
            } else {
                theme.edge_stroke_width
            };
            let stroke = Stroke { width: stroke_width, ..Default::default() };

            let stroke_paint = Paint {
                shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                    theme.edge_stroke[0],
                    theme.edge_stroke[1],
                    theme.edge_stroke[2],
                    theme.edge_stroke[3],
                )),
                anti_alias: true,
                ..Paint::default()
            };

            pixmap.stroke_path(&path, &stroke_paint, &stroke, transform, None);
        }

        // Draw Arrowhead
        if let Some(ref arrow_path) = edge.arrowhead {
            if let Some(path) = kurbo_to_tiny_skia(arrow_path) {
                let fill_paint = Paint {
                    shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                        theme.edge_stroke[0],
                        theme.edge_stroke[1],
                        theme.edge_stroke[2],
                        theme.edge_stroke[3],
                    )),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.fill_path(&path, &fill_paint, FillRule::Winding, transform, None);
            }
        }

        // Draw Edge Label (if present)
        if let (Some(label), Some(pos)) = (&edge.label, edge.label_pos) {
            let label_run = fonts.shape(label, FontVariant::Roman, theme.font_size * 0.85);
            let pad = 5.0;
            let bg_w = label_run.advance + pad * 2.0;
            let bg_h = (label_run.height + label_run.depth) + pad * 2.0;
            let bg_rect = Rect::new(pos.x - bg_w / 2.0, pos.y - bg_h / 2.0, pos.x + bg_w / 2.0, pos.y + bg_h / 2.0);

            if let Some(bg_path) = kurbo_to_tiny_skia(&RoundedRect::from_rect(bg_rect, 4.0).to_path(0.1)) {
                let bg_paint = Paint {
                    shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                        theme.node_fill[0],
                        theme.node_fill[1],
                        theme.node_fill[2],
                        255,
                    )),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.fill_path(&bg_path, &bg_paint, FillRule::Winding, transform, None);

                let stroke = Stroke { width: 1.0, ..Default::default() };
                let stroke_paint = Paint {
                    shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                        theme.edge_stroke[0],
                        theme.edge_stroke[1],
                        theme.edge_stroke[2],
                        140,
                    )),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.stroke_path(&bg_path, &stroke_paint, &stroke, transform, None);
            }

            let text_origin = (
                ((pos.x - label_run.advance / 2.0) * scale as f64) as f32,
                ((pos.y + label_run.height / 2.0 - 2.0) * scale as f64) as f32,
            );
            draw_glyphs(fonts, &mut pixmap, &label_run, text_origin, scale, theme.node_text_color);
        }
    }

    // 3.5 Draw Subgraph Titles with Protective Shields (Drawn over edges so lines never cut through title)
    for sub in &graph.subgraphs {
        if sub.title.is_empty() {
            continue;
        }

        let mut sub_min_x = f64::MAX;
        let mut sub_min_y = f64::MAX;
        let mut sub_max_x = f64::MIN;

        for node_id in &sub.node_ids {
            if let Some(node) = shifted_nodes.iter().find(|n| n.original_id.as_deref() == Some(node_id)) {
                sub_min_x = sub_min_x.min(node.x - node.width / 2.0);
                sub_min_y = sub_min_y.min(node.y - node.height / 2.0);
                sub_max_x = sub_max_x.max(node.x + node.width / 2.0);
            }
        }

        if sub_min_x < sub_max_x {
            let pad_x = 22.0;
            let pad_top = 46.0;
            let title_run = fonts.shape(&sub.title, FontVariant::Bold, theme.font_size * 0.90);
            let pad_left = (title_run.advance + 24.0).max(pad_x);

            let title_x = sub_min_x - pad_left + 14.0;
            let title_y = sub_min_y - pad_top + 22.0;

            // Protective solid background pill for the title
            let shield_pad_x = 8.0;
            let shield_pad_y = 5.0;
            let shield_rect = Rect::new(
                title_x - shield_pad_x,
                title_y - title_run.height - shield_pad_y,
                title_x + title_run.advance + shield_pad_x,
                title_y + title_run.depth + shield_pad_y,
            );
            let shield_rrect = RoundedRect::from_rect(shield_rect, 5.0);
            if let Some(spath) = kurbo_to_tiny_skia(&shield_rrect.to_path(0.1)) {
                let shield_paint = Paint {
                    shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                        theme.node_fill[0],
                        theme.node_fill[1],
                        theme.node_fill[2],
                        255,
                    )),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.fill_path(&spath, &shield_paint, FillRule::Winding, transform, None);

                let shield_stroke = Stroke { width: 1.0, ..Default::default() };
                let shield_stroke_paint = Paint {
                    shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                        theme.edge_stroke[0],
                        theme.edge_stroke[1],
                        theme.edge_stroke[2],
                        100,
                    )),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.stroke_path(&spath, &shield_stroke_paint, &shield_stroke, transform, None);
            }

            let title_origin = (
                (title_x * scale as f64) as f32,
                (title_y * scale as f64) as f32,
            );
            let title_color = [
                theme.node_text_color[0],
                theme.node_text_color[1],
                theme.node_text_color[2],
                240,
            ];
            draw_glyphs(fonts, &mut pixmap, &title_run, title_origin, scale, title_color);
        }
    }

    // 4. Draw Nodes
    for node in &shifted_nodes {
        if node.is_dummy {
            continue;
        }

        let orig_id = node.original_id.as_deref().unwrap_or("");
        let shape = graph.nodes.get(orig_id).map(|n| n.shape).unwrap_or(NodeShape::Rectangle);
        let label = graph.nodes.get(orig_id).map(|n| n.label.as_str()).unwrap_or(orig_id);

        let shape_path = build_node_shape_path(node.x, node.y, node.width, node.height, shape);
        if let Some(path) = kurbo_to_tiny_skia(&shape_path) {
            let fill_paint = Paint {
                shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                    theme.node_fill[0],
                    theme.node_fill[1],
                    theme.node_fill[2],
                    theme.node_fill[3],
                )),
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.fill_path(&path, &fill_paint, FillRule::Winding, transform, None);

            let stroke = Stroke { width: theme.node_stroke_width, ..Default::default() };
            let stroke_paint = Paint {
                shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                    theme.node_stroke[0],
                    theme.node_stroke[1],
                    theme.node_stroke[2],
                    theme.node_stroke[3],
                )),
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.stroke_path(&path, &stroke_paint, &stroke, transform, None);
        }

        // Render Node Label centered in device pixels
        let shaped = fonts.shape(label, FontVariant::Roman, theme.font_size);
        let text_origin = (
            ((node.x - shaped.advance / 2.0) * scale as f64) as f32,
            ((node.y + (shaped.height - shaped.depth) / 2.0) * scale as f64) as f32,
        );
        draw_glyphs(fonts, &mut pixmap, &shaped, text_origin, scale, theme.node_text_color);
    }

    Some(pixmap)
}

fn build_node_shape_path(cx: f64, cy: f64, width: f64, height: f64, shape: NodeShape) -> BezPath {
    let hw = width / 2.0;
    let hh = height / 2.0;
    let rect = Rect::new(cx - hw, cy - hh, cx + hw, cy + hh);

    match shape {
        NodeShape::Rectangle => RoundedRect::from_rect(rect, 6.0).to_path(0.1),
        NodeShape::RoundedRect => RoundedRect::from_rect(rect, 10.0).to_path(0.1),
        NodeShape::Stadium => RoundedRect::from_rect(rect, hh).to_path(0.1),
        NodeShape::Circle => {
            let r = hw.max(hh);
            Circle::new(Point::new(cx, cy), r).to_path(0.1)
        }
        NodeShape::Diamond => {
            let mut p = BezPath::new();
            p.move_to(Point::new(cx, cy - hh));
            p.line_to(Point::new(cx + hw, cy));
            p.line_to(Point::new(cx, cy + hh));
            p.line_to(Point::new(cx - hw, cy));
            p.close_path();
            p
        }
        NodeShape::Hexagon => {
            let chamfer = (hw * 0.25).min(16.0);
            let mut p = BezPath::new();
            p.move_to(Point::new(cx - hw + chamfer, cy - hh));
            p.line_to(Point::new(cx + hw - chamfer, cy - hh));
            p.line_to(Point::new(cx + hw, cy));
            p.line_to(Point::new(cx + hw - chamfer, cy + hh));
            p.line_to(Point::new(cx - hw + chamfer, cy + hh));
            p.line_to(Point::new(cx - hw, cy));
            p.close_path();
            p
        }
        _ => RoundedRect::from_rect(rect, 6.0).to_path(0.1),
    }
}

fn kurbo_to_tiny_skia(bez: &BezPath) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for el in bez.elements() {
        match el {
            kurbo::PathEl::MoveTo(p) => pb.move_to(p.x as f32, p.y as f32),
            kurbo::PathEl::LineTo(p) => pb.line_to(p.x as f32, p.y as f32),
            kurbo::PathEl::QuadTo(p1, p2) => pb.quad_to(p1.x as f32, p1.y as f32, p2.x as f32, p2.y as f32),
            kurbo::PathEl::CurveTo(p1, p2, p3) => pb.cubic_to(
                p1.x as f32, p1.y as f32,
                p2.x as f32, p2.y as f32,
                p3.x as f32, p3.y as f32,
            ),
            kurbo::PathEl::ClosePath => pb.close(),
        }
    }
    pb.finish()
}
