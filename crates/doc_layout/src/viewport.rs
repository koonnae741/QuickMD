//! Virtual viewport culling (O(log N), zero-allocation) and inertia scroll physics.
//!
//! Hot-loop contract: once a [`ViewportCuller`] is built, [`ViewportCuller::query`],
//! [`ViewportCuller::query_range`], [`ViewportCuller::query_into`] (with a warmed-up
//! `Vec`) and [`ScrollPhysics::step`] never touch the heap. This is verified
//! by a counting-allocator test below.
//!
//! Invariant: blocks are laid out top-to-bottom without overlap, so both
//! `y` and `y + height` are monotonically non-decreasing. That is exactly
//! what makes two `partition_point` binary searches sufficient.

use std::ops::Range;

use crate::layout::{BlockGeometry, DocumentLayout};

/// Largest timestep integrated in one go (frame hitches, debugger pauses).
pub const MAX_STEP_DT: f32 = 0.1;

#[inline]
fn sane(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// Document-space culling window `[scroll_y − overscan, scroll_y + viewport_height + overscan]`.
/// Non-finite inputs are sanitised (NaN scroll → 0, NaN sizes → 0).
#[inline]
pub fn cull_window(scroll_y: f32, viewport_height: f32, overscan: f32) -> (f32, f32) {
    let s = if scroll_y.is_finite() { scroll_y } else { 0.0 };
    let vh = sane(viewport_height);
    let o = sane(overscan);
    (s - o, s + vh + o)
}

/// Indices of items whose `[y, y + h)` intersects `(top, bottom)`.
///
/// Two binary searches, O(log N), no allocation. The second search only scans
/// the suffix after `start`, so `end >= start` always holds.
#[inline]
pub fn binary_search_range<T, Y, H>(
    items: &[T],
    top: f32,
    bottom: f32,
    y_of: Y,
    height_of: H,
) -> Range<usize>
where
    Y: Fn(&T) -> f32,
    H: Fn(&T) -> f32,
{
    let start = items.partition_point(|it| y_of(it) + height_of(it) <= top);
    let end = start + items[start..].partition_point(|it| y_of(it) < bottom);
    start..end
}

/// Pre-baked, renderer-ready block rectangle (document space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisibleBlock {
    pub block_index: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl VisibleBlock {
    #[inline]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Y position on screen for the given scroll offset.
    #[inline]
    pub fn screen_y(&self, scroll_y: f32) -> f32 {
        self.y - scroll_y
    }

    pub fn screen_rect(&self, scroll_y: f32) -> Option<tiny_skia::Rect> {
        tiny_skia::Rect::from_xywh(self.x, self.screen_y(scroll_y), self.width, self.height)
    }

    /// Visible slice `(top, bottom)` in block-local coordinates. Lets the
    /// renderer rasterise only the on-screen lines of a 100k-line code block.
    #[inline]
    pub fn clip_local(&self, scroll_y: f32, viewport_height: f32) -> (f32, f32) {
        let h = sane(self.height);
        let top = (scroll_y - self.y).clamp(0.0, h);
        let bottom = (scroll_y + sane(viewport_height) - self.y).clamp(0.0, h);
        (top, bottom.max(top))
    }
}

/// O(log N) viewport culler over a flat, sorted array of [`VisibleBlock`]s.
#[derive(Debug, Clone, Default)]
pub struct ViewportCuller {
    blocks: Vec<VisibleBlock>,
    total_height: f32,
}

impl ViewportCuller {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_layout(layout: &DocumentLayout, content_x: f32) -> Self {
        let mut culler = Self::new();
        culler.rebuild(layout, content_x);
        culler
    }

    /// Rebuilds after (re)layout. Reuses the existing allocation when possible.
    pub fn rebuild(&mut self, layout: &DocumentLayout, content_x: f32) {
        self.rebuild_from_geometries(&layout.geometries, content_x, layout.total_height);
    }

    pub fn rebuild_from_geometries(&mut self, geometries: &[BlockGeometry], content_x: f32, total_height: f32) {
        let x = if content_x.is_finite() { content_x } else { 0.0 };
        self.blocks.clear();
        self.blocks.reserve(geometries.len());
        self.blocks.extend(geometries.iter().map(|g| VisibleBlock {
            block_index: g.block_index,
            x,
            y: g.y_offset,
            width: g.width,
            height: sane(g.height),
        }));
        debug_assert!(
            self.blocks.windows(2).all(|w| w[0].y <= w[1].y && w[0].bottom() <= w[1].bottom()),
            "block geometries must be sorted and non-overlapping"
        );
        self.total_height = sane(total_height);
    }

    /// In-place synchronization of modified block geometries without clearing or reallocating `self.blocks`.
    /// Zero memory allocation during incremental scrolling and dynamic hydration.
    pub fn sync_geometries_in_place(
        &mut self,
        geometries: &[BlockGeometry],
        modified_range: Range<usize>,
        suffix_delta: f32,
        total_height: f32,
    ) {
        if self.blocks.len() != geometries.len() {
            self.rebuild_from_geometries(geometries, 0.0, total_height);
            return;
        }

        let start = modified_range.start.min(self.blocks.len());
        let end = modified_range.end.min(self.blocks.len());

        for i in start..end {
            let g = &geometries[i];
            let b = &mut self.blocks[i];
            b.y = g.y_offset;
            b.width = g.width;
            b.height = sane(g.height);
        }

        if suffix_delta.abs() > 0.001 && end < self.blocks.len() {
            for b in &mut self.blocks[end..] {
                b.y += suffix_delta;
            }
        }
        self.total_height = sane(total_height);
        debug_assert!(
            self.blocks.windows(2).all(|w| w[0].y <= w[1].y && w[0].bottom() <= w[1].bottom()),
            "block geometries must remain sorted and non-overlapping after in-place sync"
        );
    }

    /// Index range of visible blocks. O(log N), zero allocation.
    #[inline]
    pub fn query_range(&self, scroll_y: f32, viewport_height: f32, overscan: f32) -> Range<usize> {
        let (top, bottom) = cull_window(scroll_y, viewport_height, overscan);
        binary_search_range(&self.blocks, top, bottom, |b| b.y, |b| b.height)
    }

    /// Visible blocks as a borrowed slice. O(log N), zero allocation.
    #[inline]
    pub fn query(&self, scroll_y: f32, viewport_height: f32, overscan: f32) -> &[VisibleBlock] {
        &self.blocks[self.query_range(scroll_y, viewport_height, overscan)]
    }

    /// Fills `out` with `&VisibleBlock`s. Allocation-free once `out` has
    /// enough capacity (keep one `Vec` alive across frames).
    #[inline]
    pub fn query_into<'a>(
        &'a self,
        scroll_y: f32,
        viewport_height: f32,
        overscan: f32,
        out: &mut Vec<&'a VisibleBlock>,
    ) {
        out.clear();
        out.extend(self.query(scroll_y, viewport_height, overscan));
    }

    /// Convenience `Vec<&VisibleBlock>`. **Allocates** — not for the render loop.
    pub fn visible(&self, scroll_y: f32, viewport_height: f32, overscan: f32) -> Vec<&VisibleBlock> {
        self.query(scroll_y, viewport_height, overscan).iter().collect()
    }

    /// First block whose bottom edge is below `scroll_y` (TOC highlighting, scroll anchors).
    #[inline]
    pub fn first_visible(&self, scroll_y: f32) -> Option<usize> {
        let i = self.blocks.partition_point(|b| b.bottom() <= scroll_y);
        self.blocks.get(i).map(|b| b.block_index)
    }

    pub fn blocks(&self) -> &[VisibleBlock] {
        &self.blocks
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn total_height(&self) -> f32 {
        self.total_height
    }

    pub fn capacity(&self) -> usize {
        self.blocks.capacity()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Motion {
    Idle,
    Fling,
    Target(f32),
}

/// Frame-rate independent scroll physics.
///
/// * **Fling:** exponential velocity decay `v(t) = v₀·e^(−k·t)`, integrated
///   *exactly* per step (`Δx = v·(1 − e^(−k·dt))/k`), so 60 Hz and 144 Hz
///   produce the same trajectory.
/// * **Animated scroll** (wheel, `scroll_to`, heading jumps): exponential
///   approach `x += (target − x)·(1 − e^(−r·dt))`, also exact per step.
/// * Hard clamping at `[0, max_scroll]`; hitting a bound ends a fling.
#[derive(Debug, Clone)]
pub struct ScrollPhysics {
    position: f32,
    velocity: f32,
    motion: Motion,
    max_scroll: f32,
    /// Fling velocity decay rate (1/s). Higher stops sooner.
    pub deceleration_rate: f32,
    /// Animated-scroll approach rate (1/s).
    pub smoothing_rate: f32,
    /// Flings end below this speed (px/s).
    pub min_velocity: f32,
    /// Fling speed cap (px/s).
    pub max_velocity: f32,
    /// Animated scrolls snap to target within this distance (px).
    pub snap_distance: f32,
}

impl Default for ScrollPhysics {
    fn default() -> Self {
        Self {
            position: 0.0,
            velocity: 0.0,
            motion: Motion::Idle,
            max_scroll: 0.0,
            deceleration_rate: 3.0,
            smoothing_rate: 18.0,
            min_velocity: 2.0,
            max_velocity: 15_000.0,
            snap_distance: 0.25,
        }
    }
}

impl ScrollPhysics {
    #[inline]
    pub fn position(&self) -> f32 {
        self.position
    }

    #[inline]
    pub fn velocity(&self) -> f32 {
        if self.motion == Motion::Fling { self.velocity } else { 0.0 }
    }

    #[inline]
    pub fn max_scroll(&self) -> f32 {
        self.max_scroll
    }

    #[inline]
    pub fn is_animating(&self) -> bool {
        self.motion != Motion::Idle
    }

    pub fn target(&self) -> Option<f32> {
        match self.motion {
            Motion::Target(t) => Some(t),
            _ => None,
        }
    }

    /// Scroll position as a fraction in `[0, 1]` (scrollbar thumb).
    pub fn progress(&self) -> f32 {
        if self.max_scroll <= 0.0 { 0.0 } else { (self.position / self.max_scroll).clamp(0.0, 1.0) }
    }

    /// Scrollbar thumb drag.
    pub fn set_progress(&mut self, fraction: f32) {
        if fraction.is_finite() {
            self.jump_to(fraction.clamp(0.0, 1.0) * self.max_scroll);
        }
    }

    #[inline]
    fn clamp(&self, y: f32) -> f32 {
        y.clamp(0.0, self.max_scroll)
    }

    pub fn set_bounds(&mut self, content_height: f32, viewport_height: f32) {
        self.max_scroll = (sane(content_height) - sane(viewport_height)).max(0.0);
        self.position = self.clamp(self.position);
        if let Motion::Target(t) = self.motion {
            self.motion = Motion::Target(self.clamp(t));
        }
    }

    pub fn stop(&mut self) {
        self.velocity = 0.0;
        self.motion = Motion::Idle;
    }

    /// Instant jump (anchoring after relayout, programmatic restore).
    pub fn jump_to(&mut self, y: f32) {
        if !y.is_finite() {
            return;
        }
        self.position = self.clamp(y);
        self.stop();
    }

    /// Direct manipulation (finger/drag): moves immediately, cancels motion.
    pub fn scroll_by(&mut self, dy: f32) {
        if !dy.is_finite() {
            return;
        }
        self.position = self.clamp(self.position + dy);
        self.stop();
    }

    /// Wheel notches accumulate onto the pending target.
    pub fn smooth_scroll_by(&mut self, dy: f32) {
        if !dy.is_finite() {
            return;
        }
        let base = match self.motion {
            Motion::Target(t) => t,
            _ => self.position,
        };
        self.animate_to(base + dy);
    }

    pub fn animate_to(&mut self, y: f32) {
        if !y.is_finite() {
            return;
        }
        let target = self.clamp(y);
        self.velocity = 0.0;
        if (target - self.position).abs() <= self.snap_distance {
            self.position = target;
            self.motion = Motion::Idle;
        } else {
            self.motion = Motion::Target(target);
        }
    }

    /// Start inertial scrolling with `velocity` px/s (positive = down).
    pub fn fling(&mut self, velocity: f32) {
        if !velocity.is_finite() || velocity.abs() < self.min_velocity {
            self.stop();
            return;
        }
        self.velocity = velocity.clamp(-self.max_velocity, self.max_velocity);
        self.motion = Motion::Fling;
    }

    /// Where the current motion will come to rest. Use it to pre-shape
    /// blocks around the landing zone before the fling arrives.
    pub fn predicted_rest_position(&self) -> f32 {
        match self.motion {
            Motion::Idle => self.position,
            Motion::Target(t) => t,
            Motion::Fling => {
                let k = self.deceleration_rate.max(1e-3);
                self.clamp(self.position + self.velocity / k)
            }
        }
    }

    /// Advance by `dt` seconds. Returns `true` while still animating.
    /// Pure arithmetic: zero allocation.
    pub fn step(&mut self, dt: f32) -> bool {
        let dt = if dt.is_finite() { dt.clamp(0.0, MAX_STEP_DT) } else { 0.0 };
        match self.motion {
            Motion::Idle => false,
            Motion::Fling => {
                let k = self.deceleration_rate.max(1e-3);
                let decay = (-k * dt).exp();
                let travel = self.velocity * (1.0 - decay) / k;
                self.velocity *= decay;
                let next = self.position + travel;
                let clamped = self.clamp(next);
                self.position = clamped;
                if clamped != next || self.velocity.abs() < self.min_velocity {
                    self.stop();
                    false
                } else {
                    true
                }
            }
            Motion::Target(target) => {
                let r = self.smoothing_rate.max(1e-3);
                let alpha = 1.0 - (-r * dt).exp();
                self.position += (target - self.position) * alpha;
                if (target - self.position).abs() <= self.snap_distance {
                    self.position = target;
                    self.stop();
                    false
                } else {
                    true
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::MarkdownBlock;
    use crate::layout::LayoutConfig;
    use crate::test_support::allocation_count;
    use std::collections::HashMap;
    use std::hint::black_box;

    /// Synthetic 100k-block document (no fonts needed): varying heights + 4px gaps.
    fn synthetic(n: usize) -> (Vec<BlockGeometry>, f32) {
        let mut y = 24.0f32;
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let h = 20.0 + (i % 7) as f32 * 4.0;
            out.push(BlockGeometry { block_index: i, y_offset: y, height: h, width: 800.0 });
            y += h + 4.0;
        }
        (out, y + 48.0)
    }

    fn culler_for(n: usize) -> (ViewportCuller, f32) {
        let (geoms, total) = synthetic(n);
        let mut c = ViewportCuller::new();
        c.rebuild_from_geometries(&geoms, 32.0, total);
        (c, total)
    }

    fn brute(c: &ViewportCuller, top: f32, bottom: f32) -> Vec<usize> {
        c.blocks()
            .iter()
            .filter(|b| b.y + b.height > top && b.y < bottom)
            .map(|b| b.block_index)
            .collect()
    }

    struct Lcg(u64);
    impl Lcg {
        fn next_f32(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    #[test]
    fn binary_search_matches_brute_force_on_100k_blocks() {
        let (c, total) = culler_for(100_000);
        let mut rng = Lcg(0x9E37_79B9_7F4A_7C15);
        for _ in 0..300 {
            let scroll = rng.next_f32() * (total + 2000.0) - 1000.0;
            let vh = rng.next_f32() * 1500.0;
            let overscan = rng.next_f32() * 800.0;
            let range = c.query_range(scroll, vh, overscan);
            let (top, bottom) = cull_window(scroll, vh, overscan);
            let expected = brute(&c, top, bottom);
            if expected.is_empty() {
                assert!(range.is_empty(), "scroll {scroll}: expected empty, got {range:?}");
            } else {
                assert_eq!(range, expected[0]..expected[expected.len() - 1] + 1, "scroll {scroll}");
            }
        }
    }

    #[test]
    fn query_edge_cases() {
        let (c, total) = culler_for(1000);
        assert!(c.query(-5000.0, 100.0, 0.0).is_empty());
        assert!(c.query(total + 5000.0, 100.0, 0.0).is_empty());
        assert_eq!(c.query(0.0, f32::INFINITY, 0.0).len(), 0, "infinite viewport is sanitised to 0");
        assert_eq!(c.query(0.0, total, 0.0).len(), 1000);
        assert_eq!(c.query(f32::NAN, 100.0, f32::NAN)[0].block_index, 0);
        assert!(ViewportCuller::new().query(0.0, 1000.0, 600.0).is_empty());
        assert_eq!(c.blocks()[0].x, 32.0);
        assert_eq!(c.first_visible(0.0), Some(0));
        assert_eq!(c.first_visible(total + 1.0), None);
        let visible = c.visible(500.0, 300.0, 0.0);
        assert_eq!(visible.len(), c.query(500.0, 300.0, 0.0).len());
    }

    #[test]
    fn culler_agrees_with_layout_visible_range() {
        let (geometries, total_height) = synthetic(5000);
        let layout = DocumentLayout {
            blocks: vec![MarkdownBlock::HorizontalRule; 5000],
            geometries,
            exact_measured: vec![true; 5000],
            total_height,
            content_width: 800.0,
            config: LayoutConfig::default(),
            table_layouts: HashMap::new(),
            list_layouts: HashMap::new(),
            unmeasured_cursor: std::cell::Cell::new(5000),
        };
        let c = ViewportCuller::from_layout(&layout, 0.0);
        let mut y = -300.0;
        while y < total_height + 300.0 {
            assert_eq!(c.query_range(y, 720.0, 240.0), layout.visible_range(y, 720.0, 240.0));
            y += 173.0;
        }
    }

    #[test]
    fn rebuild_reuses_storage() {
        let (geoms, total) = synthetic(10_000);
        let mut c = ViewportCuller::new();
        c.rebuild_from_geometries(&geoms, 0.0, total);
        let cap = c.capacity();
        c.rebuild_from_geometries(&geoms[..5000], 0.0, total);
        assert_eq!(c.capacity(), cap);
        assert_eq!(c.len(), 5000);
    }

    #[test]
    fn sync_geometries_in_place_preserves_accuracy() {
        let (mut geoms, mut total) = synthetic(1000);
        let mut c = ViewportCuller::new();
        c.rebuild_from_geometries(&geoms, 0.0, total);

        // Simulate block 50 growing by 80px
        let delta = 80.0;
        geoms[50].height += delta;
        for g in &mut geoms[51..] {
            g.y_offset += delta;
        }
        total += delta;

        c.sync_geometries_in_place(&geoms, 50..51, delta, total);
        assert_eq!(c.len(), 1000);
        assert_eq!(c.blocks[50].height, geoms[50].height);
        assert_eq!(c.blocks[51].y, geoms[51].y_offset);
        assert_eq!(c.blocks[999].y, geoms[999].y_offset);
        assert_eq!(c.total_height(), total);
    }

    #[test]
    fn clip_local_slices_tall_blocks() {
        let b = VisibleBlock { block_index: 0, x: 0.0, y: 1000.0, width: 100.0, height: 5000.0 };
        assert_eq!(b.clip_local(0.0, 800.0), (0.0, 0.0));
        assert_eq!(b.clip_local(1500.0, 800.0), (500.0, 1300.0));
        assert_eq!(b.clip_local(5800.0, 800.0), (4800.0, 5000.0));
        assert_eq!(b.screen_y(1500.0), -500.0);
        assert!(b.screen_rect(1500.0).is_some());
    }

    /// The 144 FPS contract: physics step + culling + `Vec<&VisibleBlock>` fill
    /// performs **zero** heap allocations per frame.
    #[test]
    fn hot_loop_is_allocation_free() {
        let (c, total) = culler_for(100_000);
        let mut physics = ScrollPhysics::default();
        physics.set_bounds(total, 900.0);
        physics.fling(8000.0);

        let mut out: Vec<&VisibleBlock> = Vec::with_capacity(1024);
        c.query_into(0.0, 900.0, 600.0, &mut out); // warm-up

        let before = allocation_count();
        let mut checksum = 0usize;
        for frame in 0..20_000u32 {
            physics.step(1.0 / 144.0);
            if !physics.is_animating() {
                physics.fling(if frame % 2 == 0 { -9000.0 } else { 9000.0 });
            }
            let y = physics.position();
            checksum += black_box(c.query(y, 900.0, 600.0)).len();
            c.query_into(y, 900.0, 600.0, &mut out);
            checksum += black_box(&out).len();
            checksum += black_box(c.query_range(y, 900.0, 600.0)).len();
        }
        let after = allocation_count();

        assert_eq!(after - before, 0, "render hot loop allocated {} times", after - before);
        assert!(checksum > 0);
    }

    // ------------------------------------------------------------ physics

    fn physics(content: f32, viewport: f32) -> ScrollPhysics {
        let mut p = ScrollPhysics::default();
        p.set_bounds(content, viewport);
        p
    }

    fn run(p: &mut ScrollPhysics, fps: f32, max_seconds: f32) {
        let frames = (fps * max_seconds) as usize;
        for _ in 0..frames {
            if !p.step(1.0 / fps) {
                break;
            }
        }
    }

    #[test]
    fn fling_decays_and_comes_to_rest_at_prediction() {
        let mut p = physics(1_000_000.0, 800.0);
        p.jump_to(10_000.0);
        p.fling(3000.0);
        let predicted = p.predicted_rest_position();
        assert!((predicted - 11_000.0).abs() < 0.01);
        run(&mut p, 144.0, 10.0);
        assert!(!p.is_animating());
        assert_eq!(p.velocity(), 0.0);
        assert!((p.position() - predicted).abs() < 1.0, "{} vs {predicted}", p.position());
    }

    #[test]
    fn fling_is_frame_rate_independent() {
        let mut a = physics(1_000_000.0, 800.0);
        let mut b = a.clone();
        a.fling(4000.0);
        b.fling(4000.0);
        run(&mut a, 60.0, 5.0);
        run(&mut b, 144.0, 5.0);
        assert!((a.position() - b.position()).abs() < 1.0, "60Hz {} vs 144Hz {}", a.position(), b.position());
    }

    #[test]
    fn animated_scroll_is_frame_rate_independent() {
        let mut a = physics(100_000.0, 800.0);
        let mut b = a.clone();
        a.animate_to(1000.0);
        b.animate_to(1000.0);
        for _ in 0..15 {
            a.step(1.0 / 60.0);
        }
        for _ in 0..36 {
            b.step(1.0 / 144.0);
        }
        assert!((a.position() - b.position()).abs() < 0.5);
    }

    #[test]
    fn fling_stops_at_bounds() {
        let mut p = physics(1000.0, 500.0);
        p.jump_to(400.0);
        p.fling(5000.0);
        run(&mut p, 144.0, 5.0);
        assert_eq!(p.position(), 500.0);
        assert!(!p.is_animating());

        p.fling(-50_000.0);
        run(&mut p, 144.0, 5.0);
        assert_eq!(p.position(), 0.0);
    }

    #[test]
    fn animate_to_converges_and_clamps() {
        let mut p = physics(5800.0, 800.0);
        p.animate_to(10_000.0);
        assert_eq!(p.target(), Some(5000.0));
        run(&mut p, 144.0, 5.0);
        assert_eq!(p.position(), 5000.0);
        assert!(!p.is_animating());
        assert_eq!(p.progress(), 1.0);
    }

    #[test]
    fn smooth_scroll_accumulates_wheel_notches() {
        let mut p = physics(10_000.0, 800.0);
        p.smooth_scroll_by(100.0);
        p.step(1.0 / 144.0);
        p.smooth_scroll_by(100.0);
        assert_eq!(p.target(), Some(200.0));
        run(&mut p, 144.0, 5.0);
        assert_eq!(p.position(), 200.0);
    }

    #[test]
    fn bounds_shrink_clamps_position_and_target() {
        let mut p = physics(10_000.0, 800.0);
        p.jump_to(9000.0);
        p.set_bounds(2000.0, 800.0);
        assert_eq!(p.position(), 1200.0);
        p.jump_to(0.0);
        p.animate_to(1200.0);
        p.set_bounds(1000.0, 800.0);
        assert_eq!(p.target(), Some(200.0));
        p.set_bounds(100.0, 800.0);
        assert_eq!(p.max_scroll(), 0.0);
        assert_eq!(p.position(), 0.0);
    }

    #[test]
    fn non_finite_inputs_are_ignored() {
        let mut p = physics(10_000.0, 800.0);
        p.jump_to(500.0);
        p.scroll_by(f32::NAN);
        p.jump_to(f32::INFINITY);
        p.animate_to(f32::NAN);
        p.smooth_scroll_by(f32::NEG_INFINITY);
        assert_eq!(p.position(), 500.0);
        p.fling(f32::NAN);
        assert!(!p.is_animating());
        p.fling(1000.0);
        p.step(f32::NAN);
        assert_eq!(p.position(), 500.0);
        p.set_bounds(f32::NAN, 800.0);
        assert_eq!(p.max_scroll(), 0.0);
    }

    #[test]
    fn scrollbar_progress_roundtrip() {
        let mut p = physics(10_800.0, 800.0);
        p.set_progress(0.25);
        assert_eq!(p.position(), 2500.0);
        assert!((p.progress() - 0.25).abs() < 1e-6);
    }
}