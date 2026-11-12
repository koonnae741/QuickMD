//! Rasterizes `MathBox` primitives into a `tiny_skia::Pixmap`.

use kurbo::{BezPath, PathEl, Rect, Vec2};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke,
    Transform,
};

use super::error::MathError;
use super::font::{MathFontContext, ShapedText};
use super::layout::{MathBox, PathPaint, Primitive};

pub const MAX_DIMENSION: u32 = 16_384;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// HiDPI factor: device pixels per logical pixel.
    pub scale: f32,
    /// Straight (non-premultiplied) RGBA.
    pub foreground: [u8; 4],
    /// `None` = fully transparent background.
    pub background: Option<[u8; 4]>,
    /// Logical-pixel margin around the formula.
    pub padding: f32,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { scale: 1.0, foreground: [0, 0, 0, 255], background: None, padding: 2.0 }
    }
}

pub fn rasterize(
    fonts: &mut MathFontContext,
    mbox: &MathBox,
    options: &RenderOptions,
) -> Result<Pixmap, MathError> {
    if !(options.scale.is_finite() && options.scale > 0.0) {
        return Err(MathError::Render { message: format!("invalid scale {}", options.scale) });
    }
    let scale = f64::from(options.scale);
    let pad = f64::from(options.padding.max(0.0));
    let width_px = ((mbox.width.max(0.0) + 2.0 * pad) * scale).ceil().max(1.0);
    let height_px = ((mbox.total_height().max(0.0) + 2.0 * pad) * scale).ceil().max(1.0);
    if !width_px.is_finite() || !height_px.is_finite()
        || width_px > f64::from(MAX_DIMENSION)
        || height_px > f64::from(MAX_DIMENSION)
    {
        return Err(MathError::Render {
            message: format!("surface {width_px}×{height_px} exceeds {MAX_DIMENSION}px"),
        });
    }
    let mut pixmap = Pixmap::new(width_px as u32, height_px as u32)
        .ok_or_else(|| MathError::Render { message: "pixmap allocation failed".into() })?;
    if let Some([r, g, b, a]) = options.background {
        pixmap.fill(Color::from_rgba8(r, g, b, a));
    }

    // Logical position of the formula's baseline origin.
    let origin = Vec2::new(pad, pad + mbox.height);
    let transform = Transform::from_row(
        options.scale,
        0.0,
        0.0,
        options.scale,
        (origin.x * scale) as f32,
        (origin.y * scale) as f32,
    );
    let [fr, fg, fb, fa] = options.foreground;
    let mut paint = Paint::default();
    paint.set_color_rgba8(fr, fg, fb, fa);
    paint.anti_alias = true;

    for item in &mbox.items {
        match item {
            Primitive::Rule(rect) => draw_rule(&mut pixmap, *rect, origin, scale, &paint),
            Primitive::Path { path, paint: style } => {
                let Some(sk_path) = to_skia_path(path) else { continue };
                match style {
                    PathPaint::Fill => {
                        pixmap.fill_path(&sk_path, &paint, FillRule::Winding, transform, None);
                    }
                    PathPaint::Stroke { width } => {
                        let stroke = Stroke {
                            width: *width as f32,
                            line_cap: LineCap::Round,
                            line_join: LineJoin::Round,
                            ..Stroke::default()
                        };
                        pixmap.stroke_path(&sk_path, &paint, &stroke, transform, None);
                    }
                }
            }
            Primitive::Glyphs { origin: run_origin, run } => {
                let device = (
                    ((origin.x + run_origin.x) * scale) as f32,
                    ((origin.y + run_origin.y) * scale) as f32,
                );
                draw_glyphs(fonts, &mut pixmap, run, device, options.scale, options.foreground);
            }
        }
    }
    Ok(pixmap)
}

/// Rules are snapped to ≥ 1 device pixel so thin bars never fade out.
fn draw_rule(pixmap: &mut Pixmap, rect: Rect, origin: Vec2, scale: f64, paint: &Paint<'_>) {
    let rect = rect.abs();
    let mut x0 = (rect.x0 + origin.x) * scale;
    let mut x1 = (rect.x1 + origin.x) * scale;
    let mut y0 = (rect.y0 + origin.y) * scale;
    let mut y1 = (rect.y1 + origin.y) * scale;
    if x1 - x0 < 1.0 {
        let c = (x0 + x1) / 2.0;
        x0 = c - 0.5;
        x1 = c + 0.5;
    }
    if y1 - y0 < 1.0 {
        let c = (y0 + y1) / 2.0;
        y0 = c - 0.5;
        y1 = c + 0.5;
    }
    if let Some(r) = tiny_skia::Rect::from_ltrb(x0 as f32, y0 as f32, x1 as f32, y1 as f32) {
        pixmap.fill_rect(r, paint, Transform::identity(), None);
    }
}

fn to_skia_path(path: &BezPath) -> Option<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    for element in path.elements() {
        match *element {
            PathEl::MoveTo(p) => builder.move_to(p.x as f32, p.y as f32),
            PathEl::LineTo(p) => builder.line_to(p.x as f32, p.y as f32),
            PathEl::QuadTo(a, b) => builder.quad_to(a.x as f32, a.y as f32, b.x as f32, b.y as f32),
            PathEl::CurveTo(a, b, c) => builder.cubic_to(
                a.x as f32, a.y as f32, b.x as f32, b.y as f32, c.x as f32, c.y as f32,
            ),
            PathEl::ClosePath => builder.close(),
        }
    }
    builder.finish()
}

pub(crate) fn draw_glyphs(
    fonts: &mut MathFontContext,
    pixmap: &mut Pixmap,
    run: &ShapedText,
    origin: (f32, f32),
    scale: f32,
    foreground: [u8; 4],
) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let pixels = pixmap.pixels_mut();
    let base = cosmic_text::Color::rgba(foreground[0], foreground[1], foreground[2], 255);
    let opacity = u32::from(foreground[3]);
    for glyph in &run.glyphs {
        let physical = glyph.physical(origin, scale);
        fonts.swash.with_pixels(&mut fonts.font_system, physical.cache_key, base, |dx, dy, color| {
            let x = physical.x + dx;
            let y = physical.y + dy;
            if x < 0 || y < 0 || x >= width || y >= height {
                return;
            }
            let alpha = mul_div255(u32::from(color.a()), opacity);
            if alpha == 0 {
                return;
            }
            let index = y as usize * width as usize + x as usize;
            blend_over(&mut pixels[index], color.r(), color.g(), color.b(), alpha);
        });
    }
}

#[inline]
fn mul_div255(a: u32, b: u32) -> u32 {
    (a * b + 127) / 255
}

/// Source-over compositing of a straight-alpha colour onto a premultiplied pixel.
#[inline]
fn blend_over(dst: &mut PremultipliedColorU8, r: u8, g: u8, b: u8, alpha: u32) {
    let sa = alpha.min(255);
    let inv = 255 - sa;
    let oa = sa + mul_div255(u32::from(dst.alpha()), inv);
    let or = (mul_div255(u32::from(r), sa) + mul_div255(u32::from(dst.red()), inv)).min(oa);
    let og = (mul_div255(u32::from(g), sa) + mul_div255(u32::from(dst.green()), inv)).min(oa);
    let ob = (mul_div255(u32::from(b), sa) + mul_div255(u32::from(dst.blue()), inv)).min(oa);
    if let Some(c) = PremultipliedColorU8::from_rgba(or as u8, og as u8, ob as u8, oa.min(255) as u8) {
        *dst = c;
    }
}