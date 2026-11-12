//! # doc_layout — QuickMD 3.0 native Markdown layout engine
//!
//! Pipeline:
//! 1. [`parser::parse_markdown`] turns Markdown into a flat `Vec<MarkdownBlock>`.
//! 2. [`layout::DocumentLayout::measure`] shapes text with cosmic-text, solves
//!    tables and computes one [`layout::BlockGeometry`] per block (sorted by Y).
//! 3. [`viewport::ViewportCuller`] answers "what is on screen?" in O(log N) with
//!    zero heap allocation, and [`viewport::ScrollPhysics`] drives inertia scrolling.
//!
//! No HTML, no CSS, no JavaScript, no WebView — just Rust.

#![cfg_attr(not(test), forbid(unsafe_code))]

pub mod ast;
pub mod layout;
pub mod parser;
pub mod shaper;
pub mod table;
pub mod viewport;

pub use cosmic_text;
pub use tiny_skia;

pub use ast::{
    plain_text, spans_are_blank, Alignment, CalloutKind, InlineSpan, ListType, MarkdownBlock,
    TableCell, TableData,
};
pub use layout::{
    measure_block, BlockGeometry, BlockMeasure, DocumentLayout, EmbedMeasurer,
    HeuristicEmbedMeasurer, LayoutConfig,
};
pub use parser::{parse_markdown, parser_options, slugify};
pub use shaper::{thai, BufferPool, TextMetrics, TextShaper, TextStyle, TypographyConfig};
pub use table::{
    layout_table, measure_columns, solve_column_widths, ColumnMetrics, TableLayout, TableStyle,
};
pub use viewport::{ScrollPhysics, ViewportCuller, VisibleBlock};

use cosmic_text::FontSystem;

/// Default overscan (px) above and below the viewport.
pub const DEFAULT_OVERSCAN: f32 = 600.0;

/// High-level façade: owns fonts, shaper, layout, culler and scroll physics.
///
/// Typical frame loop:
/// ```text
/// engine.tick(dt);                       // physics, zero-alloc
/// for vb in engine.visible_blocks() {    // O(log N), zero-alloc slice
///     let block = &engine.layout().blocks[vb.block_index];
///     let screen_y = vb.screen_y(engine.scroll_y());
///     // ... draw block at (vb.x, screen_y)
/// }
/// ```
pub struct DocumentLayoutEngine {
    font_system: FontSystem,
    shaper: TextShaper,
    embeds: Box<dyn EmbedMeasurer>,
    layout: DocumentLayout,
    culler: ViewportCuller,
    physics: ScrollPhysics,
    content_width: f32,
    viewport_height: f32,
    overscan: f32,
    content_x: f32,
}

impl DocumentLayoutEngine {
    pub fn new(font_system: FontSystem) -> Self {
        Self::with_config(font_system, LayoutConfig::default(), TypographyConfig::default())
    }

    pub fn with_config(
        font_system: FontSystem,
        layout_config: LayoutConfig,
        typography: TypographyConfig,
    ) -> Self {
        let content_width = 800.0;
        let base = layout_config.base_font_size;
        Self {
            font_system,
            shaper: TextShaper::new(typography),
            embeds: Box::new(HeuristicEmbedMeasurer::new(base)),
            layout: DocumentLayout::empty(content_width, layout_config),
            culler: ViewportCuller::new(),
            physics: ScrollPhysics::default(),
            content_width,
            viewport_height: 600.0,
            overscan: DEFAULT_OVERSCAN,
            content_x: 0.0,
        }
    }

    /// Plug in exact measurers backed by `native_math` / `native_mermaid`.
    pub fn set_embed_measurer(&mut self, measurer: Box<dyn EmbedMeasurer>) {
        self.embeds = measurer;
        if !self.layout.blocks.is_empty() {
            self.relayout_preserving_anchor();
        }
    }

    pub fn load_markdown(&mut self, source: &str) {
        let blocks = parse_markdown(source);
        self.load_blocks(blocks);
    }

    pub fn load_blocks(&mut self, blocks: Vec<MarkdownBlock>) {
        let config = self.layout.config.clone();
        self.layout = DocumentLayout::measure_with(
            blocks,
            self.content_width,
            &mut self.font_system,
            &mut self.shaper,
            &config,
            self.embeds.as_mut(),
        );
        self.culler.rebuild(&self.layout, self.content_x);
        self.physics.set_bounds(self.layout.total_height, self.viewport_height);
        self.physics.jump_to(0.0);
    }

    /// Resize. Re-flows only when the width really changed, and keeps the
    /// block under the top of the viewport anchored (no "jump" on resize).
    pub fn set_viewport(&mut self, content_width: f32, viewport_height: f32) {
        self.viewport_height = if viewport_height.is_finite() { viewport_height.max(0.0) } else { 0.0 };
        let width = if content_width.is_finite() {
            content_width.max(layout::MIN_CONTENT_WIDTH)
        } else {
            self.content_width
        };
        if (width - self.content_width).abs() > 0.5 {
            self.content_width = width;
            if !self.layout.blocks.is_empty() {
                self.relayout_preserving_anchor();
                return;
            }
            self.layout.content_width = width;
        }
        self.physics.set_bounds(self.layout.total_height, self.viewport_height);
    }

    pub fn set_layout_config(&mut self, config: LayoutConfig) {
        self.layout.config = config;
        self.relayout_preserving_anchor();
    }

    pub fn set_content_x(&mut self, x: f32) {
        self.content_x = x;
        self.culler.rebuild(&self.layout, x);
    }

    pub fn set_overscan(&mut self, overscan: f32) {
        self.overscan = if overscan.is_finite() { overscan.max(0.0) } else { DEFAULT_OVERSCAN };
    }

    fn relayout_preserving_anchor(&mut self) {
        let anchor = self.layout.anchor_for(self.physics.position());
        self.layout.relayout(
            self.content_width,
            &mut self.font_system,
            &mut self.shaper,
            self.embeds.as_mut(),
        );
        self.culler.rebuild(&self.layout, self.content_x);
        self.physics.set_bounds(self.layout.total_height, self.viewport_height);
        if let Some(anchor) = anchor {
            self.physics.jump_to(self.layout.scroll_for_anchor(anchor));
        }
    }

    /// Direct manipulation (drag / scrollbar thumb).
    pub fn scroll_by(&mut self, dy: f32) {
        self.physics.scroll_by(dy);
    }

    /// Mouse-wheel style smooth scroll (accumulates).
    pub fn smooth_scroll_by(&mut self, dy: f32) {
        self.physics.smooth_scroll_by(dy);
    }

    /// Touchpad / touch release velocity in px/s.
    pub fn fling(&mut self, velocity: f32) {
        self.physics.fling(velocity);
    }

    pub fn scroll_to(&mut self, y: f32) {
        self.physics.animate_to(y);
    }

    pub fn scroll_to_heading(&mut self, id: &str) -> bool {
        match self.layout.heading_offset(id) {
            Some(y) => {
                self.physics.animate_to(y - self.layout.config.block_spacing);
                true
            }
            None => false,
        }
    }

    /// Advance physics by `dt` seconds. Returns `true` while still animating.
    #[inline]
    pub fn tick(&mut self, dt: f32) -> bool {
        self.physics.step(dt)
    }

    #[inline]
    pub fn scroll_y(&self) -> f32 {
        self.physics.position()
    }

    /// Zero-allocation O(log N) visible set.
    #[inline]
    pub fn visible_blocks(&self) -> &[VisibleBlock] {
        self.culler.query(self.physics.position(), self.viewport_height, self.overscan)
    }

    /// `Vec<&VisibleBlock>` variant; reuses `out`'s capacity (no alloc once warm).
    pub fn visible_into<'a>(&'a self, out: &mut Vec<&'a VisibleBlock>) {
        self.culler
            .query_into(self.physics.position(), self.viewport_height, self.overscan, out);
    }

    pub fn layout(&self) -> &DocumentLayout {
        &self.layout
    }

    pub fn block(&self, index: usize) -> Option<&MarkdownBlock> {
        self.layout.blocks.get(index)
    }

    pub fn culler(&self) -> &ViewportCuller {
        &self.culler
    }

    pub fn physics(&self) -> &ScrollPhysics {
        &self.physics
    }

    pub fn physics_mut(&mut self) -> &mut ScrollPhysics {
        &mut self.physics
    }

    pub fn viewport_height(&self) -> f32 {
        self.viewport_height
    }

    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Renderer access: shape visible blocks with exactly the same attributes
    /// that were used for measurement.
    pub fn font_system_and_shaper(&mut self) -> (&mut FontSystem, &mut TextShaper) {
        (&mut self.font_system, &mut self.shaper)
    }

    /// Disjoint split for rendering: mutably borrows fonts and shaper, while immutably borrowing layout blocks.
    pub fn render_context(&mut self) -> (&mut FontSystem, &mut TextShaper, &DocumentLayout) {
        (&mut self.font_system, &mut self.shaper, &self.layout)
    }

    /// Visible blocks plus a mutable render context, borrowed together without copying.
    pub fn render_frame(
        &mut self,
    ) -> (&[VisibleBlock], &mut FontSystem, &mut TextShaper, &DocumentLayout) {
        let visible = self.culler.query(
            self.physics.position(),
            self.viewport_height,
            self.overscan,
        );
        (visible, &mut self.font_system, &mut self.shaper, &self.layout)
    }

    /// Ensures that all blocks in or near the current viewport are exact-measured.
    /// Returns any height delta that occurred from transitioning from estimates to exact.
    pub fn ensure_visible_exact(&mut self) -> f32 {
        let range = self.layout.visible_range(
            self.physics.position(),
            self.viewport_height,
            self.overscan,
        );
        let range_copy = range.clone();
        let delta = self.layout.ensure_range_exact(
            range,
            &mut self.font_system,
            &mut self.shaper,
            self.embeds.as_mut(),
        );
        if delta.abs() > 0.001 {
            self.culler.sync_geometries_in_place(
                &self.layout.geometries,
                range_copy,
                delta,
                self.layout.total_height,
            );
            self.physics.set_bounds(self.layout.total_height, self.viewport_height);
        }
        delta
    }

    /// Progressively measures a batch of offscreen blocks during idle frames.
    /// Returns true if any unmeasured blocks were processed.
    pub fn step_idle_measurement(&mut self, batch_size: usize) -> bool {
        if !self.layout.has_unmeasured_blocks() {
            return false;
        }
        let first = self.layout.unmeasured_cursor.get();
        let end = (first + batch_size).min(self.layout.blocks.len());
        let range = first..end;
        let delta = self.layout.measure_batch_exact(
            batch_size,
            &mut self.font_system,
            &mut self.shaper,
            self.embeds.as_mut(),
        );
        if delta.abs() > 0.001 {
            self.culler.sync_geometries_in_place(
                &self.layout.geometries,
                range,
                delta,
                self.layout.total_height,
            );
            self.physics.set_bounds(self.layout.total_height, self.viewport_height);
        }
        true
    }

    #[inline]
    pub fn has_unmeasured_blocks(&self) -> bool {
        self.layout.has_unmeasured_blocks()
    }
}


#[cfg(test)]
pub(crate) mod test_support {
    //! Shared FontSystem + a thread-local counting allocator used to *prove*
    //! the culling hot loop performs zero heap allocations.
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use cosmic_text::FontSystem;

    thread_local! {
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    }

    #[inline]
    fn bump() {
        let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
    }

    struct CountingAllocator;

    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            bump();
            System.alloc(layout)
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            bump();
            System.alloc_zeroed(layout)
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            bump();
            System.realloc(ptr, layout, new_size)
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            System.dealloc(ptr, layout)
        }
    }

    #[global_allocator]
    static GLOBAL: CountingAllocator = CountingAllocator;

    pub fn allocation_count() -> u64 {
        ALLOCATIONS.with(|c| c.get())
    }

    pub fn font_system() -> MutexGuard<'static, FontSystem> {
        static FONTS: OnceLock<Mutex<FontSystem>> = OnceLock::new();
        FONTS
            .get_or_init(|| Mutex::new(FontSystem::new()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Title\n\nสวัสดีครับ นี่คือเอกสารทดสอบ\n\n## Section A\n\nParagraph.\n\n```rust\nfn main() {}\n```\n\n## Section B\n\nEnd.\n";

    #[test]
    fn engine_end_to_end() {
        let mut engine = DocumentLayoutEngine::new(FontSystem::new());
        engine.set_viewport(720.0, 400.0);
        engine.load_markdown(DOC);

        assert!(engine.layout().blocks.len() >= 7);
        let first = engine.visible_blocks();
        assert!(!first.is_empty());
        assert_eq!(first[0].block_index, 0);

        // Jump far past the end: clamps to max scroll, last block visible.
        engine.scroll_by(1.0e9);
        let max = engine.physics().max_scroll();
        assert!((engine.scroll_y() - max).abs() < 0.01);
        let last = engine.layout().blocks.len() - 1;
        assert!(engine.visible_blocks().iter().any(|b| b.block_index == last));

        // Heading navigation animates to the target.
        assert!(engine.scroll_to_heading("section-a"));
        for _ in 0..600 {
            if !engine.tick(1.0 / 144.0) {
                break;
            }
        }
        let target = engine.layout().heading_offset("section-a").unwrap()
            - engine.layout().config.block_spacing;
        let expected = target.clamp(0.0, engine.physics().max_scroll());
        assert!((engine.scroll_y() - expected).abs() < 1.0);

        // Resize keeps working and stays within bounds.
        engine.set_viewport(360.0, 400.0);
        assert!(engine.scroll_y() <= engine.physics().max_scroll() + 0.01);

        let mut refs = Vec::with_capacity(64);
        engine.visible_into(&mut refs);
        assert_eq!(refs.len(), engine.visible_blocks().len());
    }
}