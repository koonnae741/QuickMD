//! Font access: cosmic-text shaping, ink metrics and math-constant calibration.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use cosmic_text::{
    fontdb, Attrs, Buffer, Family, FontSystem, LayoutGlyph, Metrics, Shaping, Style, SwashCache,
    Weight,
};

use super::ast::FontVariant;

/// Pixel size at which glyph ink boxes are measured (then normalized to em).
const INK_REFERENCE_PX: f32 = 96.0;
const MAX_SHAPE_CACHE: usize = 4096;

/// TeX font parameters (σ/ξ values of cmsy10/cmex10) in em units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathConstants {
    pub x_height: f64,
    pub quad: f64,
    pub num1: f64,
    pub num2: f64,
    pub num3: f64,
    pub denom1: f64,
    pub denom2: f64,
    pub sup1: f64,
    pub sup2: f64,
    pub sup3: f64,
    pub sub1: f64,
    pub sub2: f64,
    pub sup_drop: f64,
    pub sub_drop: f64,
    pub delim1: f64,
    pub delim2: f64,
    pub axis_height: f64,
    pub rule_thickness: f64,
    pub big_op_spacing1: f64,
    pub big_op_spacing2: f64,
    pub big_op_spacing3: f64,
    pub big_op_spacing4: f64,
    pub big_op_spacing5: f64,
    pub null_delimiter_space: f64,
    pub script_space: f64,
    pub delimiter_factor: f64,
    pub delimiter_shortfall: f64,
}

impl MathConstants {
    pub const COMPUTER_MODERN: Self = Self {
        x_height: 0.430554,
        quad: 1.0,
        num1: 0.676508,
        num2: 0.393732,
        num3: 0.443731,
        denom1: 0.685951,
        denom2: 0.344841,
        sup1: 0.412892,
        sup2: 0.362892,
        sup3: 0.288889,
        sub1: 0.15,
        sub2: 0.247217,
        sup_drop: 0.386108,
        sub_drop: 0.05,
        delim1: 2.39,
        delim2: 1.01,
        axis_height: 0.25,
        rule_thickness: 0.04,
        big_op_spacing1: 0.111112,
        big_op_spacing2: 0.166667,
        big_op_spacing3: 0.2,
        big_op_spacing4: 0.6,
        big_op_spacing5: 0.1,
        null_delimiter_space: 0.12,
        script_space: 0.05,
        delimiter_factor: 0.901,
        delimiter_shortfall: 0.5,
    };
}

impl Default for MathConstants {
    fn default() -> Self {
        Self::COMPUTER_MODERN
    }
}

/// A shaped run with TeX-style metrics (logical px, baseline-relative).
#[derive(Clone)]
pub struct ShapedText {
    pub glyphs: Vec<LayoutGlyph>,
    /// Advance width.
    pub advance: f64,
    /// Ink extent above the baseline.
    pub height: f64,
    /// Ink extent below the baseline (negative if the ink is entirely above it).
    pub depth: f64,
    pub ink_left: f64,
    pub ink_right: f64,
    /// Ink overhang past the advance (TeX italic correction).
    pub italic: f64,
}

impl fmt::Debug for ShapedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShapedText")
            .field("glyphs", &self.glyphs.len())
            .field("advance", &self.advance)
            .field("height", &self.height)
            .field("depth", &self.depth)
            .field("italic", &self.italic)
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
struct Ink {
    ascent: f64,
    descent: f64,
    left: f64,
    right: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ShapeKey {
    text: String,
    variant: FontVariant,
    size_bits: u64,
}

pub struct MathFontContext {
    pub(crate) font_system: FontSystem,
    pub(crate) swash: SwashCache,
    family: Option<String>,
    constants: MathConstants,
    shapes: HashMap<ShapeKey, Arc<ShapedText>>,
    inks: HashMap<(fontdb::ID, u16), Option<Ink>>,
}

impl MathFontContext {
    /// Scans installed system fonts (convenient, but slower cold boot).
    pub fn from_system(family: Option<&str>) -> Self {
        Self::build(FontSystem::new(), family.map(str::to_owned))
    }

    /// Fastest cold boot: only the given font files (e.g. `include_bytes!`) are loaded.
    pub fn from_font_data<I>(fonts: I, family: Option<&str>) -> Self
    where
        I: IntoIterator<Item = Vec<u8>>,
    {
        let mut db = fontdb::Database::new();
        for data in fonts {
            db.load_font_data(data);
        }
        let detected = family.map(str::to_owned).or_else(|| {
            db.faces().find_map(|face| face.families.first().map(|(name, _)| name.clone()))
        });
        let font_system = FontSystem::new_with_locale_and_db("en-US".to_owned(), db);
        Self::build(font_system, detected)
    }

    pub fn with_font_system(font_system: FontSystem, family: Option<&str>) -> Self {
        Self::build(font_system, family.map(str::to_owned))
    }

    fn build(font_system: FontSystem, family: Option<String>) -> Self {
        let mut ctx = Self {
            font_system,
            swash: SwashCache::new(),
            family,
            constants: MathConstants::COMPUTER_MODERN,
            shapes: HashMap::new(),
            inks: HashMap::new(),
        };
        ctx.calibrate();
        ctx
    }

    pub fn constants(&self) -> MathConstants {
        self.constants
    }

    pub fn set_constants(&mut self, constants: MathConstants) {
        self.constants = constants;
    }

    pub fn has_fonts(&self) -> bool {
        self.font_system.db().faces().next().is_some()
    }

    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Releases shaped runs, ink metrics and rasterized glyph images.
    pub fn clear_caches(&mut self) {
        self.shapes.clear();
        self.inks.clear();
        self.swash = SwashCache::new();
    }

    /// Shapes `text` at `size` logical px (cached).
    pub fn shape(&mut self, text: &str, variant: FontVariant, size: f64) -> Arc<ShapedText> {
        let size = if size.is_finite() { size.clamp(0.5, 4096.0) } else { 16.0 };
        let key = ShapeKey { text: text.to_owned(), variant, size_bits: size.to_bits() };
        if let Some(hit) = self.shapes.get(&key) {
            return Arc::clone(hit);
        }

        let glyphs = self.shape_glyphs(text, variant, size as f32);
        let mut advance: f64 = 0.0;
        let mut ascent = f64::NEG_INFINITY;
        let mut descent = f64::NEG_INFINITY;
        let mut left = f64::INFINITY;
        let mut right = f64::NEG_INFINITY;
        for glyph in &glyphs {
            advance = advance.max(f64::from(glyph.x + glyph.w));
            let Some(ink) = self.ink(glyph) else { continue };
            let em = f64::from(glyph.font_size);
            let gx = f64::from(glyph.x) + f64::from(glyph.x_offset) * em;
            let gy = f64::from(glyph.y) - f64::from(glyph.y_offset) * em;
            ascent = ascent.max(ink.ascent * em - gy);
            descent = descent.max(ink.descent * em + gy);
            left = left.min(gx + ink.left * em);
            right = right.max(gx + ink.right * em);
        }

        let shaped = if ascent.is_finite() {
            ShapedText {
                glyphs,
                advance,
                height: ascent,
                depth: descent,
                ink_left: left,
                ink_right: right,
                italic: (right - advance).max(0.0),
            }
        } else {
            ShapedText {
                glyphs,
                advance,
                height: 0.0,
                depth: 0.0,
                ink_left: 0.0,
                ink_right: 0.0,
                italic: 0.0,
            }
        };
        let shaped = Arc::new(shaped);
        if self.shapes.len() >= MAX_SHAPE_CACHE {
            self.shapes.clear();
        }
        self.shapes.insert(key, Arc::clone(&shaped));
        shaped
    }

    fn shape_glyphs(&mut self, text: &str, variant: FontVariant, size: f32) -> Vec<LayoutGlyph> {
        let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        if clean.is_empty() {
            return Vec::new();
        }
        let family = match &self.family {
            Some(name) => Family::Name(name.as_str()),
            None => Family::Serif,
        };
        let attrs = Attrs::new().family(family);
        let attrs = match variant {
            FontVariant::Roman => attrs,
            FontVariant::Italic => attrs.style(Style::Italic),
            FontVariant::Bold => attrs.weight(Weight::BOLD),
            FontVariant::BoldItalic => attrs.style(Style::Italic).weight(Weight::BOLD),
        };
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(size, size * 1.25));
        buffer.set_text(&mut self.font_system, &clean, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);
        let mut glyphs = Vec::new();
        for run in buffer.layout_runs() {
            glyphs.extend(run.glyphs.iter().cloned());
        }
        glyphs
    }

    /// Ink bounding box of a glyph in em units (None for blank glyphs).
    fn ink(&mut self, glyph: &LayoutGlyph) -> Option<Ink> {
        let key = (glyph.font_id, glyph.glyph_id);
        if let Some(cached) = self.inks.get(&key) {
            return *cached;
        }
        let scale = INK_REFERENCE_PX / glyph.font_size.max(0.01);
        let physical = glyph.physical((0.0, 0.0), scale);
        let reference = f64::from(INK_REFERENCE_PX);
        let ink = match self.swash.get_image(&mut self.font_system, physical.cache_key) {
            Some(image) if image.placement.width > 0 && image.placement.height > 0 => {
                let top = f64::from(image.placement.top);
                let left = f64::from(image.placement.left);
                let width = f64::from(image.placement.width);
                let height = f64::from(image.placement.height);
                Some(Ink {
                    ascent: top / reference,
                    descent: (height - top) / reference,
                    left: left / reference,
                    right: (left + width) / reference,
                })
            }
            _ => None,
        };
        self.inks.insert(key, ink);
        ink
    }

    /// Measures x-height, axis height and rule thickness from the active font.
    fn calibrate(&mut self) {
        const PROBE: f64 = 100.0;
        let x = self.shape("x", FontVariant::Italic, PROBE);
        let x_height = x.height / PROBE;
        if (0.30..=0.65).contains(&x_height) {
            self.constants.x_height = x_height;
        }
        let minus = self.shape("\u{2212}", FontVariant::Roman, PROBE);
        if !minus.glyphs.is_empty() {
            let thickness = (minus.height + minus.depth) / PROBE;
            let axis = (minus.height - minus.depth) / 2.0 / PROBE;
            if (0.015..=0.12).contains(&thickness) {
                self.constants.rule_thickness = thickness;
            }
            if (0.15..=0.40).contains(&axis) {
                self.constants.axis_height = axis;
            }
        }
    }
}