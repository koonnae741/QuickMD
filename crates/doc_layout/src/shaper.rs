//! cosmic-text shaping front-end: buffer pool, Thai-safe line metrics and
//! cluster-level break hints.
//!
//! Tone-mark protection strategy:
//! * line height = `ceil(font_size × 1.45 + 8px)`; cosmic-text centres glyphs
//!   in the line box, so the extra clearance splits evenly above (stacked tone
//!   marks such as ปั้, ผู้) and below (สระอุ/อู) — nothing gets clipped.
//! * Thai has no spaces between words, so the default UAX#14 breaker would
//!   fall back to *glyph* wrapping and could split a base consonant from its
//!   marks. We insert U+200B (ZWSP) only at Thai Character Cluster boundaries,
//!   so wrapping can only ever happen *between* complete clusters.

use std::ops::Range;

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, Weight, Wrap};

use crate::ast::InlineSpan;

pub const DEFAULT_LINE_HEIGHT_MULTIPLIER: f32 = 1.45;
pub const DEFAULT_LINE_CLEARANCE: f32 = 8.0;

#[derive(Debug, Clone, PartialEq)]
pub struct TypographyConfig {
    pub line_height_multiplier: f32,
    /// Extra px per line reserved for Thai above/below-base marks.
    pub line_clearance: f32,
    pub body_family: Option<String>,
    pub mono_family: Option<String>,
    pub math_family: Option<String>,
    /// Insert ZWSP at Thai cluster boundaries before shaping.
    pub thai_cluster_breaks: bool,
    pub max_pooled_buffers: usize,
}

impl Default for TypographyConfig {
    fn default() -> Self {
        Self {
            line_height_multiplier: DEFAULT_LINE_HEIGHT_MULTIPLIER,
            line_clearance: DEFAULT_LINE_CLEARANCE,
            body_family: None,
            mono_family: None,
            math_family: None,
            thai_cluster_breaks: true,
            max_pooled_buffers: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub monospace: bool,
}

impl TextStyle {
    pub const fn body(font_size: f32) -> Self {
        Self { font_size, bold: false, italic: false, monospace: false }
    }
    pub const fn bold(font_size: f32) -> Self {
        Self { font_size, bold: true, italic: false, monospace: false }
    }
    pub const fn mono(font_size: f32) -> Self {
        Self { font_size, bold: false, italic: false, monospace: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextMetrics {
    /// Widest laid-out line (px).
    pub width: f32,
    /// `line_count × line_height` (px).
    pub height: f32,
    pub line_count: usize,
    pub line_height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpanFace {
    Regular,
    Bold,
    Italic,
    BoldItalic,
    Mono,
    Link,
    Math,
}

/// Recycles cosmic-text buffers so measurement does not reallocate their
/// internal line/glyph vectors for every block.
pub struct BufferPool {
    free: Vec<Buffer>,
    max_pooled: usize,
}

impl BufferPool {
    pub fn new(max_pooled: usize) -> Self {
        Self { free: Vec::with_capacity(max_pooled), max_pooled: max_pooled.max(1) }
    }

    pub fn acquire(&mut self, font_system: &mut FontSystem, metrics: Metrics) -> Buffer {
        match self.free.pop() {
            Some(mut buffer) => {
                buffer.set_metrics(font_system, metrics);
                buffer
            }
            None => Buffer::new(font_system, metrics),
        }
    }

    pub fn release(&mut self, buffer: Buffer) {
        if self.free.len() < self.max_pooled {
            self.free.push(buffer);
        }
    }

    pub fn len(&self) -> usize {
        self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.free.is_empty()
    }
}

pub struct TextShaper {
    config: TypographyConfig,
    pool: BufferPool,
    text: String,
    runs: Vec<(Range<usize>, SpanFace)>,
    mono_advances: Vec<(u32, f32)>,
}

impl TextShaper {
    pub fn new(config: TypographyConfig) -> Self {
        let pool = BufferPool::new(config.max_pooled_buffers);
        Self {
            config,
            pool,
            text: String::with_capacity(1024),
            runs: Vec::with_capacity(32),
            mono_advances: Vec::new(),
        }
    }

    pub fn config(&self) -> &TypographyConfig {
        &self.config
    }

    pub fn pooled_buffers(&self) -> usize {
        self.pool.len()
    }

    /// `ceil(size × multiplier + clearance)`: rounded *up* so clearance is never lost.
    pub fn line_height(&self, font_size: f32) -> f32 {
        let size = sanitize_size(font_size);
        (size * self.config.line_height_multiplier.max(1.0) + self.config.line_clearance.max(0.0))
            .ceil()
    }

    pub fn measure_spans(
        &mut self,
        font_system: &mut FontSystem,
        spans: &[InlineSpan],
        style: TextStyle,
        max_width: Option<f32>,
        wrap: Wrap,
    ) -> TextMetrics {
        self.load_spans(spans, style.monospace);
        self.measure_loaded(font_system, style, max_width, wrap)
    }

    pub fn measure_text(
        &mut self,
        font_system: &mut FontSystem,
        text: &str,
        style: TextStyle,
        max_width: Option<f32>,
        wrap: Wrap,
    ) -> TextMetrics {
        self.load_text(text, style.monospace);
        self.measure_loaded(font_system, style, max_width, wrap)
    }

    /// CSS min-content: widest unbreakable segment (word / Thai cluster run).
    pub fn min_content_width(
        &mut self,
        font_system: &mut FontSystem,
        spans: &[InlineSpan],
        style: TextStyle,
    ) -> f32 {
        self.measure_spans(font_system, spans, style, Some(1.0), Wrap::Word).width
    }

    /// CSS max-content: unwrapped width (widest hard line).
    pub fn max_content_width(
        &mut self,
        font_system: &mut FontSystem,
        spans: &[InlineSpan],
        style: TextStyle,
    ) -> f32 {
        self.measure_spans(font_system, spans, style, None, Wrap::None).width
    }

    /// Monospace advance per column (cached per font size).
    pub fn mono_advance(&mut self, font_system: &mut FontSystem, font_size: f32) -> f32 {
        let size = sanitize_size(font_size);
        let key = size.to_bits();
        if let Some(&(_, adv)) = self.mono_advances.iter().find(|(k, _)| *k == key) {
            return adv;
        }
        const PROBE: &str = "MMMMMMMMMMiiiiiiiiii0000000000";
        let m = self.measure_text(font_system, PROBE, TextStyle::mono(size), None, Wrap::None);
        let adv = if m.width > 0.0 { m.width / PROBE.len() as f32 } else { size * 0.6 };
        self.mono_advances.push((key, adv));
        adv
    }

    /// Shaped buffer for the renderer (same attrs/hints as measurement).
    /// Hand it back with [`TextShaper::recycle`] when done.
    pub fn shape_spans(
        &mut self,
        font_system: &mut FontSystem,
        spans: &[InlineSpan],
        style: TextStyle,
        max_width: Option<f32>,
    ) -> Buffer {
        self.load_spans(spans, style.monospace);
        self.fill_buffer(font_system, style, max_width, Wrap::WordOrGlyph)
    }

    pub fn shape_text(
        &mut self,
        font_system: &mut FontSystem,
        text: &str,
        style: TextStyle,
        max_width: Option<f32>,
        wrap: Wrap,
    ) -> Buffer {
        self.load_text(text, style.monospace);
        self.fill_buffer(font_system, style, max_width, wrap)
    }

    pub fn recycle(&mut self, buffer: Buffer) {
        self.pool.release(buffer);
    }

    // ------------------------------------------------------------ internals

    fn load_spans(&mut self, spans: &[InlineSpan], monospace: bool) {
        self.text.clear();
        self.runs.clear();
        let hints = self.config.thai_cluster_breaks && !monospace;
        for span in spans {
            let (s, face) = match span {
                InlineSpan::Text(s) => (s.as_str(), SpanFace::Regular),
                InlineSpan::Bold(s) => (s.as_str(), SpanFace::Bold),
                InlineSpan::Italic(s) => (s.as_str(), SpanFace::Italic),
                InlineSpan::BoldItalic(s) => (s.as_str(), SpanFace::BoldItalic),
                InlineSpan::InlineCode(s) => (s.as_str(), SpanFace::Mono),
                InlineSpan::Link { text, .. } => (text.as_str(), SpanFace::Link),
                InlineSpan::InlineMath(s) => (s.as_str(), SpanFace::Math),
                InlineSpan::Image { alt, .. } => (alt.as_str(), SpanFace::Regular),
            };
            if s.is_empty() {
                continue;
            }
            let start = self.text.len();
            if hints && face != SpanFace::Mono && face != SpanFace::Math {
                thai::push_with_break_hints(&mut self.text, s);
            } else {
                self.text.push_str(s);
            }
            let end = self.text.len();
            if let Some((range, last_face)) = self.runs.last_mut() {
                if *last_face == face && range.end == start {
                    range.end = end;
                    continue;
                }
            }
            self.runs.push((start..end, face));
        }
    }

    fn load_text(&mut self, text: &str, monospace: bool) {
        self.text.clear();
        self.runs.clear();
        if self.config.thai_cluster_breaks && !monospace {
            thai::push_with_break_hints(&mut self.text, text);
        } else {
            self.text.push_str(text);
        }
        if !self.text.is_empty() {
            let face = if monospace { SpanFace::Mono } else { SpanFace::Regular };
            self.runs.push((0..self.text.len(), face));
        }
    }

    fn fill_buffer(
        &mut self,
        font_system: &mut FontSystem,
        style: TextStyle,
        max_width: Option<f32>,
        wrap: Wrap,
    ) -> Buffer {
        let size = sanitize_size(style.font_size);
        let line_height = self.line_height(size);
        let mut buffer = self.pool.acquire(font_system, Metrics::new(size, line_height));
        buffer.set_wrap(font_system, wrap);
        buffer.set_size(font_system, max_width.map(|w| w.max(1.0)), None);

        let cfg = &self.config;
        let text = self.text.as_str();
        let runs = &self.runs;
        let default_attrs = attrs_for(cfg, SpanFace::Regular, style);
        buffer.set_rich_text(
            font_system,
            runs.iter().map(|(r, face)| (&text[r.clone()], attrs_for(cfg, *face, style))),
            default_attrs,
            Shaping::Advanced,
        );
        buffer.shape_until_scroll(font_system, false);
        buffer
    }

    fn measure_loaded(
        &mut self,
        font_system: &mut FontSystem,
        style: TextStyle,
        max_width: Option<f32>,
        wrap: Wrap,
    ) -> TextMetrics {
        let line_height = self.line_height(style.font_size);
        if self.text.is_empty() {
            return TextMetrics { width: 0.0, height: line_height, line_count: 1, line_height };
        }
        let buffer = self.fill_buffer(font_system, style, max_width, wrap);
        let mut line_count = 0usize;
        let mut width = 0.0f32;
        for run in buffer.layout_runs() {
            line_count += 1;
            width = width.max(run.line_w);
        }
        self.pool.release(buffer);
        let line_count = line_count.max(1);
        TextMetrics { width, height: line_count as f32 * line_height, line_count, line_height }
    }
}

fn sanitize_size(size: f32) -> f32 {
    if size.is_finite() && size > 0.0 { size } else { 1.0 }
}

fn attrs_for<'a>(cfg: &'a TypographyConfig, face: SpanFace, style: TextStyle) -> Attrs<'a> {
    let mono = style.monospace || face == SpanFace::Mono;
    let family = if mono {
        cfg.mono_family.as_deref().map(Family::Name).unwrap_or(Family::Monospace)
    } else if face == SpanFace::Math {
        cfg.math_family.as_deref().map(Family::Name).unwrap_or(Family::Serif)
    } else {
        cfg.body_family.as_deref().map(Family::Name).unwrap_or(Family::SansSerif)
    };
    let bold = style.bold || matches!(face, SpanFace::Bold | SpanFace::BoldItalic);
    let italic =
        style.italic || matches!(face, SpanFace::Italic | SpanFace::BoldItalic | SpanFace::Math);
    let mut attrs = Attrs::new().family(family);
    if bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if italic {
        attrs = attrs.style(Style::Italic);
    }
    attrs
}

/// Thai script helpers (U+0E00–U+0E7F).
pub mod thai {
    use std::borrow::Cow;

    pub const ZWSP: char = '\u{200B}';
    pub const MAI_HAN_AKAT: char = '\u{0E31}';
    pub const MAI_TAIKHU: char = '\u{0E47}';
    pub const THANTHAKHAT: char = '\u{0E4C}';

    #[inline]
    pub fn is_thai(c: char) -> bool {
        ('\u{0E00}'..='\u{0E7F}').contains(&c)
    }
    #[inline]
    pub fn is_consonant(c: char) -> bool {
        ('\u{0E01}'..='\u{0E2E}').contains(&c)
    }
    /// เ แ โ ใ ไ — written before the consonant they belong to.
    #[inline]
    pub fn is_leading_vowel(c: char) -> bool {
        ('\u{0E40}'..='\u{0E44}').contains(&c)
    }
    /// Above/below-base marks: ั ิ ี ึ ื ุ ู ฺ ็ ่ ้ ๊ ๋ ์ ํ ๎
    #[inline]
    pub fn is_combining_mark(c: char) -> bool {
        c == MAI_HAN_AKAT || ('\u{0E34}'..='\u{0E3A}').contains(&c) || ('\u{0E47}'..='\u{0E4E}').contains(&c)
    }
    /// ่ ้ ๊ ๋
    #[inline]
    pub fn is_tone_mark(c: char) -> bool {
        ('\u{0E48}'..='\u{0E4B}').contains(&c)
    }
    /// ะ า ำ ๅ ๆ ฯ — spacing vowels/signs that stay with the preceding cluster.
    #[inline]
    pub fn is_following_sign(c: char) -> bool {
        matches!(c, '\u{0E30}' | '\u{0E32}' | '\u{0E33}' | '\u{0E45}' | '\u{0E46}' | '\u{0E2F}')
    }
    #[inline]
    pub fn is_digit(c: char) -> bool {
        ('\u{0E50}'..='\u{0E59}').contains(&c)
    }

    /// Fast UTF-8 probe: Thai code points encode as `E0 B8 xx` / `E0 B9 xx`.
    #[inline]
    pub fn contains_thai(text: &str) -> bool {
        text.as_bytes().windows(2).any(|w| w[0] == 0xE0 && (w[1] == 0xB8 || w[1] == 0xB9))
    }

    /// Appends `text` to `out`, inserting ZWSP at Thai word boundaries found by
    /// the dictionary-based ICU word segmenter. Non-Thai text is copied verbatim.
    pub fn push_with_break_hints(out: &mut String, text: &str) {
        if !contains_thai(text) {
            out.push_str(text);
            return;
        }
        out.reserve(text.len() + text.len() / 6);
        let segmenter = icu_segmenter::WordSegmenter::new_dictionary(Default::default());
        let mut last = 0;
        for boundary in segmenter.segment_str(text) {
            if boundary == 0 || boundary >= text.len() {
                continue;
            }
            out.push_str(&text[last..boundary]);
            if is_thai_run_edge(&text[..boundary], &text[boundary..]) {
                out.push(ZWSP);
            }
            last = boundary;
        }
        out.push_str(&text[last..]);
    }

    /// Only mark breaks between two Thai characters, and never in front of a
    /// vowel sign, combining mark or following sign (ๆ ฯ ะ า ำ), which must stay
    /// with the preceding cluster.
    fn is_thai_run_edge(before: &str, after: &str) -> bool {
        let p = before.chars().next_back();
        let n = after.chars().next();
        match (p, n) {
            (Some(a), Some(b)) => {
                is_thai(a) && is_thai(b) && !is_combining_mark(b) && !is_following_sign(b)
            }
            _ => false,
        }
    }

    pub fn with_break_hints(text: &str) -> Cow<'_, str> {
        if !contains_thai(text) {
            return Cow::Borrowed(text);
        }
        let mut out = String::with_capacity(text.len() + 16);
        push_with_break_hints(&mut out, text);
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::thai::*;
    use super::*;
    use std::borrow::Cow;
    use crate::test_support::font_system;

    #[test]
    fn line_height_includes_thai_clearance() {
        let shaper = TextShaper::new(TypographyConfig::default());
        assert_eq!(shaper.line_height(16.0), (16.0f32 * 1.45 + 8.0).ceil());
        assert_eq!(shaper.line_height(16.0), 32.0);
        assert!(shaper.line_height(14.0) >= 14.0 * 1.45 + 8.0);
    }

    #[test]
    fn thai_hints_respect_words() {
        let s = with_break_hints("ประเทศไทย");
        assert!(!s.contains("\u{200B}ระ") && !s.contains("\u{200B}เทศ"));
        assert!(!s.contains("ป\u{200B}"), "ประเทศ must not split after ป: {s}");
        assert!(matches!(with_break_hints("hello world"), Cow::Borrowed(_)));
    }

    #[test]
    fn thai_hints_never_split_inside_a_word() {
        for sample in ["ประเทศไทย", "ภาษาไทย", "สวัสดีครับ", "ประเทศไทยน่าอยู่"] {
            let hinted = with_break_hints(sample);
            let parts: Vec<&str> = hinted.split(ZWSP).collect();
            for part in &parts {
                assert!(!part.is_empty());
            }
            assert_eq!(parts.concat(), sample);
        }
        let s = with_break_hints("ประเทศไทย");
        assert!(!s.contains("ป\u{200B}ร"), "ป|ระ split: {s}");
    }

    #[test]
    fn thai_hints_never_orphan_marks() {
        let samples = [
            "ผู้ใหญ่ลี้ตีกลองประชุม",
            "กินข้าวหรือยังครับ",
            "สวัสดีชาวโลก ภาษาไทย",
            "น้ำแข็งใสๆ ฯลฯ",
            "เกี๊ยวกุ้ง",
        ];
        for sample in samples {
            let hinted = with_break_hints(sample);
            assert_eq!(hinted.replace(ZWSP, ""), sample);
            let chars: Vec<char> = hinted.chars().collect();
            for w in chars.windows(2) {
                if w[1] == ZWSP {
                    assert!(!is_leading_vowel(w[0]), "break after leading vowel in {sample}");
                }
                if w[0] == ZWSP {
                    assert!(!is_combining_mark(w[1]), "mark orphaned in {sample}");
                    assert!(!is_following_sign(w[1]), "following vowel orphaned in {sample}");
                }
            }
        }
    }

    #[test]
    fn thai_text_height_is_whole_lines() {
        let mut fs = font_system();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        let m = shaper.measure_text(
            &mut fs,
            "ภาษาไทย ปั้นน้ำเป็นตัว ผู้ใหญ่ลี้",
            TextStyle::body(16.0),
            Some(2000.0),
            Wrap::WordOrGlyph,
        );
        assert_eq!(m.line_height, 32.0);
        assert!(m.height >= 16.0 * 1.45 + 8.0);
        assert_eq!(m.height, m.line_count as f32 * m.line_height);
    }

    #[test]
    fn narrow_width_never_reduces_line_count() {
        let mut fs = font_system();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        let text = [InlineSpan::Text(
            "The quick brown fox jumps over the lazy dog. ภาษาไทยไม่มีการเว้นวรรคระหว่างคำ ".repeat(4),
        )];
        let wide = shaper.measure_spans(&mut fs, &text, TextStyle::body(16.0), Some(4000.0), Wrap::WordOrGlyph);
        let narrow = shaper.measure_spans(&mut fs, &text, TextStyle::body(16.0), Some(120.0), Wrap::WordOrGlyph);
        assert!(narrow.line_count >= wide.line_count);
        assert!(narrow.height >= wide.height);
    }

    #[test]
    fn min_content_not_wider_than_max_content() {
        let mut fs = font_system();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        let spans = [InlineSpan::Text("alpha beta gamma-delta".into()), InlineSpan::Bold(" bold".into())];
        let min = shaper.min_content_width(&mut fs, &spans, TextStyle::body(16.0));
        let max = shaper.max_content_width(&mut fs, &spans, TextStyle::body(16.0));
        assert!(min <= max + 0.5, "min {min} > max {max}");
    }

    #[test]
    fn buffer_pool_is_reused() {
        let mut fs = font_system();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        shaper.measure_text(&mut fs, "one", TextStyle::body(16.0), Some(300.0), Wrap::WordOrGlyph);
        assert_eq!(shaper.pooled_buffers(), 1);
        shaper.measure_text(&mut fs, "two", TextStyle::bold(20.0), Some(300.0), Wrap::WordOrGlyph);
        assert_eq!(shaper.pooled_buffers(), 1);
        let buf = shaper.shape_text(&mut fs, "x", TextStyle::body(16.0), None, Wrap::None);
        assert_eq!(shaper.pooled_buffers(), 0);
        shaper.recycle(buf);
        assert_eq!(shaper.pooled_buffers(), 1);
    }

    #[test]
    fn mono_advance_is_positive_and_cached() {
        let mut fs = font_system();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        let a = shaper.mono_advance(&mut fs, 14.0);
        let b = shaper.mono_advance(&mut fs, 14.0);
        assert!(a > 0.0);
        assert_eq!(a, b);
    }
}