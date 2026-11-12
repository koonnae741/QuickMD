//! painter.rs — QuickMD 3.0 (Native Rust Edition)
//! 100% modeled after QuickMD 2.0 (Linear/Notion/Obsidian design).
//! Zero WebView2, Zero Chromium, Zero HTML/CSS/JS.

use cosmic_text::{
    Attrs, Buffer, Color as CosmicColor, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
    SwashContent, Weight, Wrap,
};
use tiny_skia::{
    Color, FillRule, Paint, PathBuilder, Pixmap, PixmapMut, PremultipliedColorU8, Rect, Stroke,
    StrokeDash, Transform,
};

use doc_layout::ast::{Alignment, BadgeItem, CalloutKind, InlineSpan, MarkdownBlock, TableData};
use doc_layout::shaper::thai;
use doc_layout::table::TableLayout;
use doc_layout::{DocumentLayout, DocumentLayoutEngine, VisibleBlock};
use crate::bridge::RealEmbedMeasurer;
use crate::windows_assoc::DefaultAssociationState;

// =============================================================================
// CONSTANTS & LAYOUT METRICS (Identical to QuickMD 2.0)
// =============================================================================

pub const HEADER_HEIGHT: f32 = 52.0;
pub const STATUS_BAR_HEIGHT: f32 = 28.0;
pub const WIDE_CONTENT_WIDTH: f32 = 1200.0;
pub const OUTLINE_WIDTH: f32 = 260.0;
pub const SCROLLBAR_WIDTH: f32 = 8.0;
pub const CONTENT_TOP_PADDING: f32 = 28.0;

// Thai tone-mark clearance guard, per spec: ceil(size * 1.45 + 8px).
const THAI_LINE_CLEARANCE_MULT: f32 = 1.45;
const THAI_LINE_CLEARANCE_PX: f32 = 8.0;

const BODY_FAMILIES: &str = "Segoe UI";
const MONO_FAMILIES: &str = "Cascadia Code";

const META_NORMAL: usize = 0;
const META_INLINE_CODE: usize = 1;
const META_LINK: usize = 2;
const META_SEARCH_MATCH: usize = 3;
const META_SEARCH_ACTIVE: usize = 4;

// =============================================================================
// ANALYTICAL 2ND-ORDER SPRING PHYSICS & COLOR BLENDING
// =============================================================================

/// 2nd-order damped harmonic oscillator (`m = 1.0`) with fixed `1/240s` substepping
/// so stiff UI springs (`k = 320, d = 28`) remain unconditionally stable at any FPS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub pos: f32,
    pub vel: f32,
}

impl Default for Spring {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl Spring {
    pub const fn new(pos: f32) -> Self {
        Self { pos, vel: 0.0 }
    }

    pub fn snap(&mut self, pos: f32) {
        self.pos = pos;
        self.vel = 0.0;
    }

    pub fn step(&mut self, target: f32, stiffness: f32, damping: f32, dt: f32) -> bool {
        if !dt.is_finite() || dt <= 0.0 {
            return (self.pos - target).abs() > 0.001 || self.vel.abs() > 0.005;
        }
        let dt_clamped = dt.clamp(0.0, 0.08);
        let substeps = ((dt_clamped / (1.0 / 240.0)).ceil() as usize).clamp(1, 16);
        let h = dt_clamped / substeps as f32;
        for _ in 0..substeps {
            let force = (target - self.pos) * stiffness - self.vel * damping;
            self.vel += force * h;
            self.pos += self.vel * h;
        }
        if (self.pos - target).abs() <= 0.001 && self.vel.abs() <= 0.005 {
            self.pos = target;
            self.vel = 0.0;
            false
        } else {
            true
        }
    }
}

#[inline]
pub fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let r = (a.red() + (b.red() - a.red()) * t).clamp(0.0, 1.0);
    let g = (a.green() + (b.green() - a.green()) * t).clamp(0.0, 1.0);
    let bl = (a.blue() + (b.blue() - a.blue()) * t).clamp(0.0, 1.0);
    let al = (a.alpha() + (b.alpha() - a.alpha()) * t).clamp(0.0, 1.0);
    Color::from_rgba(r, g, bl, al).unwrap_or(b)
}

#[inline]
pub fn with_alpha(c: Color, alpha_mult: f32) -> Color {
    let al = (c.alpha() * alpha_mult.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    Color::from_rgba(c.red(), c.green(), c.blue(), al).unwrap_or(c)
}

// =============================================================================
// READING WIDTH PRESET (QuickMD 2.0 Quiet Luxury: Standard 880px <-> Wide 1120px)
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReadingWidthMode {
    #[default]
    Normal,  // Standard 880px
    Wide,    // Wide 1120px
}

impl ReadingWidthMode {
    pub fn next(self) -> Self {
        match self {
            Self::Normal => Self::Wide,
            Self::Wide => Self::Normal,
        }
    }

    pub fn toggle(self) -> Self {
        self.next()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Standard (880px)",
            Self::Wide => "Wide (1120px)",
        }
    }

    pub fn target_px(self, _avail_w: f32) -> f32 {
        match self {
            Self::Normal => 880.0,
            Self::Wide => 1120.0,
        }
    }
}

// =============================================================================
// HOVER TARGET & MICRO-INTERACTION STATE
// =============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct LinkHitBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiHitBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub target: HoverTarget,
}

impl UiHitBox {
    #[inline]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HoverTarget {
    #[default]
    None,
    BrandLogo,
    ToolbarBtn(usize), // 0..12
    WelcomeLogo,
    WelcomeDropzone,
    WelcomeChooseBtn,
    WelcomeShortcut(usize), // 0..6
    OutlineItem(usize),     // AST block_index of the heading
    CodeCopyBtn(usize),
    TableCopyBtn(usize),
    HeadingAnchor(usize),
    SearchInput,
    SearchPrev,
    SearchNext,
    SearchClose,
    ModalClose,
    Scrollbar,
    DocLink(String),
    NavBack,
    NavForward,
    MermaidCard(String),
    DiagramCanvas,
    DiagramZoomIn,
    DiagramZoomOut,
    DiagramZoomReset,
    DiagramFit,
    DiagramClose,
    DiagramLightboxBackdrop,
    SetDefaultMdBtn,
}

impl HoverTarget {
    pub fn is_interactive(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn uses_hand_cursor(&self) -> bool {
        !matches!(
            self,
            Self::None
                | Self::Scrollbar
                | Self::SearchInput
                | Self::DiagramCanvas
                | Self::DiagramLightboxBackdrop
        )
    }

    pub fn tooltip(&self) -> Option<std::borrow::Cow<'static, str>> {
        match self {
            Self::BrandLogo => Some("Return to Welcome Screen".into()),
            Self::ToolbarBtn(0) => Some("Open Markdown File (Ctrl+O)".into()),
            Self::ToolbarBtn(1) => Some("Toggle Document Outline (Ctrl+T)".into()),
            Self::ToolbarBtn(2) => Some("Search in Document (Ctrl+F)".into()),
            Self::NavBack => Some("Navigate Back (Alt+Left)".into()),
            Self::NavForward => Some("Navigate Forward (Alt+Right)".into()),
            Self::ToolbarBtn(3) => Some("About QuickMD (Ctrl+I)".into()),
            Self::ToolbarBtn(4) => Some("Document Statistics & Metrics".into()),
            Self::ToolbarBtn(5) => Some("Toggle Wide Mode (880px / 1120px)".into()),
            Self::ToolbarBtn(6) => Some("Switch Color Theme (Ctrl+Shift+T)".into()),
            Self::ToolbarBtn(7) => Some("Zoom Out (Ctrl+-)".into()),
            Self::ToolbarBtn(8) => Some("Reset Zoom 100% (Ctrl+0)".into()),
            Self::ToolbarBtn(9) => Some("Zoom In (Ctrl++)".into()),
            Self::ToolbarBtn(10) => Some("Copy Raw Markdown".into()),
            Self::ToolbarBtn(11) => Some("Export PNG Snapshot (Ctrl+P)".into()),
            Self::CodeCopyBtn(_) => Some("Copy Code Block".into()),
            Self::TableCopyBtn(_) => Some("Copy Table as TSV".into()),
            Self::HeadingAnchor(_) => Some("Scroll & Copy Section Anchor".into()),
            Self::SearchInput => Some("Find in document... (Ctrl+V to paste)".into()),
            Self::SearchPrev => Some("Previous Match (Shift+Enter)".into()),
            Self::SearchNext => Some("Next Match (Enter)".into()),
            Self::SearchClose => Some("Close Search (Esc)".into()),
            Self::DocLink(url) => Some(format!("Link: {}", url).into()),
            Self::MermaidCard(_) => Some("Click to Inspect, Pan & Zoom Diagram".into()),
            Self::DiagramZoomIn => Some("Zoom In (+)".into()),
            Self::DiagramZoomOut => Some("Zoom Out (-)".into()),
            Self::DiagramZoomReset => Some("Reset Zoom 100% (0)".into()),
            Self::DiagramFit => Some("Fit Diagram to Window (F)".into()),
            Self::DiagramClose => Some("Close Viewer (Esc)".into()),
            Self::SetDefaultMdBtn => Some("Register QuickMD for .md / .markdown files".into()),
            _ => None,
        }
    }
}

// =============================================================================
// THEME ENGINE — 5 Built-in Themes (Dark Noir, Tokyo Night, OLED, Sepia, Light)
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    DarkNoir,
    TokyoNight,
    ObsidianOled,
    SolarizedSepia,
    GithubLight,
}

impl ThemeMode {
    pub fn next(self) -> Self {
        match self {
            Self::DarkNoir => Self::TokyoNight,
            Self::TokyoNight => Self::ObsidianOled,
            Self::ObsidianOled => Self::SolarizedSepia,
            Self::SolarizedSepia => Self::GithubLight,
            Self::GithubLight => Self::DarkNoir,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::DarkNoir => "Dark Noir",
            Self::TokyoNight => "Tokyo Night",
            Self::ObsidianOled => "Obsidian OLED",
            Self::SolarizedSepia => "Editorial Sepia",
            Self::GithubLight => "Clean Light",
        }
    }

    pub fn is_light(self) -> bool {
        matches!(self, Self::SolarizedSepia | Self::GithubLight)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThemePalette {
    pub bg_primary: Color,
    pub bg_secondary: Color,
    pub bg_tertiary: Color,
    pub border_default: Color,
    pub border_subtle: Color,
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_heading: Color,
    pub accent: Color,
    pub accent_cyan: Color,
    pub code_red: Color,
    pub scroll_thumb: Color,
    pub table_zebra: Color,
}

impl ThemePalette {
    pub fn lerp(from: &Self, to: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            bg_primary: lerp_color(from.bg_primary, to.bg_primary, t),
            bg_secondary: lerp_color(from.bg_secondary, to.bg_secondary, t),
            bg_tertiary: lerp_color(from.bg_tertiary, to.bg_tertiary, t),
            border_default: lerp_color(from.border_default, to.border_default, t),
            border_subtle: lerp_color(from.border_subtle, to.border_subtle, t),
            text_primary: lerp_color(from.text_primary, to.text_primary, t),
            text_secondary: lerp_color(from.text_secondary, to.text_secondary, t),
            text_muted: lerp_color(from.text_muted, to.text_muted, t),
            text_heading: lerp_color(from.text_heading, to.text_heading, t),
            accent: lerp_color(from.accent, to.accent, t),
            accent_cyan: lerp_color(from.accent_cyan, to.accent_cyan, t),
            code_red: lerp_color(from.code_red, to.code_red, t),
            scroll_thumb: lerp_color(from.scroll_thumb, to.scroll_thumb, t),
            table_zebra: lerp_color(from.table_zebra, to.table_zebra, t),
        }
    }

    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::DarkNoir => Self {
                bg_primary: Color::from_rgba8(13, 17, 23, 255),        // #0D1117
                bg_secondary: Color::from_rgba8(22, 27, 34, 255),      // #161B22
                bg_tertiary: Color::from_rgba8(33, 38, 45, 255),       // #21262D
                border_default: Color::from_rgba8(48, 54, 61, 255),    // #30363D
                border_subtle: Color::from_rgba8(33, 38, 45, 255),     // #21262D
                text_primary: Color::from_rgba8(230, 237, 243, 255),   // #E6EDF3
                text_secondary: Color::from_rgba8(139, 148, 158, 255), // #8B949E
                text_muted: Color::from_rgba8(110, 118, 129, 255),     // #6E7681
                text_heading: Color::from_rgba8(240, 246, 252, 255),   // #F0F6FC
                accent: Color::from_rgba8(56, 139, 253, 255),          // #388BFD
                accent_cyan: Color::from_rgba8(56, 189, 248, 255),     // #38BDF8
                code_red: Color::from_rgba8(255, 123, 114, 255),       // #FF7B72
                scroll_thumb: Color::from_rgba8(48, 54, 61, 255),      // #30363D
                table_zebra: Color::from_rgba8(18, 23, 31, 255),
            },
            ThemeMode::TokyoNight => Self {
                bg_primary: Color::from_rgba8(26, 27, 38, 255),        // #1A1B26
                bg_secondary: Color::from_rgba8(22, 22, 30, 255),      // #16161E
                bg_tertiary: Color::from_rgba8(36, 40, 59, 255),       // #24283B
                border_default: Color::from_rgba8(65, 72, 104, 255),   // #414868
                border_subtle: Color::from_rgba8(41, 46, 66, 255),     // #292E42
                text_primary: Color::from_rgba8(192, 202, 245, 255),   // #C0CAF5
                text_secondary: Color::from_rgba8(154, 165, 206, 255), // #9AA5CE
                text_muted: Color::from_rgba8(86, 95, 137, 255),       // #565F89
                text_heading: Color::from_rgba8(224, 228, 255, 255),
                accent: Color::from_rgba8(122, 162, 247, 255),         // #7AA2F7
                accent_cyan: Color::from_rgba8(125, 207, 255, 255),    // #7DCFFF
                code_red: Color::from_rgba8(247, 118, 142, 255),       // #F7768E
                scroll_thumb: Color::from_rgba8(65, 72, 104, 255),
                table_zebra: Color::from_rgba8(31, 35, 53, 255),
            },
            ThemeMode::ObsidianOled => Self {
                bg_primary: Color::from_rgba8(8, 9, 12, 255),          // #08090C (Wadhah non-pure black)
                bg_secondary: Color::from_rgba8(16, 18, 24, 255),      // #101218
                bg_tertiary: Color::from_rgba8(26, 29, 38, 255),       // #1A1D26
                border_default: Color::from_rgba8(42, 46, 58, 255),    // #2A2E3A
                border_subtle: Color::from_rgba8(28, 31, 40, 255),
                text_primary: Color::from_rgba8(232, 236, 242, 255),
                text_secondary: Color::from_rgba8(148, 156, 170, 255),
                text_muted: Color::from_rgba8(104, 112, 126, 255),
                text_heading: Color::from_rgba8(246, 248, 252, 255),
                accent: Color::from_rgba8(99, 102, 241, 255),          // #6366F1 Indigo
                accent_cyan: Color::from_rgba8(56, 189, 248, 255),
                code_red: Color::from_rgba8(251, 113, 133, 255),
                scroll_thumb: Color::from_rgba8(48, 52, 66, 255),
                table_zebra: Color::from_rgba8(13, 15, 20, 255),
            },
            ThemeMode::SolarizedSepia => Self {
                bg_primary: Color::from_rgba8(245, 239, 226, 255),     // #F5EFE2 Warm Sepia
                bg_secondary: Color::from_rgba8(236, 228, 212, 255),   // #ECE4D4
                bg_tertiary: Color::from_rgba8(225, 215, 196, 255),    // #E1D7C4
                border_default: Color::from_rgba8(204, 192, 170, 255), // #CCC0AA
                border_subtle: Color::from_rgba8(218, 208, 188, 255),
                text_primary: Color::from_rgba8(58, 50, 40, 255),      // #3A3228
                text_secondary: Color::from_rgba8(102, 90, 74, 255),   // #665A4A
                text_muted: Color::from_rgba8(138, 124, 106, 255),
                text_heading: Color::from_rgba8(36, 28, 20, 255),
                accent: Color::from_rgba8(37, 99, 235, 255),           // #2563EB
                accent_cyan: Color::from_rgba8(13, 148, 136, 255),
                code_red: Color::from_rgba8(185, 28, 28, 255),
                scroll_thumb: Color::from_rgba8(186, 174, 152, 255),
                table_zebra: Color::from_rgba8(240, 233, 218, 255),
            },
            ThemeMode::GithubLight => Self {
                bg_primary: Color::from_rgba8(248, 250, 252, 255),     // #F8FAFC (Wadhah non-pure white)
                bg_secondary: Color::from_rgba8(241, 245, 249, 255),   // #F1F5F9
                bg_tertiary: Color::from_rgba8(226, 232, 240, 255),    // #E2E8F0
                border_default: Color::from_rgba8(203, 213, 225, 255), // #CBD5E1
                border_subtle: Color::from_rgba8(226, 232, 240, 255),
                text_primary: Color::from_rgba8(30, 41, 59, 255),      // #1E293B
                text_secondary: Color::from_rgba8(71, 85, 105, 255),   // #475569
                text_muted: Color::from_rgba8(100, 116, 139, 255),     // #64748B
                text_heading: Color::from_rgba8(15, 23, 42, 255),      // #0F172A
                accent: Color::from_rgba8(37, 99, 235, 255),           // #2563EB
                accent_cyan: Color::from_rgba8(2, 132, 199, 255),
                code_red: Color::from_rgba8(220, 38, 38, 255),
                scroll_thumb: Color::from_rgba8(148, 163, 184, 255),
                table_zebra: Color::from_rgba8(241, 245, 249, 255),
            },
        }
    }
}

// =============================================================================
// VIEW OPTIONS & 144HZ MICRO-MOTION STATE
// =============================================================================

#[derive(Debug, Clone)]
pub struct ViewOptions {
    pub theme: ThemeMode,
    pub zoom_pct: u32,
    pub wide_mode: bool,
    pub width_mode: ReadingWidthMode,
    pub outline_open: bool,
    pub stats_open: bool,
    pub about_open: bool,
    pub search_open: bool,
    pub search_query: String,
    pub search_matches: usize,
    pub search_current_idx: usize,
    pub search_active_block: Option<(usize, usize)>,
    pub raw_char_count: usize,
    pub raw_word_count: usize,
    pub raw_line_count: usize,
    pub encoding_label: &'static str,
    pub toast_msg: Option<String>,
    pub hover: HoverTarget,
    pub cursor_pos: (f32, f32),
    pub copied_block_idx: Option<usize>,
    // 144Hz Smooth UI Animation Channels (0.0..=1.0)
    pub sidebar_anim: f32,
    pub search_anim: f32,
    pub modal_anim: f32,
    pub toast_anim: f32,
    pub progress_anim: f32,
    // Micro-Motion Spring Extensions
    pub anim_ticked: bool,
    pub anim_clock: f32,
    pub file_drag_hover: bool,
    pub scrollbar_dragging: bool,
    pub drag_anim: f32,
    pub scrollbar_hover: f32,
    pub theme_from_palette: Option<ThemePalette>,
    pub theme_anim: f32,
    pub last_modal_is_about: bool,
    pub modal_scale: Spring,
    pub toast_retained_msg: String,
    pub toast_spring: Spring,
    pub copy_spring: Spring,
    pub active_heading_ordinal: Option<usize>,
    pub outline_active_row: Spring,
    pub outline_hover_idx: Option<usize>,
    pub outline_hover_anim: f32,
    pub tooltip_anim: f32,
    pub retained_tooltip: String,
    pub toolbar_hover: [f32; 12],
    pub welcome_drop_hover: f32,
    pub welcome_btn_hover: f32,
    pub hovered_table_idx: Option<usize>,
    pub last_hovered_table_idx: Option<usize>,
    pub table_hover_anim: f32,
    pub can_nav_back: bool,
    pub can_nav_forward: bool,
    pub nav_back_hover: f32,
    pub nav_forward_hover: f32,
    pub reading_width_spring: Spring,
    pub text_selection: Option<TextSelection>,
    pub last_viewport_w: f32,
    pub cached_headings: Vec<HeadingItem>,
    pub outline_scroll_y: f32,
    pub diagram_lightbox: DiagramLightboxState,
    pub default_md_state: DefaultAssociationState,
    pub default_md_btn_hover: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagramLightboxState {
    pub is_open: bool,
    pub source: String,
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub is_dragging: bool,
    pub drag_start_x: f32,
    pub drag_start_y: f32,
    pub drag_start_pan_x: f32,
    pub drag_start_pan_y: f32,
    pub natural_w: f32,
    pub natural_h: f32,
}

impl Default for DiagramLightboxState {
    fn default() -> Self {
        Self {
            is_open: false,
            source: String::new(),
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            is_dragging: false,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            drag_start_pan_x: 0.0,
            drag_start_pan_y: 0.0,
            natural_w: 1200.0,
            natural_h: 600.0,
        }
    }
}

impl DiagramLightboxState {
    pub fn open(&mut self, source: String, natural_w: f32, natural_h: f32, vw: f32, vh: f32) {
        self.is_open = true;
        self.source = source;
        self.natural_w = natural_w.max(100.0);
        self.natural_h = natural_h.max(60.0);
        self.pan_x = 0.0;
        self.pan_y = 0.0;
        self.is_dragging = false;

        let avail_w = (vw - 80.0).max(200.0);
        let avail_h = (vh - 140.0).max(200.0);
        let fit_scale = (avail_w / self.natural_w).min(avail_h / self.natural_h);
        self.zoom = fit_scale.min(1.0).clamp(0.2, 5.0);
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.is_dragging = false;
    }

    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom * 1.25).min(5.0);
    }

    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom / 1.25).max(0.2);
    }

    pub fn reset_zoom(&mut self) {
        self.zoom = 1.0;
        self.pan_x = 0.0;
        self.pan_y = 0.0;
    }

    pub fn fit_to_window(&mut self, vw: f32, vh: f32) {
        let avail_w = (vw - 80.0).max(200.0);
        let avail_h = (vh - 140.0).max(200.0);
        let fit_scale = (avail_w / self.natural_w).min(avail_h / self.natural_h);
        self.zoom = fit_scale.clamp(0.2, 5.0);
        self.pan_x = 0.0;
        self.pan_y = 0.0;
    }

    pub fn zoom_at(&mut self, mx: f32, my: f32, dy: f32, vw: f32, vh: f32) {
        let factor = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
        let new_zoom = (self.zoom * factor).clamp(0.2, 5.0);
        let actual_factor = new_zoom / self.zoom;

        let mouse_rel_x = mx - (vw / 2.0 + self.pan_x);
        let mouse_rel_y = my - (vh / 2.0 + self.pan_y);

        self.pan_x -= mouse_rel_x * (actual_factor - 1.0);
        self.pan_y -= mouse_rel_y * (actual_factor - 1.0);
        self.zoom = new_zoom;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeadingItem {
    pub block_idx: usize,
    pub level: u8,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextSelection {
    pub is_active: bool,
    pub start_block: usize,
    pub start_char: usize,
    pub end_block: usize,
    pub end_char: usize,
    pub selected_text: String,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            theme: ThemeMode::DarkNoir,
            zoom_pct: 100,
            wide_mode: false,
            width_mode: ReadingWidthMode::Normal,
            outline_open: false,
            stats_open: false,
            about_open: false,
            search_open: false,
            search_query: String::new(),
            search_matches: 0,
            search_current_idx: 0,
            search_active_block: None,
            raw_char_count: 0,
            raw_word_count: 0,
            raw_line_count: 0,
            encoding_label: "UTF-8",
            toast_msg: None,
            hover: HoverTarget::None,
            cursor_pos: (-100.0, -100.0),
            copied_block_idx: None,
            sidebar_anim: 0.0,
            search_anim: 0.0,
            modal_anim: 0.0,
            toast_anim: 0.0,
            progress_anim: 0.0,
            anim_ticked: false,
            anim_clock: 0.0,
            file_drag_hover: false,
            scrollbar_dragging: false,
            drag_anim: 0.0,
            scrollbar_hover: 0.0,
            theme_from_palette: None,
            theme_anim: 1.0,
            last_modal_is_about: false,
            modal_scale: Spring::new(1.0),
            toast_retained_msg: String::new(),
            toast_spring: Spring::new(1.0),
            copy_spring: Spring::new(1.0),
            active_heading_ordinal: None,
            outline_active_row: Spring::new(-1.0),
            outline_hover_idx: None,
            outline_hover_anim: 0.0,
            tooltip_anim: 0.0,
            retained_tooltip: String::new(),
            toolbar_hover: [0.0; 12],
            welcome_drop_hover: 0.0,
            welcome_btn_hover: 0.0,
            hovered_table_idx: None,
            last_hovered_table_idx: None,
            table_hover_anim: 0.0,
            can_nav_back: false,
            can_nav_forward: false,
            nav_back_hover: 0.0,
            nav_forward_hover: 0.0,
            reading_width_spring: Spring::new(880.0),
            text_selection: None,
            last_viewport_w: 1200.0,
            cached_headings: Vec::new(),
            outline_scroll_y: 0.0,
            diagram_lightbox: DiagramLightboxState::default(),
            default_md_state: DefaultAssociationState::NotRegistered,
            default_md_btn_hover: 0.0,
        }
    }
}

impl ViewOptions {
    /// Returns the active `ThemePalette`, smoothly crossfaded during theme transitions.
    pub fn effective_palette(&self) -> ThemePalette {
        let target = ThemePalette::for_mode(self.theme);
        if let Some(ref from) = self.theme_from_palette {
            if self.theme_anim < 0.999 {
                return ThemePalette::lerp(from, &target, self.theme_anim);
            }
        }
        target
    }

    /// Initiates a smooth 220ms color crossfade to `next` from the exact current interpolated palette.
    pub fn switch_theme(&mut self, next: ThemeMode) {
        if self.theme == next && self.theme_anim >= 0.999 {
            return;
        }
        let current = self.effective_palette();
        self.theme = next;
        self.theme_from_palette = Some(current);
        self.theme_anim = 0.0;
    }

    /// Triggers an analytical spring pop-in when opening Stats or About modal.
    pub fn trigger_modal_open(&mut self, is_about: bool) {
        self.last_modal_is_about = is_about;
        if is_about {
            self.about_open = true;
            self.stats_open = false;
        } else {
            self.stats_open = true;
            self.about_open = false;
        }
        if self.modal_anim < 0.15 {
            self.modal_scale = Spring { pos: 0.91, vel: 1.4 };
        } else {
            self.modal_scale = Spring { pos: 0.96, vel: 1.1 };
        }
    }

    /// Triggers a bottom-center pill toast with damped spring bounce.
    pub fn trigger_toast(&mut self, msg: String) {
        self.toast_retained_msg = msg.clone();
        self.toast_msg = Some(msg);
        if self.toast_anim < 0.15 {
            self.toast_spring = Spring { pos: 0.82, vel: 2.2 };
        } else {
            self.toast_spring.vel = 1.4;
        }
    }

    /// Triggers checkmark bounce on CodeBlock / Table copy button.
    pub fn trigger_copy_feedback(&mut self, block_idx: usize) {
        self.copied_block_idx = Some(block_idx);
        self.copy_spring = Spring { pos: 0.84, vel: 2.6 };
    }

    pub fn rebuild_headings(&mut self, layout: &DocumentLayout) {
        self.cached_headings.clear();
        for (idx, block) in layout.blocks.iter().enumerate() {
            if let MarkdownBlock::Heading { level, text, .. } = block {
                self.cached_headings.push(HeadingItem {
                    block_idx: idx,
                    level: *level,
                    text: text.clone(),
                });
            }
        }
    }

    /// Updates ScrollSpy active heading ordinal for smooth spring indicator glide.
    /// Uses O(log H) binary search (partition_point) to ensure zero frame drop on 5,000+ heading documents.
    pub fn update_scroll_spy(&mut self, layout: &DocumentLayout, scroll_y: f32) {
        if self.cached_headings.is_empty() {
            self.active_heading_ordinal = None;
            return;
        }

        let target_y = scroll_y + 68.0;
        let idx = self.cached_headings.partition_point(|h| {
            let block_y = layout.geometries.get(h.block_idx).map(|g| g.y_offset).unwrap_or(0.0);
            block_y <= target_y
        });
        let active_ord = idx.saturating_sub(1);
        self.active_heading_ordinal = Some(active_ord);
        if let Some(ord) = self.active_heading_ordinal {
            if self.outline_active_row.pos < 0.0 {
                self.outline_active_row.snap(ord as f32);
            }
        }
    }

    #[inline]
    pub fn toolbar_hover_t(&self, idx: usize) -> f32 {
        if !self.anim_ticked {
            return if self.hover == HoverTarget::ToolbarBtn(idx) { 1.0 } else { 0.0 };
        }
        self.toolbar_hover.get(idx).copied().unwrap_or(0.0).clamp(0.0, 1.0)
    }

    #[inline]
    pub fn effective_sidebar_anim(&self) -> f32 {
        if !self.anim_ticked && self.outline_open {
            self.sidebar_anim.max(1.0)
        } else {
            self.sidebar_anim.clamp(0.0, 1.0)
        }
    }

    #[inline]
    pub fn effective_search_anim(&self) -> f32 {
        if !self.anim_ticked && self.search_open {
            self.search_anim.max(1.0)
        } else {
            self.search_anim.clamp(0.0, 1.0)
        }
    }

    /// Target reading column width for `DocumentLayoutEngine::set_viewport` (uses target `outline_open`
    /// so layout reflows at most once per toggle, avoiding per-frame full-document re-shaping).
    pub fn target_content_width(&self, vw: f32) -> f32 {
        let target_sidebar = if self.outline_open { OUTLINE_WIDTH } else { 0.0 };
        let avail_w = (vw - target_sidebar - SCROLLBAR_WIDTH).max(240.0);
        let max_col = if self.wide_mode && self.width_mode == ReadingWidthMode::Normal {
            WIDE_CONTENT_WIDTH
        } else {
            self.width_mode.target_px(avail_w)
        };
        (avail_w - 48.0).min(max_col).max(240.0)
    }

    /// Computes the exact reading column `(content_x, content_w)` for the current frame,
    /// gliding `content_x` at 144Hz with `sidebar_anim` without Frame-0 teleport.
    pub fn reading_column(&self, vw: f32) -> (f32, f32) {
        let is_mobile = vw < 640.0;
        let anim_sidebar = if is_mobile {
            0.0
        } else {
            OUTLINE_WIDTH * self.effective_sidebar_anim()
        };
        let target_w = self.target_content_width(vw);
        let content_w = if self.reading_width_spring.pos > 100.0 && self.anim_ticked {
            let max_allowed = (vw - anim_sidebar - SCROLLBAR_WIDTH - 32.0).max(200.0);
            self.reading_width_spring.pos.clamp(200.0, max_allowed)
        } else {
            target_w
        };
        let avail_w_anim = (vw - anim_sidebar - SCROLLBAR_WIDTH).max(240.0);
        let content_x = anim_sidebar + ((avail_w_anim - content_w) / 2.0).max(16.0);
        (content_x, content_w)
    }

    /// Advances all 144Hz exponential & 2nd-order spring channels.
    /// Returns `true` if any visual motion is still in flight.
    pub fn tick_animations(&mut self, dt: f32, target_progress: f32) -> bool {
        self.anim_ticked = true;
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.08) } else { 0.0 };
        self.anim_clock = (self.anim_clock + dt) % 3600.0;
        let mut active = false;

        let step = |val: &mut f32, target: f32, rate: f32, act: &mut bool| {
            let diff = target - *val;
            if diff.abs() > 0.002 {
                let alpha = 1.0 - (-rate * dt).exp();
                *val += diff * alpha;
                *act = true;
            } else if (*val - target).abs() > 0.0 {
                *val = target;
            }
        };

        if self.stats_open {
            self.last_modal_is_about = false;
        } else if self.about_open {
            self.last_modal_is_about = true;
        }

        if let Some(ref m) = self.toast_msg {
            if self.toast_retained_msg != *m {
                self.toast_retained_msg = m.clone();
            }
        }

        if let Some(tip) = self.hover.tooltip() {
            if self.retained_tooltip != tip.as_ref() {
                self.retained_tooltip = tip.into_owned();
            }
        }

        // Core structural panels & bars
        step(&mut self.sidebar_anim, if self.outline_open { 1.0 } else { 0.0 }, 19.0, &mut active);
        step(&mut self.search_anim, if self.search_open { 1.0 } else { 0.0 }, 22.0, &mut active);
        step(&mut self.modal_anim, if self.stats_open || self.about_open { 1.0 } else { 0.0 }, 22.0, &mut active);
        step(&mut self.toast_anim, if self.toast_msg.is_some() { 1.0 } else { 0.0 }, 20.0, &mut active);
        step(&mut self.progress_anim, target_progress.clamp(0.0, 1.0), 16.0, &mut active);
        step(&mut self.drag_anim, if self.file_drag_hover { 1.0 } else { 0.0 }, 20.0, &mut active);

        // Theme crossfade
        if self.theme_anim < 1.0 {
            step(&mut self.theme_anim, 1.0, 14.0, &mut active);
            if self.theme_anim >= 0.998 {
                self.theme_anim = 1.0;
                self.theme_from_palette = None;
            }
        }

        // Analytical 2nd-order Springs
        if self.modal_scale.step(1.0, 340.0, 26.0, dt) {
            active = true;
        }
        if self.toast_spring.step(1.0, 320.0, 24.0, dt) {
            active = true;
        }
        if self.copy_spring.step(1.0, 340.0, 22.0, dt) {
            active = true;
        }
        if let Some(ord) = self.active_heading_ordinal {
            if self.outline_active_row.pos < 0.0 {
                self.outline_active_row.snap(ord as f32);
            } else if self.outline_active_row.step(ord as f32, 300.0, 28.0, dt) {
                active = true;
            }
        }

        // Per-widget hover channels (~140ms smooth ease)
        for i in 0..12 {
            let tgt = if self.hover == HoverTarget::ToolbarBtn(i) { 1.0 } else { 0.0 };
            step(&mut self.toolbar_hover[i], tgt, 22.0, &mut active);
        }
        let drop_hov = matches!(self.hover, HoverTarget::WelcomeDropzone | HoverTarget::WelcomeChooseBtn) || self.file_drag_hover;
        step(&mut self.welcome_drop_hover, if drop_hov { 1.0 } else { 0.0 }, 18.0, &mut active);
        step(&mut self.welcome_btn_hover, if self.hover == HoverTarget::WelcomeChooseBtn { 1.0 } else { 0.0 }, 22.0, &mut active);
        step(&mut self.default_md_btn_hover, if self.hover == HoverTarget::SetDefaultMdBtn { 1.0 } else { 0.0 }, 22.0, &mut active);

        let sb_hov = self.hover == HoverTarget::Scrollbar || self.scrollbar_dragging;
        step(&mut self.scrollbar_hover, if sb_hov { 1.0 } else { 0.0 }, 20.0, &mut active);

        step(&mut self.nav_back_hover, if self.hover == HoverTarget::NavBack { 1.0 } else { 0.0 }, 22.0, &mut active);
        step(&mut self.nav_forward_hover, if self.hover == HoverTarget::NavForward { 1.0 } else { 0.0 }, 22.0, &mut active);

        let target_col_w = self.target_content_width(self.last_viewport_w);
        if self.reading_width_spring.pos <= 100.0 {
            self.reading_width_spring.snap(target_col_w);
        } else if self.reading_width_spring.step(target_col_w, 280.0, 26.0, dt) {
            active = true;
        }

        let has_tip = self.hover.tooltip().is_some();
        step(&mut self.tooltip_anim, if has_tip { 1.0 } else { 0.0 }, 20.0, &mut active);

        if let HoverTarget::OutlineItem(idx) = self.hover {
            self.outline_hover_idx = Some(idx);
            step(&mut self.outline_hover_anim, 1.0, 22.0, &mut active);
        } else {
            step(&mut self.outline_hover_anim, 0.0, 22.0, &mut active);
            if self.outline_hover_anim <= 0.002 {
                self.outline_hover_idx = None;
            }
        }

        if let Some(tidx) = self.hovered_table_idx {
            self.last_hovered_table_idx = Some(tidx);
            step(&mut self.table_hover_anim, 1.0, 20.0, &mut active);
        } else {
            step(&mut self.table_hover_anim, 0.0, 20.0, &mut active);
            if self.table_hover_anim <= 0.002 {
                self.last_hovered_table_idx = None;
            }
        }

        // Continuous subtle motion while marching ants are visible
        if self.file_drag_hover || drop_hov {
            active = true;
        }

        active
    }
}

// =============================================================================
// PNG PIXMAP LOADER (Zero-distortion 1:1 premultiplied alpha)
// =============================================================================

fn load_png_pixmap(bytes: &[u8]) -> Option<Pixmap> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for p in img.pixels() {
        let [r, g, b, a] = p.0;
        let af = a as f32 / 255.0;
        let pr = ((r as f32 * af) + 0.5).min(255.0) as u8;
        let pg = ((g as f32 * af) + 0.5).min(255.0) as u8;
        let pb = ((b as f32 * af) + 0.5).min(255.0) as u8;
        data.push(pr);
        data.push(pg);
        data.push(pb);
        data.push(a);
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}

#[inline]
fn draw_pixmap_at(pixmap: &mut PixmapMut, src: &Pixmap, x: f32, y: f32) {
    draw_pixmap_at_alpha(pixmap, src, x, y, 1.0);
}

#[inline]
fn draw_pixmap_at_alpha(pixmap: &mut PixmapMut, src: &Pixmap, x: f32, y: f32, opacity: f32) {
    let op = opacity.clamp(0.0, 1.0);
    if op <= 0.002 {
        return;
    }
    let mut paint = tiny_skia::PixmapPaint::default();
    paint.opacity = op;
    let ts = Transform::from_translate(x.round(), y.round());
    pixmap.draw_pixmap(0, 0, src.as_ref(), &paint, ts, None);
}

#[inline]
fn draw_pixmap_at_alpha_scaled(
    pixmap: &mut PixmapMut,
    src: &Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    opacity: f32,
) {
    let op = opacity.clamp(0.0, 1.0);
    if op <= 0.002 {
        return;
    }
    let sx = w / src.width() as f32;
    let sy = h / src.height() as f32;
    let mut paint = tiny_skia::PixmapPaint::default();
    paint.opacity = op;
    paint.quality = tiny_skia::FilterQuality::Bilinear;
    let ts = Transform::from_scale(sx, sy).post_translate(x.round(), y.round());
    pixmap.draw_pixmap(0, 0, src.as_ref(), &paint, ts, None);
}

/// Blits a small monochrome icon `Pixmap` tinted with `color` (using source alpha as coverage),
/// giving razor-sharp contrast across Dark, Sepia, and Light themes plus smooth hover/active tinting.
fn draw_pixmap_tinted(pixmap: &mut PixmapMut, src: &Pixmap, x: f32, y: f32, color: Color) {
    if color.alpha() <= 0.002 {
        return;
    }
    let ox = x.round() as i32;
    let oy = y.round() as i32;
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    let src_px = src.pixels();
    for r in 0..sh {
        for c in 0..sw {
            let cov = src_px[(r * sw + c) as usize].alpha();
            if cov > 0 {
                blend_pixel(pixmap, ox + c, oy + r, color, cov);
            }
        }
    }
}

// =============================================================================
// PAINTER STRUCT & UNIFIED PIXEL-EXACT HITBOX REGISTRY
// =============================================================================

pub struct Painter {
    swash_cache: SwashCache,
    scratch_buffers: Vec<Buffer>,
    pub link_boxes: Vec<LinkHitBox>,
    pub ui_boxes: Vec<UiHitBox>,
    pub modal_card_rect: Option<(f32, f32, f32, f32)>,
    pub hovered_table_under_cursor: Option<usize>,
    frame_hovered_link: Option<String>,
    frame_active_search: Option<(usize, usize)>,
    current_block_idx: usize,
    block_match_counter: usize,
    pub logo_hero: Option<Pixmap>,
    pub logo_header: Option<Pixmap>,
    pub icon_file_up: Option<Pixmap>,
    pub icon_folder_white: Option<Pixmap>,
    pub icon_folder_gray: Option<Pixmap>,
    pub icon_search: Option<Pixmap>,
    pub icon_list_tree: Option<Pixmap>,
    pub icon_moon: Option<Pixmap>,
    pub icon_info: Option<Pixmap>,
    pub icon_stats: Option<Pixmap>,
    pub icon_width: Option<Pixmap>,
    pub icon_copy: Option<Pixmap>,
    pub icon_print: Option<Pixmap>,
    pub icon_zoom_in: Option<Pixmap>,
    pub icon_zoom_out: Option<Pixmap>,
}

impl Painter {
    pub fn new() -> Self {
        Self {
            swash_cache: SwashCache::new(),
            scratch_buffers: Vec::with_capacity(2),
            link_boxes: Vec::new(),
            ui_boxes: Vec::new(),
            modal_card_rect: None,
            hovered_table_under_cursor: None,
            frame_hovered_link: None,
            frame_active_search: None,
            current_block_idx: 0,
            block_match_counter: 0,
            logo_hero: load_png_pixmap(include_bytes!("../assets/icons/logo_hero_76.png")),
            logo_header: load_png_pixmap(include_bytes!("../assets/icons/logo_header_22.png")),
            icon_file_up: load_png_pixmap(include_bytes!("../assets/icons/lucide/file_up_48.png")),
            icon_folder_white: load_png_pixmap(include_bytes!("../assets/icons/lucide/folder_open_white_16.png")),
            icon_folder_gray: load_png_pixmap(include_bytes!("../assets/icons/lucide/folder_open_gray_15.png")),
            icon_search: load_png_pixmap(include_bytes!("../assets/icons/lucide/search_16.png")),
            icon_list_tree: load_png_pixmap(include_bytes!("../assets/icons/lucide/list_tree_16.png")),
            icon_moon: load_png_pixmap(include_bytes!("../assets/icons/lucide/moon_16.png")),
            icon_info: load_png_pixmap(include_bytes!("../assets/icons/lucide/info_16.png")),
            icon_stats: load_png_pixmap(include_bytes!("../assets/icons/lucide/stats_16.png")),
            icon_width: load_png_pixmap(include_bytes!("../assets/icons/lucide/width_16.png")),
            icon_copy: load_png_pixmap(include_bytes!("../assets/icons/lucide/copy_16.png")),
            icon_print: load_png_pixmap(include_bytes!("../assets/icons/lucide/print_16.png")),
            icon_zoom_in: load_png_pixmap(include_bytes!("../assets/icons/lucide/zoom_in_15.png")),
            icon_zoom_out: load_png_pixmap(include_bytes!("../assets/icons/lucide/zoom_out_15.png")),
        }
    }

    #[inline]
    fn take_buffer(&mut self, font_system: &mut FontSystem, metrics: Metrics) -> Buffer {
        match self.scratch_buffers.pop() {
            Some(mut buffer) => {
                buffer.set_metrics(font_system, metrics);
                buffer
            }
            None => Buffer::new(font_system, metrics),
        }
    }

    #[inline]
    fn give_buffer(&mut self, buffer: Buffer) {
        if self.scratch_buffers.len() < 2 {
            self.scratch_buffers.push(buffer);
        }
    }

    #[inline]
    pub fn push_hitbox(&mut self, x: f32, y: f32, w: f32, h: f32, target: HoverTarget) {
        self.ui_boxes.push(UiHitBox { x, y, w, h, target });
    }

    /// Unified Z-ordered hit-testing against the exact rectangles painted in the last frame.
    pub fn hit_test(&self, mx: f32, my: f32, opts: &ViewOptions, vh: f32) -> HoverTarget {
        // 0. Diagram Lightbox modal shields all underlying background elements
        if opts.diagram_lightbox.is_open {
            for hb in self.ui_boxes.iter().rev() {
                if matches!(
                    hb.target,
                    HoverTarget::DiagramClose
                        | HoverTarget::DiagramZoomIn
                        | HoverTarget::DiagramZoomOut
                        | HoverTarget::DiagramZoomReset
                        | HoverTarget::DiagramFit
                        | HoverTarget::DiagramCanvas
                ) && hb.contains(mx, my)
                {
                    return hb.target.clone();
                }
            }
            return HoverTarget::DiagramCanvas;
        }

        // 1. Header controls are always topmost (y <= HEADER_HEIGHT)
        if my <= HEADER_HEIGHT {
            for hb in self.ui_boxes.iter().rev() {
                if matches!(hb.target, HoverTarget::BrandLogo | HoverTarget::ToolbarBtn(_) | HoverTarget::NavBack | HoverTarget::NavForward)
                    && hb.contains(mx, my)
                {
                    return hb.target.clone();
                }
            }
            return HoverTarget::None;
        }

        // 2. When a modal is open, block background targets and only allow ModalClose
        if opts.stats_open || opts.about_open {
            for hb in self.ui_boxes.iter().rev() {
                if hb.target == HoverTarget::ModalClose && hb.contains(mx, my) {
                    return HoverTarget::ModalClose;
                }
            }
            return HoverTarget::None;
        }

        // 3. Reverse Z-order scan (Search bar -> Scrollbar -> Outline -> Copy/Anchor/Welcome buttons)
        for hb in self.ui_boxes.iter().rev() {
            if hb.contains(mx, my) {
                return hb.target.clone();
            }
        }

        // 4. Document links (excluding region covered by Outline Sidebar)
        let sidebar_cover = OUTLINE_WIDTH * opts.effective_sidebar_anim();
        if my > HEADER_HEIGHT && my < vh - STATUS_BAR_HEIGHT && mx > sidebar_cover {
            for link in self.link_boxes.iter().rev() {
                if mx >= link.x && mx <= link.x + link.w && my >= link.y && my <= link.y + link.h {
                    return HoverTarget::DocLink(link.url.clone());
                }
            }
        }

        HoverTarget::None
    }
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// MAIN ENTRY POINTS
// =============================================================================

pub fn paint_document(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    engine: &mut DocumentLayoutEngine,
    embed_cache: &RealEmbedMeasurer,
    viewport_width: u32,
    viewport_height: u32,
    file_path: Option<&str>,
    is_welcome: bool,
) {
    let default_opts = ViewOptions::default();
    paint_document_with_options(
        painter,
        pixmap,
        engine,
        embed_cache,
        viewport_width,
        viewport_height,
        file_path,
        is_welcome,
        &default_opts,
    );
}

pub fn paint_document_with_options(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    engine: &mut DocumentLayoutEngine,
    embed_cache: &RealEmbedMeasurer,
    viewport_width: u32,
    viewport_height: u32,
    file_path: Option<&str>,
    is_welcome: bool,
    opts: &ViewOptions,
) {
    let vw = viewport_width as f32;
    let vh = viewport_height as f32;
    let pal = opts.effective_palette();

    // 0. Reset per-frame hitboxes and transient link/search trackers
    painter.link_boxes.clear();
    painter.ui_boxes.clear();
    painter.modal_card_rect = None;
    painter.hovered_table_under_cursor = None;
    painter.frame_hovered_link = match &opts.hover {
        HoverTarget::DocLink(u) => Some(u.clone()),
        _ => None,
    };
    painter.frame_active_search = opts.search_active_block;

    // 1. Clear background
    fill_rect(pixmap, 0.0, 0.0, vw, vh, pal.bg_primary);

    // Tier-2 Dynamic Hydration: ensure visible blocks are exact-measured before querying visible_blocks
    if !is_welcome {
        let _ = engine.ensure_visible_exact();
    }

    let scroll_y = engine.scroll_y();
    let total_height = engine.layout().total_height;

    let (visible, font_system, _, layout) = engine.render_frame();
    let visible: &[VisibleBlock] = if is_welcome { &[] } else { visible };

    if is_welcome {
        draw_welcome_screen(painter, pixmap, font_system, &pal, opts, vw, vh);
    } else {
        let (content_x, content_width) = opts.reading_column(vw);
        let search_shift = 44.0 * opts.effective_search_anim();
        let top_offset = HEADER_HEIGHT + search_shift;
        let bottom_limit = vh - STATUS_BAR_HEIGHT;

        // Seamless luxury reading canvas (V2.0 parity):
        // The background is uniformly pal.bg_primary, allowing content to glide
        // smoothly under the sticky header without any static border frame collision.

        for vb in visible {
            let screen_y = top_offset + CONTENT_TOP_PADDING + vb.screen_y(scroll_y);
            let block_x = content_x + vb.x;
            let block_w = vb.width.min(content_width);

            if screen_y + vb.height < top_offset || screen_y > bottom_limit {
                continue;
            }

            // --- Visual Text Selection Highlight ---
            if let Some(ref sel) = opts.text_selection {
                if sel.is_active {
                    let min_b = sel.start_block.min(sel.end_block);
                    let max_b = sel.start_block.max(sel.end_block);
                    if vb.block_index >= min_b && vb.block_index <= max_b {
                        let sel_bg = match opts.theme {
                            ThemeMode::SolarizedSepia | ThemeMode::GithubLight => Color::from_rgba8(56, 139, 253, 45),
                            _ => Color::from_rgba8(56, 139, 253, 65),
                        };
                        fill_rounded_rect(
                            pixmap,
                            block_x - 6.0,
                            screen_y - 2.0,
                            block_w + 12.0,
                            vb.height + 4.0,
                            4.0,
                            sel_bg,
                        );
                    }
                }
            }

            if let Some(block) = layout.blocks.get(vb.block_index) {
                draw_block(
                    painter,
                    pixmap,
                    font_system,
                    embed_cache,
                    &pal,
                    layout,
                    opts,
                    vb,
                    block,
                    block_x,
                    screen_y,
                    block_w,
                );
            }
        }

        // Left Outline Sidebar (smoothly slides in/out via sidebar_anim)
        if opts.effective_sidebar_anim() > 0.002 {
            draw_outline_sidebar(painter, pixmap, font_system, &pal, layout, opts, scroll_y, vh);
        }

        // Top Search Bar (smoothly slides down & fades via search_anim)
        if opts.effective_search_anim() > 0.002 {
            draw_search_bar(painter, pixmap, font_system, &pal, opts, vw);
        }

        // Right Scrollbar (with smooth hover/drag expansion)
        draw_scrollbar(painter, pixmap, &pal, opts, scroll_y, total_height, vw, vh);
    }

    // Bottom Status Bar (#bottom-status, 28px height — exact QuickMD 2.0)
    draw_bottom_status_bar(
        painter,
        pixmap,
        font_system,
        &pal,
        opts,
        scroll_y,
        total_height,
        vw,
        vh,
        is_welcome,
    );

    // Centered Modal Dialogs (Stats or About with backdrop fade + 2nd-order spring pop)
    let modal_alpha = if !opts.anim_ticked && (opts.stats_open || opts.about_open) {
        opts.modal_anim.max(1.0)
    } else {
        opts.modal_anim.clamp(0.0, 1.0)
    };
    if modal_alpha > 0.004 {
        let show_about = if opts.about_open {
            true
        } else if opts.stats_open {
            false
        } else {
            opts.last_modal_is_about
        };
        if show_about {
            draw_about_modal(painter, pixmap, font_system, &pal, opts, modal_alpha, vw, vh);
        } else {
            draw_stats_modal(painter, pixmap, font_system, &pal, layout, opts, modal_alpha, vw, vh);
        }
    }

    // Full-Window Drag & Drop Overlay when dragging external files over window
    if opts.drag_anim > 0.004 || (!opts.anim_ticked && opts.file_drag_hover) {
        let da = if !opts.anim_ticked && opts.file_drag_hover { 1.0 } else { opts.drag_anim.clamp(0.0, 1.0) };
        draw_drag_drop_overlay(painter, pixmap, font_system, &pal, opts, da, vw, vh);
    }

    // Toast notification pill (bottom center, spring slide-up + exit fade)
    let toast_alpha = if !opts.anim_ticked && opts.toast_msg.is_some() {
        opts.toast_anim.max(1.0)
    } else {
        opts.toast_anim.clamp(0.0, 1.0)
    };
    if toast_alpha > 0.004 {
        let msg = opts.toast_msg.as_deref().unwrap_or(&opts.toast_retained_msg);
        if !msg.is_empty() {
            draw_toast(painter, pixmap, font_system, &pal, opts, msg, toast_alpha, vw, vh);
        }
    }

    // Sticky Top Header / Toolbar (Fixed at y: 0..52)
    draw_top_header(
        painter,
        pixmap,
        font_system,
        &pal,
        opts,
        scroll_y,
        total_height,
        vw,
        vh,
        file_path,
        is_welcome,
    );

    // Diagram Lightbox Pan-Zoom Modal (Full-window studio lightbox)
    if opts.diagram_lightbox.is_open {
        draw_diagram_lightbox(painter, pixmap, font_system, &pal, opts, embed_cache, vw, vh);
    }

    // Floating Tooltip (smooth fade + micro-settle) - Shielded when Lightbox is active
    if !opts.diagram_lightbox.is_open {
        let tip_alpha = if !opts.anim_ticked {
            if opts.hover.tooltip().is_some() { 1.0 } else { 0.0 }
        } else {
            opts.tooltip_anim.clamp(0.0, 1.0)
        };
        if tip_alpha > 0.01 {
            let live_tip = opts.hover.tooltip();
            let tip_str: &str = live_tip.as_deref().unwrap_or(&opts.retained_tooltip);
            if !tip_str.is_empty() {
                draw_floating_tooltip(painter, pixmap, font_system, &pal, tip_str, tip_alpha, opts.cursor_pos, vw, vh);
            }
        }
    }
}

// =============================================================================
// QUICKMD 2.0 TOP HEADER / TOOLBAR (Height: 52px)
// =============================================================================

fn draw_toolbar_btn_box(
    pixmap: &mut PixmapMut,
    pal: &ThemePalette,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    is_active: bool,
    hover_t: f32,
) {
    let ht = hover_t.clamp(0.0, 1.0);
    if is_active {
        // Active panel/modal open: subtle clean accent pill
        fill_rounded_rect(pixmap, x, y, w, h, 6.0, with_alpha(pal.accent, 0.16 + 0.08 * ht));
        stroke_rounded_rect(pixmap, x, y, w, h, 6.0, with_alpha(pal.accent, 0.6 + 0.3 * ht), 1.0);
    } else if ht > 0.01 {
        // V2 style hover: subtle translucent background with no harsh border
        fill_rounded_rect(pixmap, x, y, w, h, 6.0, with_alpha(pal.bg_tertiary, ht * 0.75));
    }
    // When inactive and not hovered: completely transparent, bare icon
}

fn draw_top_header(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    scroll_y: f32,
    total_height: f32,
    vw: f32,
    vh: f32,
    file_path: Option<&str>,
    is_welcome: bool,
) {
    fill_rect(pixmap, 0.0, 0.0, vw, HEADER_HEIGHT, pal.bg_secondary);
    fill_rect(pixmap, 0.0, HEADER_HEIGHT - 1.0, vw, 1.0, pal.border_default);

    // Reading Progress Bar at the very top (2.5px dual-tone gradient: #388BFD -> #38BDF8)
    if !is_welcome {
        let viewport_h = (vh - HEADER_HEIGHT - STATUS_BAR_HEIGHT - 44.0 * opts.search_anim.clamp(0.0, 1.0)).max(100.0);
        let max_scroll = (total_height - viewport_h).max(1.0);
        let raw_progress = (scroll_y / max_scroll).clamp(0.0, 1.0);
        let progress = if opts.anim_ticked || opts.progress_anim > 0.001 {
            opts.progress_anim.clamp(0.0, 1.0)
        } else {
            raw_progress
        };
        let bar_w = vw * progress;
        if bar_w > 0.5 {
            fill_rect(pixmap, 0.0, 0.0, bar_w, 2.5, pal.accent);
            if bar_w > 24.0 {
                fill_rect(pixmap, bar_w * 0.65, 0.0, bar_w * 0.35, 2.5, pal.accent_cyan);
            }
        }
    }

    // --- LEFT CONTROLS ---
    // 1. App Brand Logo & Name (Static non-interactive badge, zero hover, zero click)
    let brand_compact = vw < 480.0;
    let brand_w = if brand_compact { 34.0 } else { 146.0 };
    if let Some(ref logo) = painter.logo_header {
        draw_pixmap_at(pixmap, logo, 14.0, 15.0);
    }
    if !brand_compact {
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            "QuickMD",
            42.0,
            16.5,
            15.0,
            Weight::BOLD,
            pal.text_heading,
            BODY_FAMILIES,
        );
        let pro_x = 114.0;
        let pro_y = 17.0;
        let pro_w = 36.0;
        let pro_h = 18.0;
        fill_rounded_rect(pixmap, pro_x, pro_y, pro_w, pro_h, 4.0, pal.accent);
        let pro_tw = measure_ui_text_width(font_system, "PRO", 10.0, Weight::BOLD, BODY_FAMILIES);
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            "PRO",
            pro_x + (pro_w - pro_tw) / 2.0,
            pro_y + 2.5,
            10.0,
            Weight::BOLD,
            Color::WHITE,
            BODY_FAMILIES,
        );
    }

    let mut left_cursor = 8.0 + brand_w + 6.0;

    // 2. Navigation Back Button [ < ]
    let back_x = left_cursor;
    let back_w = 28.0;
    if opts.can_nav_back {
        painter.push_hitbox(back_x, 11.0, back_w, 30.0, HoverTarget::NavBack);
    }
    let h_back = if opts.can_nav_back { opts.nav_back_hover.clamp(0.0, 1.0) } else { 0.0 };
    draw_toolbar_btn_box(pixmap, pal, back_x, 11.0 , back_w, 30.0, false, h_back);
    {
        let cx = back_x + 13.5;
        let cy = 26.0 ;
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(cx + 2.5, cy - 5.0);
        pb.line_to(cx - 3.0, cy);
        pb.line_to(cx + 2.5, cy + 5.0);
        if let Some(p) = pb.finish() {
            let stroke = tiny_skia::Stroke {
                width: 2.0,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Default::default()
            };
            let c = if opts.can_nav_back {
                lerp_color(pal.text_primary, pal.accent, h_back)
            } else {
                with_alpha(pal.text_muted, 0.35)
            };
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(c);
            pixmap.stroke_path(&p, &paint, &stroke, tiny_skia::Transform::identity(), None);
        }
    }
    left_cursor += back_w + 4.0;

    // 3. Navigation Forward Button [ > ]
    let fwd_x = left_cursor;
    let fwd_w = 28.0;
    if opts.can_nav_forward {
        painter.push_hitbox(fwd_x, 11.0, fwd_w, 30.0, HoverTarget::NavForward);
    }
    let h_fwd = if opts.can_nav_forward { opts.nav_forward_hover.clamp(0.0, 1.0) } else { 0.0 };
    draw_toolbar_btn_box(pixmap, pal, fwd_x, 11.0 , fwd_w, 30.0, false, h_fwd);
    {
        let cx = fwd_x + 13.5;
        let cy = 26.0 ;
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(cx - 2.5, cy - 5.0);
        pb.line_to(cx + 3.0, cy);
        pb.line_to(cx - 2.5, cy + 5.0);
        if let Some(p) = pb.finish() {
            let stroke = tiny_skia::Stroke {
                width: 2.0,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Default::default()
            };
            let c = if opts.can_nav_forward {
                lerp_color(pal.text_primary, pal.accent, h_fwd)
            } else {
                with_alpha(pal.text_muted, 0.35)
            };
            let mut paint = tiny_skia::Paint::default();
            paint.set_color(c);
            pixmap.stroke_path(&p, &paint, &stroke, tiny_skia::Transform::identity(), None);
        }
    }
    left_cursor += fwd_w + 6.0;

    // 4. [Open] Button (x: left_cursor, responsive width)
    let compact_open = vw < 860.0;
    let open_btn_w = if compact_open { 30.0 } else { 76.0 };
    let open_btn_x = left_cursor;
    painter.push_hitbox(open_btn_x, 11.0, open_btn_w, 30.0, HoverTarget::ToolbarBtn(0));
    let h0 = opts.toolbar_hover_t(0);
    draw_toolbar_btn_box(pixmap, pal, open_btn_x, 11.0, open_btn_w, 30.0, false, h0);
    if let Some(ref icon) = painter.icon_folder_gray {
        let ic0 = lerp_color(pal.text_secondary, pal.text_heading, h0);
        let ic_x = if compact_open { open_btn_x + 7.0 } else { open_btn_x + 11.0 };
        draw_pixmap_tinted(pixmap, icon, ic_x, 18.0, ic0);
    }
    if !compact_open {
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            "Open",
            open_btn_x + 32.0,
            18.0 ,
            13.0,
            Weight::MEDIUM,
            lerp_color(pal.text_primary, pal.text_heading, h0),
            BODY_FAMILIES,
        );
    }
    left_cursor += open_btn_w + 6.0;

    // 5. Outline Icon Button -> ToolbarBtn(1)
    let outline_btn_x = left_cursor;
    painter.push_hitbox(outline_btn_x, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(1));
    let h1 = opts.toolbar_hover_t(1);
    draw_toolbar_btn_box(pixmap, pal, outline_btn_x, 11.0, 30.0, 30.0, opts.outline_open, h1);
    if let Some(ref icon) = painter.icon_list_tree {
        let ic1 = if opts.outline_open { pal.accent } else { lerp_color(pal.text_secondary, pal.text_heading, h1) };
        draw_pixmap_tinted(pixmap, icon, outline_btn_x + 7.0, 18.0, ic1);
    }
    left_cursor += 36.0;

    // 6. Search Icon Button -> ToolbarBtn(2)
    let search_btn_x = left_cursor;
    painter.push_hitbox(search_btn_x, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(2));
    let h2 = opts.toolbar_hover_t(2);
    draw_toolbar_btn_box(pixmap, pal, search_btn_x, 11.0, 30.0, 30.0, opts.search_open, h2);
    if let Some(ref icon) = painter.icon_search {
        let ic2 = if opts.search_open { pal.accent } else { lerp_color(pal.text_secondary, pal.text_heading, h2) };
        draw_pixmap_tinted(pixmap, icon, search_btn_x + 7.0, 18.0, ic2);
    }
    left_cursor += 36.0;

    let left_end_x = left_cursor;

    // --- RESPONSIVE RIGHT TOOLBAR CONTROLS ---
    let show_print_copy = vw >= 740.0;
    let full_zoom = vw >= 640.0;
    let show_stats = vw >= 540.0;
    let show_about = vw >= 460.0;

    let mut cur_rx = vw - 10.0;

    // Print button (ToolbarBtn(11))
    if show_print_copy && cur_rx - 30.0 >= left_end_x + 8.0 {
        cur_rx -= 30.0;
        painter.push_hitbox(cur_rx, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(11));
        let h11 = opts.toolbar_hover_t(11);
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 30.0, 30.0, false, h11);
        if let Some(ref icon) = painter.icon_print {
            draw_pixmap_tinted(pixmap, icon, cur_rx + 7.0, 18.0, lerp_color(pal.text_secondary, pal.text_heading, h11));
        }
        cur_rx -= 6.0;
    }

    // Copy Raw button (ToolbarBtn(10))
    if show_print_copy && cur_rx - 30.0 >= left_end_x + 8.0 {
        cur_rx -= 30.0;
        painter.push_hitbox(cur_rx, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(10));
        let h10 = opts.toolbar_hover_t(10);
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 30.0, 30.0, false, h10);
        if let Some(ref icon) = painter.icon_copy {
            draw_pixmap_tinted(pixmap, icon, cur_rx + 7.0, 18.0, lerp_color(pal.text_secondary, pal.text_heading, h10));
        }
        cur_rx -= 6.0;
    }

    // Divider
    if show_print_copy && cur_rx - 9.0 >= left_end_x + 8.0 {
        cur_rx -= 8.0;
        fill_rect(pixmap, cur_rx, 16.0, 1.0, 20.0, pal.border_default);
        cur_rx -= 8.0;
    }

    // Zoom In (ToolbarBtn(9))
    if full_zoom && cur_rx - 30.0 >= left_end_x + 8.0 {
        cur_rx -= 30.0;
        painter.push_hitbox(cur_rx, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(9));
        let h9 = opts.toolbar_hover_t(9);
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 30.0, 30.0, false, h9);
        if let Some(ref icon) = painter.icon_zoom_in {
            draw_pixmap_tinted(pixmap, icon, cur_rx + 7.5, 18.0, lerp_color(pal.text_secondary, pal.text_heading, h9));
        }
        cur_rx -= 6.0;
    }

    // Zoom Label (ToolbarBtn(8))
    if cur_rx - 38.0 >= left_end_x + 8.0 {
        cur_rx -= 38.0;
        painter.push_hitbox(cur_rx, 11.0, 38.0, 30.0, HoverTarget::ToolbarBtn(8));
        let h8 = opts.toolbar_hover_t(8);
        let z_str = format!("{}%", opts.zoom_pct);
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 38.0, 30.0, false, h8);
        let z_tw = measure_ui_text_width(font_system, &z_str, 11.5, Weight::MEDIUM, BODY_FAMILIES);
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            &z_str,
            cur_rx + (38.0 - z_tw) / 2.0,
            19.0 ,
            11.5,
            Weight::MEDIUM,
            if opts.zoom_pct != 100 { pal.accent } else { lerp_color(pal.text_secondary, pal.text_heading, h8) },
            BODY_FAMILIES,
        );
        cur_rx -= 6.0;
    }

    // Zoom Out (ToolbarBtn(7))
    if full_zoom && cur_rx - 30.0 >= left_end_x + 8.0 {
        cur_rx -= 30.0;
        painter.push_hitbox(cur_rx, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(7));
        let h7 = opts.toolbar_hover_t(7);
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 30.0, 30.0, false, h7);
        if let Some(ref icon) = painter.icon_zoom_out {
            draw_pixmap_tinted(pixmap, icon, cur_rx + 7.5, 18.0, lerp_color(pal.text_secondary, pal.text_heading, h7));
        }
        cur_rx -= 6.0;
    }

    // Divider
    if cur_rx - 9.0 >= left_end_x + 8.0 {
        cur_rx -= 8.0;
        fill_rect(pixmap, cur_rx, 16.0, 1.0, 20.0, pal.border_default);
        cur_rx -= 8.0;
    }

    // Theme (ToolbarBtn(6))
    if cur_rx - 30.0 >= left_end_x + 8.0 {
        cur_rx -= 30.0;
        painter.push_hitbox(cur_rx, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(6));
        let h6 = opts.toolbar_hover_t(6);
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 30.0, 30.0, false, h6);
        if let Some(ref icon) = painter.icon_moon {
            let ic6 = lerp_color(pal.text_secondary, pal.text_heading, h6);
            draw_pixmap_tinted(pixmap, icon, cur_rx + 7.0, 18.0, ic6);
        }
        cur_rx -= 6.0;
    }

    // Reading Width (ToolbarBtn(5))
    if cur_rx - 30.0 >= left_end_x + 8.0 {
        cur_rx -= 30.0;
        painter.push_hitbox(cur_rx, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(5));
        let h5 = opts.toolbar_hover_t(5);
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 30.0, 30.0, false, h5);
        if let Some(ref icon) = painter.icon_width {
            let ic5 = lerp_color(pal.text_secondary, pal.text_heading, h5);
            draw_pixmap_tinted(pixmap, icon, cur_rx + 7.0, 18.0, ic5);
        }
        cur_rx -= 6.0;
    }

    // Stats (ToolbarBtn(4))
    if show_stats && cur_rx - 30.0 >= left_end_x + 8.0 {
        cur_rx -= 30.0;
        painter.push_hitbox(cur_rx, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(4));
        let h4 = opts.toolbar_hover_t(4);
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 30.0, 30.0, opts.stats_open, h4);
        if let Some(ref icon) = painter.icon_stats {
            let ic4 = if opts.stats_open { pal.accent } else { lerp_color(pal.text_secondary, pal.text_heading, h4) };
            draw_pixmap_tinted(pixmap, icon, cur_rx + 7.0, 18.0, ic4);
        }
        cur_rx -= 6.0;
    }

    // Info / About (ToolbarBtn(3))
    if show_about && cur_rx - 30.0 >= left_end_x + 8.0 {
        cur_rx -= 30.0;
        painter.push_hitbox(cur_rx, 11.0, 30.0, 30.0, HoverTarget::ToolbarBtn(3));
        let h3 = opts.toolbar_hover_t(3);
        let info_active = opts.about_open;
        draw_toolbar_btn_box(pixmap, pal, cur_rx, 11.0, 30.0, 30.0, info_active, h3);
        if let Some(ref icon) = painter.icon_info {
            let ic3 = if info_active { pal.accent } else { lerp_color(pal.text_secondary, pal.text_heading, h3) };
            draw_pixmap_tinted(pixmap, icon, cur_rx + 7.0, 18.0, ic3);
        }
    }

    let right_start_x = cur_rx;

    // --- CENTER BREADCRUMBS ---
    let (center_text, is_active) = match file_path {
        Some(p) => {
            let path = std::path::Path::new(p);
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or(p);
            let parent = path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()).unwrap_or("");
            let s = if parent.is_empty() {
                fname.to_string()
            } else {
                format!("{}  /  {}", parent, fname)
            };
            (s, true)
        }
        None => {
            if is_welcome {
                ("No file opened".to_string(), false)
            } else {
                ("Untitled Document".to_string(), true)
            }
        }
    };

    let c_weight = if is_active { Weight::MEDIUM } else { Weight::NORMAL };
    let text_w = measure_ui_text_width(font_system, &center_text, 13.0, c_weight, BODY_FAMILIES);
    let total_center_w = if is_active { text_w + 22.0 } else { text_w };
    let center_x = (vw - total_center_w) / 2.0;

    if center_x > left_end_x + 16.0 && center_x + total_center_w < right_start_x - 16.0 {
        let mut tx = center_x;
        if is_active {
            if let Some(ref icon) = painter.icon_folder_gray {
                draw_pixmap_tinted(pixmap, icon, tx, 18.5, pal.text_secondary);
            }
            tx += 22.0;
        }
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            &center_text,
            tx,
            18.0,
            13.0,
            c_weight,
            if is_active { pal.text_primary } else { pal.text_muted },
            BODY_FAMILIES,
        );
    }
}

// =============================================================================
// QUICKMD 2.0 WELCOME HERO SCREEN (When NO file is loaded)
// =============================================================================

fn draw_welcome_screen(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    vw: f32,
    vh: f32,
) {
    let center_x = vw / 2.0;
    let card_w = 720.0_f32.min(vw - 48.0);
    let card_x = center_x - card_w / 2.0;

    let y_start = ((vh - STATUS_BAR_HEIGHT - 580.0) / 2.0 + HEADER_HEIGHT / 2.0).max(HEADER_HEIGHT + 24.0);

    // 1. Hero Logo (76x76 static brand icon, zero mouse interaction, zero gimmicks)
    let logo_size = 76.0;
    let logo_x = center_x - logo_size / 2.0;
    let logo_y = y_start;

    // Static 4-step soft ambient glow behind the squircle
    for step in 0..4 {
        let pad = (4 - step) as f32 * 3.0;
        fill_rounded_rect(
            pixmap,
            logo_x - pad,
            logo_y - pad + 3.0,
            logo_size + pad * 2.0,
            logo_size + pad * 2.0,
            18.0 + pad * 0.5,
            Color::from_rgba8(99, 102, 241, (10 - step * 2) as u8),
        );
    }

    if let Some(ref logo) = painter.logo_hero {
        draw_pixmap_at(pixmap, logo, logo_x, logo_y);
    }

    // 2. Hero Title: "QuickMD" (exact QuickMD 2.0: font-size 26px, Weight::BOLD)
    let title_text = "QuickMD";
    let title_w = measure_ui_text_width(font_system, title_text, 26.0, Weight::BOLD, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        title_text,
        center_x - title_w / 2.0,
        logo_y + logo_size + 14.0,
        26.0,
        Weight::BOLD,
        pal.text_heading,
        BODY_FAMILIES,
    );

    // 3. Hero Tagline: "High Performance Markdown Reader"
    let tag_text = "High Performance Markdown Reader";
    let tag_w = measure_ui_text_width(font_system, tag_text, 13.0, Weight::MEDIUM, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        tag_text,
        center_x - tag_w / 2.0,
        logo_y + logo_size + 48.0,
        13.0,
        Weight::MEDIUM,
        pal.text_muted,
        BODY_FAMILIES,
    );

    // 4. Interactive Drag & Drop Box (exact QuickMD 2.0 720px dashed box + animated marching dashes)
    let drop_y = logo_y + logo_size + 76.0;
    let drop_h = 176.0;
    painter.push_hitbox(card_x, drop_y, card_w, drop_h, HoverTarget::WelcomeDropzone);

    let drop_ht = if !opts.anim_ticked {
        if matches!(opts.hover, HoverTarget::WelcomeDropzone | HoverTarget::WelcomeChooseBtn) { 1.0 } else { 0.0 }
    } else {
        opts.welcome_drop_hover.clamp(0.0, 1.0)
    };

    fill_rounded_rect(pixmap, card_x, drop_y, card_w, drop_h, 16.0, pal.bg_secondary);
    if drop_ht > 0.005 {
        fill_rounded_rect(pixmap, card_x, drop_y, card_w, drop_h, 16.0, with_alpha(pal.accent, 0.09 * drop_ht));
    }
    let dash_color = lerp_color(pal.border_default, pal.accent, drop_ht);
    let dash_phase = if drop_ht > 0.01 { (opts.anim_clock * 28.0) % 14.0 } else { 0.0 };
    stroke_dashed_rounded_rect_phased(pixmap, card_x, drop_y, card_w, drop_h, 16.0, dash_color, 2.0, 8.0, 6.0, dash_phase);

    // Lucide FileUp icon (48x48) in QuickMD Blue (smoothly floats upward on dropzone hover)
    if let Some(ref icon) = painter.icon_file_up {
        let float_y = -3.5 * drop_ht + if drop_ht > 0.05 { (opts.anim_clock * 3.5).sin() * 2.0 * drop_ht } else { 0.0 };
        draw_pixmap_at(pixmap, icon, center_x - 24.0, drop_y + 20.0 + float_y);
    }

    // Dropzone Title
    let dtitle = "Drag & Drop your Markdown file here";
    let dtitle_w = measure_ui_text_width(font_system, dtitle, 19.0, Weight::BOLD, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        dtitle,
        center_x - dtitle_w / 2.0,
        drop_y + 78.0,
        19.0,
        Weight::BOLD,
        pal.text_primary,
        BODY_FAMILIES,
    );

    // Dropzone Subtitle
    let dsub = "Or click below to browse your computer (.md, .markdown, .txt)";
    let dsub_w = measure_ui_text_width(font_system, dsub, 13.0, Weight::NORMAL, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        dsub,
        center_x - dsub_w / 2.0,
        drop_y + 106.0,
        13.0,
        Weight::NORMAL,
        pal.text_secondary,
        BODY_FAMILIES,
    );

    // [Choose File] Button inside Dropzone: 140x36px (pushed AFTER WelcomeDropzone so it has higher Z-priority)
    let btn_w = 140.0;
    let btn_h = 36.0;
    let btn_x = center_x - btn_w / 2.0;
    let btn_y = drop_y + 128.0;
    painter.push_hitbox(btn_x, btn_y, btn_w, btn_h, HoverTarget::WelcomeChooseBtn);

    let btn_ht = if !opts.anim_ticked {
        if opts.hover == HoverTarget::WelcomeChooseBtn { 1.0 } else { 0.0 }
    } else {
        opts.welcome_btn_hover.clamp(0.0, 1.0)
    };
    let btn_lift = -1.5 * btn_ht;
    if btn_ht > 0.01 {
        fill_rounded_rect(pixmap, btn_x - 2.0, btn_y + 3.0 + btn_lift, btn_w + 4.0, btn_h + 2.0, 10.0, with_alpha(pal.accent, 0.28 * btn_ht));
    }
    let btn_color = lerp_color(pal.accent, Color::from_rgba8(88, 166, 255, 255), btn_ht);
    fill_rounded_rect(pixmap, btn_x, btn_y + btn_lift, btn_w, btn_h, 8.0, btn_color);

    if let Some(ref icon) = painter.icon_folder_white {
        draw_pixmap_at(pixmap, icon, btn_x + 18.0, btn_y + 10.0 + btn_lift);
    }
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        "Choose File",
        btn_x + 42.0,
        btn_y + 9.5 + btn_lift,
        13.5,
        Weight::BOLD,
        Color::WHITE,
        BODY_FAMILIES,
    );

    // 5. Shortcuts Grid (6 Cards, 3 columns x 2 rows, smooth hover crossfade + -2px lift)
    let grid_y = drop_y + drop_h + 20.0;
    let col_w = (card_w - 24.0) / 3.0;
    let row_h = 48.0;

    let shortcuts = [
        ("Open File", "Ctrl + O"),
        ("Find in Text", "Ctrl + F"),
        ("Toggle Outline", "Ctrl + T"),
        ("Cycle Theme", "Ctrl + Shift + T"),
        ("Zoom In / Out", "Ctrl + / -"),
        ("About QuickMD", "F1"),
    ];

    for (idx, (label, key)) in shortcuts.iter().enumerate() {
        let col = idx % 3;
        let row = idx / 3;
        let sx = card_x + col as f32 * (col_w + 12.0);
        let sy = grid_y + row as f32 * (row_h + 12.0);

        // Static shortcut card: no hitbox, no hover effect, zero mouse interaction
        fill_rounded_rect(pixmap, sx, sy, col_w, row_h, 8.0, pal.bg_secondary);
        stroke_rounded_rect(pixmap, sx, sy, col_w, row_h, 8.0, pal.border_subtle, 1.0);

        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            label,
            sx + 14.0,
            sy + 16.0,
            13.0,
            Weight::NORMAL,
            pal.text_primary,
            BODY_FAMILIES,
        );

        let kw = measure_ui_text_width(font_system, key, 11.0, Weight::NORMAL, MONO_FAMILIES);
        let kbd_w = kw + 14.0;
        let kbd_x = sx + col_w - kbd_w - 12.0;
        let kbd_y = sy + 12.0;
        fill_rounded_rect(pixmap, kbd_x, kbd_y, kbd_w, 24.0, 4.0, pal.bg_tertiary);
        stroke_rounded_rect(
            pixmap,
            kbd_x,
            kbd_y,
            kbd_w,
            24.0,
            4.0,
            pal.border_default,
            1.0,
        );
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            key,
            kbd_x + 7.0,
            kbd_y + 5.5,
            11.0,
            Weight::NORMAL,
            pal.text_secondary,
            MONO_FAMILIES,
        );
    }

    // 6. Set as Default .md Action Button (auto-hides once DefaultActive)
    if let Some(label) = default_md_button_label(opts.default_md_state) {
        let label_w = measure_ui_text_width(font_system, label, 13.0, Weight::MEDIUM, BODY_FAMILIES);
        let pill_w = (label_w + 56.0).max(196.0).min(card_w);
        let pill_h = 36.0;
        let pill_x = center_x - pill_w / 2.0;
        let pill_y = grid_y + row_h * 2.0 + 12.0 + 20.0;
        if pill_y + pill_h <= vh - STATUS_BAR_HEIGHT - 8.0 {
            draw_default_md_button(
                painter,
                pixmap,
                font_system,
                pal,
                opts,
                pill_x,
                pill_y,
                pill_w,
                pill_h,
                pal.bg_secondary,
                1.0,
                true,
            );
        }
    }
}

fn default_md_button_label(state: DefaultAssociationState) -> Option<&'static str> {
    match state {
        DefaultAssociationState::DefaultActive => None,
        DefaultAssociationState::NeedsWindowsConfirmation => Some("Confirm .md in Windows Settings"),
        DefaultAssociationState::NotRegistered => Some("Set as Default .md"),
    }
}

/// Renders the subtle QuickMD-native "Set as Default .md" button when .md is not yet active.
/// Automatically omitted whenever `opts.default_md_state == DefaultAssociationState::DefaultActive`.
fn draw_default_md_button(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    surface_bg: Color,
    alpha: f32,
    push_hitbox: bool,
) {
    let Some(label) = default_md_button_label(opts.default_md_state) else {
        return;
    };
    if push_hitbox {
        painter.push_hitbox(x, y, w, h, HoverTarget::SetDefaultMdBtn);
    }
    let ht = if !opts.anim_ticked {
        if opts.hover == HoverTarget::SetDefaultMdBtn { 1.0 } else { 0.0 }
    } else {
        opts.default_md_btn_hover.clamp(0.0, 1.0)
    };
    let lift = -1.5 * ht;
    let draw_y = y + lift;

    if ht > 0.01 {
        fill_rounded_rect(
            pixmap,
            x - 1.5,
            draw_y + 2.5,
            w + 3.0,
            h + 1.0,
            9.5,
            with_alpha(pal.accent, 0.14 * ht * alpha),
        );
    }

    let bg = lerp_color(surface_bg, lerp_color(pal.bg_tertiary, pal.accent, 0.14), ht);
    let border = lerp_color(pal.border_default, pal.accent, ht);
    fill_rounded_rect(pixmap, x, draw_y, w, h, 8.0, with_alpha(bg, alpha));
    stroke_rounded_rect(pixmap, x, draw_y, w, h, 8.0, with_alpha(border, alpha), 1.0);

    // Measure text + 14px Lucide file-check vector icon so the pair sits dead-center
    let text_w = measure_ui_text_width(font_system, label, 13.0, Weight::MEDIUM, BODY_FAMILIES);
    let icon_gap = 8.0;
    let icon_w = 14.0;
    let content_x = x + (w - (icon_w + icon_gap + text_w)) / 2.0;
    let icon_cx = content_x + icon_w * 0.5;
    let icon_cy = draw_y + h * 0.5;

    let stroke = Stroke {
        width: 1.5,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    let mut icon_paint = Paint::default();
    icon_paint.set_color(with_alpha(lerp_color(pal.text_secondary, pal.accent, ht), alpha));

    // Lucide file-check silhouette (folded document corner + subtle checkmark inside)
    let mut pb = PathBuilder::new();
    pb.move_to(icon_cx - 4.0, icon_cy - 5.5);
    pb.line_to(icon_cx + 1.2, icon_cy - 5.5);
    pb.line_to(icon_cx + 4.5, icon_cy - 2.2);
    pb.line_to(icon_cx + 4.5, icon_cy + 5.5);
    pb.line_to(icon_cx - 4.0, icon_cy + 5.5);
    pb.close();
    // Folded corner crease
    pb.move_to(icon_cx + 1.2, icon_cy - 5.5);
    pb.line_to(icon_cx + 1.2, icon_cy - 2.2);
    pb.line_to(icon_cx + 4.5, icon_cy - 2.2);
    // Checkmark inside page
    pb.move_to(icon_cx - 1.8, icon_cy + 1.4);
    pb.line_to(icon_cx - 0.3, icon_cy + 2.9);
    pb.line_to(icon_cx + 2.3, icon_cy - 0.1);
    if let Some(p) = pb.finish() {
        pixmap.stroke_path(&p, &icon_paint, &stroke, Transform::identity(), None);
    }

    let text_color = lerp_color(pal.text_primary, pal.text_heading, ht);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        label,
        content_x + icon_w + icon_gap,
        draw_y + (h - 16.0) * 0.5,
        13.0,
        Weight::MEDIUM,
        with_alpha(text_color, alpha),
        BODY_FAMILIES,
    );
}

// =============================================================================
// INTERACTIVE PANELS (OUTLINE SIDEBAR, SEARCH BAR, MODALS, STATUS BAR, TOAST)
// =============================================================================

fn draw_outline_sidebar(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    _layout: &DocumentLayout,
    opts: &ViewOptions,
    _scroll_y: f32,
    vh: f32,
) {
    let anim = opts.effective_sidebar_anim();
    let slide_x = -OUTLINE_WIDTH * (1.0 - anim);
    let sy = HEADER_HEIGHT;
    let sh = (vh - HEADER_HEIGHT - STATUS_BAR_HEIGHT).max(0.0);

    // Subtle elevation shadow on right edge of sidebar
    if anim > 0.02 {
        for s in 0..4 {
            let sw = (4 - s) as f32 * 2.0;
            fill_rect(
                pixmap,
                slide_x + OUTLINE_WIDTH,
                sy,
                sw,
                sh,
                Color::from_rgba8(0, 0, 0, (14.0 * anim) as u8),
            );
        }
    }

    fill_rect(pixmap, slide_x, sy, OUTLINE_WIDTH, sh, pal.bg_secondary);
    fill_rect(pixmap, slide_x + OUTLINE_WIDTH - 1.0, sy, 1.0, sh, pal.border_default);

    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        "DOCUMENT OUTLINE",
        slide_x + 16.0,
        sy + 16.0,
        11.0,
        Weight::BOLD,
        pal.text_muted,
        BODY_FAMILIES,
    );
    fill_rect(pixmap, slide_x + 16.0, sy + 38.0, OUTLINE_WIDTH - 32.0, 1.0, pal.border_subtle);

    let (owned_headings, headings_ref) = if opts.cached_headings.is_empty() {
        let mut list = Vec::new();
        for (idx, block) in _layout.blocks.iter().enumerate() {
            if let MarkdownBlock::Heading { level, text, .. } = block {
                list.push(HeadingItem {
                    block_idx: idx,
                    level: *level,
                    text: text.clone(),
                });
            }
        }
        (Some(list), None)
    } else {
        (None, Some(&opts.cached_headings))
    };
    let empty_headings = Vec::new();
    let headings = if let Some(r) = headings_ref {
        r
    } else if let Some(ref o) = owned_headings {
        o
    } else {
        &empty_headings
    };

    if headings.is_empty() {
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            "No headings in document",
            slide_x + 16.0,
            sy + 54.0,
            12.0,
            Weight::NORMAL,
            pal.text_muted,
            BODY_FAMILIES,
        );
        return;
    }

    let active_ord = opts.active_heading_ordinal;

    let visible_rows = ((sh - 56.0) / 34.0).floor().max(1.0) as usize;
    let active_row_f = if opts.anim_ticked && opts.outline_active_row.pos >= 0.0 {
        opts.outline_active_row.pos
    } else {
        active_ord.unwrap_or(0) as f32
    };
    let max_first_row = headings.len().saturating_sub(visible_rows);
    let first_row = (active_row_f.round().max(0.0) as usize)
        .saturating_sub(visible_rows / 2)
        .min(max_first_row);

    // Gliding 2nd-order Spring ScrollSpy pill + 3px accent bar
    if active_ord.is_some() {
        let rel_row = active_row_f - first_row as f32;
        if rel_row >= -0.5 && rel_row <= visible_rows as f32 - 0.5 {
            let pill_y = (sy + 46.0 + rel_row * 34.0)
                .clamp(sy + 44.0, (vh - STATUS_BAR_HEIGHT - 34.0).max(sy + 44.0));
            fill_rounded_rect(
                pixmap,
                slide_x + 8.0,
                pill_y,
                OUTLINE_WIDTH - 16.0,
                28.0,
                5.0,
                with_alpha(pal.accent, 0.16),
            );
            fill_rounded_rect(pixmap, slide_x + 8.0, pill_y + 4.0, 3.0, 20.0, 1.5, pal.accent);
        }
    }

    for (ord, h) in headings
        .iter()
        .enumerate()
        .skip(first_row)
        .take(visible_rows)
    {
        let block_idx = h.block_idx;
        let level = h.level;
        let text = h.text.as_str();
        let item_y = sy + 46.0 + (ord - first_row) as f32 * 34.0;
        if item_y + 32.0 > vh - STATUS_BAR_HEIGHT - 4.0 {
            break;
        }

        if anim > 0.25 {
            let hb_x = (slide_x + 6.0).max(0.0);
            let hb_w = (slide_x + OUTLINE_WIDTH - 6.0 - hb_x).max(0.0);
            if hb_w > 8.0 {
                painter.push_hitbox(hb_x, item_y, hb_w, 32.0, HoverTarget::OutlineItem(block_idx));
            }
        }

        let indent = ((level as f32 - 1.0).max(0.0) * 12.0).min(36.0);
        let is_current = active_ord == Some(ord);
        let hov_t = if !opts.anim_ticked {
            if opts.hover == HoverTarget::OutlineItem(block_idx) { 1.0 } else { 0.0 }
        } else if opts.outline_hover_idx == Some(block_idx) {
            opts.outline_hover_anim.clamp(0.0, 1.0)
        } else {
            0.0
        };

        if !is_current && hov_t > 0.01 {
            fill_rounded_rect(
                pixmap,
                slide_x + 8.0,
                item_y,
                OUTLINE_WIDTH - 16.0,
                28.0,
                5.0,
                with_alpha(pal.bg_tertiary, hov_t),
            );
        }

        let (font_sz, weight) = if level == 1 {
            (14.5, Weight::BOLD)
        } else if is_current {
            (13.5, Weight::BOLD)
        } else {
            (13.5, Weight::NORMAL)
        };
        let base_fg = if level == 1 { pal.text_primary } else { pal.text_secondary };
        let color = if is_current {
            pal.accent
        } else {
            lerp_color(base_fg, pal.text_heading, hov_t)
        };

        draw_ui_text_line_clipped(
            painter,
            pixmap,
            font_system,
            text,
            slide_x + 18.0 + indent + 2.0 * hov_t,
            item_y + 6.0,
            OUTLINE_WIDTH - 30.0 - indent,
            font_sz,
            weight,
            color,
            BODY_FAMILIES,
        );
    }
}

fn draw_search_bar(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    vw: f32,
) {
    let anim = opts.effective_search_anim();
    let bar_w = 480.0_f32.min(vw - 40.0);
    let bar_x = (vw - bar_w) / 2.0;
    let bar_y = HEADER_HEIGHT + 8.0 - (1.0 - anim) * 14.0;
    let bar_h = 36.0;

    // Soft floating shadow beneath search bar
    fill_rounded_rect(
        pixmap,
        bar_x - 2.0,
        bar_y + 3.0,
        bar_w + 4.0,
        bar_h + 2.0,
        10.0,
        Color::from_rgba8(0, 0, 0, (68.0 * anim) as u8),
    );
    fill_rounded_rect(pixmap, bar_x, bar_y, bar_w, bar_h, 8.0, with_alpha(pal.bg_secondary, anim));
    stroke_rounded_rect(pixmap, bar_x, bar_y, bar_w, bar_h, 8.0, with_alpha(pal.accent, anim), 1.5);

    if let Some(ref icon) = painter.icon_search {
        draw_pixmap_tinted(pixmap, icon, bar_x + 12.0, bar_y + 10.0, with_alpha(pal.accent, anim));
    }

    let max_q_w = (bar_w - 196.0).max(60.0);
    let is_empty = opts.search_query.is_empty();
    let display_q: &str = if is_empty {
        "Find in document... (Enter / Shift+Enter)"
    } else {
        opts.search_query.as_str()
    };
    let q_color = if is_empty {
        with_alpha(pal.text_muted, anim)
    } else {
        with_alpha(pal.text_primary, anim)
    };

    draw_ui_text_line_clipped(
        painter,
        pixmap,
        font_system,
        display_q,
        bar_x + 36.0,
        bar_y + 10.0,
        max_q_w,
        13.0,
        Weight::NORMAL,
        q_color,
        BODY_FAMILIES,
    );

    // Crisp vector caret bar (zero kerning shift, clean 2Hz blink)
    if opts.search_open {
        let caret_on = if !opts.anim_ticked {
            true
        } else {
            ((opts.anim_clock * 2.0) as u32) % 2 == 0
        };
        let caret_alpha = (if caret_on { 1.0 } else { 0.16 }) * anim;
        let qw = if is_empty {
            0.0
        } else {
            measure_ui_text_width(font_system, &opts.search_query, 13.0, Weight::NORMAL, BODY_FAMILIES)
                .min(max_q_w)
        };
        fill_rounded_rect(
            pixmap,
            bar_x + 36.0 + qw + 1.5,
            bar_y + 10.0,
            1.8,
            16.0,
            0.9,
            with_alpha(pal.accent, caret_alpha),
        );
    }

    // Match counter pill + [<] [>] [×] buttons on right side of search bar
    let btn_close_x = bar_x + bar_w - 32.0;
    let btn_next_x = btn_close_x - 28.0;
    let btn_prev_x = btn_next_x - 28.0;

    if opts.search_open && anim > 0.35 {
        painter.push_hitbox(bar_x, bar_y, bar_w, bar_h, HoverTarget::SearchInput);
        painter.push_hitbox(btn_prev_x, bar_y + 6.0, 24.0, 24.0, HoverTarget::SearchPrev);
        painter.push_hitbox(btn_next_x, bar_y + 6.0, 24.0, 24.0, HoverTarget::SearchNext);
        painter.push_hitbox(btn_close_x, bar_y + 6.0, 24.0, 24.0, HoverTarget::SearchClose);
    }

    if !is_empty {
        let badge = if opts.search_matches > 0 {
            format!(
                "{} / {}",
                (opts.search_current_idx + 1).min(opts.search_matches),
                opts.search_matches
            )
        } else {
            "0 results".to_string()
        };
        let bw = measure_ui_text_width(font_system, &badge, 11.5, Weight::MEDIUM, MONO_FAMILIES);
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            &badge,
            btn_prev_x - bw - 10.0,
            bar_y + 11.0,
            11.5,
            Weight::MEDIUM,
            with_alpha(
                if opts.search_matches > 0 { pal.accent } else { pal.code_red },
                anim,
            ),
            MONO_FAMILIES,
        );
    }

    // [<] Prev button
    let prev_hov = opts.hover == HoverTarget::SearchPrev;
    fill_rounded_rect(
        pixmap,
        btn_prev_x,
        bar_y + 6.0,
        24.0,
        24.0,
        4.0,
        with_alpha(if prev_hov { pal.accent } else { pal.bg_tertiary }, anim),
    );
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        "<",
        btn_prev_x + 8.0,
        bar_y + 9.5,
        12.0,
        Weight::BOLD,
        with_alpha(if prev_hov { Color::WHITE } else { pal.text_primary }, anim),
        MONO_FAMILIES,
    );

    // [>] Next button
    let next_hov = opts.hover == HoverTarget::SearchNext;
    fill_rounded_rect(
        pixmap,
        btn_next_x,
        bar_y + 6.0,
        24.0,
        24.0,
        4.0,
        with_alpha(if next_hov { pal.accent } else { pal.bg_tertiary }, anim),
    );
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        ">",
        btn_next_x + 8.0,
        bar_y + 9.5,
        12.0,
        Weight::BOLD,
        with_alpha(if next_hov { Color::WHITE } else { pal.text_primary }, anim),
        MONO_FAMILIES,
    );

    // [×] Close button
    let close_hov = opts.hover == HoverTarget::SearchClose;
    fill_rounded_rect(
        pixmap,
        btn_close_x,
        bar_y + 6.0,
        24.0,
        24.0,
        4.0,
        with_alpha(if close_hov { pal.code_red } else { pal.bg_tertiary }, anim),
    );
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        "×",
        btn_close_x + 7.5,
        bar_y + 9.0,
        13.0,
        Weight::BOLD,
        with_alpha(if close_hov { Color::WHITE } else { pal.text_secondary }, anim),
        BODY_FAMILIES,
    );
}

fn draw_stats_modal(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    opts: &ViewOptions,
    alpha: f32,
    vw: f32,
    vh: f32,
) {
    let a = alpha.clamp(0.0, 1.0);
    // 1. Dimmed backdrop overlay
    fill_rect(
        pixmap,
        0.0,
        HEADER_HEIGHT,
        vw,
        (vh - HEADER_HEIGHT).max(0.0),
        Color::from_rgba8(4, 6, 10, (152.0 * a) as u8),
    );

    let scale = if opts.anim_ticked {
        opts.modal_scale.pos.clamp(0.86, 1.08)
    } else {
        1.0
    };
    let base_w = 380.0_f32.min(vw - 40.0);
    let base_h = 280.0;
    let pop_w = base_w * scale;
    let pop_h = base_h * scale;
    let pop_x = (vw - pop_w) / 2.0;
    let pop_y = ((vh - pop_h) / 2.0).max(HEADER_HEIGHT + 20.0) + (1.0 - a) * 10.0;
    painter.modal_card_rect = Some((pop_x, pop_y, pop_w, pop_h));

    // Ambient outer glow behind modal
    let ar = (pal.accent.red() * 255.0) as u8;
    let ag = (pal.accent.green() * 255.0) as u8;
    let ab = (pal.accent.blue() * 255.0) as u8;
    for step in 0..6 {
        let pad = (6 - step) as f32 * 3.0;
        fill_rounded_rect(
            pixmap,
            pop_x - pad,
            pop_y - pad + 2.0,
            pop_w + pad * 2.0,
            pop_h + pad * 2.0,
            12.0 + pad * 0.5,
            Color::from_rgba8(ar, ag, ab, (10.0 * a) as u8),
        );
    }

    fill_rounded_rect(pixmap, pop_x, pop_y, pop_w, pop_h, 12.0, with_alpha(pal.bg_secondary, a));
    stroke_rounded_rect(pixmap, pop_x, pop_y, pop_w, pop_h, 12.0, with_alpha(pal.accent, a), 1.2);

    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        "DOCUMENT STATISTICS",
        pop_x + 22.0,
        pop_y + 18.0,
        12.0,
        Weight::BOLD,
        with_alpha(pal.accent, a),
        BODY_FAMILIES,
    );

    // Close [×] button in modal corner (exact hitbox registered)
    let close_x = pop_x + pop_w - 38.0;
    let close_y = pop_y + 12.0;
    if opts.stats_open {
        painter.push_hitbox(close_x, close_y, 24.0, 24.0, HoverTarget::ModalClose);
    }
    let close_hov = opts.hover == HoverTarget::ModalClose;
    fill_rounded_rect(
        pixmap,
        close_x,
        close_y,
        24.0,
        24.0,
        6.0,
        with_alpha(if close_hov { pal.code_red } else { pal.bg_tertiary }, a),
    );
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        "×",
        close_x + 7.5,
        close_y + 3.0,
        14.0,
        Weight::BOLD,
        with_alpha(if close_hov { Color::WHITE } else { pal.text_secondary }, a),
        BODY_FAMILIES,
    );

    fill_rect(pixmap, pop_x + 22.0, pop_y + 44.0, pop_w - 44.0, 1.0, with_alpha(pal.border_subtle, a));

    let read_mins = ((opts.raw_word_count as f32 / 200.0).ceil() as usize).max(1);
    let rows = [
        ("Word Count", format!("{}", opts.raw_word_count)),
        ("Character Count", format!("{}", opts.raw_char_count)),
        ("Total Lines", format!("{}", opts.raw_line_count)),
        ("AST Layout Blocks", format!("{}", layout.blocks.len())),
        ("Reading Column", opts.width_mode.label().to_string()),
        ("Estimated Reading Time", format!("~{} min", read_mins)),
    ];

    let row_step = 30.0 * scale;
    for (i, (k, v)) in rows.iter().enumerate() {
        let ry = pop_y + 58.0 * scale + i as f32 * row_step;
        if i % 2 == 1 {
            fill_rounded_rect(
                pixmap,
                pop_x + 14.0,
                ry - 4.0,
                pop_w - 28.0,
                26.0,
                4.0,
                with_alpha(pal.bg_tertiary, a),
            );
        }
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            k,
            pop_x + 22.0,
            ry,
            13.0,
            Weight::NORMAL,
            with_alpha(pal.text_secondary, a),
            BODY_FAMILIES,
        );
        let vw_px = measure_ui_text_width(font_system, v, 13.0, Weight::BOLD, MONO_FAMILIES);
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            v,
            pop_x + pop_w - vw_px - 22.0,
            ry,
            13.0,
            Weight::BOLD,
            with_alpha(pal.text_heading, a),
            MONO_FAMILIES,
        );
    }

    let hint = "Click anywhere outside or press Esc to close";
    let hw = measure_ui_text_width(font_system, hint, 11.0, Weight::NORMAL, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        hint,
        pop_x + (pop_w - hw) / 2.0,
        pop_y + pop_h - 24.0,
        11.0,
        Weight::NORMAL,
        with_alpha(pal.text_muted, a),
        BODY_FAMILIES,
    );
}

fn draw_about_modal(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    alpha: f32,
    vw: f32,
    vh: f32,
) {
    let a = alpha.clamp(0.0, 1.0);
    fill_rect(
        pixmap,
        0.0,
        HEADER_HEIGHT,
        vw,
        (vh - HEADER_HEIGHT).max(0.0),
        Color::from_rgba8(4, 6, 10, (152.0 * a) as u8),
    );

    let scale = if opts.anim_ticked {
        opts.modal_scale.pos.clamp(0.86, 1.08)
    } else {
        1.0
    };
    let show_default_btn = opts.default_md_state != DefaultAssociationState::DefaultActive;
    let base_w = 390.0_f32.min(vw - 32.0);
    let base_h = if show_default_btn { 344.0 } else { 292.0 };
    let pop_w = base_w * scale;
    let pop_h = base_h * scale;
    let pop_x = (vw - pop_w) / 2.0;
    let pop_y = ((vh - pop_h) / 2.0).max(HEADER_HEIGHT + 20.0) + (1.0 - a) * 10.0;
    painter.modal_card_rect = Some((pop_x, pop_y, pop_w, pop_h));

    // Multi-layer ambient drop shadow
    let ar = (pal.accent.red() * 255.0) as u8;
    let ag = (pal.accent.green() * 255.0) as u8;
    let ab = (pal.accent.blue() * 255.0) as u8;
    for step in 0..6 {
        let pad = (6 - step) as f32 * 3.0;
        fill_rounded_rect(
            pixmap,
            pop_x - pad,
            pop_y - pad + 3.0,
            pop_w + pad * 2.0,
            pop_h + pad * 2.0,
            14.0 + pad * 0.5,
            Color::from_rgba8(ar, ag, ab, (8.0 * a) as u8),
        );
    }

    fill_rounded_rect(pixmap, pop_x, pop_y, pop_w, pop_h, 14.0, with_alpha(pal.bg_secondary, a));
    stroke_rounded_rect(pixmap, pop_x, pop_y, pop_w, pop_h, 14.0, with_alpha(pal.border_default, a), 1.0);

    // Close button [×] at top right
    let close_x = pop_x + pop_w - 36.0;
    let close_y = pop_y + 12.0;
    if opts.about_open {
        painter.push_hitbox(close_x, close_y, 24.0, 24.0, HoverTarget::ModalClose);
    }
    let close_hov = opts.hover == HoverTarget::ModalClose;
    fill_rounded_rect(
        pixmap,
        close_x,
        close_y,
        24.0,
        24.0,
        6.0,
        with_alpha(if close_hov { pal.code_red } else { pal.bg_tertiary }, a),
    );
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        "×",
        close_x + 7.5,
        close_y + 3.0,
        14.0,
        Weight::BOLD,
        with_alpha(if close_hov { Color::WHITE } else { pal.text_secondary }, a),
        BODY_FAMILIES,
    );

    // Center Logo (Enlarged 64x64 Hero Icon with Ambient Drop Shadow)
    let logo_size = 64.0;
    let logo_x = pop_x + (pop_w - logo_size) / 2.0;
    let logo_y = pop_y + 24.0;
    for s in 0..4 {
        let pad = (4 - s) as f32 * 2.5;
        fill_rounded_rect(
            pixmap,
            logo_x - pad,
            logo_y - pad + 3.0,
            logo_size + pad * 2.0,
            logo_size + pad * 2.0,
            16.0 + pad * 0.4,
            Color::from_rgba8(99, 102, 241, (12.0 * a * (4 - s) as f32 / 4.0) as u8),
        );
    }
    if let Some(ref logo) = painter.logo_hero {
        draw_pixmap_at_alpha_scaled(pixmap, logo, logo_x, logo_y, logo_size, logo_size, a);
    }

    // App Title "QuickMD" (21px, Bold)
    let title = "QuickMD";
    let tw = measure_ui_text_width(font_system, title, 21.0, Weight::BOLD, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        title,
        pop_x + (pop_w - tw) / 2.0,
        pop_y + 102.0,
        21.0,
        Weight::BOLD,
        with_alpha(pal.text_heading, a),
        BODY_FAMILIES,
    );

    // Subtitle "Version 3.0.0 (Pure Native Rust)" (12px, Muted)
    let subtitle = "Version 3.0.0 (Pure Native Rust)";
    let sw = measure_ui_text_width(font_system, subtitle, 12.0, Weight::NORMAL, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        subtitle,
        pop_x + (pop_w - sw) / 2.0,
        pop_y + 130.0,
        12.0,
        Weight::NORMAL,
        with_alpha(pal.text_muted, a),
        BODY_FAMILIES,
    );

    // Author Box (100% V2.0 Style: bg_tertiary, border_subtle, R10)
    let box_x = pop_x + 28.0;
    let box_w = pop_w - 56.0;
    let box_y = pop_y + 158.0;
    let box_h = 66.0;
    fill_rounded_rect(pixmap, box_x, box_y, box_w, box_h, 10.0, with_alpha(pal.bg_tertiary, a));
    stroke_rounded_rect(pixmap, box_x, box_y, box_w, box_h, 10.0, with_alpha(pal.border_subtle, a), 1.0);

    let auth_lbl = "Created & Authored by";
    let alw = measure_ui_text_width(font_system, auth_lbl, 12.5, Weight::MEDIUM, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        auth_lbl,
        box_x + (box_w - alw) / 2.0,
        box_y + 12.0,
        12.5,
        Weight::MEDIUM,
        with_alpha(pal.text_secondary, a),
        BODY_FAMILIES,
    );

    let author = "koonnae741";
    let atw = measure_ui_text_width(font_system, author, 15.0, Weight::BOLD, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        author,
        box_x + (box_w - atw) / 2.0,
        box_y + 36.0,
        15.0,
        Weight::BOLD,
        with_alpha(pal.accent, a),
        BODY_FAMILIES,
    );

    // Default .md Association Button (shown only when not yet active)
    if show_default_btn {
        let btn_y = box_y + box_h + 14.0;
        let btn_h = 36.0;
        draw_default_md_button(
            painter,
            pixmap,
            font_system,
            pal,
            opts,
            box_x,
            btn_y,
            box_w,
            btn_h,
            pal.bg_tertiary,
            a,
            opts.about_open,
        );
    }

    // Hint: Esc to close
    let hint = "Press Esc to close";
    let hw = measure_ui_text_width(font_system, hint, 11.0, Weight::NORMAL, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        hint,
        pop_x + (pop_w - hw) / 2.0,
        pop_y + pop_h - 22.0,
        11.0,
        Weight::NORMAL,
        with_alpha(pal.text_muted, a),
        BODY_FAMILIES,
    );
}

fn draw_drag_drop_overlay(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    alpha: f32,
    vw: f32,
    vh: f32,
) {
    let a = alpha.clamp(0.0, 1.0);
    let body_h = (vh - HEADER_HEIGHT - STATUS_BAR_HEIGHT).max(0.0);
    fill_rect(
        pixmap,
        0.0,
        HEADER_HEIGHT,
        vw,
        body_h,
        with_alpha(pal.bg_primary, 0.84 * a),
    );

    let card_w = 500.0_f32.min(vw - 48.0);
    let card_h = 220.0;
    let card_x = (vw - card_w) / 2.0;
    let card_y = HEADER_HEIGHT + ((body_h - card_h) / 2.0).max(16.0) + (1.0 - a) * 12.0;

    fill_rounded_rect(pixmap, card_x, card_y, card_w, card_h, 18.0, with_alpha(pal.bg_secondary, a));
    fill_rounded_rect(
        pixmap,
        card_x,
        card_y,
        card_w,
        card_h,
        18.0,
        with_alpha(pal.accent, 0.10 * a),
    );

    let phase = (opts.anim_clock * 32.0) % 16.0;
    stroke_dashed_rounded_rect_phased(
        pixmap,
        card_x,
        card_y,
        card_w,
        card_h,
        18.0,
        with_alpha(pal.accent, a),
        2.4,
        10.0,
        6.0,
        phase,
    );

    if let Some(ref icon) = painter.icon_file_up {
        let bob = (opts.anim_clock * 4.0).sin() * 3.5;
        draw_pixmap_at_alpha(pixmap, icon, card_x + card_w / 2.0 - 24.0, card_y + 34.0 + bob, a);
    }

    let title = "Drop Markdown File to Open Instantly";
    let tw = measure_ui_text_width(font_system, title, 18.5, Weight::BOLD, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        title,
        card_x + (card_w - tw) / 2.0,
        card_y + 104.0,
        18.5,
        Weight::BOLD,
        with_alpha(pal.text_heading, a),
        BODY_FAMILIES,
    );

    let sub = "Zero-Latency Native Render  •  .md  •  .markdown  •  .txt";
    let sw = measure_ui_text_width(font_system, sub, 12.5, Weight::MEDIUM, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        sub,
        card_x + (card_w - sw) / 2.0,
        card_y + 138.0,
        12.5,
        Weight::MEDIUM,
        with_alpha(pal.accent_cyan, a),
        BODY_FAMILIES,
    );
}

fn draw_bottom_status_bar(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    scroll_y: f32,
    total_height: f32,
    vw: f32,
    vh: f32,
    is_welcome: bool,
) {
    let sy = (vh - STATUS_BAR_HEIGHT).max(HEADER_HEIGHT);
    fill_rect(pixmap, 0.0, sy, vw, STATUS_BAR_HEIGHT, pal.bg_secondary);
    fill_rect(pixmap, 0.0, sy, vw, 1.0, pal.border_default);

    // Left status metrics (document mode only, zero fake green dots or hype strings)
    if !is_welcome {
        let max_scroll = (total_height - (vh - HEADER_HEIGHT - STATUS_BAR_HEIGHT)).max(1.0);
        let pct = ((scroll_y / max_scroll).clamp(0.0, 1.0) * 100.0).round() as u32;
        let read_mins = (opts.raw_word_count + 199) / 200;
        let kb_str = format!("{:.1} KB", opts.raw_char_count as f32 / 1024.0);
        let left_status = format!(
            "{}% Read   •   {} min read   •   {}   •   {} words   •   {} lines",
            pct, read_mins, kb_str, opts.raw_word_count, opts.raw_line_count
        );
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            &left_status,
            16.0,
            sy + 6.5,
            11.5,
            Weight::MEDIUM,
            pal.text_secondary,
            BODY_FAMILIES,
        );
    }

    // Right status telemetry: Width Mode | Theme Name | Zoom % | Encoding
    let right_status = format!(
        "{}   •   {}   •   {}%   •   {}",
        opts.width_mode.label(),
        opts.theme.name(),
        opts.zoom_pct,
        opts.encoding_label
    );
    let rw = measure_ui_text_width(font_system, &right_status, 11.5, Weight::MEDIUM, BODY_FAMILIES);
    if vw - rw - 16.0 > 340.0 {
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            &right_status,
            vw - rw - 16.0,
            sy + 6.5,
            11.5,
            Weight::MEDIUM,
            pal.text_muted,
            BODY_FAMILIES,
        );
    }
}

fn draw_floating_tooltip(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    tip: &str,
    alpha: f32,
    cursor: (f32, f32),
    vw: f32,
    vh: f32,
) {
    let a = alpha.clamp(0.0, 1.0);
    let tw = measure_ui_text_width(font_system, tip, 11.5, Weight::MEDIUM, BODY_FAMILIES);
    let box_w = tw + 20.0;
    let box_h = 26.0;
    let box_x = (cursor.0 - box_w / 2.0).clamp(8.0, (vw - box_w - 8.0).max(8.0));
    let settle_y = (1.0 - a) * 3.0;
    let box_y = if cursor.1 <= HEADER_HEIGHT + 4.0 {
        HEADER_HEIGHT + 6.0 - settle_y
    } else {
        (cursor.1 - box_h - 10.0 + settle_y).clamp(8.0, (vh - box_h - 8.0).max(8.0))
    };

    fill_rounded_rect(
        pixmap,
        box_x - 1.0,
        box_y + 2.0,
        box_w + 2.0,
        box_h + 1.0,
        6.0,
        Color::from_rgba8(0, 0, 0, (64.0 * a) as u8),
    );
    fill_rounded_rect(pixmap, box_x, box_y, box_w, box_h, 5.0, with_alpha(pal.bg_tertiary, a));
    stroke_rounded_rect(pixmap, box_x, box_y, box_w, box_h, 5.0, with_alpha(pal.accent, a), 1.0);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        tip,
        box_x + 10.0,
        box_y + 5.5,
        11.5,
        Weight::MEDIUM,
        with_alpha(pal.text_heading, a),
        BODY_FAMILIES,
    );
}

fn draw_toast(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    msg: &str,
    alpha: f32,
    vw: f32,
    vh: f32,
) {
    let a = alpha.clamp(0.0, 1.0);
    let tw = measure_ui_text_width(font_system, msg, 12.5, Weight::MEDIUM, BODY_FAMILIES);
    let box_w = tw + 34.0;
    let box_h = 34.0;
    let box_x = (vw - box_w) / 2.0;
    let spring_offset = if opts.anim_ticked {
        (1.0 - opts.toast_spring.pos) * 18.0
    } else {
        0.0
    };
    let exit_offset = (1.0 - a) * 12.0;
    let box_y = vh - STATUS_BAR_HEIGHT - box_h - 16.0 + spring_offset + exit_offset;

    fill_rounded_rect(
        pixmap,
        box_x - 2.0,
        box_y + 3.0,
        box_w + 4.0,
        box_h + 2.0,
        18.0,
        Color::from_rgba8(0, 0, 0, (75.0 * a) as u8),
    );
    fill_rounded_rect(pixmap, box_x, box_y, box_w, box_h, 17.0, with_alpha(pal.bg_tertiary, a));
    stroke_rounded_rect(pixmap, box_x, box_y, box_w, box_h, 17.0, with_alpha(pal.accent, a), 1.5);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        msg,
        box_x + 17.0,
        box_y + 9.0,
        12.5,
        Weight::MEDIUM,
        with_alpha(pal.text_heading, a),
        BODY_FAMILIES,
    );
}

// =============================================================================
// BLOCK DISPATCH & RENDERERS
// =============================================================================

fn draw_block(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    embed_cache: &RealEmbedMeasurer,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    opts: &ViewOptions,
    vb: &VisibleBlock,
    block: &MarkdownBlock,
    x: f32,
    y: f32,
    width: f32,
) {
    painter.current_block_idx = vb.block_index;
    painter.block_match_counter = 0;
    let sq = opts.search_query.trim();
    match block {
        MarkdownBlock::Heading { level, text, .. } => {
            draw_heading(painter, pixmap, font_system, pal, layout, opts, vb.block_index, *level, text, x, y, width);
        }
        MarkdownBlock::Paragraph(spans) => {
            draw_rich_spans_with_search(painter, pixmap, font_system, pal, spans, sq, x, y, width, layout.config.base_font_size, pal.text_primary);
        }
        MarkdownBlock::Blockquote(spans) => {
            draw_blockquote(painter, pixmap, font_system, pal, layout, sq, vb.height, spans, x, y, width);
        }
        MarkdownBlock::Callout { kind, title, content } => {
            draw_callout(painter, pixmap, font_system, pal, layout, sq, vb.height, *kind, title, content, x, y, width);
        }
        MarkdownBlock::CodeBlock { lang, code } => {
            draw_code_block(painter, pixmap, font_system, pal, layout, opts, vb.block_index, vb.height, lang, code, x, y, width);
        }
        MarkdownBlock::MathBlock(latex) => {
            draw_math_embed(pixmap, embed_cache.get_cached_math(latex), pal.text_heading, x, y + layout.config.embed_padding, width);
        }
        MarkdownBlock::MermaidBlock(source) => {
            draw_mermaid_card(
                painter,
                pixmap,
                font_system,
                pal,
                opts,
                source,
                embed_cache.get_cached_mermaid(source),
                x,
                y,
                width,
                vb.height,
                layout.config.embed_padding,
            );
        }
        MarkdownBlock::Table(table) => {
            let tl = layout.table_layouts.get(&vb.block_index);
            draw_table(painter, pixmap, font_system, pal, layout, opts, vb.block_index, table, tl, x, y, width);
        }
        MarkdownBlock::List { ordered, start, items } => {
            draw_list(painter, pixmap, font_system, pal, layout, sq, vb.block_index, *ordered, *start, items, x, y, width);
        }
        MarkdownBlock::CheckList(items) => {
            draw_checklist(painter, pixmap, font_system, pal, layout, sq, vb.block_index, items, x, y, width);
        }
        MarkdownBlock::HorizontalRule => {
            draw_hr(pixmap, pal, x, y, width);
        }
        MarkdownBlock::Image { url, alt, link_url, width_override, align } => {
            let _max_w = width_override.unwrap_or(width).min(width);
            if let Some(pm) = embed_cache.get_cached_image(url) {
                let img_w = pm.width() as f32;
                let img_h = pm.height() as f32;
                let draw_x = match align {
                    Alignment::Center => x + (width - img_w) / 2.0,
                    Alignment::Right => x + width - img_w,
                    Alignment::Left => x,
                };
                let draw_y = y + 8.0;
                pixmap.draw_pixmap(
                    draw_x.round() as i32,
                    draw_y.round() as i32,
                    pm.as_ref(),
                    &tiny_skia::PixmapPaint::default(),
                    tiny_skia::Transform::identity(),
                    None,
                );
                if let Some(target) = link_url {
                    painter.link_boxes.push(LinkHitBox {
                        x: draw_x,
                        y: draw_y,
                        w: img_w,
                        h: img_h,
                        url: target.clone(),
                    });
                }
            } else if !url.trim().is_empty() {
                // Sleek Luxury Fallback Card for Remote / Unloaded Images
                let card_w = width.min(760.0);
                let card_h = 48.0f32;
                let card_x = match align {
                    Alignment::Center => x + (width - card_w) / 2.0,
                    Alignment::Right => x + width - card_w,
                    Alignment::Left => x,
                };
                let card_y = y + 4.0;
                let target_url = link_url.as_deref().unwrap_or(url.as_str());
                let is_hovered = painter.frame_hovered_link.as_deref() == Some(target_url);

                let bg_col = if is_hovered {
                    lerp_color(pal.bg_secondary, pal.accent, 0.12)
                } else {
                    pal.bg_secondary
                };
                let border_col = if is_hovered { pal.accent } else { pal.border_default };

                fill_rounded_rect(pixmap, card_x, card_y, card_w, card_h, 8.0, bg_col);
                stroke_rounded_rect(pixmap, card_x, card_y, card_w, card_h, 8.0, border_col, 1.0);

                let icon_text = "🖼";
                draw_ui_text_line(
                    painter,
                    pixmap,
                    font_system,
                    icon_text,
                    card_x + 14.0,
                    card_y + 14.0,
                    14.0,
                    Weight::NORMAL,
                    pal.text_secondary,
                    BODY_FAMILIES,
                );

                let title_text = if !alt.trim().is_empty() {
                    alt.trim()
                } else {
                    url.split('/').last().unwrap_or(url.as_str())
                };
                let sub_text = if link_url.is_some() {
                    "Remote Asset  •  Click to open target link in browser"
                } else {
                    "Remote Asset  •  Click to view original asset in browser"
                };

                draw_ui_text_line(
                    painter,
                    pixmap,
                    font_system,
                    title_text,
                    card_x + 42.0,
                    card_y + 8.0,
                    12.5,
                    Weight::SEMIBOLD,
                    if is_hovered { pal.accent_cyan } else { pal.text_primary },
                    BODY_FAMILIES,
                );

                draw_ui_text_line(
                    painter,
                    pixmap,
                    font_system,
                    sub_text,
                    card_x + 42.0,
                    card_y + 26.0,
                    10.5,
                    Weight::NORMAL,
                    pal.text_muted,
                    BODY_FAMILIES,
                );

                painter.link_boxes.push(LinkHitBox {
                    x: card_x,
                    y: card_y,
                    w: card_w,
                    h: card_h,
                    url: target_url.to_string(),
                });
            }
        }
        MarkdownBlock::BadgeRow { badges, align } => {
            draw_badge_row(painter, pixmap, font_system, embed_cache, badges, *align, x, y, width);
        }
    }
}

fn draw_badge_row(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    embed_cache: &RealEmbedMeasurer,
    badges: &[BadgeItem],
    align: Alignment,
    x: f32,
    y: f32,
    width: f32,
) {
    if badges.is_empty() {
        return;
    }
    let pill_h = 20.0f32;
    let gap_x = 8.0f32;
    let gap_y = 8.0f32;

    let mut rendered: Vec<(Option<Pixmap>, f32, &BadgeItem)> = Vec::with_capacity(badges.len());
    for item in badges {
        let pm = embed_cache.get_or_render_badge(item, font_system, &mut painter.swash_cache);
        let w = pm.as_ref().map(|p| p.width() as f32).unwrap_or(60.0);
        rendered.push((pm, w, item));
    }

    let mut lines: Vec<Vec<(Option<Pixmap>, f32, &BadgeItem)>> = Vec::new();
    let mut current_line: Vec<(Option<Pixmap>, f32, &BadgeItem)> = Vec::new();
    let mut current_w = 0.0f32;

    for entry in rendered {
        let bw = entry.1;
        if current_w + bw > width && !current_line.is_empty() {
            lines.push(std::mem::take(&mut current_line));
            current_w = 0.0;
        }
        if !current_line.is_empty() {
            current_w += gap_x;
        }
        current_w += bw;
        current_line.push(entry);
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    let mut line_y = y + 4.0;
    for line in lines {
        let line_w: f32 = line.iter().map(|e| e.1).sum::<f32>() + (line.len().saturating_sub(1) as f32 * gap_x);
        let mut cur_x = match align {
            Alignment::Center => x + (width - line_w) / 2.0,
            Alignment::Right => x + width - line_w,
            Alignment::Left => x,
        };

        for (pm_opt, bw, item) in line {
            if let Some(ref pm) = pm_opt {
                pixmap.draw_pixmap(
                    cur_x.round() as i32,
                    line_y.round() as i32,
                    pm.as_ref(),
                    &tiny_skia::PixmapPaint::default(),
                    tiny_skia::Transform::identity(),
                    None,
                );
            }
            if let Some(ref link) = item.link_url {
                painter.link_boxes.push(LinkHitBox {
                    x: cur_x,
                    y: line_y,
                    w: bw,
                    h: pill_h,
                    url: link.clone(),
                });
            }
            cur_x += bw + gap_x;
        }
        line_y += pill_h + gap_y;
    }
}

fn draw_heading(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    opts: &ViewOptions,
    block_idx: usize,
    level: u8,
    text: &str,
    x: f32,
    y: f32,
    width: f32,
) {
    let size = layout.config.heading_font_size(level);
    let sq = opts.search_query.trim();
    let spans = [InlineSpan::Bold(text.to_string())];
    let used_h = draw_rich_spans_with_search(
        painter,
        pixmap,
        font_system,
        pal,
        &spans,
        sq,
        x,
        y,
        width,
        size,
        pal.text_heading,
    );

    // Heading hover '#' anchor reveal & clickable gutter hitbox
    let (mx, my) = opts.cursor_pos;
    if level <= 3 {
        let anchor_x = (x - 24.0).max(6.0);
        painter.push_hitbox(anchor_x - 4.0, y, 26.0, used_h, HoverTarget::HeadingAnchor(block_idx));
        let row_hov = mx >= x - 28.0 && mx <= x + width && my >= y && my <= y + used_h;
        let anchor_hov = opts.hover == HoverTarget::HeadingAnchor(block_idx);
        if row_hov || anchor_hov {
            draw_ui_text_line(
                painter,
                pixmap,
                font_system,
                "#",
                anchor_x,
                y + 4.0,
                size * 0.78,
                Weight::BOLD,
                if anchor_hov { pal.accent_cyan } else { with_alpha(pal.accent, 0.78) },
                MONO_FAMILIES,
            );
        }
    }

    if level <= 2 {
        fill_rect(pixmap, x, y + used_h + 6.0, width, 1.0, pal.border_subtle);
    }
}

fn draw_blockquote(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    search_query: &str,
    measured_h: f32,
    spans: &[InlineSpan],
    x: f32,
    y: f32,
    width: f32,
) {
    let pad_x = layout.config.quote_padding_x;
    let pad_y = layout.config.quote_padding_y;
    let bar_w = layout.config.quote_bar_width;
    let inner_w = (width - bar_w - 2.0 * pad_x).max(40.0);

    let h = measured_h.max(40.0);
    fill_rounded_rect(pixmap, x, y, width, h, 6.0, pal.bg_secondary);
    fill_rect(pixmap, x, y, bar_w, h, pal.accent);
    draw_rich_spans_with_search(
        painter,
        pixmap,
        font_system,
        pal,
        spans,
        search_query,
        x + bar_w + pad_x,
        y + pad_y,
        inner_w,
        layout.config.base_font_size,
        pal.text_secondary,
    );
}

fn draw_callout(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    search_query: &str,
    measured_h: f32,
    kind: CalloutKind,
    title: &str,
    content: &[InlineSpan],
    x: f32,
    y: f32,
    width: f32,
) {
    let pad = layout.config.callout_padding;
    let bar_w = layout.config.quote_bar_width;
    let inner_w = (width - bar_w - 2.0 * pad).max(40.0);
    let height = measured_h.max(72.0);
    let accent = kind.accent_color();
    let card_bg = lerp_color(pal.bg_primary, accent, 0.11);
    let card_border = lerp_color(pal.border_subtle, accent, 0.28);

    fill_rounded_rect(pixmap, x, y, width, height, 8.0, card_bg);
    stroke_rounded_rect(pixmap, x, y, width, height, 8.0, card_border, 1.0);
    fill_rect(pixmap, x, y, bar_w, height, accent);

    let label = if title.is_empty() { kind.label() } else { title };
    let title_h = draw_doc_text(
        painter,
        pixmap,
        font_system,
        label,
        x + bar_w + pad,
        y + pad - 2.0,
        inner_w,
        layout.config.base_font_size * 0.95,
        Weight::BOLD,
        Style::Normal,
        accent,
        BODY_FAMILIES,
    );

    draw_rich_spans_with_search(
        painter,
        pixmap,
        font_system,
        pal,
        content,
        search_query,
        x + bar_w + pad,
        y + pad + title_h + layout.config.callout_title_gap - 2.0,
        inner_w,
        layout.config.base_font_size * 0.95,
        pal.text_primary,
    );
}

fn draw_code_block(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    opts: &ViewOptions,
    block_idx: usize,
    measured_h: f32,
    lang: &str,
    code: &str,
    x: f32,
    y: f32,
    width: f32,
) {
    let pad = layout.config.code_padding;
    let code_size = layout.config.code_font_size;
    let height = measured_h.max(54.0);
    let header_h = 28.0;

    // 1. Code block outer card + top .code-header bar
    fill_rounded_rect(pixmap, x, y, width, height, 8.0, pal.bg_secondary);
    fill_rounded_rect(pixmap, x, y, width, header_h, 8.0, pal.bg_tertiary);
    fill_rect(pixmap, x, y + header_h - 1.0, width, 1.0, pal.border_subtle);
    stroke_rounded_rect(pixmap, x, y, width, height, 8.0, pal.border_default, 1.0);

    // 2. Left terminal traffic-light accents + uppercase language label
    fill_circle(pixmap, x + 14.0, y + 14.0, 4.0, Color::from_rgba8(255, 95, 86, 210));
    fill_circle(pixmap, x + 26.0, y + 14.0, 4.0, Color::from_rgba8(255, 189, 46, 210));
    fill_circle(pixmap, x + 38.0, y + 14.0, 4.0, Color::from_rgba8(39, 201, 63, 210));

    let lang_label = if lang.is_empty() { "CODE".to_string() } else { lang.to_uppercase() };
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        &lang_label,
        x + 52.0,
        y + 7.5,
        10.5,
        Weight::BOLD,
        pal.text_muted,
        MONO_FAMILIES,
    );

    // 3. Right interactive [Copy] / [✓ Copied] button inside .code-header (rest-position hitbox!)
    let is_copied = opts.copied_block_idx == Some(block_idx);
    let is_hovered = opts.hover == HoverTarget::CodeCopyBtn(block_idx);
    let btn_label = if is_copied { "✓ Copied" } else { "Copy" };
    let base_w = if is_copied { 68.0 } else { 52.0 };
    let base_h = 20.0;
    let rest_x = x + width - base_w - 8.0;
    let rest_y = y + 4.0;
    painter.push_hitbox(rest_x, rest_y, base_w, base_h, HoverTarget::CodeCopyBtn(block_idx));

    let pop_scale = if is_copied && opts.anim_ticked {
        opts.copy_spring.pos.clamp(0.84, 1.12)
    } else {
        1.0
    };
    let btn_w = base_w * pop_scale;
    let btn_h = base_h * pop_scale;
    let btn_x = rest_x + (base_w - btn_w) * 0.5;
    let btn_y = rest_y + (base_h - btn_h) * 0.5 - if is_hovered && !is_copied { 1.0 } else { 0.0 };

    let (btn_bg, btn_border, btn_fg) = if is_copied {
        (
            Color::from_rgba8(46, 160, 67, 52),
            Color::from_rgba8(63, 185, 80, 255),
            Color::from_rgba8(63, 185, 80, 255),
        )
    } else if is_hovered {
        (pal.accent, pal.accent, Color::WHITE)
    } else {
        (pal.bg_secondary, pal.border_default, pal.text_secondary)
    };

    fill_rounded_rect(pixmap, btn_x, btn_y, btn_w, btn_h, 4.0, btn_bg);
    stroke_rounded_rect(pixmap, btn_x, btn_y, btn_w, btn_h, 4.0, btn_border, 1.0);
    let l_tw = measure_ui_text_width(font_system, btn_label, 10.5, Weight::MEDIUM, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        btn_label,
        btn_x + (btn_w - l_tw) / 2.0,
        btn_y + (btn_h - 13.0) * 0.5,
        10.5,
        Weight::MEDIUM,
        btn_fg,
        BODY_FAMILIES,
    );

    // 4. Code body text below header
    draw_doc_text(
        painter,
        pixmap,
        font_system,
        code,
        x + pad,
        y + header_h + 6.0,
        (width - 2.0 * pad).max(40.0),
        code_size,
        Weight::NORMAL,
        Style::Normal,
        pal.text_primary,
        MONO_FAMILIES,
    );
}

fn draw_mermaid_card(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    source: &str,
    cached: Option<Pixmap>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    embed_pad: f32,
) {
    // Architectural diagram studio surface (preserves crisp contrast even in Light/Sepia themes)
    let card_h = height.max(80.0);
    let is_light_bg = (pal.bg_primary.red() + pal.bg_primary.green() + pal.bg_primary.blue()) > 1.5;
    let surface = if is_light_bg {
        lerp_color(pal.bg_secondary, Color::from_rgba8(20, 24, 33, 255), 0.94)
    } else {
        pal.bg_secondary
    };
    fill_rounded_rect(pixmap, x, y, width, card_h, 8.0, surface);
    stroke_rounded_rect(pixmap, x, y, width, card_h, 8.0, pal.border_default, 1.0);

    // Hitbox for the entire card to trigger lightbox inspect
    painter.push_hitbox(x, y, width, card_h, HoverTarget::MermaidCard(source.to_string()));

    draw_embed(pixmap, cached, x, y + embed_pad, width);

    // Floating inspect / zoom button in top-right of card
    let is_hovered = matches!(&opts.hover, HoverTarget::MermaidCard(s) if s == source);
    let btn_w = if is_hovered { 128.0f32 } else { 108.0f32 };
    let btn_h = 24.0f32;
    let btn_x = x + width - btn_w - 10.0;
    let btn_y = y + 10.0;

    let (btn_bg, btn_border, btn_fg) = if is_hovered {
        (pal.accent, pal.accent, Color::WHITE)
    } else {
        (with_alpha(pal.bg_primary, 0.88), pal.border_default, pal.text_secondary)
    };

    fill_rounded_rect(pixmap, btn_x, btn_y, btn_w, btn_h, 5.0, btn_bg);
    stroke_rounded_rect(pixmap, btn_x, btn_y, btn_w, btn_h, 5.0, btn_border, 1.0);

    let btn_text = if is_hovered { "🔍 Click to Zoom" } else { "🔍 Zoom & Pan" };
    let l_tw = measure_ui_text_width(font_system, btn_text, 10.5, Weight::MEDIUM, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        btn_text,
        btn_x + (btn_w - l_tw) / 2.0,
        btn_y + (btn_h - 13.0) * 0.5,
        10.5,
        Weight::MEDIUM,
        btn_fg,
        BODY_FAMILIES,
    );
}

fn draw_diagram_lightbox(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    opts: &ViewOptions,
    embed_measurer: &RealEmbedMeasurer,
    vw: f32,
    vh: f32,
) {
    let lb = &opts.diagram_lightbox;
    if !lb.is_open {
        return;
    }

    // 1. Fullscreen dark backdrop
    fill_rect(pixmap, 0.0, 0.0, vw, vh, Color::from_rgba8(8, 11, 16, 240));

    // Canvas drag hitbox covering the whole screen
    painter.push_hitbox(0.0, 0.0, vw, vh, HoverTarget::DiagramCanvas);

    // 2. Render / Fetch high-res diagram
    if let Some(pm) = embed_measurer.render_unscaled_mermaid(&lb.source) {
        let natural_w = pm.width() as f32;
        let natural_h = pm.height() as f32;

        let draw_w = natural_w * lb.zoom;
        let draw_h = natural_h * lb.zoom;
        let draw_x = (vw - draw_w) / 2.0 + lb.pan_x;
        let draw_y = (vh - draw_h) / 2.0 + lb.pan_y;

        // Diagram container shadow & border
        fill_rounded_rect(pixmap, draw_x, draw_y, draw_w, draw_h, 8.0, Color::from_rgba8(13, 17, 23, 200));
        stroke_rounded_rect(pixmap, draw_x, draw_y, draw_w, draw_h, 8.0, with_alpha(pal.border_default, 0.6), 1.0);

        // Transform scaled and translated
        let transform = Transform::from_scale(lb.zoom, lb.zoom).post_translate(draw_x, draw_y);
        pixmap.draw_pixmap(0, 0, pm.as_ref(), &tiny_skia::PixmapPaint::default(), transform, None);
    }

    // 3. Top Hint Pill
    let hint_w = 340.0f32;
    let hint_h = 28.0f32;
    let hint_x = (vw - hint_w) / 2.0;
    let hint_y = 18.0f32;
    fill_rounded_rect(pixmap, hint_x, hint_y, hint_w, hint_h, 14.0, Color::from_rgba8(22, 27, 34, 210));
    stroke_rounded_rect(pixmap, hint_x, hint_y, hint_w, hint_h, 14.0, pal.border_default, 1.0);
    let hint_text = "Drag to pan  •  Wheel to zoom  •  Esc to close";
    let h_tw = measure_ui_text_width(font_system, hint_text, 11.0, Weight::NORMAL, BODY_FAMILIES);
    draw_ui_text_line(
        painter,
        pixmap,
        font_system,
        hint_text,
        hint_x + (hint_w - h_tw) / 2.0,
        hint_y + 7.0,
        11.0,
        Weight::NORMAL,
        pal.text_secondary,
        BODY_FAMILIES,
    );

    // 4. Floating Luxury Bottom Control Bar
    let bar_w = 360.0f32;
    let bar_h = 42.0f32;
    let bar_x = (vw - bar_w) / 2.0;
    let bar_y = vh - 64.0;

    // Ambient drop shadow
    fill_rounded_rect(pixmap, bar_x - 4.0, bar_y - 2.0, bar_w + 8.0, bar_h + 8.0, 24.0, Color::from_rgba8(0, 0, 0, 80));
    fill_rounded_rect(pixmap, bar_x, bar_y, bar_w, bar_h, 21.0, pal.bg_secondary);
    stroke_rounded_rect(pixmap, bar_x, bar_y, bar_w, bar_h, 21.0, pal.border_default, 1.2);

    // Buttons inside Control Bar:
    // [ - ]  (Zoom Out)
    let b_out_x = bar_x + 8.0;
    painter.push_hitbox(b_out_x, bar_y + 5.0, 32.0, 32.0, HoverTarget::DiagramZoomOut);
    let h_out = matches!(&opts.hover, HoverTarget::DiagramZoomOut);
    if h_out {
        fill_rounded_rect(pixmap, b_out_x, bar_y + 5.0, 32.0, 32.0, 16.0, pal.bg_tertiary);
    }
    draw_ui_text_line(painter, pixmap, font_system, "−", b_out_x + 11.0, bar_y + 11.0, 16.0, Weight::BOLD, if h_out { pal.accent } else { pal.text_primary }, BODY_FAMILIES);

    // [ {pct}% ] (Zoom Reset)
    let zoom_pct = (lb.zoom * 100.0).round() as u32;
    let pct_str = format!("{}%", zoom_pct);
    let pct_w = 64.0f32;
    let pct_x = b_out_x + 36.0;
    painter.push_hitbox(pct_x, bar_y + 5.0, pct_w, 32.0, HoverTarget::DiagramZoomReset);
    let h_pct = matches!(&opts.hover, HoverTarget::DiagramZoomReset);
    if h_pct {
        fill_rounded_rect(pixmap, pct_x, bar_y + 5.0, pct_w, 32.0, 16.0, pal.bg_tertiary);
    }
    let p_tw = measure_ui_text_width(font_system, &pct_str, 12.0, Weight::MEDIUM, MONO_FAMILIES);
    draw_ui_text_line(painter, pixmap, font_system, &pct_str, pct_x + (pct_w - p_tw) / 2.0, bar_y + 14.0, 12.0, Weight::MEDIUM, if h_pct { pal.accent } else { pal.text_primary }, MONO_FAMILIES);

    // [ + ]  (Zoom In)
    let b_in_x = pct_x + pct_w + 4.0;
    painter.push_hitbox(b_in_x, bar_y + 5.0, 32.0, 32.0, HoverTarget::DiagramZoomIn);
    let h_in = matches!(&opts.hover, HoverTarget::DiagramZoomIn);
    if h_in {
        fill_rounded_rect(pixmap, b_in_x, bar_y + 5.0, 32.0, 32.0, 16.0, pal.bg_tertiary);
    }
    draw_ui_text_line(painter, pixmap, font_system, "+", b_in_x + 10.0, bar_y + 11.0, 16.0, Weight::BOLD, if h_in { pal.accent } else { pal.text_primary }, BODY_FAMILIES);

    // Divider
    let div1_x = b_in_x + 38.0;
    fill_rect(pixmap, div1_x, bar_y + 11.0, 1.0, 20.0, pal.border_subtle);

    // [ ⊡ Fit ]
    let fit_x = div1_x + 10.0;
    let fit_w = 68.0f32;
    painter.push_hitbox(fit_x, bar_y + 5.0, fit_w, 32.0, HoverTarget::DiagramFit);
    let h_fit = matches!(&opts.hover, HoverTarget::DiagramFit);
    if h_fit {
        fill_rounded_rect(pixmap, fit_x, bar_y + 5.0, fit_w, 32.0, 16.0, pal.bg_tertiary);
    }
    let f_tw = measure_ui_text_width(font_system, "⊡ Fit", 12.0, Weight::MEDIUM, BODY_FAMILIES);
    draw_ui_text_line(painter, pixmap, font_system, "⊡ Fit", fit_x + (fit_w - f_tw) / 2.0, bar_y + 14.0, 12.0, Weight::MEDIUM, if h_fit { pal.accent } else { pal.text_primary }, BODY_FAMILIES);

    // Divider
    let div2_x = fit_x + fit_w + 8.0;
    fill_rect(pixmap, div2_x, bar_y + 11.0, 1.0, 20.0, pal.border_subtle);

    // [ ✕ Close ]
    let close_x = div2_x + 10.0;
    let close_w = 72.0f32;
    painter.push_hitbox(close_x, bar_y + 5.0, close_w, 32.0, HoverTarget::DiagramClose);
    let h_close = matches!(&opts.hover, HoverTarget::DiagramClose);
    if h_close {
        fill_rounded_rect(pixmap, close_x, bar_y + 5.0, close_w, 32.0, 16.0, Color::from_rgba8(239, 68, 68, 45));
    }
    let c_tw = measure_ui_text_width(font_system, "✕ Close", 12.0, Weight::MEDIUM, BODY_FAMILIES);
    draw_ui_text_line(painter, pixmap, font_system, "✕ Close", close_x + (close_w - c_tw) / 2.0, bar_y + 14.0, 12.0, Weight::MEDIUM, if h_close { Color::from_rgba8(239, 68, 68, 255) } else { pal.text_secondary }, BODY_FAMILIES);
}

fn draw_math_embed(pixmap: &mut PixmapMut, cached: Option<Pixmap>, ink: Color, x: f32, y: f32, width: f32) {
    if let Some(pm) = cached {
        let embed_w = pm.width() as f32;
        let center_x = x + (width - embed_w) / 2.0;
        let draw_x = center_x.max(x).round() as i32;
        let draw_y = y.round() as i32;
        let src_w = pm.width() as i32;
        let src_h = pm.height() as i32;
        let data = pm.data();
        for row in 0..src_h {
            for col in 0..src_w {
                let off = ((row * src_w + col) * 4) as usize;
                let cov = data[off + 3];
                if cov != 0 {
                    blend_pixel(pixmap, draw_x + col, draw_y + row, ink, cov);
                }
            }
        }
    }
}

fn draw_embed(pixmap: &mut PixmapMut, cached: Option<Pixmap>, x: f32, y: f32, width: f32) {
    if let Some(pm) = cached {
        let embed_w = pm.width() as f32;
        let center_x = x + (width - embed_w) / 2.0;
        let draw_x = center_x.max(x);

        pixmap.draw_pixmap(
            draw_x.round() as i32,
            y.round() as i32,
            pm.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

fn draw_table(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    opts: &ViewOptions,
    block_idx: usize,
    table: &TableData,
    table_layout: Option<&TableLayout>,
    x: f32,
    y: f32,
    width: f32,
) {
    let col_count = table.headers.len().max(1);
    let font_size = layout.config.base_font_size * 0.90;
    let pad_x = layout.config.table.cell_padding_x;
    let pad_y = layout.config.table.cell_padding_y;
    let sq = opts.search_query.trim();

    let (col_widths, row_heights, total_h) = if let Some(tl) = table_layout {
        let sum_w: f32 = tl.column_widths.iter().sum::<f32>().max(1.0);
        let scale = if sum_w < width { width / sum_w } else { 1.0 };
        let cw: Vec<f32> = tl.column_widths.iter().map(|&w| w * scale).collect();
        (cw, tl.row_heights.clone(), tl.total_height)
    } else {
        let cw = vec![width / col_count as f32; col_count];
        let rh = vec![38.0; table.rows.len() + 1];
        let th = rh.len() as f32 * 38.0;
        (cw, rh, th)
    };

    let mut col_xs = Vec::with_capacity(col_count);
    let mut curr_x = x;
    for &cw in &col_widths {
        col_xs.push(curr_x);
        curr_x += cw;
    }
    let table_w = (curr_x - x).min(width);
    let (mx, my) = opts.cursor_pos;
    let tab_bridge_hov = opts.hovered_table_idx == Some(block_idx)
        && mx >= x + table_w - 96.0
        && mx <= x + table_w + 4.0
        && my >= y - 28.0
        && my <= y;
    let table_hovered =
        (mx >= x && mx <= x + table_w && my >= y && my <= y + total_h) || tab_bridge_hov;
    if table_hovered {
        painter.hovered_table_under_cursor = Some(block_idx);
    }

    // 1. Header background
    let header_h = row_heights.first().copied().unwrap_or(38.0);
    fill_rounded_rect(pixmap, x, y, table_w, total_h, 6.0, pal.bg_primary);
    fill_rounded_rect(pixmap, x, y, table_w, header_h, 6.0, pal.bg_secondary);

    let vh = pixmap.height() as f32;
    let top_clip = 44.0f32; // Shielded under header toolbar

    // 2. Data rows backgrounds (with subtle editorial accent row highlight) & horizontal dividers
    let mut curr_y = y + header_h;
    for (row_idx, _row) in table.rows.iter().enumerate() {
        let rh = row_heights.get(row_idx + 1).copied().unwrap_or(38.0);
        if curr_y + rh >= top_clip && curr_y <= vh {
            let row_hovered = mx >= x && mx <= x + table_w && my >= curr_y && my < curr_y + rh;
            let base_bg = if row_idx % 2 == 0 { pal.bg_primary } else { pal.table_zebra };
            let row_bg = if row_hovered {
                lerp_color(base_bg, pal.accent, 0.09)
            } else {
                base_bg
            };
            fill_rect(pixmap, x + 1.0, curr_y, table_w - 2.0, rh - 1.0, row_bg);
            if row_hovered {
                fill_rect(pixmap, x + 1.0, curr_y + 2.0, 2.5, (rh - 4.0).max(2.0), pal.accent);
            }
            fill_rect(pixmap, x, curr_y, table_w, 1.0, pal.border_subtle);
        }
        curr_y += rh;
    }

    // 3. Vertical column dividers
    for &cx in col_xs.iter().skip(1) {
        fill_rect(pixmap, cx, y, 1.0, total_h, pal.border_subtle);
    }

    // 4. Outer rounded border (drawn AFTER fills so it is crisp and unbroken)
    stroke_rounded_rect(pixmap, x, y, table_w, total_h, 6.0, pal.border_default, 1.0);

    let sq_lower = if sq.is_empty() { String::new() } else { sq.to_lowercase() };

    // 5. Render header cells (rich spans, bold)
    if y + header_h >= top_clip && y <= vh {
        for (i, cell) in table.headers.iter().enumerate().take(col_count) {
            let cx = col_xs[i];
            let cw = col_widths[i];
            draw_table_cell_spans(
                painter,
                pixmap,
                font_system,
                pal,
                &cell.spans,
                sq,
                cx + pad_x,
                y + pad_y,
                (cw - 2.0 * pad_x).max(20.0),
                font_size,
                cell.alignment,
                pal.text_heading,
                true,
            );
        }
    } else if !sq_lower.is_empty() {
        for cell in table.headers.iter().take(col_count) {
            painter.block_match_counter += count_spans_search_matches(&cell.spans, &sq_lower);
        }
    }

    // 6. Render body cells (rich spans) with row-level viewport culling
    let mut ry = y + header_h;
    for (row_idx, row) in table.rows.iter().enumerate() {
        let rh = row_heights.get(row_idx + 1).copied().unwrap_or(38.0);
        if ry + rh >= top_clip && ry <= vh {
            for (i, cell) in row.iter().enumerate().take(col_count) {
                let cx = col_xs[i];
                let cw = col_widths[i];
                draw_table_cell_spans(
                    painter,
                    pixmap,
                    font_system,
                    pal,
                    &cell.spans,
                    sq,
                    cx + pad_x,
                    ry + pad_y,
                    (cw - 2.0 * pad_x).max(20.0),
                    font_size,
                    cell.alignment,
                    pal.text_primary,
                    false,
                );
            }
        } else if !sq_lower.is_empty() {
            for cell in row.iter().take(col_count) {
                painter.block_match_counter += count_spans_search_matches(&cell.spans, &sq_lower);
            }
        }
        ry += rh;
    }

    // 7. Smoothly fading [Copy Table] pill button in top-right corner of table
    let is_copied = opts.copied_block_idx == Some(block_idx);
    let is_btn_hov = opts.hover == HoverTarget::TableCopyBtn(block_idx);
    let is_idx_hov = opts.hovered_table_idx == Some(block_idx);
    let btn_alpha = if is_copied {
        1.0
    } else if !opts.anim_ticked {
        if table_hovered || is_btn_hov || is_idx_hov { 1.0 } else { 0.0 }
    } else if table_hovered || is_btn_hov || is_idx_hov {
        opts.table_hover_anim.clamp(0.18, 1.0)
    } else if opts.last_hovered_table_idx == Some(block_idx) {
        opts.table_hover_anim.clamp(0.0, 1.0)
    } else {
        0.0
    };

    if btn_alpha > 0.01 {
        let btn_label = if is_copied { "✓ Copied TSV" } else { "Copy Table" };
        let base_w = if is_copied { 86.0 } else { 72.0 };
        let base_h = 22.0;
        let rest_x = x + table_w - base_w;
        let rest_y = (y - 25.0).max(HEADER_HEIGHT + 4.0);
        if table_hovered || is_copied || is_btn_hov || is_idx_hov || btn_alpha > 0.25 {
            painter.push_hitbox(rest_x, rest_y, base_w, base_h + 4.0, HoverTarget::TableCopyBtn(block_idx));
        }

        let pop_scale = if is_copied && opts.anim_ticked {
            opts.copy_spring.pos.clamp(0.84, 1.12)
        } else {
            1.0
        };
        let btn_w = base_w * pop_scale;
        let btn_h = base_h * pop_scale;
        let btn_x = rest_x + (base_w - btn_w) * 0.5;
        let btn_y = rest_y + (base_h - btn_h) * 0.5 - if is_btn_hov && !is_copied { 1.0 } else { 0.0 } + (1.0 - btn_alpha) * 2.0;

        let (bg, border, fg) = if is_copied {
            (
                Color::from_rgba8(46, 160, 67, 65),
                Color::from_rgba8(63, 185, 80, 255),
                Color::from_rgba8(63, 185, 80, 255),
            )
        } else if is_btn_hov {
            (pal.accent, pal.accent, Color::WHITE)
        } else {
            (pal.bg_tertiary, pal.border_default, pal.text_secondary)
        };
        fill_rounded_rect(pixmap, btn_x, btn_y, btn_w, btn_h, 4.0, with_alpha(bg, btn_alpha));
        stroke_rounded_rect(pixmap, btn_x, btn_y, btn_w, btn_h, 4.0, with_alpha(border, btn_alpha), 1.0);
        let ltw = measure_ui_text_width(font_system, btn_label, 10.5, Weight::MEDIUM, BODY_FAMILIES);
        draw_ui_text_line(
            painter,
            pixmap,
            font_system,
            btn_label,
            btn_x + (btn_w - ltw) / 2.0,
            btn_y + (btn_h - 13.0) * 0.5,
            10.5,
            Weight::MEDIUM,
            with_alpha(fg, btn_alpha),
            BODY_FAMILIES,
        );
    }
}

fn draw_table_cell_spans(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    spans: &[InlineSpan],
    search_query: &str,
    x: f32,
    y: f32,
    inner_w: f32,
    font_size: f32,
    alignment: Alignment,
    base_color: Color,
    force_bold: bool,
) {
    let mut adjusted_spans: Vec<InlineSpan> = Vec::with_capacity(spans.len());
    if force_bold {
        for s in spans {
            match s {
                InlineSpan::Text(t) => adjusted_spans.push(InlineSpan::Bold(t.clone())),
                other => adjusted_spans.push(other.clone()),
            }
        }
    }
    let use_spans = if force_bold { &adjusted_spans } else { spans };

    let draw_x = match alignment {
        Alignment::Left => x,
        Alignment::Center => {
            let plain: String = use_spans.iter().map(|s| s.text()).collect();
            let tw = measure_ui_text_width(font_system, &plain, font_size, if force_bold { Weight::BOLD } else { Weight::NORMAL }, BODY_FAMILIES);
            x + ((inner_w - tw) / 2.0).max(0.0)
        }
        Alignment::Right => {
            let plain: String = use_spans.iter().map(|s| s.text()).collect();
            let tw = measure_ui_text_width(font_system, &plain, font_size, if force_bold { Weight::BOLD } else { Weight::NORMAL }, BODY_FAMILIES);
            x + (inner_w - tw).max(0.0)
        }
    };

    draw_rich_spans_with_search(painter, pixmap, font_system, pal, use_spans, search_query, draw_x, y, inner_w, font_size, base_color);
}

fn draw_list(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    search_query: &str,
    block_idx: usize,
    ordered: bool,
    start: u64,
    items: &[Vec<InlineSpan>],
    x: f32,
    y: f32,
    width: f32,
) {
    let base_size = layout.config.base_font_size;
    let indent = layout.config.list_indent;
    let gap = layout.config.list_item_gap;
    let mut cursor_y = y;

    let vh = pixmap.height() as f32;
    let top_clip = 44.0f32;
    let cached_heights = layout.list_layouts.get(&block_idx);
    let sq_lower = if search_query.is_empty() { String::new() } else { search_query.to_lowercase() };

    for (idx, spans) in items.iter().enumerate() {
        let item_h = cached_heights
            .and_then(|h| h.get(idx).copied())
            .unwrap_or_else(|| doc_line_height(base_size));

        // Culling 1: Below viewport -> stop processing remaining items
        if cursor_y > vh {
            break;
        }

        // Culling 2: Above viewport -> advance cursor without shaping/blitting text
        if cursor_y + item_h < top_clip {
            if !sq_lower.is_empty() {
                painter.block_match_counter += count_spans_search_matches(spans, &sq_lower);
            }
            cursor_y += item_h + gap;
            continue;
        }

        let marker_x = x + 6.0;
        if ordered {
            let marker = format!("{}.", start + idx as u64);
            draw_doc_text(
                painter,
                pixmap,
                font_system,
                &marker,
                marker_x,
                cursor_y,
                indent,
                base_size,
                Weight::BOLD,
                Style::Normal,
                pal.accent,
                BODY_FAMILIES,
            );
        } else {
            fill_circle(pixmap, marker_x + 6.0, cursor_y + base_size * 0.85, 3.2, pal.accent);
        }

        let used_h = draw_rich_spans_with_search(
            painter,
            pixmap,
            font_system,
            pal,
            spans,
            search_query,
            x + indent,
            cursor_y,
            (width - indent).max(40.0),
            base_size,
            pal.text_primary,
        );
        cursor_y += used_h.max(item_h) + gap;
    }
}

fn draw_checklist(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    layout: &DocumentLayout,
    search_query: &str,
    block_idx: usize,
    items: &[(bool, Vec<InlineSpan>)],
    x: f32,
    y: f32,
    width: f32,
) {
    let base_size = layout.config.base_font_size;
    let indent = layout.config.list_indent;
    let gap = layout.config.list_item_gap;
    let mut cursor_y = y;

    let vh = pixmap.height() as f32;
    let top_clip = 44.0f32;
    let cached_heights = layout.list_layouts.get(&block_idx);
    let sq_lower = if search_query.is_empty() { String::new() } else { search_query.to_lowercase() };

    for (idx, (checked, spans)) in items.iter().enumerate() {
        let item_h = cached_heights
            .and_then(|h| h.get(idx).copied())
            .unwrap_or_else(|| doc_line_height(base_size));

        // Culling 1: Below viewport -> stop processing remaining items
        if cursor_y > vh {
            break;
        }

        // Culling 2: Above viewport -> advance cursor without shaping/blitting text
        if cursor_y + item_h < top_clip {
            if !sq_lower.is_empty() {
                painter.block_match_counter += count_spans_search_matches(spans, &sq_lower);
            }
            cursor_y += item_h + gap;
            continue;
        }

        let box_x = x + 6.0;
        let box_y = cursor_y + 8.5;

        if *checked {
            fill_rounded_rect(pixmap, box_x, box_y, 16.0, 16.0, 4.0, pal.accent);
            draw_checkmark(pixmap, box_x, box_y);
        } else {
            stroke_rounded_rect(pixmap, box_x, box_y, 16.0, 16.0, 4.0, pal.border_default, 1.5);
        }

        let used_h = draw_rich_spans_with_search(
            painter,
            pixmap,
            font_system,
            pal,
            spans,
            search_query,
            x + indent,
            cursor_y,
            (width - indent).max(40.0),
            base_size,
            if *checked { pal.text_secondary } else { pal.text_primary },
        );
        cursor_y += used_h.max(item_h) + gap;
    }
}

fn draw_checkmark(pixmap: &mut PixmapMut, box_x: f32, box_y: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(box_x + 3.5, box_y + 8.0);
    pb.line_to(box_x + 6.8, box_y + 11.5);
    pb.line_to(box_x + 12.5, box_y + 4.5);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(Color::WHITE);
        paint.anti_alias = true;
        let mut stroke = Stroke::default();
        stroke.width = 1.8;
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn draw_hr(pixmap: &mut PixmapMut, pal: &ThemePalette, x: f32, y: f32, width: f32) {
    fill_rect(pixmap, x, y + 12.0, width, 1.0, pal.border_subtle);
}

fn draw_scrollbar(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    pal: &ThemePalette,
    opts: &ViewOptions,
    scroll_y: f32,
    total_height: f32,
    viewport_w: f32,
    viewport_h: f32,
) {
    let track_x = viewport_w - SCROLLBAR_WIDTH;
    let track_y = HEADER_HEIGHT;
    let track_h = (viewport_h - HEADER_HEIGHT - STATUS_BAR_HEIGHT).max(0.0);
    painter.push_hitbox(track_x - 6.0, track_y, SCROLLBAR_WIDTH + 6.0, track_h, HoverTarget::Scrollbar);

    let sb_ht = if !opts.anim_ticked {
        if opts.hover == HoverTarget::Scrollbar || opts.scrollbar_dragging { 1.0 } else { 0.0 }
    } else {
        opts.scrollbar_hover.clamp(0.0, 1.0)
    };

    if sb_ht > 0.01 {
        fill_rect(pixmap, track_x - 2.0 * sb_ht, track_y, SCROLLBAR_WIDTH + 2.0 * sb_ht, track_h, with_alpha(pal.bg_secondary, 0.65 * sb_ht));
    }

    let doc_h = total_height.max(track_h);
    let thumb_h = (track_h * track_h / doc_h).clamp(36.0, track_h);
    let max_scroll = (doc_h - track_h).max(1.0);
    let thumb_y = track_y + (scroll_y / max_scroll).clamp(0.0, 1.0) * (track_h - thumb_h);

    let thumb_w = 5.0 + 3.0 * sb_ht;
    let thumb_x = viewport_w - thumb_w - 1.5;
    let thumb_color = lerp_color(pal.scroll_thumb, pal.accent, sb_ht * 0.88);
    fill_rounded_rect(pixmap, thumb_x, thumb_y, thumb_w, thumb_h, thumb_w * 0.5, thumb_color);
}

/// Safe, panic-free case-insensitive substring search supporting ASCII fast-paths
/// and Unicode character boundary mapping without slicing panic traps.
pub fn find_case_insensitive_matches(raw: &str, query_lower: &str) -> Vec<(usize, usize)> {
    if query_lower.is_empty() || raw.is_empty() {
        return Vec::new();
    }
    // Fast path: if both strings are ASCII, byte-offsets match 1:1 and cannot land on non-char boundaries
    if raw.is_ascii() && query_lower.is_ascii() {
        let q_bytes = query_lower.as_bytes();
        let r_bytes = raw.as_bytes();
        let q_len = q_bytes.len();
        if q_len > r_bytes.len() {
            return Vec::new();
        }
        let mut matches = Vec::new();
        let mut i = 0;
        while i + q_len <= r_bytes.len() {
            if r_bytes[i..i + q_len].eq_ignore_ascii_case(q_bytes) {
                matches.push((i, i + q_len));
                i += q_len;
            } else {
                i += 1;
            }
        }
        return matches;
    }

    // Unicode path: Map lower string bytes back to valid char boundary ranges in `raw`
    let mut lower_str = String::with_capacity(raw.len());
    let mut byte_map = Vec::new();
    for (start, ch) in raw.char_indices() {
        let raw_start = start;
        let raw_end = start + ch.len_utf8();
        for lch in ch.to_lowercase() {
            let l_len = lch.len_utf8();
            lower_str.push(lch);
            for _ in 0..l_len {
                byte_map.push((raw_start, raw_end));
            }
        }
    }

    let mut matches = Vec::new();
    let mut cursor = 0usize;
    while let Some(pos) = lower_str[cursor..].find(query_lower) {
        let l_start = cursor + pos;
        let l_end = l_start + query_lower.len();
        if l_start < byte_map.len() && l_end <= byte_map.len() {
            let raw_start = byte_map[l_start].0;
            let raw_end = byte_map[l_end - 1].1;
            if raw.is_char_boundary(raw_start) && raw.is_char_boundary(raw_end) && raw_start < raw_end {
                matches.push((raw_start, raw_end));
            }
        }
        cursor = l_end;
    }
    matches
}

/// Counts case-insensitive search matches in `block` using the exact same span traversal
/// order as `draw_block` / `draw_rich_spans_with_search`.
pub fn count_spans_search_matches(spans: &[InlineSpan], query_lower: &str) -> usize {
    if query_lower.is_empty() {
        return 0;
    }
    let mut total = 0usize;
    for sp in spans {
        match sp {
            InlineSpan::Text(t)
            | InlineSpan::Bold(t)
            | InlineSpan::Italic(t)
            | InlineSpan::BoldItalic(t) => total += find_case_insensitive_matches(t, query_lower).len(),
            InlineSpan::Link { text, .. } => total += find_case_insensitive_matches(text, query_lower).len(),
            InlineSpan::InlineCode(c) => total += find_case_insensitive_matches(c, query_lower).len(),
            InlineSpan::InlineMath(_) | InlineSpan::Image { .. } => {}
        }
    }
    total
}

pub fn count_block_search_matches(block: &MarkdownBlock, query_lower: &str) -> usize {
    if query_lower.is_empty() {
        return 0;
    }

    fn count_str(raw: &str, q: &str) -> usize {
        find_case_insensitive_matches(raw, q).len()
    }

    match block {
        MarkdownBlock::Heading { text, .. } => count_str(text, query_lower),
        MarkdownBlock::Paragraph(spans) | MarkdownBlock::Blockquote(spans) => count_spans_search_matches(spans, query_lower),
        MarkdownBlock::Callout { content, .. } => count_spans_search_matches(content, query_lower),
        MarkdownBlock::Table(table) => {
            let col_count = table.headers.len().max(1);
            let mut total = 0usize;
            for cell in table.headers.iter().take(col_count) {
                total += count_spans_search_matches(&cell.spans, query_lower);
            }
            for row in &table.rows {
                for cell in row.iter().take(col_count) {
                    total += count_spans_search_matches(&cell.spans, query_lower);
                }
            }
            total
        }
        MarkdownBlock::List { items, .. } => items.iter().map(|s| count_spans_search_matches(s, query_lower)).sum(),
        MarkdownBlock::CheckList(items) => items.iter().map(|(_, s)| count_spans_search_matches(s, query_lower)).sum(),
        MarkdownBlock::CodeBlock { code, .. } => count_str(code, query_lower),
        MarkdownBlock::MathBlock(_)
        | MarkdownBlock::MermaidBlock(_)
        | MarkdownBlock::HorizontalRule
        | MarkdownBlock::Image { .. }
        | MarkdownBlock::BadgeRow { .. } => 0,
    }
}

// =============================================================================
// TYPOGRAPHY & UNIFIED RICH-TEXT ENGINE
// =============================================================================

#[inline]
fn doc_line_height(size: f32) -> f32 {
    (size * THAI_LINE_CLEARANCE_MULT + THAI_LINE_CLEARANCE_PX).ceil()
}

/// Crisp single-line UI text (toolbar buttons, pills, badges, welcome titles).
/// Uses tight `size * 1.2` line height so vertical centering inside buttons is exact.
fn draw_ui_text_line(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    weight: Weight,
    color: Color,
    family: &'static str,
) {
    draw_ui_text_line_clipped(painter, pixmap, font_system, text, x, y, 10000.0, size, weight, color, family);
}

fn draw_ui_text_line_clipped(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    text: &str,
    x: f32,
    y: f32,
    max_width: f32,
    size: f32,
    weight: Weight,
    color: Color,
    family: &'static str,
) {
    let mut display_str: std::borrow::Cow<'_, str> = std::borrow::Cow::Borrowed(text);
    if max_width < 5000.0 && measure_ui_text_width(font_system, text, size, weight, family) > max_width {
        let mut truncated = String::new();
        let mut candidate = String::new();
        for ch in text.chars() {
            candidate.clear();
            candidate.push_str(&truncated);
            candidate.push('…');
            if measure_ui_text_width(font_system, &candidate, size, weight, family) > max_width {
                break;
            }
            truncated.push(ch);
        }
        truncated.push('…');
        display_str = std::borrow::Cow::Owned(truncated);
    }

    let lh = (size * 1.2).ceil();
    let metrics = Metrics::new(size, lh);
    let mut buffer = painter.take_buffer(font_system, metrics);
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_size(font_system, Some(max_width.max(1.0) + 10.0), Some(lh * 1.5));

    let attrs = Attrs::new().family(Family::Name(family)).weight(weight);
    buffer.set_text(font_system, &display_str, attrs, Shaping::Advanced);
    buffer.shape_until_scroll(font_system, false);

    blit_buffer(painter, pixmap, font_system, None, &buffer, x, y, to_cosmic_color(color), None);
    painter.give_buffer(buffer);
}

fn measure_ui_text_width(
    font_system: &mut FontSystem,
    text: &str,
    size: f32,
    weight: Weight,
    family: &'static str,
) -> f32 {
    let lh = (size * 1.2).ceil();
    let metrics = Metrics::new(size, lh);
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_size(font_system, None, None);
    let attrs = Attrs::new().family(Family::Name(family)).weight(weight);
    buffer.set_text(font_system, text, attrs, Shaping::Advanced);
    buffer.shape_until_scroll(font_system, false);

    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0_f32, f32::max)
}

/// Multi-line document text with Thai tone-mark clearance (`ceil(size * 1.45 + 8px)`).
fn draw_doc_text(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    text: &str,
    x: f32,
    y: f32,
    max_width: f32,
    size: f32,
    weight: Weight,
    style: Style,
    color: Color,
    family: &'static str,
) -> f32 {
    let line_height = doc_line_height(size);
    let metrics = Metrics::new(size, line_height);
    let mut buffer = painter.take_buffer(font_system, metrics);
    buffer.set_wrap(font_system, Wrap::WordOrGlyph);
    buffer.set_size(font_system, Some(max_width.max(1.0)), None);

    let hinted = if family == MONO_FAMILIES {
        std::borrow::Cow::Borrowed(text)
    } else {
        thai::with_break_hints(text)
    };

    let attrs = Attrs::new()
        .family(Family::Name(family))
        .weight(weight)
        .style(style);

    buffer.set_text(font_system, &hinted, attrs, Shaping::Advanced);
    buffer.shape_until_scroll(font_system, false);

    blit_buffer(painter, pixmap, font_system, None, &buffer, x, y, to_cosmic_color(color), None);

    let line_count = buffer.layout_runs().count().max(1);
    painter.give_buffer(buffer);
    line_count as f32 * line_height
}

/// Unified Rich-Text Span Renderer using `Buffer::set_rich_text`.
/// Eliminates premature line breaks before bold/code/link spans and matches `doc_layout` heights 100%.
fn draw_rich_spans_with_search(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: &ThemePalette,
    spans: &[InlineSpan],
    search_query: &str,
    x: f32,
    y: f32,
    max_width: f32,
    base_size: f32,
    base_color: Color,
) -> f32 {
    let line_height = doc_line_height(base_size);
    if spans.is_empty() {
        return line_height;
    }

    let metrics = Metrics::new(base_size, line_height);
    let mut buffer = painter.take_buffer(font_system, metrics);
    buffer.set_wrap(font_system, Wrap::WordOrGlyph);
    buffer.set_size(font_system, Some(max_width.max(1.0)), None);

    struct PreparedSpan {
        text: String,
        color: CosmicColor,
        weight: Weight,
        style: Style,
        family: &'static str,
        meta: usize,
    }

    let sq_lower = search_query.trim().to_lowercase();
    let active_search = painter.frame_active_search;
    let cur_block = painter.current_block_idx;
    let mut local_match_counter = painter.block_match_counter;

    let mut push_with_search = |out: &mut Vec<PreparedSpan>,
                                raw_text: &str,
                                color: CosmicColor,
                                weight: Weight,
                                style: Style,
                                family: &'static str,
                                meta: usize| {
        if sq_lower.is_empty() {
            out.push(PreparedSpan {
                text: thai::with_break_hints(raw_text).into_owned(),
                color,
                weight,
                style,
                family,
                meta,
            });
            return;
        }

        let matches = find_case_insensitive_matches(raw_text, &sq_lower);
        if matches.is_empty() {
            out.push(PreparedSpan {
                text: thai::with_break_hints(raw_text).into_owned(),
                color,
                weight,
                style,
                family,
                meta,
            });
            return;
        }

        let mut cursor = 0usize;
        for (match_start, match_end) in matches {
            if match_start > cursor {
                out.push(PreparedSpan {
                    text: thai::with_break_hints(&raw_text[cursor..match_start]).into_owned(),
                    color,
                    weight,
                    style,
                    family,
                    meta,
                });
            }
            let local_idx = local_match_counter;
            local_match_counter += 1;
            let is_active = active_search == Some((cur_block, local_idx));
            out.push(PreparedSpan {
                text: thai::with_break_hints(&raw_text[match_start..match_end]).into_owned(),
                color: if is_active {
                    CosmicColor::rgba(15, 23, 42, 255)
                } else {
                    CosmicColor::rgba(255, 255, 255, 255)
                },
                weight: Weight::BOLD,
                style,
                family,
                meta: if is_active { META_SEARCH_ACTIVE } else { META_SEARCH_MATCH },
            });
            cursor = match_end;
        }
        if cursor < raw_text.len() {
            out.push(PreparedSpan {
                text: thai::with_break_hints(&raw_text[cursor..]).into_owned(),
                color,
                weight,
                style,
                family,
                meta,
            });
        }
    };

    let mut link_urls: Vec<String> = Vec::new();
    let mut prepared: Vec<PreparedSpan> = Vec::with_capacity(spans.len() + 2);
    for span in spans {
        match span {
            InlineSpan::Text(t) => push_with_search(
                &mut prepared,
                t,
                to_cosmic_color(base_color),
                Weight::NORMAL,
                Style::Normal,
                BODY_FAMILIES,
                META_NORMAL,
            ),
            InlineSpan::Bold(t) => push_with_search(
                &mut prepared,
                t,
                to_cosmic_color(pal.text_heading),
                Weight::BOLD,
                Style::Normal,
                BODY_FAMILIES,
                META_NORMAL,
            ),
            InlineSpan::Italic(t) => push_with_search(
                &mut prepared,
                t,
                to_cosmic_color(pal.text_secondary),
                Weight::NORMAL,
                Style::Italic,
                BODY_FAMILIES,
                META_NORMAL,
            ),
            InlineSpan::BoldItalic(t) => push_with_search(
                &mut prepared,
                t,
                to_cosmic_color(pal.text_heading),
                Weight::BOLD,
                Style::Italic,
                BODY_FAMILIES,
                META_NORMAL,
            ),
            InlineSpan::InlineCode(t) => prepared.push(PreparedSpan {
                text: format!(" {} ", t),
                color: to_cosmic_color(pal.code_red),
                weight: Weight::NORMAL,
                style: Style::Normal,
                family: MONO_FAMILIES,
                meta: META_INLINE_CODE,
            }),
            InlineSpan::Link { text, url } => {
                let link_idx = link_urls.len();
                link_urls.push(url.clone());
                let is_link_hovered = painter.frame_hovered_link.as_deref() == Some(url.as_str());
                push_with_search(
                    &mut prepared,
                    text,
                    to_cosmic_color(if is_link_hovered { pal.accent_cyan } else { pal.accent }),
                    Weight::MEDIUM,
                    Style::Normal,
                    BODY_FAMILIES,
                    1000 + link_idx,
                );
            }
            InlineSpan::InlineMath(latex) => prepared.push(PreparedSpan {
                text: latex_to_unicode(latex),
                color: to_cosmic_color(pal.accent_cyan),
                weight: Weight::NORMAL,
                style: Style::Italic,
                family: BODY_FAMILIES,
                meta: META_NORMAL,
            }),
            InlineSpan::Image { alt, url, link_url } => {
                let target = link_url.as_deref().unwrap_or(url.as_str());
                let link_idx = link_urls.len();
                link_urls.push(target.to_string());
                let is_link_hovered = painter.frame_hovered_link.as_deref() == Some(target);
                push_with_search(
                    &mut prepared,
                    alt,
                    to_cosmic_color(if is_link_hovered { pal.accent_cyan } else { pal.accent }),
                    Weight::MEDIUM,
                    Style::Normal,
                    BODY_FAMILIES,
                    1000 + link_idx,
                );
            }
        }
    }
    painter.block_match_counter = local_match_counter;

    let default_attrs = Attrs::new()
        .family(Family::Name(BODY_FAMILIES))
        .color(to_cosmic_color(base_color));

    buffer.set_rich_text(
        font_system,
        prepared.iter().map(|p| {
            (
                p.text.as_str(),
                Attrs::new()
                    .family(Family::Name(p.family))
                    .weight(p.weight)
                    .style(p.style)
                    .color(p.color)
                    .metadata(p.meta),
            )
        }),
        default_attrs,
        Shaping::Advanced,
    );
    buffer.shape_until_scroll(font_system, false);

    blit_buffer(
        painter,
        pixmap,
        font_system,
        Some(pal),
        &buffer,
        x,
        y,
        to_cosmic_color(base_color),
        Some(&link_urls),
    );

    let line_count = buffer.layout_runs().count().max(1);
    painter.give_buffer(buffer);
    line_count as f32 * line_height
}

fn to_cosmic_color(c: Color) -> CosmicColor {
    CosmicColor::rgba(
        (c.red() * 255.0) as u8,
        (c.green() * 255.0) as u8,
        (c.blue() * 255.0) as u8,
        (c.alpha() * 255.0) as u8,
    )
}

fn blit_buffer(
    painter: &mut Painter,
    pixmap: &mut PixmapMut,
    font_system: &mut FontSystem,
    pal: Option<&ThemePalette>,
    buffer: &Buffer,
    origin_x: f32,
    origin_y: f32,
    color: CosmicColor,
    link_urls: Option<&[String]>,
) {
    let vh = pixmap.height() as f32;
    for run in buffer.layout_runs() {
        let line_top = origin_y + run.line_top;
        let line_bottom = line_top + run.line_height;
        if line_bottom < 0.0 || line_top > vh {
            continue;
        }

        let line_y = origin_y + run.line_y;

        // Pass 1: Draw InlineCode pills, Search Highlight pills & Link underlines using glyph metadata
        if let Some(pal) = pal {
            let mut i = 0;
            let glyphs = run.glyphs;
            while i < glyphs.len() {
                let meta = glyphs[i].metadata;
                if meta == META_INLINE_CODE
                    || meta == META_LINK
                    || meta == META_SEARCH_MATCH
                    || meta == META_SEARCH_ACTIVE
                    || meta >= 1000
                {
                    let start_x = origin_x + glyphs[i].x;
                    let mut end_x = start_x + glyphs[i].w;
                    let mut j = i + 1;
                    while j < glyphs.len() && glyphs[j].metadata == meta {
                        end_x = origin_x + glyphs[j].x + glyphs[j].w;
                        j += 1;
                    }
                    let span_w = (end_x - start_x).max(4.0);
                    if meta == META_INLINE_CODE {
                        let pill_h = (run.line_height * 0.70).max(18.0);
                        let pill_y = origin_y + run.line_top + (run.line_height - pill_h) * 0.5 + 1.0;
                        fill_rounded_rect(pixmap, start_x + 2.0, pill_y, (span_w - 4.0).max(6.0), pill_h, 4.0, pal.bg_tertiary);
                        stroke_rounded_rect(pixmap, start_x + 2.0, pill_y, (span_w - 4.0).max(6.0), pill_h, 4.0, pal.border_default, 1.0);
                    } else if meta == META_SEARCH_ACTIVE {
                        let pill_h = (run.line_height * 0.76).max(19.0);
                        let pill_y = origin_y + run.line_top + (run.line_height - pill_h) * 0.5 + 1.0;
                        fill_rounded_rect(
                            pixmap,
                            start_x - 4.0,
                            pill_y - 1.5,
                            span_w + 8.0,
                            pill_h + 3.0,
                            5.0,
                            Color::from_rgba8(245, 158, 11, 72),
                        );
                        fill_rounded_rect(
                            pixmap,
                            start_x - 2.5,
                            pill_y,
                            span_w + 5.0,
                            pill_h,
                            4.0,
                            Color::from_rgba8(251, 191, 36, 245), // Active match bright amber-gold
                        );
                        stroke_rounded_rect(
                            pixmap,
                            start_x - 2.5,
                            pill_y,
                            span_w + 5.0,
                            pill_h,
                            4.0,
                            Color::WHITE,
                            1.4,
                        );
                    } else if meta == META_SEARCH_MATCH {
                        let pill_h = (run.line_height * 0.72).max(18.0);
                        let pill_y = origin_y + run.line_top + (run.line_height - pill_h) * 0.5 + 1.0;
                        fill_rounded_rect(
                            pixmap,
                            start_x - 2.0,
                            pill_y,
                            span_w + 4.0,
                            pill_h,
                            4.0,
                            Color::from_rgba8(210, 153, 34, 195), // Passive match amber
                        );
                        stroke_rounded_rect(
                            pixmap,
                            start_x - 2.0,
                            pill_y,
                            span_w + 4.0,
                            pill_h,
                            4.0,
                            pal.accent,
                            1.0,
                        );
                    } else if meta == META_LINK || meta >= 1000 {
                        let mut is_hovered = false;
                        if meta >= 1000 {
                            if let Some(urls) = link_urls {
                                let url_idx = meta - 1000;
                                if let Some(url) = urls.get(url_idx) {
                                    is_hovered = painter.frame_hovered_link.as_deref() == Some(url.as_str());
                                    painter.link_boxes.push(LinkHitBox {
                                        x: start_x - 1.0,
                                        y: origin_y + run.line_top,
                                        w: span_w + 2.0,
                                        h: run.line_height,
                                        url: url.clone(),
                                    });
                                }
                            }
                        }
                        if is_hovered {
                            let pill_h = (run.line_height * 0.74).max(18.0);
                            let pill_y = origin_y + run.line_top + (run.line_height - pill_h) * 0.5 + 1.0;
                            fill_rounded_rect(
                                pixmap,
                                start_x - 3.0,
                                pill_y,
                                span_w + 6.0,
                                pill_h,
                                4.0,
                                with_alpha(pal.accent_cyan, 0.16),
                            );
                            fill_rect(pixmap, start_x, line_y + 2.5, span_w, 2.0, pal.accent_cyan);
                        } else {
                            fill_rect(pixmap, start_x, line_y + 3.0, span_w, 1.0, with_alpha(pal.accent, 0.78));
                        }
                    }
                    i = j;
                } else {
                    i += 1;
                }
            }
        }

        // Pass 2: Blit shaped glyphs
        for glyph in run.glyphs.iter() {
            let physical = glyph.physical((origin_x, line_y), 1.0);
            let glyph_color = glyph.color_opt.unwrap_or(color);

            if let Some(image) = painter.swash_cache.get_image(font_system, physical.cache_key) {
                let img_w = image.placement.width as i32;
                let img_h = image.placement.height as i32;
                if img_w == 0 || img_h == 0 {
                    continue;
                }

                match image.content {
                    SwashContent::Mask | SwashContent::SubpixelMask => blit_mask_glyph(
                        pixmap,
                        &image.data,
                        img_w,
                        img_h,
                        physical.x + image.placement.left,
                        physical.y - image.placement.top,
                        glyph_color,
                    ),
                    SwashContent::Color => blit_color_glyph(
                        pixmap,
                        &image.data,
                        img_w,
                        img_h,
                        physical.x + image.placement.left,
                        physical.y - image.placement.top,
                        glyph_color.a(),
                    ),
                }
            }
        }
    }
}

fn blit_mask_glyph(
    pixmap: &mut PixmapMut,
    data: &[u8],
    w: i32,
    h: i32,
    origin_x: i32,
    origin_y: i32,
    color: CosmicColor,
) {
    let src_color = Color::from_rgba8(color.r(), color.g(), color.b(), color.a());
    for row in 0..h {
        for col in 0..w {
            let idx = (row * w + col) as usize;
            if idx >= data.len() {
                continue;
            }
            let coverage = data[idx];
            if coverage == 0 {
                continue;
            }
            blend_pixel(pixmap, origin_x + col, origin_y + row, src_color, coverage);
        }
    }
}

fn blit_color_glyph(
    pixmap: &mut PixmapMut,
    data: &[u8],
    w: i32,
    h: i32,
    origin_x: i32,
    origin_y: i32,
    alpha_scale: u8,
) {
    if alpha_scale == 0 {
        return;
    }
    for row in 0..h {
        for col in 0..w {
            let idx = ((row * w + col) * 4) as usize;
            if idx + 3 >= data.len() {
                continue;
            }
            let (r, g, b, a) = (data[idx], data[idx + 1], data[idx + 2], data[idx + 3]);
            if a == 0 {
                continue;
            }
            let eff_a = ((a as u16 * alpha_scale as u16) / 255) as u8;
            blend_pixel(
                pixmap,
                origin_x + col,
                origin_y + row,
                Color::from_rgba8(r, g, b, 255),
                eff_a,
            );
        }
    }
}

fn blend_pixel(pixmap: &mut PixmapMut, x: i32, y: i32, color: Color, coverage: u8) {
    let w = pixmap.width() as i32;
    let h = pixmap.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h || coverage == 0 {
        return;
    }

    let idx = (y * w + x) as usize;
    let pixels = pixmap.pixels_mut();
    let dst = pixels[idx];

    let cov = coverage as f32 / 255.0;
    let sa = color.alpha() * cov;
    let sr = color.red() * sa;
    let sg = color.green() * sa;
    let sb = color.blue() * sa;

    let da = dst.alpha() as f32 / 255.0;
    let dr = dst.red() as f32 / 255.0;
    let dg = dst.green() as f32 / 255.0;
    let db = dst.blue() as f32 / 255.0;

    let out_a = sa + da * (1.0 - sa);
    let out_r = sr + dr * (1.0 - sa);
    let out_g = sg + dg * (1.0 - sa);
    let out_b = sb + db * (1.0 - sa);

    if let Some(px) = PremultipliedColorU8::from_rgba(
        (out_r.clamp(0.0, 1.0) * 255.0) as u8,
        (out_g.clamp(0.0, 1.0) * 255.0) as u8,
        (out_b.clamp(0.0, 1.0) * 255.0) as u8,
        (out_a.clamp(0.0, 1.0) * 255.0) as u8,
    ) {
        pixels[idx] = px;
    }
}

// =============================================================================
// VECTOR DRAWING PRIMITIVES
// =============================================================================

fn fill_rect(pixmap: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, color: Color) {
    if let Some(rect) = Rect::from_xywh(x, y, w.max(0.0), h.max(0.0)) {
        let path = PathBuilder::from_rect(rect);
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn stroke_rounded_rect(pixmap: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color, width: f32) {
    if let Some(path) = rounded_rect_path(x, y, w, h, r) {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let mut stroke = Stroke::default();
        stroke.width = width;
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn stroke_dashed_rounded_rect_phased(
    pixmap: &mut PixmapMut,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    color: Color,
    width: f32,
    dash_len: f32,
    gap_len: f32,
    phase: f32,
) {
    if let Some(path) = rounded_rect_path(x, y, w, h, r) {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let mut stroke = Stroke::default();
        stroke.width = width;
        stroke.dash = StrokeDash::new(vec![dash_len, gap_len], phase);
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn fill_rounded_rect(pixmap: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color) {
    if let Some(path) = rounded_rect_path(x, y, w, h, r) {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_circle(pixmap: &mut PixmapMut, cx: f32, cy: f32, r: f32, color: Color) {
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// Translates common LaTeX math notation to readable Unicode symbols for inline math.
pub fn latex_to_unicode(latex: &str) -> String {
    let mut s = latex.trim().to_string();
    if s.starts_with('$') && s.ends_with('$') && s.len() >= 2 {
        s = s[1..s.len() - 1].trim().to_string();
    }

    let replacements: &[(&str, &str)] = &[
        // Greek letters (lowercase)
        (r"\alpha", "α"), (r"\beta", "β"), (r"\gamma", "γ"), (r"\delta", "δ"),
        (r"\epsilon", "ε"), (r"\varepsilon", "ε"), (r"\zeta", "ζ"), (r"\eta", "η"),
        (r"\theta", "θ"), (r"\iota", "ι"), (r"\kappa", "κ"), (r"\lambda", "λ"),
        (r"\mu", "μ"), (r"\nu", "ν"), (r"\xi", "ξ"), (r"\pi", "π"),
        (r"\rho", "ρ"), (r"\sigma", "σ"), (r"\tau", "τ"), (r"\upsilon", "υ"),
        (r"\phi", "φ"), (r"\varphi", "ϕ"), (r"\chi", "χ"), (r"\psi", "ψ"), (r"\omega", "ω"),
        // Greek letters (uppercase)
        (r"\Gamma", "Γ"), (r"\Delta", "Δ"), (r"\Theta", "Θ"), (r"\Lambda", "Λ"),
        (r"\Xi", "Ξ"), (r"\Pi", "Π"), (r"\Sigma", "Σ"), (r"\Phi", "Φ"),
        (r"\Psi", "Ψ"), (r"\Omega", "Ω"),
        // Relational and arithmetic operators
        (r"\leq", "≤"), (r"\le", "≤"), (r"\geq", "≥"), (r"\ge", "≥"),
        (r"\neq", "≠"), (r"\ne", "≠"), (r"\approx", "≈"), (r"\equiv", "≡"),
        (r"\times", "×"), (r"\div", "÷"), (r"\cdot", "·"), (r"\pm", "±"), (r"\mp", "∓"),
        // Logic and set symbols
        (r"\in", "∈"), (r"\notin", "∉"), (r"\subset", "⊂"), (r"\subseteq", "⊆"),
        (r"\cap", "∩"), (r"\cup", "∪"), (r"\forall", "∀"), (r"\exists", "∃"),
        // Arrows and calculus
        (r"\rightarrow", "→"), (r"\to", "→"), (r"\leftarrow", "←"),
        (r"\Rightarrow", "⇒"), (r"\Leftarrow", "⇐"), (r"\iff", "⇔"),
        (r"\infty", "∞"), (r"\partial", "∂"), (r"\nabla", "∇"),
        (r"\sum", "∑"), (r"\prod", "∏"), (r"\int", "∫"), (r"\sqrt", "√"),
        (r"\dots", "…"), (r"\cdots", "⋯"), (r"\quad", "  "), (r"\qquad", "    "), (r"\,", " "),
        // Superscripts
        ("^0", "⁰"), ("^1", "¹"), ("^2", "²"), ("^3", "³"), ("^4", "⁴"),
        ("^5", "⁵"), ("^6", "⁶"), ("^7", "⁷"), ("^8", "⁸"), ("^9", "⁹"),
        ("^+", "⁺"), ("^-", "⁻"), ("^n", "ⁿ"), ("^i", "ⁱ"), ("^x", "ˣ"),
        // Subscripts
        ("_0", "₀"), ("_1", "₁"), ("_2", "₂"), ("_3", "₃"), ("_4", "₄"),
        ("_5", "₅"), ("_6", "₆"), ("_7", "₇"), ("_8", "₈"), ("_9", "₉"),
        ("_+", "₊"), ("_-", "₋"), ("_i", "ᵢ"), ("_j", "ⱼ"), ("_k", "ₖ"),
        ("_n", "ₙ"), ("_m", "ₘ"), ("_t", "ₜ"),
    ];

    for (from, to) in replacements {
        s = s.replace(from, to);
    }
    // Clean remaining structural braces like {x} -> x
    s = s.replace('{', "").replace('}', "");
    s
}


#[cfg(test)]
mod tests_verified {
    use super::*;

    #[test]
    fn test_latex_to_unicode_conversion() {
        assert_eq!(latex_to_unicode(r"\rho(A) < 1"), "ρ(A) < 1");
        assert_eq!(latex_to_unicode(r"E = mc^2"), "E = mc²");
        assert_eq!(latex_to_unicode(r"\alpha + \beta \le \gamma"), "α + β ≤ γ");
        assert_eq!(latex_to_unicode(r"x_i \neq y_j"), "xᵢ ≠ yⱼ");
    }

    #[test]
    fn test_viewport_width_spring_target() {
        let mut opts = ViewOptions::default();
        opts.last_viewport_w = 1920.0;
        opts.width_mode = ReadingWidthMode::Normal;
        let target_norm = opts.target_content_width(opts.last_viewport_w);
        assert_eq!(target_norm, 880.0);

        opts.width_mode = ReadingWidthMode::Wide;
        let target_wide = opts.target_content_width(opts.last_viewport_w);
        assert_eq!(target_wide, 1120.0);
    }

    #[test]
    fn test_scroll_spy_binary_search_on_massive_document() {
        use doc_layout::ast::MarkdownBlock;
        use doc_layout::layout::{BlockGeometry, LayoutConfig};
        use std::cell::Cell;
        use std::collections::HashMap;

        let num_headings = 10_000;
        let mut geometries = Vec::with_capacity(num_headings);
        let mut blocks = Vec::with_capacity(num_headings);
        let mut cached_headings = Vec::with_capacity(num_headings);

        for i in 0..num_headings {
            geometries.push(BlockGeometry {
                block_index: i,
                y_offset: i as f32 * 100.0,
                height: 40.0,
                width: 800.0,
            });
            blocks.push(MarkdownBlock::Heading {
                level: 2,
                id: format!("heading-{}", i),
                text: format!("Heading {}", i),
            });
            cached_headings.push(HeadingItem {
                block_idx: i,
                level: 2,
                text: format!("Heading {}", i),
            });
        }

        let layout = DocumentLayout {
            blocks,
            geometries,
            exact_measured: vec![true; num_headings],
            total_height: num_headings as f32 * 100.0,
            content_width: 800.0,
            config: LayoutConfig::default(),
            table_layouts: HashMap::new(),
            list_layouts: HashMap::new(),
            unmeasured_cursor: Cell::new(num_headings),
        };

        let mut opts = ViewOptions::default();
        opts.cached_headings = cached_headings;

        // Top: scroll_y = 0.0 -> target_y = 68.0 -> ordinal 0
        opts.update_scroll_spy(&layout, 0.0);
        assert_eq!(opts.active_heading_ordinal, Some(0));

        // Mid-point: scroll_y = 500_000.0 -> target_y = 500_068.0 -> ordinal 5000
        opts.update_scroll_spy(&layout, 500_000.0);
        assert_eq!(opts.active_heading_ordinal, Some(5000));

        // Deep end: scroll_y = 999_900.0 -> ordinal 9999
        opts.update_scroll_spy(&layout, 999_900.0);
        assert_eq!(opts.active_heading_ordinal, Some(9999));
    }

    #[test]
    fn test_default_md_button_auto_hides_when_active() {
        let mut painter = Painter::new();
        let mut pixmap = Pixmap::new(1280, 800).expect("pixmap");
        let mut font_system = crate::bridge::SharedFonts::new().into_font_system();
        let pal = ThemePalette::for_mode(ThemeMode::DarkNoir);

        let mut opts = ViewOptions::default();
        opts.default_md_state = DefaultAssociationState::NotRegistered;
        painter.ui_boxes.clear();
        draw_welcome_screen(&mut painter, &mut pixmap.as_mut(), &mut font_system, &pal, &opts, 1280.0, 800.0);
        assert!(painter.ui_boxes.iter().any(|b| b.target == HoverTarget::SetDefaultMdBtn));

        opts.default_md_state = DefaultAssociationState::DefaultActive;
        painter.ui_boxes.clear();
        draw_welcome_screen(&mut painter, &mut pixmap.as_mut(), &mut font_system, &pal, &opts, 1280.0, 800.0);
        assert!(!painter.ui_boxes.iter().any(|b| b.target == HoverTarget::SetDefaultMdBtn));

        opts.about_open = true;
        opts.modal_anim = 1.0;
        painter.ui_boxes.clear();
        draw_about_modal(&mut painter, &mut pixmap.as_mut(), &mut font_system, &pal, &opts, 1.0, 1280.0, 800.0);
        assert!(!painter.ui_boxes.iter().any(|b| b.target == HoverTarget::SetDefaultMdBtn));
        assert_eq!(painter.modal_card_rect.map(|(_, _, _, h)| h.round() as i32), Some(292));

        opts.default_md_state = DefaultAssociationState::NeedsWindowsConfirmation;
        painter.ui_boxes.clear();
        draw_about_modal(&mut painter, &mut pixmap.as_mut(), &mut font_system, &pal, &opts, 1.0, 1280.0, 800.0);
        assert!(painter.ui_boxes.iter().any(|b| b.target == HoverTarget::SetDefaultMdBtn));
        assert_eq!(painter.modal_card_rect.map(|(_, _, _, h)| h.round() as i32), Some(344));
    }
}
