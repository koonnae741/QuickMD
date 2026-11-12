//! Measures every block once and assigns monotonically increasing Y offsets,
//! producing the sorted geometry array used for O(log N) culling.

use std::cell::Cell;
use std::collections::HashMap;

use cosmic_text::{FontSystem, Wrap};
use unicode_width::UnicodeWidthChar;

use crate::ast::{spans_are_blank, InlineSpan, MarkdownBlock};
use crate::shaper::{TextShaper, TextStyle, TypographyConfig};
use crate::table::{layout_table, TableLayout, TableStyle};
use crate::viewport::binary_search_range;

pub const MIN_CONTENT_WIDTH: f32 = 120.0;
const MIN_INNER_WIDTH: f32 = 40.0;

#[derive(Debug, Clone, PartialEq)]
pub struct BlockGeometry {
    pub block_index: usize,
    pub y_offset: f32,
    pub height: f32,
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutConfig {
    pub base_font_size: f32,
    pub heading_scales: [f32; 6],
    /// Gap + 1px rule under H1/H2.
    pub heading_rule_gap: f32,
    pub code_font_size: f32,
    pub code_padding: f32,
    /// `false`: code scrolls horizontally and height is computed arithmetically (fast).
    pub wrap_code: bool,
    pub tab_width: usize,
    pub block_spacing: f32,
    pub padding_top: f32,
    pub padding_bottom: f32,
    pub quote_bar_width: f32,
    pub quote_padding_x: f32,
    pub quote_padding_y: f32,
    pub callout_padding: f32,
    pub callout_title_gap: f32,
    pub list_indent: f32,
    pub list_item_gap: f32,
    pub embed_padding: f32,
    pub hr_height: f32,
    pub table: TableStyle,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            base_font_size: 16.0,
            heading_scales: [2.0, 1.5, 1.25, 1.0, 0.875, 0.85],
            heading_rule_gap: 8.0,
            code_font_size: 14.0,
            code_padding: 14.0,
            wrap_code: false,
            tab_width: 4,
            block_spacing: 16.0,
            padding_top: 24.0,
            padding_bottom: 48.0,
            quote_bar_width: 4.0,
            quote_padding_x: 16.0,
            quote_padding_y: 8.0,
            callout_padding: 14.0,
            callout_title_gap: 6.0,
            list_indent: 28.0,
            list_item_gap: 4.0,
            embed_padding: 12.0,
            hr_height: 25.0,
            table: TableStyle::default(),
        }
    }
}

impl LayoutConfig {
    pub fn heading_font_size(&self, level: u8) -> f32 {
        let idx = (level.clamp(1, 6) - 1) as usize;
        self.base_font_size * self.heading_scales[idx]
    }
}

/// Hook for exact `native_math` / `native_mermaid` sizing. Return `(width, height)` in px.
pub trait EmbedMeasurer {
    fn measure_math(&mut self, latex: &str, max_width: f32) -> (f32, f32);
    fn measure_mermaid(&mut self, source: &str, max_width: f32) -> (f32, f32);
    fn measure_image(&mut self, _url: &str, _max_width: f32) -> (f32, f32) {
        (0.0, 0.0)
    }
}

/// Font-free estimator used when no real measurer is plugged in.
#[derive(Debug, Clone, Copy)]
pub struct HeuristicEmbedMeasurer {
    pub font_size: f32,
}

impl HeuristicEmbedMeasurer {
    pub fn new(font_size: f32) -> Self {
        Self { font_size }
    }
}

impl Default for HeuristicEmbedMeasurer {
    fn default() -> Self {
        Self::new(16.0)
    }
}

impl EmbedMeasurer for HeuristicEmbedMeasurer {
    fn measure_math(&mut self, latex: &str, max_width: f32) -> (f32, f32) {
        let rows = latex.split("\\\\").count().max(1);
        let tall = ["\\frac", "\\sum", "\\int", "\\prod", "\\begin", "\\sqrt", "\\over", "\\lim"]
            .iter()
            .any(|k| latex.contains(k));
        let row_h = self.font_size * if tall { 2.6 } else { 1.6 };
        let longest = latex.split("\\\\").map(|r| r.chars().count()).max().unwrap_or(0);
        let w = (longest as f32 * self.font_size * 0.45).clamp(self.font_size, max_width.max(self.font_size));
        (w, rows as f32 * row_h)
    }

    fn measure_mermaid(&mut self, source: &str, max_width: f32) -> (f32, f32) {
        let mut lines = source.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("%%"));
        let header = lines.next().unwrap_or("");
        let statements = lines.count().max(1) as f32;
        let vertical = header.contains("TD")
            || header.contains("TB")
            || header.contains("BT")
            || header.starts_with("sequenceDiagram")
            || !header.starts_with("graph") && !header.starts_with("flowchart");
        let ranks = if vertical { statements.min(40.0) } else { statements.sqrt().ceil() };
        let h = (ranks * 72.0 + 48.0).clamp(120.0, 4000.0);
        (max_width, h)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockMeasure {
    pub height: f32,
    pub width: f32,
    pub table: Option<TableLayout>,
    pub list_item_heights: Option<Vec<f32>>,
}

impl BlockMeasure {
    fn new(height: f32, width: f32) -> Self {
        Self {
            height: sane(height).ceil(),
            width: sane(width),
            table: None,
            list_item_heights: None,
        }
    }
}

#[inline]
fn sane(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

#[inline]
fn inner_width(w: f32) -> f32 {
    w.max(MIN_INNER_WIDTH)
}

fn spans_single_line_plain_len(spans: &[InlineSpan]) -> Option<usize> {
    let mut total = 0usize;
    for span in spans {
        let s = match span {
            InlineSpan::Text(s) => s.as_str(),
            InlineSpan::Bold(s) => s.as_str(),
            InlineSpan::Italic(s) => s.as_str(),
            InlineSpan::BoldItalic(s) => s.as_str(),
            InlineSpan::InlineCode(s) => s.as_str(),
            InlineSpan::Link { text, .. } => text.as_str(),
            InlineSpan::InlineMath(_) => return None,
            InlineSpan::Image { .. } => return None,
        };
        if s.contains('\n') || s.contains('\r') {
            return None;
        }
        total += s.chars().count();
    }
    Some(total)
}

/// Measures one block at `content_width`.
pub fn measure_block(
    block: &MarkdownBlock,
    content_width: f32,
    font_system: &mut FontSystem,
    shaper: &mut TextShaper,
    cfg: &LayoutConfig,
    embeds: &mut dyn EmbedMeasurer,
) -> BlockMeasure {
    let w = content_width;
    let body = TextStyle::body(cfg.base_font_size);
    let wrap = Wrap::WordOrGlyph;

    match block {
        MarkdownBlock::Heading { level, text, .. } => {
            let size = cfg.heading_font_size(*level);
            let m = shaper.measure_text(font_system, text, TextStyle::bold(size), Some(w), wrap);
            let rule = if *level <= 2 { cfg.heading_rule_gap + 1.0 } else { 0.0 };
            BlockMeasure::new(m.height + rule, w)
        }
        MarkdownBlock::Paragraph(spans) => {
            let avail = w;
            let line_h = shaper.line_height(cfg.base_font_size);
            if let Some(chars) = spans_single_line_plain_len(spans) {
                let max_char_w = cfg.base_font_size * 0.75;
                if chars as f32 * max_char_w < avail {
                    return BlockMeasure::new(line_h, w);
                }
            }
            let m = shaper.measure_spans(font_system, spans, body, Some(w), wrap);
            BlockMeasure::new(m.height, w)
        }
        MarkdownBlock::Blockquote(spans) => {
            let inner = inner_width(w - cfg.quote_bar_width - 2.0 * cfg.quote_padding_x);
            let m = shaper.measure_spans(font_system, spans, body, Some(inner), wrap);
            BlockMeasure::new(m.height + 2.0 * cfg.quote_padding_y, w)
        }
        MarkdownBlock::Callout { title, content, .. } => {
            let inner = inner_width(w - cfg.quote_bar_width - 2.0 * cfg.callout_padding);
            let t = shaper.measure_text(font_system, title, TextStyle::bold(cfg.base_font_size), Some(inner), wrap);
            let mut h = 2.0 * cfg.callout_padding + 8.0 + t.height;
            if !spans_are_blank(content) {
                let c = shaper.measure_spans(font_system, content, body, Some(inner), wrap);
                h += cfg.callout_title_gap + c.height;
            }
            BlockMeasure::new(h, w)
        }
        MarkdownBlock::CodeBlock { code, .. } => measure_code(code, w, font_system, shaper, cfg),
        MarkdownBlock::MathBlock(src) => {
            let pad = cfg.embed_padding;
            let (ew, eh) = embeds.measure_math(src, inner_width(w - 2.0 * pad));
            BlockMeasure::new(sane(eh) + 2.0 * pad, w.max(sane(ew) + 2.0 * pad))
        }
        MarkdownBlock::MermaidBlock(src) => {
            let pad = cfg.embed_padding;
            let (ew, eh) = embeds.measure_mermaid(src, inner_width(w - 2.0 * pad));
            BlockMeasure::new(sane(eh) + 2.0 * pad, w.max(sane(ew) + 2.0 * pad))
        }
        MarkdownBlock::Table(table) => {
            let tl = layout_table(table, w, cfg.base_font_size, &cfg.table, shaper, font_system);
            let mut m = BlockMeasure::new(tl.total_height, tl.total_width);
            m.table = Some(tl);
            m
        }
        MarkdownBlock::List { ordered, start, items } => {
            let indent = list_indent(cfg, *ordered, *start, items.len());
            let (h, item_heights) = measure_items(items.iter(), indent, w, font_system, shaper, cfg);
            let mut m = BlockMeasure::new(h, w);
            m.list_item_heights = Some(item_heights);
            m
        }
        MarkdownBlock::CheckList(items) => {
            let (h, item_heights) = measure_items(items.iter().map(|(_, s)| s), cfg.list_indent, w, font_system, shaper, cfg);
            let mut m = BlockMeasure::new(h, w);
            m.list_item_heights = Some(item_heights);
            m
        }
        MarkdownBlock::HorizontalRule => BlockMeasure::new(cfg.hr_height, w),
        MarkdownBlock::Image { url, width_override, .. } => {
            let max_w = width_override.unwrap_or(w).min(w);
            let (ew, eh) = embeds.measure_image(url, max_w);
            let h = if eh > 0.0 {
                eh + 20.0
            } else if !url.trim().is_empty() {
                56.0 // Compact luxury fallback card: 48px + 8px clearance
            } else {
                0.0
            };
            let actual_w = if ew > 0.0 { ew } else { max_w };
            BlockMeasure::new(h, w.max(actual_w))
        }
        MarkdownBlock::BadgeRow { badges, .. } => {
            let pill_h = 20.0;
            let gap_x = 8.0;
            let mut row_w = 0.0;
            let mut rows = 1usize;
            for b in badges {
                let bw = (b.label.len() + b.value.len()) as f32 * 7.5 + 24.0;
                if row_w + bw > w && row_w > 0.0 {
                    rows += 1;
                    row_w = bw + gap_x;
                } else {
                    row_w += bw + gap_x;
                }
            }
            let total_h = rows as f32 * (pill_h + 8.0) + 6.0;
            BlockMeasure::new(total_h, w)
        }
    }
}

fn list_indent(cfg: &LayoutConfig, ordered: bool, start: u64, count: usize) -> f32 {
    if !ordered {
        return cfg.list_indent;
    }
    let last = start.saturating_add(count.saturating_sub(1) as u64);
    let digits = last.checked_ilog10().unwrap_or(0) as f32 + 1.0;
    cfg.list_indent.max((digits + 2.0) * cfg.base_font_size * 0.6)
}

fn measure_items<'a, I>(
    items: I,
    indent: f32,
    w: f32,
    font_system: &mut FontSystem,
    shaper: &mut TextShaper,
    cfg: &LayoutConfig,
) -> (f32, Vec<f32>)
where
    I: Iterator<Item = &'a Vec<InlineSpan>>,
{
    let inner = inner_width(w - indent);
    let body = TextStyle::body(cfg.base_font_size);
    let line = shaper.line_height(cfg.base_font_size);
    let mut total = 0.0;
    let mut count = 0usize;
    let mut heights = Vec::new();
    for spans in items {
        let item_h = if spans_are_blank(spans) {
            line
        } else {
            shaper.measure_spans(font_system, spans, body, Some(inner), Wrap::WordOrGlyph).height
        };
        heights.push(item_h);
        total += item_h;
        count += 1;
    }
    if count > 1 {
        total += (count - 1) as f32 * cfg.list_item_gap;
    }
    (total, heights)
}

/// `(line_count, max_display_columns)` with tab expansion and East-Asian widths.
pub fn code_extent(code: &str, tab_width: usize) -> (usize, usize) {
    let tab = tab_width.max(1);
    let mut lines = 0usize;
    let mut max_cols = 0usize;
    for line in code.split('\n') {
        lines += 1;
        let mut cols = 0usize;
        for ch in line.chars() {
            match ch {
                '\t' => cols += tab - cols % tab,
                '\r' => {}
                _ => cols += ch.width().unwrap_or(0),
            }
        }
        max_cols = max_cols.max(cols);
    }
    (lines.max(1), max_cols)
}

/// `(line_count, max_display_columns)` with tab expansion, East-Asian widths, and wrapping columns.
pub fn code_extent_wrapped(code: &str, tab_width: usize, max_cols: usize) -> (usize, usize) {
    let tab = tab_width.max(1);
    let mut total_lines = 0usize;
    let mut max_observed_cols = 0usize;
    for line in code.split('\n') {
        let mut cols = 0usize;
        for ch in line.chars() {
            match ch {
                '\t' => cols += tab - cols % tab,
                '\r' => {}
                _ => cols += ch.width().unwrap_or(0),
            }
        }
        max_observed_cols = max_observed_cols.max(cols);
        let wrapped = if max_cols > 0 {
            (cols.max(1) + max_cols - 1) / max_cols
        } else {
            1
        };
        total_lines += wrapped.max(1);
    }
    (total_lines.max(1), max_observed_cols)
}

fn measure_code(
    code: &str,
    w: f32,
    font_system: &mut FontSystem,
    shaper: &mut TextShaper,
    cfg: &LayoutConfig,
) -> BlockMeasure {
    let pad = cfg.code_padding;
    let header_extra = 12.0; // Accounts for 28px header bar + 6px top offset in painter::draw_code_block
    let line_height = shaper.line_height(cfg.code_font_size);
    let advance = shaper.mono_advance(font_system, cfg.code_font_size);

    if cfg.wrap_code {
        let avail_w = inner_width(w - 2.0 * pad);
        let max_cols = if advance > 0.0 {
            (avail_w / advance).floor().max(1.0) as usize
        } else {
            80
        };
        let (lines, cols) = code_extent_wrapped(code, cfg.tab_width, max_cols);
        let content_w = (cols as f32 * advance + 2.0 * pad).min(w);
        return BlockMeasure::new(lines as f32 * line_height + 2.0 * pad + header_extra, w.max(content_w));
    }

    // Arithmetic fast path: O(bytes), no shaping — 100k-line code blocks stay cheap.
    let (lines, cols) = code_extent(code, cfg.tab_width);
    BlockMeasure::new(lines as f32 * line_height + 2.0 * pad + header_extra, w.max(cols as f32 * advance + 2.0 * pad))
}

fn estimate_spans(spans: &[InlineSpan], w: f32, body_lh: f32, cfg: &LayoutConfig) -> f32 {
    if let Some(chars) = spans_single_line_plain_len(spans) {
        let max_char_w = cfg.base_font_size * 0.75;
        if chars as f32 * max_char_w < w {
            return body_lh;
        }
    }
    let mut total_chars = 0usize;
    let mut explicit_nl = 0usize;
    for span in spans {
        let s = match span {
            InlineSpan::Text(s) => s.as_str(),
            InlineSpan::Bold(s) => s.as_str(),
            InlineSpan::Italic(s) => s.as_str(),
            InlineSpan::BoldItalic(s) => s.as_str(),
            InlineSpan::InlineCode(s) => s.as_str(),
            InlineSpan::Link { text, .. } => text.as_str(),
            InlineSpan::InlineMath(s) => s.as_str(),
            InlineSpan::Image { alt, .. } => alt.as_str(),
        };
        for c in s.chars() {
            if c == '\n' {
                explicit_nl += 1;
            } else {
                total_chars += 1;
            }
        }
    }
    let avg_char_w = (cfg.base_font_size * 0.52).max(4.0);
    let cols = ((w / avg_char_w).floor() as usize).max(1);
    let wrapped = if total_chars > 0 { (total_chars + cols - 1) / cols } else { 1 };
    let total_lines = (wrapped + explicit_nl).max(1);
    total_lines as f32 * body_lh
}

fn estimate_block(
    block: &MarkdownBlock,
    content_width: f32,
    shaper: &mut TextShaper,
    font_system: &mut FontSystem,
    cfg: &LayoutConfig,
) -> BlockMeasure {
    let w = content_width;
    let body_lh = shaper.line_height(cfg.base_font_size);

    match block {
        MarkdownBlock::Heading { level, text, .. } => {
            let size = cfg.heading_font_size(*level);
            let lh = shaper.line_height(size);
            let rule = if *level <= 2 { cfg.heading_rule_gap + 1.0 } else { 0.0 };
            let max_char_w = size * 0.75;
            let cols = ((w / max_char_w).floor() as usize).max(1);
            let lines = (text.chars().count() + cols - 1) / cols;
            BlockMeasure::new(lines.max(1) as f32 * lh + rule, w)
        }
        MarkdownBlock::Paragraph(spans) => {
            BlockMeasure::new(estimate_spans(spans, w, body_lh, cfg), w)
        }
        MarkdownBlock::CodeBlock { code, .. } => {
            measure_code(code, w, font_system, shaper, cfg)
        }
        MarkdownBlock::Blockquote(spans) => {
            let inner = inner_width(w - cfg.quote_bar_width - 2.0 * cfg.quote_padding_x);
            let h = estimate_spans(spans, inner, body_lh, cfg);
            BlockMeasure::new(h + 2.0 * cfg.quote_padding_y, w)
        }
        MarkdownBlock::Callout { title, content, .. } => {
            let inner = inner_width(w - cfg.quote_bar_width - 2.0 * cfg.callout_padding);
            let t = estimate_block(&MarkdownBlock::Heading { level: 4, text: title.clone(), id: String::new() }, inner, shaper, font_system, cfg);
            let c_h = estimate_spans(content, inner, body_lh, cfg);
            BlockMeasure::new(2.0 * cfg.callout_padding + 8.0 + t.height + cfg.callout_title_gap + c_h, w)
        }
        MarkdownBlock::List { items, .. } => {
            let inner = (w - cfg.list_indent).max(40.0);
            let mut h = 0.0f32;
            let mut heights = Vec::with_capacity(items.len());
            for it in items {
                let ih = estimate_spans(it, inner, body_lh, cfg);
                h += ih + cfg.list_item_gap;
                heights.push(ih);
            }
            let mut bm = BlockMeasure::new(h.max(body_lh), w);
            bm.list_item_heights = Some(heights);
            bm
        }
        MarkdownBlock::CheckList(items) => {
            let inner = (w - cfg.list_indent).max(40.0);
            let mut h = 0.0f32;
            let mut heights = Vec::with_capacity(items.len());
            for (_, s) in items {
                let ih = estimate_spans(s, inner, body_lh, cfg);
                h += ih + cfg.list_item_gap;
                heights.push(ih);
            }
            let mut bm = BlockMeasure::new(h.max(body_lh), w);
            bm.list_item_heights = Some(heights);
            bm
        }
        MarkdownBlock::HorizontalRule => BlockMeasure::new(cfg.hr_height, w),
        MarkdownBlock::Table(table) => {
            let row_count = table.rows.len() + 1;
            let row_h = body_lh + cfg.table.cell_padding_y * 2.0 + 2.0;
            BlockMeasure::new(row_count as f32 * row_h, w)
        }
        MarkdownBlock::MathBlock(_) | MarkdownBlock::MermaidBlock(_) => {
            BlockMeasure::new(120.0, w)
        }
        MarkdownBlock::Image { .. } => {
            BlockMeasure::new(260.0, w)
        }
        MarkdownBlock::BadgeRow { badges, .. } => {
            let pill_h = 20.0;
            let gap_x = 8.0;
            let mut row_w = 0.0;
            let mut rows = 1usize;
            for b in badges {
                let bw = (b.label.len() + b.value.len()) as f32 * 7.5 + 24.0;
                if row_w + bw > w && row_w > 0.0 {
                    rows += 1;
                    row_w = bw + gap_x;
                } else {
                    row_w += bw + gap_x;
                }
            }
            let total_h = rows as f32 * (pill_h + 8.0) + 6.0;
            BlockMeasure::new(total_h, w)
        }
    }
}

#[derive(Debug, Clone)]
pub struct DocumentLayout {
    pub blocks: Vec<MarkdownBlock>,
    pub geometries: Vec<BlockGeometry>,
    pub exact_measured: Vec<bool>,
    pub total_height: f32,
    pub content_width: f32,
    pub config: LayoutConfig,
    /// Solved table layouts keyed by block index.
    pub table_layouts: HashMap<usize, TableLayout>,
    /// Solved item heights for List and CheckList blocks keyed by block index.
    pub list_layouts: HashMap<usize, Vec<f32>>,
    /// Monotonic cursor pointing to the earliest potentially unmeasured block.
    pub unmeasured_cursor: Cell<usize>,
}

impl DocumentLayout {
    pub fn empty(content_width: f32, config: LayoutConfig) -> Self {
        let total_height = config.padding_top + config.padding_bottom;
        Self {
            blocks: Vec::new(),
            geometries: Vec::new(),
            exact_measured: Vec::new(),
            total_height,
            content_width,
            config,
            table_layouts: HashMap::new(),
            list_layouts: HashMap::new(),
            unmeasured_cursor: Cell::new(0),
        }
    }

    /// Measures with default typography/config and the heuristic embed measurer.
    pub fn measure(blocks: Vec<MarkdownBlock>, content_width: f32, font_system: &mut FontSystem) -> Self {
        let config = LayoutConfig::default();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        let mut embeds = HeuristicEmbedMeasurer::new(config.base_font_size);
        Self::measure_with(blocks, content_width, font_system, &mut shaper, &config, &mut embeds)
    }

    pub fn measure_with(
        blocks: Vec<MarkdownBlock>,
        content_width: f32,
        font_system: &mut FontSystem,
        shaper: &mut TextShaper,
        config: &LayoutConfig,
        embeds: &mut dyn EmbedMeasurer,
    ) -> Self {
        let content_width = if content_width.is_finite() {
            content_width.max(MIN_CONTENT_WIDTH)
        } else {
            MIN_CONTENT_WIDTH
        };
        let count = blocks.len();
        let mut geometries = Vec::with_capacity(count);
        let mut exact_measured = Vec::with_capacity(count);
        let mut table_layouts = HashMap::new();
        let mut list_layouts = HashMap::new();
        let mut y = config.padding_top;

        // Progressive threshold: for documents > 250 blocks, measure the first 80 blocks exactly
        // and estimate the rest arithmetically. This drops Time to First Frame from 16.5s to < 25ms!
        let progressive = count > 250;
        let initial_exact_count = if progressive { 80.min(count) } else { count };

        for (index, block) in blocks.iter().enumerate() {
            let is_exact = index < initial_exact_count;
            let m = if is_exact {
                measure_block(block, content_width, font_system, shaper, config, embeds)
            } else {
                estimate_block(block, content_width, shaper, font_system, config)
            };

            geometries.push(BlockGeometry {
                block_index: index,
                y_offset: y,
                height: m.height,
                width: m.width,
            });
            exact_measured.push(is_exact);

            if let Some(t) = m.table {
                table_layouts.insert(index, t);
            }
            if let Some(l) = m.list_item_heights {
                list_layouts.insert(index, l);
            }
            y += m.height + config.block_spacing;
        }
        if !blocks.is_empty() {
            y -= config.block_spacing;
        }

        Self {
            blocks,
            geometries,
            exact_measured,
            total_height: y + config.padding_bottom,
            content_width,
            config: config.clone(),
            table_layouts,
            list_layouts,
            unmeasured_cursor: Cell::new(initial_exact_count),
        }
    }

    /// Ensures that all blocks in `range` are exact-measured.
    /// If any block's height changed from its estimated height,
    /// shifts all subsequent block geometries and returns the total delta.
    pub fn ensure_range_exact(
        &mut self,
        range: std::ops::Range<usize>,
        font_system: &mut FontSystem,
        shaper: &mut TextShaper,
        embeds: &mut dyn EmbedMeasurer,
    ) -> f32 {
        let start = range.start.min(self.blocks.len());
        let end = range.end.min(self.blocks.len());
        if start >= end {
            return 0.0;
        }

        let mut cumulative_delta = 0.0f32;
        let content_width = self.content_width;
        let config = &self.config;

        for idx in start..end {
            if !self.exact_measured[idx] {
                // If previous blocks had delta, update this block's y_offset first
                if cumulative_delta.abs() > 0.001 {
                    self.geometries[idx].y_offset += cumulative_delta;
                }

                let m = measure_block(&self.blocks[idx], content_width, font_system, shaper, config, embeds);
                let old_h = self.geometries[idx].height;
                let new_h = m.height;
                let delta = new_h - old_h;

                self.geometries[idx].height = new_h;
                self.geometries[idx].width = m.width;
                self.exact_measured[idx] = true;

                if let Some(t) = m.table {
                    self.table_layouts.insert(idx, t);
                }
                if let Some(l) = m.list_item_heights {
                    self.list_layouts.insert(idx, l);
                }

                cumulative_delta += delta;
            } else if cumulative_delta.abs() > 0.001 {
                self.geometries[idx].y_offset += cumulative_delta;
            }
        }

        // Apply cumulative delta to all blocks after `end`
        if cumulative_delta.abs() > 0.001 && end < self.geometries.len() {
            for g in &mut self.geometries[end..] {
                g.y_offset += cumulative_delta;
            }
        }

        if cumulative_delta.abs() > 0.001 {
            self.total_height += cumulative_delta;
        }
        cumulative_delta
    }

    pub fn has_unmeasured_blocks(&self) -> bool {
        let mut cur = self.unmeasured_cursor.get();
        while cur < self.exact_measured.len() && self.exact_measured[cur] {
            cur += 1;
        }
        self.unmeasured_cursor.set(cur);
        cur < self.exact_measured.len()
    }

    /// Progressively measures up to `batch_size` unmeasured blocks starting from the first unmeasured one.
    pub fn measure_batch_exact(
        &mut self,
        batch_size: usize,
        font_system: &mut FontSystem,
        shaper: &mut TextShaper,
        embeds: &mut dyn EmbedMeasurer,
    ) -> f32 {
        let mut cur = self.unmeasured_cursor.get();
        while cur < self.exact_measured.len() && self.exact_measured[cur] {
            cur += 1;
        }
        self.unmeasured_cursor.set(cur);
        if cur >= self.exact_measured.len() {
            return 0.0;
        }
        let first_unmeasured = cur;
        let end = (first_unmeasured + batch_size).min(self.blocks.len());
        self.ensure_range_exact(first_unmeasured..end, font_system, shaper, embeds)
    }

    /// Re-flow at a new width, reusing the stored blocks and config.
    pub fn relayout(
        &mut self,
        content_width: f32,
        font_system: &mut FontSystem,
        shaper: &mut TextShaper,
        embeds: &mut dyn EmbedMeasurer,
    ) {
        let blocks = std::mem::take(&mut self.blocks);
        let config = self.config.clone();
        *self = Self::measure_with(blocks, content_width, font_system, shaper, &config, embeds);
    }

    /// Indices of blocks intersecting `[scroll_y − overscan, scroll_y + viewport_height + overscan]`.
    /// Two binary searches, O(log N), zero allocation.
    #[inline]
    pub fn visible_range(&self, scroll_y: f32, viewport_height: f32, overscan: f32) -> std::ops::Range<usize> {
        let (top, bottom) = crate::viewport::cull_window(scroll_y, viewport_height, overscan);
        binary_search_range(&self.geometries, top, bottom, |g| g.y_offset, |g| g.height)
    }

    #[inline]
    pub fn visible_geometries(&self, scroll_y: f32, viewport_height: f32, overscan: f32) -> &[BlockGeometry] {
        &self.geometries[self.visible_range(scroll_y, viewport_height, overscan)]
    }

    /// Block containing document-space `y` (None in gaps/padding).
    pub fn block_at(&self, y: f32) -> Option<usize> {
        let i = self.geometries.partition_point(|g| g.y_offset + g.height <= y);
        let g = self.geometries.get(i)?;
        (g.y_offset <= y).then_some(i)
    }

    pub fn heading_offset(&self, id: &str) -> Option<f32> {
        self.blocks.iter().position(|b| matches!(b, MarkdownBlock::Heading { id: hid, .. } if hid == id))
            .map(|i| self.geometries[i].y_offset)
    }

    /// Scroll anchor `(block_index, fraction)` used to keep position across re-flows.
    pub fn anchor_for(&self, scroll_y: f32) -> Option<(usize, f32)> {
        if self.geometries.is_empty() {
            return None;
        }
        let i = self
            .geometries
            .partition_point(|g| g.y_offset + g.height <= scroll_y)
            .min(self.geometries.len() - 1);
        let g = &self.geometries[i];
        let frac = ((scroll_y - g.y_offset) / g.height.max(1.0)).clamp(-1.0, 1.0);
        Some((i, frac))
    }

    pub fn scroll_for_anchor(&self, (index, frac): (usize, f32)) -> f32 {
        match self.geometries.get(index) {
            Some(g) => (g.y_offset + frac * g.height.max(1.0)).max(0.0),
            None => 0.0,
        }
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::plain_text;
    use crate::parser::parse_markdown;
    use crate::test_support::font_system;

    const SAMPLE: &str = "# QuickMD 3.0\n\nภาษาไทยต้องไม่ถูกตัด: ปั้นน้ำเป็นตัว ผู้ใหญ่ลี้ตีกลองประชุม **ตัวหนา** and *italic*.\n\n> [!TIP]\n> ใช้ `cargo test`\n\n| Name | Value |\n|:-----|------:|\n| อัลฟ่า | 1 |\n| beta | 22 |\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n$$E = mc^2$$\n\n```mermaid\ngraph TD\nA-->B\nB-->C\n```\n\n- one\n- two\n  - nested\n\nBetween.\n\n- [x] done\n- [ ] todo\n\n---\n\n## End\n\nสุดท้าย\n";

    /// Acquires the shared FontSystem only for the duration of the measurement
    /// (never call this while holding another `font_system()` guard).
    fn layout_at(width: f32) -> DocumentLayout {
        let mut fs = font_system();
        DocumentLayout::measure(parse_markdown(SAMPLE), width, &mut fs)
    }

    fn brute_force(layout: &DocumentLayout, top: f32, bottom: f32) -> Vec<usize> {
        layout
            .geometries
            .iter()
            .filter(|g| g.y_offset + g.height > top && g.y_offset < bottom)
            .map(|g| g.block_index)
            .collect()
    }

    #[test]
    fn sample_contains_every_block_kind() {
        let blocks = parse_markdown(SAMPLE);
        let kinds: Vec<&str> = blocks.iter().map(MarkdownBlock::kind_name).collect();
        for k in ["heading", "paragraph", "callout", "table", "code", "math", "mermaid", "list", "checklist", "rule"] {
            assert!(kinds.contains(&k), "missing {k} in {kinds:?}");
        }
    }

    #[test]
    fn geometry_is_sorted_and_contiguous() {
        let layout = layout_at(720.0);
        let cfg = &layout.config;
        assert_eq!(layout.geometries.len(), layout.blocks.len());
        assert_eq!(layout.geometries[0].y_offset, cfg.padding_top);
        for (i, pair) in layout.geometries.windows(2).enumerate() {
            assert_eq!(pair[0].block_index, i);
            assert!(pair[0].height > 0.0, "block {i} has zero height");
            let expected = pair[0].y_offset + pair[0].height + cfg.block_spacing;
            assert!((pair[1].y_offset - expected).abs() < 1e-3);
        }
        let last = layout.geometries.last().unwrap();
        assert!((layout.total_height - (last.y_offset + last.height + cfg.padding_bottom)).abs() < 1e-3);
    }

    #[test]
    fn thai_paragraph_reserves_tone_mark_clearance() {
        let layout = layout_at(720.0);
        let idx = layout
            .blocks
            .iter()
            .position(|b| matches!(b, MarkdownBlock::Paragraph(s) if plain_text(s).contains("ปั้น")))
            .expect("thai paragraph");
        let h = layout.geometries[idx].height;
        let line = (16.0f32 * 1.45 + 8.0).ceil();
        assert!(h >= line, "height {h} < one safe line {line}");
        assert_eq!(h % line, 0.0, "paragraph height must be a whole number of safe lines");
    }

    #[test]
    fn narrower_layout_is_never_shorter() {
        let wide = layout_at(1200.0);
        let narrow = layout_at(240.0);
        assert!(narrow.total_height >= wide.total_height);
        assert_eq!(narrow.content_width, 240.0);
        assert_eq!(layout_at(10.0).content_width, MIN_CONTENT_WIDTH);
    }

    #[test]
    fn table_layouts_are_recorded() {
        let layout = layout_at(720.0);
        let idx = layout.blocks.iter().position(|b| matches!(b, MarkdownBlock::Table(_))).unwrap();
        let tl = &layout.table_layouts[&idx];
        assert_eq!(tl.column_widths.len(), 2);
        assert_eq!(tl.row_heights.len(), 3);
        assert_eq!(layout.geometries[idx].height, tl.total_height.ceil());
    }

    #[test]
    fn visible_range_matches_brute_force() {
        let layout = layout_at(480.0);
        let (vh, overscan) = (300.0, 50.0);
        let mut scroll = -400.0;
        while scroll < layout.total_height + 400.0 {
            let range = layout.visible_range(scroll, vh, overscan);
            let expected = brute_force(&layout, scroll - overscan, scroll + vh + overscan);
            assert_eq!(range.clone().collect::<Vec<_>>(), expected, "scroll_y = {scroll}");
            assert_eq!(layout.visible_geometries(scroll, vh, overscan).len(), range.len());
            scroll += 37.0;
        }
    }

    #[test]
    fn visible_range_edge_cases() {
        let layout = layout_at(720.0);
        let n = layout.blocks.len();
        assert!(layout.visible_range(-10_000.0, 500.0, 0.0).is_empty());
        assert!(layout.visible_range(layout.total_height + 10_000.0, 500.0, 0.0).is_empty());
        assert_eq!(layout.visible_range(0.0, f32::MAX, 0.0), 0..n);
        assert_eq!(layout.visible_range(f32::NAN, 400.0, f32::NAN).start, 0);
        assert!(layout.visible_range(0.0, -50.0, 0.0).len() <= 1);

        let empty = DocumentLayout::empty(720.0, LayoutConfig::default());
        assert_eq!(empty.visible_range(0.0, 1000.0, 600.0), 0..0);
        assert!(empty.anchor_for(0.0).is_none());
    }

    #[test]
    fn anchor_roundtrip_and_hit_testing() {
        let layout = layout_at(720.0);
        let g = &layout.geometries[3];
        let y = g.y_offset + g.height * 0.5;
        assert_eq!(layout.block_at(y), Some(3));
        let gap = g.y_offset + g.height + layout.config.block_spacing * 0.5;
        assert_eq!(layout.block_at(gap), None);

        let anchor = layout.anchor_for(y).unwrap();
        assert_eq!(anchor.0, 3);
        assert!((layout.scroll_for_anchor(anchor) - y).abs() < 0.01);

        assert_eq!(layout.heading_offset("quickmd-30"), Some(layout.config.padding_top));
        assert!(layout.heading_offset("end").is_some());
        assert_eq!(layout.heading_offset("missing"), None);
    }

    #[test]
    fn code_extent_handles_tabs_and_wide_chars() {
        assert_eq!(code_extent("a\tb\n漢字", 4), (2, 5));
        assert_eq!(code_extent("", 4), (1, 0));
        assert_eq!(code_extent("\t\tx", 8), (1, 17));
        assert_eq!(code_extent("line\r\nnext", 4), (2, 4));
    }

    #[test]
    fn huge_code_block_uses_arithmetic_fast_path() {
        let mut code = "let x = 1;\n".repeat(99_999);
        code.push_str("\tlet y = \"漢字\";");
        let blocks = vec![MarkdownBlock::CodeBlock { lang: "rust".into(), code }];
        let layout = {
            let mut fs = font_system();
            DocumentLayout::measure(blocks, 720.0, &mut fs)
        };
        let cfg = LayoutConfig::default();
        let line = (cfg.code_font_size * 1.45 + 8.0).ceil();
        let expected = (100_000.0 * line + 2.0 * cfg.code_padding).ceil();
        assert_eq!(layout.geometries[0].height, expected);
        assert_eq!(layout.visible_range(1_000_000.0, 900.0, 600.0), 0..1);
    }

    struct FixedEmbeds;

    impl EmbedMeasurer for FixedEmbeds {
        fn measure_math(&mut self, _latex: &str, _max_width: f32) -> (f32, f32) {
            (300.0, 100.0)
        }
        fn measure_mermaid(&mut self, _source: &str, max_width: f32) -> (f32, f32) {
            (max_width * 2.0, 500.0)
        }
    }

    #[test]
    fn embed_measurer_drives_math_and_mermaid() {
        let blocks = vec![
            MarkdownBlock::MathBlock("x".into()),
            MarkdownBlock::MermaidBlock("graph TD\nA-->B".into()),
        ];
        let cfg = LayoutConfig::default();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        let layout = {
            let mut fs = font_system();
            DocumentLayout::measure_with(blocks, 600.0, &mut fs, &mut shaper, &cfg, &mut FixedEmbeds)
        };
        let pad = cfg.embed_padding;
        assert_eq!(layout.geometries[0].height, 100.0 + 2.0 * pad);
        assert_eq!(layout.geometries[0].width, 600.0);
        assert_eq!(layout.geometries[1].height, 500.0 + 2.0 * pad);
        assert!(layout.geometries[1].width > 600.0, "wide diagrams report overflow width");
    }

    #[test]
    fn heuristic_embeds_are_sane() {
        let mut h = HeuristicEmbedMeasurer::default();
        let (w, ht) = h.measure_math("\\frac{a}{b} \\\\ c", 500.0);
        assert!(w > 0.0 && w <= 500.0 && ht > 16.0);
        let (mw, mh) = h.measure_mermaid("graph TD\nA-->B\nB-->C", 640.0);
        assert_eq!(mw, 640.0);
        assert!((120.0..=4000.0).contains(&mh));
    }

    #[test]
    fn ordered_list_indent_grows_with_digit_count() {
        let cfg = LayoutConfig::default();
        assert_eq!(list_indent(&cfg, false, 1, 3), cfg.list_indent);
        assert!(list_indent(&cfg, true, 998, 5) > cfg.list_indent);
        assert!(list_indent(&cfg, true, u64::MAX, 3).is_finite());
    }

    #[test]
    fn relayout_preserves_blocks() {
        let mut layout = layout_at(900.0);
        let before = layout.blocks.clone();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        let mut embeds = HeuristicEmbedMeasurer::default();
        {
            let mut fs = font_system();
            layout.relayout(300.0, &mut fs, &mut shaper, &mut embeds);
        }
        assert_eq!(layout.blocks, before);
        assert_eq!(layout.content_width, 300.0);
        assert_eq!(layout.geometries.len(), before.len());
    }

    /// 100,000-line stress test. Run with:
    /// `cargo test --release -p doc_layout -- --ignored --nocapture`
    #[test]
    #[ignore = "stress test: run with `cargo test --release -- --ignored`"]
    fn stress_100k_line_document() {
        let mut src = String::with_capacity(8 << 20);
        for i in 0..25_000 {
            src.push_str(&format!("## หัวข้อที่ {i}\n\n"));
            src.push_str("ภาษาไทยปั้นน้ำเป็นตัว ผู้ใหญ่ลี้ The quick brown fox jumps over the lazy dog.\n\n");
            src.push_str("- item one\n- item two\n\n");
            src.push_str("| a | b |\n|---|---|\n| 1 | 2 |\n\n");
        }

        let t0 = std::time::Instant::now();
        let blocks = parse_markdown(&src);
        let t_parse = t0.elapsed();
        assert_eq!(blocks.len(), 100_000);

        let t1 = std::time::Instant::now();
        let layout = {
            let mut fs = font_system();
            DocumentLayout::measure(blocks, 900.0, &mut fs)
        };
        let t_layout = t1.elapsed();

        let t2 = std::time::Instant::now();
        let mut total = 0usize;
        let queries = 1_000_000u32;
        for i in 0..queries {
            let y = (i as f32 * 7919.0) % layout.total_height;
            total += std::hint::black_box(layout.visible_range(y, 1080.0, 600.0)).len();
        }
        let t_query = t2.elapsed();
        assert!(total > 0);

        eprintln!(
            "parse {t_parse:?} | layout {t_layout:?} | {queries} culls {t_query:?} ({:.1} ns/cull) | height {:.0}px",
            t_query.as_nanos() as f64 / queries as f64,
            layout.total_height
        );
    }
}