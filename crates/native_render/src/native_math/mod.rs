//! `native_math`: pure-Rust TeX math typesetting (Appendix G) + tiny-skia rasterizer.

pub mod ast;
pub mod error;
pub mod font;
pub mod layout;
pub mod parser;
pub mod rasterizer;
pub mod symbols;
pub mod tokens;

pub use ast::{FontVariant, MathList, MathNode, MathStyle};
pub use font::ShapedText;
pub use error::MathError;
pub use font::{MathConstants, MathFontContext};
pub use layout::{LayoutEngine, MathBox, PathPaint, Primitive};
pub use rasterizer::{rasterize, RenderOptions};

use tiny_skia::Pixmap;

/// High-level façade: parse → layout → rasterize.
pub struct MathRenderer {
    fonts: MathFontContext,
}

impl MathRenderer {
    pub fn new(fonts: MathFontContext) -> Self {
        Self { fonts }
    }

    pub fn with_system_fonts() -> Self {
        Self::new(MathFontContext::from_system(None))
    }

    pub fn fonts_mut(&mut self) -> &mut MathFontContext {
        &mut self.fonts
    }

    pub fn layout(&mut self, source: &str, style: MathStyle, font_size: f64) -> Result<MathBox, MathError> {
        if !(font_size.is_finite() && font_size > 0.0 && font_size <= 4096.0) {
            return Err(MathError::Render { message: format!("invalid font size {font_size}") });
        }
        let ast = parser::parse(source)?;
        Ok(LayoutEngine::new(&mut self.fonts, font_size).layout(&ast, style))
    }

    pub fn render(
        &mut self,
        source: &str,
        style: MathStyle,
        font_size: f64,
        options: &RenderOptions,
    ) -> Result<Pixmap, MathError> {
        let mbox = self.layout(source, style, font_size)?;
        rasterize(&mut self.fonts, &mbox, options)
    }
}

#[cfg(test)]
mod tests {
    use super::ast::{FontVariant, MatrixKind};
    use super::*;

    // ---------- parser (font-independent) ----------

    #[test]
    fn parses_fraction() {
        let ast = parser::parse(r"\frac{a}{b}").unwrap();
        assert!(matches!(ast.as_slice(), [MathNode::Fraction { has_bar: true, .. }]));
    }

    #[test]
    fn groups_decimal_numbers() {
        let ast = parser::parse("3.14 + 2").unwrap();
        assert_eq!(ast[0], MathNode::Number("3.14".into()));
        assert_eq!(ast[2], MathNode::Number("2".into()));
    }

    #[test]
    fn primes_become_superscripts() {
        let ast = parser::parse("f'(x)").unwrap();
        match &ast[0] {
            MathNode::Scripts { sup: Some(sup), sub: None, .. } => assert_eq!(sup.len(), 1),
            other => panic!("unexpected node {other:?}"),
        }
    }

    #[test]
    fn parses_matrix_and_drops_trailing_row_break() {
        let ast = parser::parse(r"\begin{pmatrix} a & b \\ c & d \\ \end{pmatrix}").unwrap();
        match &ast[0] {
            MathNode::Matrix { kind: MatrixKind::Paren, rows } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].len(), 2);
            }
            other => panic!("unexpected node {other:?}"),
        }
    }

    #[test]
    fn text_preserves_spaces() {
        let ast = parser::parse(r"\text{if } x").unwrap();
        assert_eq!(ast[0], MathNode::Text { content: "if ".into(), variant: FontVariant::Roman });
        assert_eq!(ast[1], MathNode::Identifier('x'));
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(parser::parse("{a").is_err());
        assert!(parser::parse("a}").is_err());
        assert!(parser::parse(r"\left( a").is_err());
        assert!(parser::parse("x^1^2").is_err());
        assert!(matches!(parser::parse(r"\notacommand"), Err(MathError::Parse { .. })));
        assert!(matches!(parser::parse("a # b"), Err(MathError::Lex { .. })));
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let source = "{".repeat(1000) + &"}".repeat(1000);
        assert!(parser::parse(&source).is_err());
    }

    // ---------- layout & raster (skipped when no fonts are installed) ----------

    fn renderer() -> Option<MathRenderer> {
        let fonts = MathFontContext::from_system(None);
        fonts.has_fonts().then(|| MathRenderer::new(fonts))
    }

    #[test]
    fn fraction_straddles_axis() {
        let Some(mut r) = renderer() else { return };
        let plain = r.layout("x", MathStyle::Display, 20.0).unwrap();
        let frac = r.layout(r"\frac{x}{y}", MathStyle::Display, 20.0).unwrap();
        assert!(frac.height > plain.height);
        assert!(frac.depth > plain.depth.max(0.0));
        assert!(frac.items.iter().any(|i| matches!(i, Primitive::Rule(_))));
    }

    #[test]
    fn scripts_shift_vertically() {
        let Some(mut r) = renderer() else { return };
        let base = r.layout("x", MathStyle::Text, 20.0).unwrap();
        let sup = r.layout("x^2", MathStyle::Text, 20.0).unwrap();
        let sub = r.layout("x_2", MathStyle::Text, 20.0).unwrap();
        assert!(sup.height > base.height);
        assert!(sub.depth > base.depth);
        assert!(sup.width > base.width);
    }

    #[test]
    fn radical_clears_radicand() {
        let Some(mut r) = renderer() else { return };
        let body = r.layout("x", MathStyle::Text, 20.0).unwrap();
        let root = r.layout(r"\sqrt{x}", MathStyle::Text, 20.0).unwrap();
        assert!(root.height > body.height);
        assert!(root.items.iter().any(|i| matches!(i, Primitive::Path { .. })));
    }

    #[test]
    fn delimiters_grow_with_content() {
        let Some(mut r) = renderer() else { return };
        let frac = r.layout(r"\frac{a}{b}", MathStyle::Display, 20.0).unwrap();
        let wrapped = r.layout(r"\left( \frac{a}{b} \right)", MathStyle::Display, 20.0).unwrap();
        assert!(wrapped.total_height() >= frac.total_height());
        assert!(wrapped.width > frac.width);
    }

    #[test]
    fn rasterizes_visible_pixels() {
        let Some(mut r) = renderer() else { return };
        let options = RenderOptions { scale: 2.0, ..RenderOptions::default() };
        let pixmap = r.render(r"a + \sqrt{b}", MathStyle::Text, 20.0, &options).unwrap();
        assert!(pixmap.pixels().iter().any(|p| p.alpha() > 0));
    }
}