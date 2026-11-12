//! quickmd_bridge — glues `native_render` (math + mermaid) to `doc_layout`,
//! and provides Win32 softbuffer blitting + lightweight image header decoding.
//!
//! This module has ZERO HTML/CSS/JS/Chromium dependencies. Pure Rust.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

use cosmic_text::{fontdb, Attrs, Color as CosmicColor, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use doc_layout::ast::BadgeItem;

// ---- External engine crates -------------------------------------------------
use native_render::native_math::{MathRenderer, MathStyle, RenderOptions};
use native_render::native_mermaid::{DiagramTheme, MermaidRenderer};
use native_render::MathFontContext;

use doc_layout::layout::EmbedMeasurer;

// =============================================================================
// 1. CACHING LAYER (generic, reusable for math + mermaid)
// =============================================================================

/// A single cached render result, keyed by content hash.
struct CachedPixmap {
    pixmap: Pixmap,
    width: f32,
    height: f32,
    natural_width: f32,
    clamped_for_max_width: f32,
}

/// Generic content-addressed pixmap cache used by both math and mermaid.
struct PixmapCache {
    entries: HashMap<u64, CachedPixmap>,
}

impl PixmapCache {
    fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    fn hash_key(content: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    /// Returns cached size if valid for `max_width`, otherwise renders via
    /// `render_fn`, clamps to `max_width`, caches, and returns the new size.
    fn get_or_render<F>(&mut self, key: u64, max_width: f32, render_fn: F) -> (f32, f32)
    where
        F: FnOnce() -> Option<Pixmap>,
    {
        if let Some(entry) = self.entries.get(&key) {
            // Cache hit: valid if original unscaled pixmap fits within requested width,
            // or if it was already clamped to roughly this exact max_width.
            if entry.natural_width <= max_width + 0.5 && (entry.width - entry.natural_width).abs() < 0.5 {
                return (entry.width, entry.height);
            }
            if (entry.clamped_for_max_width - max_width).abs() < 1.0 {
                return (entry.width, entry.height);
            }
        }

        match render_fn() {
            Some(raw) => {
                let nat_w = raw.width() as f32;
                let clamped = clamp_pixmap_width(raw, max_width);
                let (w, h) = (clamped.width() as f32, clamped.height() as f32);
                self.entries.insert(
                    key,
                    CachedPixmap {
                        pixmap: clamped,
                        width: w,
                        height: h,
                        natural_width: nat_w,
                        clamped_for_max_width: max_width,
                    },
                );
                (w, h)
            }
            None => (0.0, 0.0),
        }
    }

    fn get(&self, key: u64) -> Option<&Pixmap> {
        self.entries.get(&key).map(|e| &e.pixmap)
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Down-scales a pixmap (never up-scales) so its width never exceeds
/// `max_width`, preserving aspect ratio. Uses tiny_skia's bilinear-ish
/// `PixmapPaint` draw with a scale transform (cheap, GPU-free).
fn clamp_pixmap_width(src: Pixmap, max_width: f32) -> Pixmap {
    let src_w = src.width() as f32;
    let src_h = src.height() as f32;

    if max_width <= 0.0 || src_w <= max_width || src_w == 0.0 {
        return src;
    }

    let scale = max_width / src_w;
    let dst_w = (src_w * scale).max(1.0).round() as u32;
    let dst_h = (src_h * scale).max(1.0).round() as u32;

    let mut dst = match Pixmap::new(dst_w, dst_h) {
        Some(p) => p,
        None => return src, // fallback: never panic
    };

    let transform = Transform::from_scale(scale, scale);
    let paint = PixmapPaint::default();

    dst.draw_pixmap(0, 0, src.as_ref(), &paint, transform, None);
    dst
}

// =============================================================================
// 2. REAL EMBED MEASURER — Arc<Mutex<...>> SHARED CACHE EDITION
// =============================================================================

struct EmbedMeasurerInner {
    math_renderer: MathRenderer,
    mermaid_renderer: MermaidRenderer,
    mermaid_theme: DiagramTheme,
    math_cache: PixmapCache,
    mermaid_cache: PixmapCache,
    image_cache: PixmapCache,
    badge_cache: HashMap<String, Pixmap>,
    base_dir: Option<PathBuf>,
}

#[derive(Clone)]
pub struct RealEmbedMeasurer {
    inner: Arc<Mutex<EmbedMeasurerInner>>,
}

impl RealEmbedMeasurer {
    pub fn new(math_font_ctx: MathFontContext, mermaid_font_ctx: MathFontContext, mermaid_theme: DiagramTheme) -> Self {
        Self {
            inner: Arc::new(Mutex::new(EmbedMeasurerInner {
                math_renderer: MathRenderer::new(math_font_ctx),
                mermaid_renderer: MermaidRenderer::new(mermaid_font_ctx),
                mermaid_theme,
                math_cache: PixmapCache::new(),
                mermaid_cache: PixmapCache::new(),
                image_cache: PixmapCache::new(),
                badge_cache: HashMap::new(),
                base_dir: None,
            })),
        }
    }

    pub fn set_base_dir(&self, dir: PathBuf) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.base_dir = Some(dir);
    }

    pub fn get_cached_image(&self, url: &str) -> Option<Pixmap> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.image_cache.get(PixmapCache::hash_key(url)).cloned()
    }

    pub fn get_or_render_badge(
        &self,
        badge: &BadgeItem,
        font_system: &mut FontSystem,
        swash: &mut SwashCache,
    ) -> Option<Pixmap> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(pm) = inner.badge_cache.get(&badge.badge_url) {
            return Some(pm.clone());
        }
        if let Some(pm) = render_badge_pill(badge, font_system, swash) {
            inner.badge_cache.insert(badge.badge_url.clone(), pm.clone());
            Some(pm)
        } else {
            None
        }
    }

    pub fn from_system(mermaid_theme: DiagramTheme) -> Self {
        Self::new(
            MathFontContext::from_system(None),
            MathFontContext::from_system(None),
            mermaid_theme,
        )
    }

    pub fn as_measurer(&self) -> Box<dyn EmbedMeasurer> {
        Box::new(self.clone())
    }

    pub fn get_cached_math(&self, latex: &str) -> Option<Pixmap> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.math_cache.get(PixmapCache::hash_key(latex)).cloned()
    }

    pub fn get_cached_mermaid(&self, source: &str) -> Option<Pixmap> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.mermaid_cache.get(PixmapCache::hash_key(source)).cloned()
    }

    pub fn render_unscaled_mermaid(&self, source: &str) -> Option<Pixmap> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let theme = inner.mermaid_theme.clone();
        inner.mermaid_renderer.render(source, &theme).ok()
    }

    pub fn clear_all(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.math_cache.clear();
        inner.mermaid_cache.clear();
        inner.image_cache.clear();
        inner.badge_cache.clear();
    }

    pub fn clear_caches(&self) {
        self.clear_all();
    }
}

impl EmbedMeasurer for RealEmbedMeasurer {
    fn measure_image(&mut self, url: &str, max_width: f32) -> (f32, f32) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let key = PixmapCache::hash_key(url);
        let base_dir = guard.base_dir.clone();
        let url_owned = url.to_string();

        guard.image_cache.get_or_render(key, max_width, || {
            let base = base_dir.as_deref().unwrap_or(Path::new("."));
            load_local_image_with_rounded_corners(base, &url_owned, max_width, 12.0)
        })
    }

    fn measure_math(&mut self, latex: &str, max_width: f32) -> (f32, f32) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let key = PixmapCache::hash_key(latex);
        let latex_owned = latex.to_string();

        let EmbedMeasurerInner {
            math_renderer,
            math_cache,
            ..
        } = &mut *guard;

        math_cache.get_or_render(key, max_width, || {
            let options = RenderOptions {
                scale: 2.0,
                foreground: [230, 237, 243, 255],
                background: None,
                padding: 6.0,
            };
            math_renderer
                .render(&latex_owned, MathStyle::Display, 19.0, &options)
                .ok()
                .map(|pm| {
                    let logical_w = (pm.width() as f32 * 0.5).max(1.0);
                    clamp_pixmap_width(pm, logical_w.min(max_width))
                })
        })
    }

    fn measure_mermaid(&mut self, source: &str, max_width: f32) -> (f32, f32) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let key = PixmapCache::hash_key(source);
        let source_owned = source.to_string();

        let EmbedMeasurerInner {
            mermaid_renderer,
            mermaid_theme,
            mermaid_cache,
            ..
        } = &mut *guard;
        let theme = mermaid_theme.clone();

        mermaid_cache.get_or_render(key, max_width, || {
            mermaid_renderer
                .render(&source_owned, &theme)
                .ok()
                .map(|pm| {
                    let logical_w = (pm.width() as f32 * 0.50).max(1.0);
                    clamp_pixmap_width(pm, logical_w.min(max_width))
                })
        })
    }
}

// =============================================================================
// 3. DARK THEME FOR MERMAID
// =============================================================================

/// Extension trait so callers can write `DiagramTheme::dark()` at the
/// call-site without modifying `native_mermaid` itself.
pub trait DarkDiagramTheme {
    fn dark() -> Self;
}

impl DarkDiagramTheme for DiagramTheme {
    fn dark() -> Self {
        DiagramTheme {
            background: None, // transparent — host window background shows through
            node_fill: [26, 29, 38, 255],      // #1A1D26
            node_stroke: [99, 102, 241, 255],  // #6366F1
            node_text_color: [226, 232, 240, 255],
            node_stroke_width: 1.8,
            edge_stroke_width: 1.6,   // #E2E8F0
            edge_stroke: [100, 116, 139, 255], // #64748B
            font_size: 14.0,
            scale: 2.0, // retina crispness
            ..Default::default() // tolerate extra fields on the real struct
        }
    }
}

// =============================================================================
// 4. SHARED FONT CONTEXT
// =============================================================================

/// A single shared `cosmic_text::FontSystem`, built once at startup, so the
/// math engine and the document layout engine never scan system fonts twice.
#[derive(Clone)]
pub struct SharedFonts {
    inner: Arc<Mutex<FontSystem>>,
}

impl SharedFonts {
    pub fn into_font_system(self) -> FontSystem {
        match Arc::try_unwrap(self.inner) {
            Ok(mutex) => mutex.into_inner().unwrap_or_else(|e| e.into_inner()),
            Err(arc) => {
                let guard = arc.lock().unwrap_or_else(|e| e.into_inner());
                let db = guard.db().clone();
                FontSystem::new_with_locale_and_db(guard.locale().to_string(), db)
            }
        }
    }

    pub fn new_math_font_context(&self) -> MathFontContext {
        MathFontContext::from_system(None)
    }

    /// Builds the font database once with the project's fallback chain.
    pub fn new() -> Self {
        const FALLBACKS: &[&str] = &["Segoe UI", "Leelawadee UI", "Tahoma", "sans-serif"];

        let mut db = fontdb::Database::new();
        db.load_system_fonts();

        let mut font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);

        for family in FALLBACKS {
            font_system
                .db_mut()
                .set_sans_serif_family(family.to_string());
            // first available family wins; cosmic-text resolves fallback at shape time
        }

        Self {
            inner: Arc::new(Mutex::new(font_system)),
        }
    }

    /// Returns a lock guard to the shared `FontSystem`. Keep lock scope short.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, FontSystem> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Builds a `MathFontContext` that shares this font database (no rescan).
    pub fn math_font_context(&self) -> MathFontContext {
        MathFontContext::from_system(None)
    }
}

impl Default for SharedFonts {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// 5. BLIT: tiny_skia RGBA -> Win32 softbuffer 0x00RRGGBB
// =============================================================================

/// Converts a `tiny_skia::Pixmap` (premultiplied RGBA8) into a softbuffer
/// `u32` buffer in native `0x00RRGGBB` word order. Bounds-checked: if
/// `pixmap` is larger or smaller than `(dest_width, dest_height)`, only the
/// overlapping region is copied (clipped), never panics.
pub fn blit_pixmap_to_softbuffer(
    pixmap: &Pixmap,
    buffer: &mut [u32],
    dest_width: usize,
    dest_height: usize,
) {
    let src_w = pixmap.width() as usize;
    let src_h = pixmap.height() as usize;

    if src_w == 0 || src_h == 0 || dest_width == 0 || dest_height == 0 {
        return;
    }

    let copy_w = src_w.min(dest_width);
    let copy_h = src_h.min(dest_height);

    if buffer.len() < dest_width * copy_h {
        // Destination buffer too small — bail safely, no panic.
        return;
    }

    let data = pixmap.data(); // RGBA8, premultiplied, 4 bytes/pixel, tightly packed

    for y in 0..copy_h {
        let src_row_off = y * src_w * 4;
        let dst_row_off = y * dest_width;

        let src_row = &data[src_row_off..src_row_off + copy_w * 4];
        let dst_row = &mut buffer[dst_row_off..dst_row_off + copy_w];

        for (dst_px, src_px) in dst_row.iter_mut().zip(src_row.chunks_exact(4)) {
            let (mut r, mut g, mut b) = (src_px[0], src_px[1], src_px[2]);
            let a = src_px[3];

            // Un-premultiply so colors aren't darkened against a black bg.
            if a > 0 && a < 255 {
                let a32 = a as u32;
                r = ((r as u32 * 255) / a32) as u8;
                g = ((g as u32 * 255) / a32) as u8;
                b = ((b as u32 * 255) / a32) as u8;
            }

            *dst_px = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        }
    }
}

// =============================================================================
// 6. IMAGE DIMENSION SNIFFING + LOCAL IMAGE LOADING
// =============================================================================

/// Fast header-only dimension extraction. No full image decode.
/// Supports PNG, JPEG (baseline + progressive SOF markers), GIF, and WebP
/// (VP8 / VP8L / VP8X).
pub fn decode_image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 {
        return None;
    }

    // --- PNG: 8-byte signature + IHDR chunk ---
    const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(&PNG_SIG) && bytes.len() >= 24 {
        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((w, h));
    }

    // --- GIF: "GIF87a" / "GIF89a" ---
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        if bytes.len() >= 10 {
            let w = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32;
            let h = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32;
            return Some((w, h));
        }
        return None;
    }

    // --- WebP: "RIFF"....."WEBP" ---
    if bytes.len() >= 30 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let chunk = &bytes[12..16];
        return match chunk {
            b"VP8 " => {
                // Lossy: dims at offset 26 (14-bit, little-endian, mask 0x3FFF)
                let w = (u16::from_le_bytes(bytes[26..28].try_into().ok()?) & 0x3FFF) as u32;
                let h = (u16::from_le_bytes(bytes[28..30].try_into().ok()?) & 0x3FFF) as u32;
                Some((w, h))
            }
            b"VP8L" => {
                // Lossless: 14 bits width-1, 14 bits height-1, packed from byte 21
                if bytes.len() < 25 {
                    return None;
                }
                let b0 = bytes[21] as u32;
                let b1 = bytes[22] as u32;
                let b2 = bytes[23] as u32;
                let b3 = bytes[24] as u32;
                let bits = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
                let w = (bits & 0x3FFF) + 1;
                let h = ((bits >> 14) & 0x3FFF) + 1;
                Some((w, h))
            }
            b"VP8X" => {
                if bytes.len() < 30 {
                    return None;
                }
                let w = (bytes[24] as u32
                    | (bytes[25] as u32) << 8
                    | (bytes[26] as u32) << 16)
                    + 1;
                let h = (bytes[27] as u32
                    | (bytes[28] as u32) << 8
                    | (bytes[29] as u32) << 16)
                    + 1;
                Some((w, h))
            }
            _ => None,
        };
    }

    // --- JPEG: scan markers for SOFx (0xC0-0xC3, 0xC5-0xC7, 0xC9-0xCB, 0xCD-0xCF) ---
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut i = 2usize;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            // Skip padding 0xFF bytes
            if marker == 0xFF {
                i += 1;
                continue;
            }
            // Standalone markers without length (none relevant here)
            if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
                i += 2;
                continue;
            }
            let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            let is_sof = matches!(
                marker,
                0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD
                    | 0xCE
                    | 0xCF
            );
            if is_sof && i + 9 < bytes.len() {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((w, h));
            }
            if marker == 0xD8 || marker == 0xD9 {
                break;
            }
            if seg_len < 2 {
                break; // malformed, bail safely
            }
            i += 2 + seg_len;
        }
        return None;
    }

    None
}

pub fn percent_decode_path(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push(((hi << 4) | lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

/// Resolves a Windows system executable inside `%SystemRoot%` so `CreateProcessW`
/// never searches the Current Working Directory (CWD binary planting prevention).
#[cfg(target_os = "windows")]
pub fn windows_system_binary(relative_under_windows: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::ffi::OsString::from(r"C:\Windows"));
    PathBuf::from(root).join(relative_under_windows)
}

/// Returns `true` if a `Path` carries a UNC network share or Windows device prefix.
pub fn has_unsafe_windows_prefix(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    path.components().any(|c| match c {
        Component::Prefix(p) => matches!(
            p.kind(),
            Prefix::UNC(_, _)
                | Prefix::VerbatimUNC(_, _)
                | Prefix::DeviceNS(_)
                | Prefix::Verbatim(_)
        ),
        _ => false,
    })
}

/// Resolves a user/document-supplied local path or `file://` URI relative to `base_dir`,
/// blocking UNC network paths, remote `file://host/` shares, control characters, and Windows
/// device namespaces (`\\.\`) both before and after percent-decoding.
pub fn resolve_safe_local_path(
    base_dir: &Path,
    raw_input: &str,
) -> Result<(PathBuf, bool), &'static str> {
    let trimmed = raw_input.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
        return Err("Blocked empty or malformed path");
    }
    if trimmed.starts_with(r"\\") || trimmed.starts_with("//") {
        return Err("Blocked network UNC path");
    }

    let mut had_file_scheme = false;
    let after_scheme = if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("file://") {
        had_file_scheme = true;
        let rest = &trimmed[7..];
        if let Some(local) = rest.strip_prefix('/') {
            // Standard `file:///C:/...` (or Linux `/...`)
            local
        } else if rest.len() >= 10 && rest[..10].eq_ignore_ascii_case("localhost/") {
            &rest[10..]
        } else if rest.len() >= 2 && rest.as_bytes().get(1) == Some(&b':') {
            // Common Windows shorthand `file://C:/...`
            rest
        } else {
            // Reject remote host in `file://server/share` (RFC 8089 UNC authority)
            return Err("Blocked remote file:// host");
        }
    } else {
        trimmed
    };

    let check_raw_unc = after_scheme.trim_start_matches('/');
    if after_scheme.starts_with(r"\\")
        || after_scheme.starts_with("//")
        || check_raw_unc.starts_with(r"\\")
        || check_raw_unc.starts_with("//")
    {
        return Err("Blocked network UNC path");
    }

    let decoded = percent_decode_path(after_scheme);
    let trimmed_dec = decoded.trim();
    if trimmed_dec.is_empty() || trimmed_dec.chars().any(|c| c.is_control()) {
        return Err("Blocked control character in path");
    }
    let check_dec_unc = trimmed_dec.trim_start_matches('/');
    if trimmed_dec.starts_with(r"\\")
        || trimmed_dec.starts_with("//")
        || check_dec_unc.starts_with(r"\\")
        || check_dec_unc.starts_with("//")
    {
        return Err("Blocked encoded UNC or device path");
    }

    let is_absolute = had_file_scheme
        || (trimmed_dec.len() >= 2 && trimmed_dec.as_bytes().get(1) == Some(&b':'))
        || trimmed_dec.starts_with('/')
        || trimmed_dec.starts_with('\\');

    let full_path = if is_absolute {
        PathBuf::from(trimmed_dec)
    } else {
        base_dir.join(trimmed_dec)
    };

    if has_unsafe_windows_prefix(&full_path) {
        return Err("Blocked network UNC or device path");
    }

    Ok((full_path, is_absolute))
}

/// Loads a local image relative to `base_dir`, decodes it fully, resizes
/// (down only) to respect `max_width` preserving aspect ratio, and converts
/// to a premultiplied `tiny_skia::Pixmap`. Returns `None` on any I/O or
/// decode failure (never panics).
pub fn load_local_image(base_dir: &Path, rel_path: &str, max_width: f32) -> Option<Pixmap> {
    let trimmed = rel_path.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Strip query strings (?raw=true) and fragments (#width=...)
    let clean_url = trimmed
        .split('?')
        .next()
        .unwrap_or(trimmed)
        .split('#')
        .next()
        .unwrap_or(trimmed);

    let (full_path, _) = resolve_safe_local_path(base_dir, clean_url).ok()?;

    // SEC-02: Limit image file size to 32MB max
    const MAX_IMAGE_FILE_BYTES: u64 = 32 * 1024 * 1024;
    let meta = std::fs::metadata(&full_path).ok()?;
    if meta.len() > MAX_IMAGE_FILE_BYTES {
        return None;
    }

    // SEC-02: Check dimensions from the header before decoding (max 4096x4096 ≈ 64 MB RGBA)
    const MAX_IMAGE_SIDE: u32 = 4096;
    let (hdr_w, hdr_h) = image::image_dimensions(&full_path).ok()?;
    if hdr_w == 0 || hdr_h == 0 || hdr_w > MAX_IMAGE_SIDE || hdr_h > MAX_IMAGE_SIDE {
        return None;
    }

    let bytes = std::fs::read(&full_path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (orig_w, orig_h) = (img.width(), img.height());
    if orig_w == 0 || orig_h == 0 || orig_w > MAX_IMAGE_SIDE || orig_h > MAX_IMAGE_SIDE {
        return None;
    }

    let (target_w, target_h) = if max_width > 0.0 && (orig_w as f32) > max_width {
        let scale = max_width / orig_w as f32;
        (
            (orig_w as f32 * scale).max(1.0).round() as u32,
            (orig_h as f32 * scale).max(1.0).round() as u32,
        )
    } else {
        (orig_w, orig_h)
    };

    let resized = if (target_w, target_h) != (orig_w, orig_h) {
        image::imageops::resize(&img, target_w, target_h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let mut pixmap = Pixmap::new(target_w, target_h)?;
    let dst = pixmap.data_mut();

    for (i, px) in resized.pixels().enumerate() {
        let [r, g, b, a] = px.0;
        // Premultiply straight-alpha RGBA -> tiny_skia's premultiplied format.
        let af = a as u32;
        let pr = ((r as u32 * af) / 255) as u8;
        let pg = ((g as u32 * af) / 255) as u8;
        let pb = ((b as u32 * af) / 255) as u8;

        let off = i * 4;
        dst[off] = pr;
        dst[off + 1] = pg;
        dst[off + 2] = pb;
        dst[off + 3] = a;
    }

    Some(pixmap)
}


pub fn load_local_image_with_rounded_corners(base_dir: &Path, rel_path: &str, max_width: f32, radius: f32) -> Option<Pixmap> {
    let mut pixmap = load_local_image(base_dir, rel_path, max_width)?;
    if radius > 0.5 {
        let (w, h) = (pixmap.width(), pixmap.height());
        let mut mask = tiny_skia::Mask::new(w, h)?;
        let mut pb = PathBuilder::new();
        pb.move_to(radius, 0.0);
        pb.line_to(w as f32 - radius, 0.0);
        pb.quad_to(w as f32, 0.0, w as f32, radius);
        pb.line_to(w as f32, h as f32 - radius);
        pb.quad_to(w as f32, h as f32, w as f32 - radius, h as f32);
        pb.line_to(radius, h as f32);
        pb.quad_to(0.0, h as f32, 0.0, h as f32 - radius);
        pb.line_to(0.0, radius);
        pb.quad_to(0.0, 0.0, radius, 0.0);
        pb.close();
        if let Some(p) = pb.finish() {
            mask.fill_path(&p, FillRule::Winding, true, Transform::identity());
            pixmap.apply_mask(&mask);
        }
    }
    Some(pixmap)
}

pub fn render_badge_pill(pill: &BadgeItem, font_system: &mut FontSystem, swash: &mut SwashCache) -> Option<Pixmap> {
    let font_size = 11.0f32;
    let pill_height = 20.0f32;
    let pad_x = 7.0f32;

    let has_label = !pill.label.trim().is_empty();

    let measure = |txt: &str, fs: &mut FontSystem| -> f32 {
        let mut buf = cosmic_text::Buffer::new(fs, Metrics::new(font_size, font_size * 1.2));
        buf.set_text(fs, txt, Attrs::new().family(Family::Name("Segoe UI")).weight(Weight::SEMIBOLD), Shaping::Advanced);
        buf.layout_runs().map(|r| r.line_w).fold(0.0f32, f32::max)
    };

    let label_w = if has_label { measure(&pill.label, font_system) } else { 0.0 };
    let val_w = measure(&pill.value, font_system);

    let left_w = if has_label { (label_w + pad_x * 2.0).ceil() } else { 0.0 };
    let right_w = (val_w + pad_x * 2.0).ceil();
    let total_w = left_w + right_w;

    let mut pixmap = Pixmap::new(total_w as u32, pill_height as u32)?;
    let radius = 4.0f32;

    if has_label {
        let mut pb_left = PathBuilder::new();
        pb_left.move_to(radius, 0.0);
        pb_left.line_to(left_w, 0.0);
        pb_left.line_to(left_w, pill_height);
        pb_left.line_to(radius, pill_height);
        pb_left.quad_to(0.0, pill_height, 0.0, pill_height - radius);
        pb_left.line_to(0.0, radius);
        pb_left.quad_to(0.0, 0.0, radius, 0.0);
        pb_left.close();
        if let Some(p) = pb_left.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(31, 36, 45, 255);
            paint.anti_alias = true;
            pixmap.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }

        let mut pb_right = PathBuilder::new();
        pb_right.move_to(left_w, 0.0);
        pb_right.line_to(total_w - radius, 0.0);
        pb_right.quad_to(total_w, 0.0, total_w, radius);
        pb_right.line_to(total_w, pill_height - radius);
        pb_right.quad_to(total_w, pill_height, total_w - radius, pill_height);
        pb_right.line_to(left_w, pill_height);
        pb_right.close();
        if let Some(p) = pb_right.finish() {
            let mut paint = Paint::default();
            let [r, g, b, a] = pill.bg_color;
            paint.set_color_rgba8(r, g, b, a);
            paint.anti_alias = true;
            pixmap.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }
    } else {
        let mut pb = PathBuilder::new();
        pb.move_to(radius, 0.0);
        pb.line_to(total_w - radius, 0.0);
        pb.quad_to(total_w, 0.0, total_w, radius);
        pb.line_to(total_w, pill_height - radius);
        pb.quad_to(total_w, pill_height, total_w - radius, pill_height);
        pb.line_to(radius, pill_height);
        pb.quad_to(0.0, pill_height, 0.0, pill_height - radius);
        pb.line_to(0.0, radius);
        pb.quad_to(0.0, 0.0, radius, 0.0);
        pb.close();
        if let Some(p) = pb.finish() {
            let mut paint = Paint::default();
            let [r, g, b, a] = pill.bg_color;
            paint.set_color_rgba8(r, g, b, a);
            paint.anti_alias = true;
            pixmap.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    // Outline stroke
    let mut pb_stroke = PathBuilder::new();
    pb_stroke.move_to(radius, 0.5);
    pb_stroke.line_to(total_w - radius, 0.5);
    pb_stroke.quad_to(total_w - 0.5, 0.5, total_w - 0.5, radius);
    pb_stroke.line_to(total_w - 0.5, pill_height - radius);
    pb_stroke.quad_to(total_w - 0.5, pill_height - 0.5, total_w - radius, pill_height - 0.5);
    pb_stroke.line_to(radius, pill_height - 0.5);
    pb_stroke.quad_to(0.5, pill_height - 0.5, 0.5, pill_height - radius);
    pb_stroke.line_to(0.5, radius);
    pb_stroke.quad_to(0.5, 0.5, radius, 0.5);
    pb_stroke.close();
    if let Some(p) = pb_stroke.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(255, 255, 255, 28);
        paint.anti_alias = true;
        let stroke = Stroke { width: 1.0, ..Default::default() };
        pixmap.stroke_path(&p, &paint, &stroke, Transform::identity(), None);
    }

    if has_label {
        draw_text_into_pixmap(
            &mut pixmap,
            &pill.label,
            font_system,
            swash,
            pad_x,
            4.0,
            CosmicColor::rgb(220, 225, 235),
            font_size,
            Weight::SEMIBOLD,
        );
    }

    let val_x = if has_label { left_w + pad_x } else { pad_x };
    draw_text_into_pixmap(
        &mut pixmap,
        &pill.value,
        font_system,
        swash,
        val_x,
        4.0,
        CosmicColor::rgb(255, 255, 255),
        font_size,
        Weight::BOLD,
    );

    Some(pixmap)
}

fn draw_text_into_pixmap(
    pixmap: &mut Pixmap,
    text: &str,
    font_system: &mut FontSystem,
    swash: &mut SwashCache,
    x: f32,
    y: f32,
    color: CosmicColor,
    size: f32,
    weight: Weight,
) {
    let mut buf = cosmic_text::Buffer::new(font_system, Metrics::new(size, size * 1.2));
    buf.set_text(
        font_system,
        text,
        Attrs::new().family(Family::Name("Segoe UI")).weight(weight).color(color),
        Shaping::Advanced,
    );
    buf.shape_until_scroll(font_system, false);

    for run in buf.layout_runs() {
        for glyph in run.glyphs {
            let physical_glyph = glyph.physical((x, y + run.line_y), 1.0);
            swash.with_pixels(
                font_system,
                physical_glyph.cache_key,
                color,
                |gx, gy, col| {
                    let px = physical_glyph.x + gx;
                    let py = physical_glyph.y + gy;
                    if px >= 0 && px < pixmap.width() as i32 && py >= 0 && py < pixmap.height() as i32 {
                        let idx = (py as usize * pixmap.width() as usize + px as usize) * 4;
                        let dst = pixmap.data_mut();
                        let a = col.a() as u32;
                        if a > 0 {
                            let dr = dst[idx] as u32;
                            let dg = dst[idx + 1] as u32;
                            let db = dst[idx + 2] as u32;
                            let da = dst[idx + 3] as u32;

                            let r = (col.r() as u32 * a + dr * (255 - a)) / 255;
                            let g = (col.g() as u32 * a + dg * (255 - a)) / 255;
                            let b = (col.b() as u32 * a + db * (255 - a)) / 255;
                            let na = a + da * (255 - a) / 255;

                            dst[idx] = r as u8;
                            dst[idx + 1] = g as u8;
                            dst[idx + 2] = b as u8;
                            dst[idx + 3] = na as u8;
                        }
                    }
                },
            );
        }
    }
}

// =============================================================================
// UNIT TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_pixmap(w: u32, h: u32, color: [u8; 4]) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        p.data_mut().chunks_exact_mut(4).for_each(|px| {
            px.copy_from_slice(&color);
        });
        p
    }

    #[test]
    fn pixmap_cache_renders_once() {
        let mut cache = PixmapCache::new();
        let key = PixmapCache::hash_key("x^2 + y^2 = r^2");
        let mut render_calls = 0;

        let (w1, h1) = cache.get_or_render(key, 400.0, || {
            render_calls += 1;
            Some(solid_pixmap(100, 40, [255, 0, 0, 255]))
        });
        assert_eq!((w1, h1), (100.0, 40.0));
        assert_eq!(render_calls, 1);

        // Second call with same/larger max_width must NOT re-render.
        let (w2, h2) = cache.get_or_render(key, 400.0, || {
            render_calls += 1;
            Some(solid_pixmap(100, 40, [255, 0, 0, 255]))
        });
        assert_eq!((w2, h2), (100.0, 40.0));
        assert_eq!(render_calls, 1, "must not render twice for same content/width");

        assert!(cache.get(key).is_some());
    }

    #[test]
    fn pixmap_cache_rerenders_on_smaller_width() {
        let mut cache = PixmapCache::new();
        let key = PixmapCache::hash_key("some-diagram");
        let mut render_calls = 0;

        cache.get_or_render(key, 400.0, || {
            render_calls += 1;
            Some(solid_pixmap(300, 100, [0, 255, 0, 255]))
        });
        // Request a much smaller max width -> must re-render/downscale.
        let (w, _h) = cache.get_or_render(key, 50.0, || {
            render_calls += 1;
            Some(solid_pixmap(300, 100, [0, 255, 0, 255]))
        });
        assert!(w <= 50.0 + 0.5);
        assert_eq!(render_calls, 2);
    }

    #[test]
    fn clamp_pixmap_width_downscales_preserving_aspect() {
        let src = solid_pixmap(200, 100, [10, 20, 30, 255]);
        let clamped = clamp_pixmap_width(src, 100.0);
        assert_eq!(clamped.width(), 100);
        assert_eq!(clamped.height(), 50);
    }

    #[test]
    fn clamp_pixmap_width_never_upscales() {
        let src = solid_pixmap(50, 20, [1, 2, 3, 255]);
        let clamped = clamp_pixmap_width(src, 500.0);
        assert_eq!(clamped.width(), 50);
        assert_eq!(clamped.height(), 20);
    }

    #[test]
    fn blit_converts_opaque_rgba_to_bgra_word() {
        let mut p = Pixmap::new(2, 1).unwrap();
        // Opaque red, opaque blue
        p.data_mut()[0..4].copy_from_slice(&[255, 0, 0, 255]);
        p.data_mut()[4..8].copy_from_slice(&[0, 0, 255, 255]);

        let mut buf = vec![0u32; 2];
        blit_pixmap_to_softbuffer(&p, &mut buf, 2, 1);

        assert_eq!(buf[0], 0x00FF0000); // red
        assert_eq!(buf[1], 0x000000FF); // blue
    }

    #[test]
    fn blit_clips_when_dest_smaller() {
        let p = solid_pixmap(10, 10, [255, 255, 255, 255]);
        let mut buf = vec![0u32; 5 * 5];
        // Must not panic even though src is larger than dest.
        blit_pixmap_to_softbuffer(&p, &mut buf, 5, 5);
        assert_eq!(buf[0], 0x00FFFFFF);
    }

    #[test]
    fn decode_png_dimensions() {
        // Minimal valid PNG header (8 sig + IHDR chunk header/len/type + 8 bytes WxH)
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0, 0, 0, 13]); // length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&64u32.to_be_bytes()); // width
        bytes.extend_from_slice(&32u32.to_be_bytes()); // height
        bytes.extend_from_slice(&[0u8; 5]); // padding to satisfy len check
        assert_eq!(decode_image_dimensions(&bytes), Some((64, 32)));
    }

    #[test]
    fn decode_gif_dimensions() {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&100u16.to_le_bytes());
        bytes.extend_from_slice(&50u16.to_le_bytes());
        assert_eq!(decode_image_dimensions(&bytes), Some((100, 50)));
    }

    #[test]
    fn dark_theme_has_correct_palette() {
        let theme = DiagramTheme::dark();
        assert_eq!(theme.background, None);
        assert_eq!(theme.node_fill, [26, 29, 38, 255]);
        assert_eq!(theme.node_stroke, [99, 102, 241, 255]);
        assert_eq!(theme.node_text_color, [226, 232, 240, 255]);
        assert_eq!(theme.edge_stroke, [100, 116, 139, 255]);
        assert_eq!(theme.font_size, 14.0);
        assert_eq!(theme.scale, 2.0);
    }

    #[test]
    fn wikilink_preprocessor_replaces_aliases_and_direct_links() {
        let raw = "See [[docs/README|Docs]] and [[Session_Logs]] plus `[[Literal_Code]]`.";
        let res = preprocess_markdown(raw);
        assert_eq!(
            res,
            "See [Docs](docs/README) and [Session_Logs](Session_Logs) plus `[[Literal_Code]]`."
        );
    }

    #[test]
    fn resolve_safe_local_path_blocks_unc_encoded_and_remote_file_hosts() {
        let base = Path::new(r"C:\Example\docs");
        assert!(resolve_safe_local_path(base, r"\\attacker\share\a.md").is_err());
        assert!(resolve_safe_local_path(base, "//attacker/share/a.md").is_err());
        assert!(resolve_safe_local_path(base, "%5C%5Cattacker%5Cshare%5Ca.md").is_err());
        assert!(resolve_safe_local_path(base, "file:///%5C%5Cattacker.com/share/a.md").is_err());
        assert!(resolve_safe_local_path(base, "file://%5C%5C.%5Cpipe%5Cevil").is_err());
        assert!(resolve_safe_local_path(base, "FILE://attacker.com/share/a.md").is_err());
        assert!(resolve_safe_local_path(base, "file://192.168.1.50/share/a.pdf").is_err());
        assert!(resolve_safe_local_path(base, r"\\.\pipe\test").is_err());
        assert!(resolve_safe_local_path(base, "bad\0path.md").is_err());

        let (local_abs, is_abs) =
            resolve_safe_local_path(base, "file:///C:/My%20Docs/guide.md").expect("local file://");
        assert!(is_abs);
        assert_eq!(local_abs, PathBuf::from(r"C:/My Docs/guide.md"));

        let (local_rel, is_abs_rel) =
            resolve_safe_local_path(base, "notes/intro%201.md").expect("relative path");
        assert!(!is_abs_rel);
        assert_eq!(local_rel, base.join(r"notes/intro 1.md"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_system_binary_never_uses_relative_cwd_name() {
        let rundll = windows_system_binary(r"System32\rundll32.exe");
        assert!(rundll.is_absolute());
        assert!(rundll.ends_with(r"System32\rundll32.exe"));
        let explorer = windows_system_binary("explorer.exe");
        assert!(explorer.is_absolute());
        assert!(explorer.ends_with("explorer.exe"));
    }
}

/// Extracts attribute value from an HTML tag string (supports double or single quotes).
fn extract_tag_attribute(tag: &str, attr: &str) -> Option<String> {
    let lower_tag = tag.to_ascii_lowercase();
    let lower_attr = format!("{}=\"", attr.to_ascii_lowercase());
    let lower_attr_single = format!("{}='", attr.to_ascii_lowercase());

    if let Some(pos) = lower_tag.find(&lower_attr) {
        let start = pos + lower_attr.len();
        let end = tag[start..].find('"')? + start;
        return Some(tag[start..end].to_string());
    }
    if let Some(pos) = lower_tag.find(&lower_attr_single) {
        let start = pos + lower_attr_single.len();
        let end = tag[start..].find('\'')? + start;
        return Some(tag[start..end].to_string());
    }
    None
}

/// Formats a single HTML table cell (`<td>...</td>`) into rich Markdown inline content.
fn format_html_cell_to_markdown(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 64);
    let mut cur = raw;

    while !cur.is_empty() {
        if let Some(tag_start) = cur.find('<') {
            if tag_start > 0 {
                let text_segment = &cur[..tag_start];
                out.push_str(&text_segment.replace('|', "\\|"));
            }

            let rem = &cur[tag_start..];
            let tag_end = match rem.find('>') {
                Some(e) => e + 1,
                None => {
                    out.push_str(&rem.replace('|', "\\|"));
                    break;
                }
            };
            let tag = &rem[..tag_end];
            let tag_lower = tag.to_ascii_lowercase();

            if tag_lower.starts_with("<a ") {
                let href = extract_tag_attribute(tag, "href").unwrap_or_default();
                let after_a = &rem[tag_end..];
                let close_a = after_a.to_ascii_lowercase().find("</a>").unwrap_or(after_a.len());
                let inside_a = &after_a[..close_a];
                let inside_lower = inside_a.to_ascii_lowercase();

                if let Some(img_pos) = inside_lower.find("<img") {
                    let img_rem = &inside_a[img_pos..];
                    let img_end = img_rem.find('>').map(|e| e + 1).unwrap_or(img_rem.len());
                    let img_tag = &img_rem[..img_end];
                    let alt = extract_tag_attribute(img_tag, "alt")
                        .unwrap_or_else(|| "Video Preview".to_string());
                    out.push_str(&format!("[▶ {}]({}) ", alt.trim(), href.trim()));
                    let next_start = (close_a + 4).min(after_a.len());
                    cur = &after_a[next_start..];
                    continue;
                } else {
                    let link_text = format_html_cell_to_markdown(inside_a);
                    let label = if link_text.trim().is_empty() { "Link" } else { link_text.trim() };
                    out.push_str(&format!("[{}]({})", label, href.trim()));
                    let next_start = (close_a + 4).min(after_a.len());
                    cur = &after_a[next_start..];
                    continue;
                }
            } else if tag_lower.starts_with("<img") {
                let alt = extract_tag_attribute(tag, "alt").unwrap_or_else(|| "Image".to_string());
                let src = extract_tag_attribute(tag, "src").unwrap_or_default();
                out.push_str(&format!("[🖼 {}]({}) ", alt.trim(), src.trim()));
            } else if tag_lower.starts_with("<b") || tag_lower.starts_with("<strong") {
                out.push_str("**");
            } else if tag_lower == "</b>" || tag_lower == "</strong>" {
                out.push_str("**");
            } else if tag_lower.starts_with("<i") || tag_lower.starts_with("<em") || tag_lower.starts_with("<sub") {
                out.push('*');
            } else if tag_lower == "</i>" || tag_lower == "</em>" || tag_lower == "</sub>" {
                out.push('*');
            } else if tag_lower.starts_with("<br") {
                out.push_str(" <br> ");
            }

            cur = &rem[tag_end..];
        } else {
            out.push_str(&cur.replace('|', "\\|"));
            break;
        }
    }

    let cleaned = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    cleaned.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Converts a single HTML `<table>...</table>` block into a standard Markdown Table.
pub fn convert_html_table_to_markdown(table_html: &str) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut max_cols = 0usize;

    let lower = table_html.to_ascii_lowercase();
    let mut tr_start_idx = 0;

    while let Some(tr_open_rel) = lower[tr_start_idx..].find("<tr") {
        let tr_open = tr_start_idx + tr_open_rel;
        let tr_open_end = match lower[tr_open..].find('>') {
            Some(g) => tr_open + g + 1,
            None => break,
        };
        let tr_close = match lower[tr_open_end..].find("</tr>") {
            Some(c) => tr_open_end + c,
            None => table_html.len(),
        };

        let row_content = &table_html[tr_open_end..tr_close];
        let lower_row = row_content.to_ascii_lowercase();

        let mut cell_start_idx = 0;
        let mut row_cells = Vec::new();

        while cell_start_idx < row_content.len() {
            let next_td = lower_row[cell_start_idx..].find("<td");
            let next_th = lower_row[cell_start_idx..].find("<th");

            let (open_tag_start, is_td) = match (next_td, next_th) {
                (Some(td), Some(th)) => {
                    if td <= th { (cell_start_idx + td, true) } else { (cell_start_idx + th, false) }
                }
                (Some(td), None) => (cell_start_idx + td, true),
                (None, Some(th)) => (cell_start_idx + th, false),
                (None, None) => break,
            };

            let open_tag_end = match lower_row[open_tag_start..].find('>') {
                Some(g) => open_tag_start + g + 1,
                None => break,
            };

            let close_str = if is_td { "</td>" } else { "</th>" };
            let close_tag_start = match lower_row[open_tag_end..].find(close_str) {
                Some(c) => open_tag_end + c,
                None => row_content.len(),
            };

            let cell_raw = &row_content[open_tag_end..close_tag_start];
            let cell_md = format_html_cell_to_markdown(cell_raw);
            row_cells.push(cell_md);

            cell_start_idx = if close_tag_start < row_content.len() {
                close_tag_start + close_str.len()
            } else {
                row_content.len()
            };
        }

        if !row_cells.is_empty() {
            max_cols = max_cols.max(row_cells.len());
            rows.push(row_cells);
        }

        tr_start_idx = if tr_close < table_html.len() {
            tr_close + 5
        } else {
            table_html.len()
        };
    }

    if max_cols == 0 {
        return table_html.to_string();
    }

    let mut out = String::new();
    out.push_str("\n\n| ");
    for i in 0..max_cols {
        out.push_str(&format!("Video {} | ", i + 1));
    }
    out.push_str("\n| ");
    for _ in 0..max_cols {
        out.push_str(":--- | ");
    }
    out.push('\n');

    for mut r in rows {
        while r.len() < max_cols {
            r.push(String::new());
        }
        out.push_str("| ");
        for cell in r {
            out.push_str(&cell);
            out.push_str(" | ");
        }
        out.push('\n');
    }
    out.push('\n');
    out
}

/// Converts all HTML `<table>...</table>` sections in markdown into GFM tables outside code blocks.
pub fn convert_all_html_tables(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 256);
    let mut in_code_fence = false;
    let mut cur = raw;

    while !cur.is_empty() {
        if let Some(next_fence) = cur.find("```").or_else(|| cur.find("~~~")) {
            let next_table = cur.to_ascii_lowercase().find("<table");
            match (next_table, in_code_fence) {
                (Some(t_idx), false) if t_idx < next_fence => {
                    out.push_str(&cur[..t_idx]);
                    let after_t = &cur[t_idx..];
                    if let Some(close_t) = after_t.to_ascii_lowercase().find("</table>") {
                        let full_table = &after_t[..close_t + 8];
                        out.push_str(&convert_html_table_to_markdown(full_table));
                        cur = &after_t[close_t + 8..];
                    } else {
                        out.push_str(&cur[t_idx..]);
                        break;
                    }
                }
                _ => {
                    let fence_end = next_fence + 3;
                    out.push_str(&cur[..fence_end]);
                    in_code_fence = !in_code_fence;
                    cur = &cur[fence_end..];
                }
            }
        } else if !in_code_fence {
            if let Some(t_idx) = cur.to_ascii_lowercase().find("<table") {
                out.push_str(&cur[..t_idx]);
                let after_t = &cur[t_idx..];
                if let Some(close_t) = after_t.to_ascii_lowercase().find("</table>") {
                    let full_table = &after_t[..close_t + 8];
                    out.push_str(&convert_html_table_to_markdown(full_table));
                    cur = &after_t[close_t + 8..];
                } else {
                    out.push_str(&cur[t_idx..]);
                    break;
                }
            } else {
                out.push_str(cur);
                break;
            }
        } else {
            out.push_str(cur);
            break;
        }
    }

    out
}

/// Obsidian Wikilink and entity preprocessor.
/// Converts `[[target|display]]` -> `[display](target)`
/// and `[[target]]` -> `[target](target)`, while preserving fenced and inline code spans.
pub fn preprocess_markdown(raw: &str) -> String {
    let converted_tables = if raw.to_ascii_lowercase().contains("<table") {
        convert_all_html_tables(raw)
    } else {
        raw.to_string()
    };

    let mut out = String::with_capacity(converted_tables.len() + 256);
    let mut in_code_fence = false;

    for line in converted_tables.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_fence = !in_code_fence;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_code_fence {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let mut i = 0;
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        while i < len {
            // Preserve inline backtick code spans verbatim (e.g. `[[docs/session_recap]]`)
            if chars[i] == '`' {
                let mut tick_len = 0;
                while i + tick_len < len && chars[i + tick_len] == '`' {
                    tick_len += 1;
                }
                for _ in 0..tick_len {
                    out.push('`');
                }
                i += tick_len;
                let mut closed = false;
                while i < len {
                    if chars[i] == '`' {
                        let mut end_ticks = 0;
                        while i + end_ticks < len && chars[i + end_ticks] == '`' {
                            end_ticks += 1;
                        }
                        for _ in 0..end_ticks {
                            out.push('`');
                        }
                        i += end_ticks;
                        if end_ticks == tick_len {
                            closed = true;
                            break;
                        }
                    } else {
                        out.push(chars[i]);
                        i += 1;
                    }
                }
                let _ = closed;
                continue;
            }

            if i + 1 < len && chars[i] == '[' && chars[i + 1] == '[' {
                if let Some(close_offset) = chars[i + 2..]
                    .windows(2)
                    .position(|w| w[0] == ']' && w[1] == ']')
                {
                    let inside: String = chars[i + 2..i + 2 + close_offset].iter().collect();
                    if !inside.trim().is_empty() {
                        if let Some(pipe_pos) = inside.find('|') {
                            let target = inside[..pipe_pos].trim();
                            let display = inside[pipe_pos + 1..].trim();
                            out.push('[');
                            out.push_str(display);
                            out.push_str("](");
                            out.push_str(target);
                            out.push(')');
                        } else {
                            let target = inside.trim();
                            out.push('[');
                            out.push_str(target);
                            out.push_str("](");
                            out.push_str(target);
                            out.push(')');
                        }
                        i += 2 + close_offset + 2;
                        continue;
                    }
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        out.push('\n');
    }

    if !raw.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}
