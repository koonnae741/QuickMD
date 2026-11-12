#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! main.rs — QuickMD 3.0. Pure Win32 native event loop.
//! ZERO WebView2, ZERO Chromium, ZERO HTML/CSS/JS.

use quickmd_lib::windows_assoc::{self, DefaultAssociationState};

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::keyboard::{Key, KeyCode, ModifiersState};
use tao::window::{CursorIcon, Icon, WindowBuilder};

use tiny_skia::Pixmap;

use doc_layout::ast::MarkdownBlock;
use doc_layout::{slugify, DocumentLayoutEngine, LayoutConfig};
use native_render::native_mermaid::DiagramTheme;

use quickmd_lib::bridge::{self, preprocess_markdown, DarkDiagramTheme, RealEmbedMeasurer, SharedFonts};
use quickmd_lib::painter::{
    self, HoverTarget, Painter, TextSelection, ViewOptions, CONTENT_TOP_PADDING, HEADER_HEIGHT, STATUS_BAR_HEIGHT,
};


// =============================================================================
// WIN32 IMMERSIVE TITLEBAR SYNC (Dark / Light DWM Sync)
// =============================================================================

#[cfg(target_os = "windows")]
fn set_window_dark_mode(window: &tao::window::Window, dark: bool) {
    use tao::platform::windows::WindowExtWindows;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    let value: i32 = if dark { 1 } else { 0 };
    unsafe {
        let hwnd = window.hwnd() as HWND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const i32 as *const std::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn set_window_dark_mode(_window: &tao::window::Window, _dark: bool) {}

fn load_icon() -> Option<Icon> {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/icons/logo_32.rgba");
    Icon::from_rgba(ICON_BYTES.to_vec(), 32, 32).ok()
}

// =============================================================================
// APP STATE
// =============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct NavLocation {
    pub path: Option<PathBuf>,
    pub is_welcome: bool,
    pub scroll_y: f32,
}

struct AppState {
    engine: DocumentLayoutEngine,
    embed_measurer: RealEmbedMeasurer,
    painter: Painter,
    current_path: Option<PathBuf>,
    raw_markdown: String,
    show_welcome: bool,
    view_opts: ViewOptions,
    modifiers: ModifiersState,
    last_frame: Instant,
    was_animating: bool,
    next_caret_blink: Instant,
    cursor_pos: (f32, f32),
    toast_until: Option<Instant>,
    copied_until: Option<Instant>,
    last_surface_size: (u32, u32),
    /// Ordered `(block_idx, local_match_idx, block_y_offset)` for active search navigation.
    search_matches_list: Vec<(usize, usize, f32)>,
    nav_back: Vec<NavLocation>,
    nav_forward: Vec<NavLocation>,
    selection_anchor: Option<(usize, usize)>,
    selection_focus: Option<(usize, usize)>,
    is_mouse_selecting: bool,
    selected_text: String,
    frame_pixmap: Option<Pixmap>,
}

impl AppState {
    fn current_title(&self) -> String {
        if self.show_welcome {
            "QuickMD 3.0".to_string()
        } else if let Some(p) = &self.current_path {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("document");
            format!("QuickMD 3.0 — {}", name)
        } else {
            "QuickMD 3.0".to_string()
        }
    }

    fn default_layout_config(scale: f32) -> LayoutConfig {
        let mut cfg = LayoutConfig::default();
        cfg.wrap_code = true;
        cfg.base_font_size = (16.0 * scale).round();
        cfg.code_font_size = (14.0 * scale).round();
        // Extra vertical padding so the 28px .code-header bar fits comfortably above code text
        cfg.code_padding = (22.0 * scale).round();
        cfg
    }

    fn set_markdown_content(&mut self, content: &str, path: Option<PathBuf>, vw: f32, vh: f32) {
        if let Some(ref p) = path {
            if let Some(parent) = p.parent() {
                self.embed_measurer.set_base_dir(parent.to_path_buf());
            }
        }
        let processed = preprocess_markdown(content);
        self.raw_markdown = content.to_string();

        let mut chars = 0usize;
        let mut words = 0usize;
        let mut lines = 0usize;
        let mut in_word = false;
        for c in content.chars() {
            chars += 1;
            if c == '\n' {
                lines += 1;
            }
            if c.is_whitespace() {
                in_word = false;
            } else if !in_word {
                in_word = true;
                words += 1;
            }
        }
        self.view_opts.raw_char_count = chars;
        self.view_opts.raw_word_count = words;
        self.view_opts.raw_line_count = lines.max(1);

        self.current_path = path;
        self.show_welcome = false;
        self.clear_selection();
        self.view_opts.active_heading_ordinal = None;
        self.view_opts.outline_active_row.snap(-1.0);
        self.view_opts.hovered_table_idx = None;
        self.embed_measurer.clear_caches();

        let content_w = self.view_opts.target_content_width(vw);
        let scale = self.view_opts.zoom_pct as f32 / 100.0;
        self.engine.set_layout_config(Self::default_layout_config(scale));
        let search_h = if self.view_opts.search_open { 44.0 } else { 0.0 };
        let doc_vh = (vh - HEADER_HEIGHT - STATUS_BAR_HEIGHT - search_h - CONTENT_TOP_PADDING * 2.0)
            .max(100.0);
        self.engine.set_viewport(content_w, doc_vh);
        self.engine.load_markdown(&processed);
        self.view_opts.rebuild_headings(self.engine.layout());
        self.view_opts
            .update_scroll_spy(self.engine.layout(), self.engine.scroll_y());

        if self.view_opts.search_open && !self.view_opts.search_query.trim().is_empty() {
            self.update_search();
        } else {
            self.search_matches_list.clear();
            self.view_opts.search_matches = 0;
            self.view_opts.search_current_idx = 0;
            self.view_opts.search_active_block = None;
        }
    }

    fn load_file(&mut self, path: PathBuf, vw: f32, vh: f32) -> bool {
        // SEC-02: Prevent unbounded memory allocation from oversized files (max 32MB)
        const MAX_MD_BYTES: u64 = 32 * 1024 * 1024;
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > MAX_MD_BYTES {
                self.show_toast("File too large (>32MB)");
                return false;
            }
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return false;
        };
        let Some((content, encoding)) = decode_text_bytes(&bytes) else {
            return false;
        };
        self.view_opts.encoding_label = encoding;
        self.set_markdown_content(&content, Some(path), vw, vh);
        true
    }

    fn push_nav_history(&mut self) {
        let loc = NavLocation {
            path: self.current_path.clone(),
            is_welcome: self.show_welcome,
            scroll_y: self.engine.scroll_y(),
        };
        if let Some(last) = self.nav_back.last() {
            if last.path == loc.path && last.is_welcome == loc.is_welcome && (last.scroll_y - loc.scroll_y).abs() < 40.0 {
                return;
            }
        }
        self.nav_back.push(loc);
        self.nav_forward.clear();
        self.update_nav_state();
    }

    fn update_nav_state(&mut self) {
        self.view_opts.can_nav_back = !self.nav_back.is_empty();
        self.view_opts.can_nav_forward = !self.nav_forward.is_empty();
    }

    fn go_back(&mut self, vw: f32, vh: f32) -> bool {
        if let Some(target) = self.nav_back.pop() {
            let current = NavLocation {
                path: self.current_path.clone(),
                is_welcome: self.show_welcome,
                scroll_y: self.engine.scroll_y(),
            };
            self.nav_forward.push(current);
            self.restore_nav_location(target, vw, vh);
            self.update_nav_state();
            self.show_toast("Navigated Back");
            true
        } else {
            false
        }
    }

    fn go_forward(&mut self, vw: f32, vh: f32) -> bool {
        if let Some(target) = self.nav_forward.pop() {
            let current = NavLocation {
                path: self.current_path.clone(),
                is_welcome: self.show_welcome,
                scroll_y: self.engine.scroll_y(),
            };
            self.nav_back.push(current);
            self.restore_nav_location(target, vw, vh);
            self.update_nav_state();
            self.show_toast("Navigated Forward");
            true
        } else {
            false
        }
    }

    fn restore_nav_location(&mut self, loc: NavLocation, vw: f32, vh: f32) {
        if loc.is_welcome {
            self.show_welcome = true;
            self.current_path = None;
            self.view_opts.outline_open = false;
            self.clear_selection();
        } else if let Some(p) = loc.path {
            if self.current_path.as_ref() != Some(&p) {
                let _ = self.load_file(p, vw, vh);
            }
            self.show_welcome = false;
            self.engine.scroll_to(loc.scroll_y);
            self.view_opts.update_scroll_spy(self.engine.layout(), loc.scroll_y);
            self.clear_selection();
        } else {
            self.show_welcome = false;
            self.engine.scroll_to(loc.scroll_y);
            self.view_opts.update_scroll_spy(self.engine.layout(), loc.scroll_y);
            self.clear_selection();
        }
    }

    fn clear_selection(&mut self) {
        self.selection_anchor = None;
        self.selection_focus = None;
        self.selected_text.clear();
        self.view_opts.text_selection = None;
    }

    fn select_all(&mut self) {
        let block_count = self.engine.layout().blocks.len();
        if block_count > 0 {
            self.selection_anchor = Some((0, 0));
            self.selection_focus = Some((block_count - 1, 999999));
            self.selected_text = self.extract_text_range(0, block_count - 1);
            self.view_opts.text_selection = Some(TextSelection {
                is_active: true,
                start_block: 0,
                start_char: 0,
                end_block: block_count - 1,
                end_char: 999999,
                selected_text: self.selected_text.clone(),
            });
            self.show_toast("Selected All Text (Ctrl+C to copy)");
        }
    }

    fn extract_text_range(&self, start_b: usize, end_b: usize) -> String {
        let mut out = String::new();
        let blocks = &self.engine.layout().blocks;
        for i in start_b..=end_b.min(blocks.len().saturating_sub(1)) {
            if let Some(b) = blocks.get(i) {
                let txt = match b {
                    doc_layout::MarkdownBlock::Heading { text, .. } => text.clone(),
                    doc_layout::MarkdownBlock::Paragraph(spans) => spans.iter().map(|s| s.text()).collect::<String>(),
                    doc_layout::MarkdownBlock::Blockquote(spans) => spans.iter().map(|s| s.text()).collect::<String>(),
                    doc_layout::MarkdownBlock::CodeBlock { code, .. } => code.clone(),
                    doc_layout::MarkdownBlock::List { items, .. } => items.iter().map(|it| it.iter().map(|s| s.text()).collect::<String>()).collect::<Vec<_>>().join("\n"),
                    doc_layout::MarkdownBlock::CheckList(items) => items.iter().map(|it| it.1.iter().map(|s| s.text()).collect::<String>()).collect::<Vec<_>>().join("\n"),
                    doc_layout::MarkdownBlock::Table(t) => {
                        let mut tsv = t.headers.iter().map(|h| h.spans.iter().map(|s| s.text()).collect::<String>()).collect::<Vec<_>>().join("\t");
                        tsv.push('\n');
                        for row in &t.rows {
                            tsv.push_str(&row.iter().map(|c| c.spans.iter().map(|s| s.text()).collect::<String>()).collect::<Vec<_>>().join("\t"));
                            tsv.push('\n');
                        }
                        tsv
                    }
                    _ => String::new(),
                };
                if !txt.is_empty() {
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(&txt);
                }
            }
        }
        out
    }

    fn find_block_at_y(&self, screen_y: f32) -> Option<usize> {
        let scroll_y = self.engine.scroll_y();
        let search_shift = 44.0 * self.view_opts.effective_search_anim();
        let top_offset = HEADER_HEIGHT + search_shift;
        let vbs = self.engine.visible_blocks();
        if vbs.is_empty() {
            return None;
        }

        let first = &vbs[0];
        let first_y = top_offset + CONTENT_TOP_PADDING + first.screen_y(scroll_y);
        if screen_y < first_y {
            return Some(first.block_index);
        }

        let last = &vbs[vbs.len() - 1];
        let last_y = top_offset + CONTENT_TOP_PADDING + last.screen_y(scroll_y) + last.height;
        if screen_y > last_y {
            return Some(last.block_index);
        }

        let mut closest_idx = first.block_index;
        let mut min_dist = f32::MAX;

        for vb in vbs {
            let by = top_offset + CONTENT_TOP_PADDING + vb.screen_y(scroll_y);
            let b_end = by + vb.height;
            if screen_y >= by && screen_y <= b_end {
                return Some(vb.block_index);
            }
            let dist = if screen_y < by {
                by - screen_y
            } else {
                screen_y - b_end
            };
            if dist < min_dist {
                min_dist = dist;
                closest_idx = vb.block_index;
            }
        }
        Some(closest_idx)
    }


    fn sync_viewport(&mut self, vw: f32, vh: f32) {
        let content_w = self.view_opts.target_content_width(vw);
        let search_h = if self.view_opts.search_open { 44.0 } else { 0.0 };
        let doc_vh = (vh - HEADER_HEIGHT - STATUS_BAR_HEIGHT - search_h - CONTENT_TOP_PADDING * 2.0)
            .max(100.0);
        self.engine.set_viewport(content_w, doc_vh);
    }

    fn apply_zoom(&mut self, new_zoom: u32, vw: f32, vh: f32) {
        let clamped = new_zoom.clamp(60, 200);
        self.view_opts.zoom_pct = clamped;
        let scale = clamped as f32 / 100.0;
        self.embed_measurer.clear_caches();
        self.engine.set_layout_config(Self::default_layout_config(scale));
        self.sync_viewport(vw, vh);
        if self.view_opts.search_open && !self.view_opts.search_query.trim().is_empty() {
            self.update_search_preserve_idx();
        }
        self.show_toast(&format!("Zoom: {}%", clamped));
    }

    fn rebuild_search_matches(&mut self) {
        let q = self.view_opts.search_query.trim().to_lowercase();
        self.search_matches_list.clear();
        if q.is_empty() {
            self.view_opts.search_matches = 0;
            self.view_opts.search_current_idx = 0;
            self.view_opts.search_active_block = None;
            return;
        }
        let layout = self.engine.layout();
        for (block_idx, block) in layout.blocks.iter().enumerate() {
            let count = painter::count_block_search_matches(block, &q);
            if count > 0 {
                let y = layout
                    .geometries
                    .get(block_idx)
                    .map(|g| g.y_offset)
                    .unwrap_or(0.0);
                for local_idx in 0..count {
                    self.search_matches_list.push((block_idx, local_idx, y));
                }
            }
        }
        self.view_opts.search_matches = self.search_matches_list.len();
    }

    fn update_search(&mut self) {
        self.rebuild_search_matches();
        self.view_opts.search_current_idx = 0;
        if let Some(&(b_idx, l_idx, y)) = self.search_matches_list.first() {
            self.view_opts.search_active_block = Some((b_idx, l_idx));
            self.engine.scroll_to((y - 36.0).max(0.0));
        } else {
            self.view_opts.search_active_block = None;
        }
    }

    fn update_search_preserve_idx(&mut self) {
        let prev = self.view_opts.search_current_idx;
        self.rebuild_search_matches();
        if self.search_matches_list.is_empty() {
            self.view_opts.search_current_idx = 0;
            self.view_opts.search_active_block = None;
        } else {
            let idx = prev.min(self.search_matches_list.len() - 1);
            self.view_opts.search_current_idx = idx;
            let (b_idx, l_idx, _) = self.search_matches_list[idx];
            self.view_opts.search_active_block = Some((b_idx, l_idx));
        }
    }

    fn step_search_match(&mut self, forward: bool) {
        let total = self.search_matches_list.len();
        if total == 0 {
            self.view_opts.search_active_block = None;
            return;
        }
        if forward {
            self.view_opts.search_current_idx = (self.view_opts.search_current_idx + 1) % total;
        } else if self.view_opts.search_current_idx == 0 {
            self.view_opts.search_current_idx = total - 1;
        } else {
            self.view_opts.search_current_idx -= 1;
        }
        if let Some(&(b_idx, l_idx, y)) =
            self.search_matches_list.get(self.view_opts.search_current_idx)
        {
            self.view_opts.search_active_block = Some((b_idx, l_idx));
            self.engine.scroll_to((y - 36.0).max(0.0));
        }
    }

    fn cycle_theme(&mut self, window: &tao::window::Window) {
        let next = self.view_opts.theme.next();
        self.view_opts.switch_theme(next);
        set_window_dark_mode(window, !next.is_light());
        self.show_toast(&format!("Theme: {}", next.name()));
    }

    fn copy_text_to_clipboard(&mut self, text: &str, toast: &str, copied_block: Option<usize>) {
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if cb.set_text(text.to_string()).is_ok() {
                self.show_toast(toast);
                if let Some(idx) = copied_block {
                    self.view_opts.trigger_copy_feedback(idx);
                    self.copied_until = Some(Instant::now() + Duration::from_millis(1800));
                }
            }
        }
    }

    fn copy_raw_to_clipboard(&mut self) {
        if self.raw_markdown.is_empty() {
            self.show_toast("No document to copy");
            return;
        }
        let text = self.raw_markdown.clone();
        self.copy_text_to_clipboard(&text, "Copied Raw Markdown to Clipboard", None);
    }

    fn show_toast(&mut self, msg: &str) {
        self.view_opts.trigger_toast(msg.to_string());
        self.toast_until = Some(Instant::now() + Duration::from_millis(2200));
    }

    fn register_as_default_md(&mut self) {
        match windows_assoc::register_associations() {
            Ok(()) => {
                let status = windows_assoc::check_default_status();
                self.view_opts.default_md_state = status;
                match status {
                    DefaultAssociationState::DefaultActive => {
                        self.show_toast("QuickMD set as default for .md files");
                    }
                    DefaultAssociationState::NeedsWindowsConfirmation => {
                        if let Err(e) = windows_assoc::open_windows_default_apps() {
                            self.show_toast(&e);
                        } else {
                            self.show_toast("Registered QuickMD — select it for .md in Windows Settings");
                        }
                    }
                    DefaultAssociationState::NotRegistered => {
                        self.show_toast("Registered QuickMD for .md files");
                    }
                }
            }
            Err(e) => {
                self.show_toast(&format!("Failed to set default .md: {}", e));
            }
        }
    }

    fn scroll_from_scrollbar_y(&mut self, my: f32, vh: f32) {
        let track_h = (vh - HEADER_HEIGHT - STATUS_BAR_HEIGHT).max(1.0);
        let ratio = ((my - HEADER_HEIGHT) / track_h).clamp(0.0, 1.0);
        let total_h = self.engine.layout().total_height;
        let max_scroll = (total_h - self.engine.viewport_height()).max(0.0);
        let target_y = ratio * max_scroll;
        let delta = target_y - self.engine.scroll_y();
        self.engine.scroll_by(delta);
    }
}

// =============================================================================
// MAIN EVENT LOOP (Adaptive 144Hz Motion + 0.0% Idle CPU Scheduler)
// =============================================================================

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        if args[1] == "--register" {
            let _ = windows_assoc::register_associations();
            return;
        } else if args[1] == "--unregister" {
            let _ = windows_assoc::unregister_associations();
            return;
        }
    }

    let event_loop = EventLoop::new();

    let window = WindowBuilder::new()
        .with_title("QuickMD 3.0")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(560.0, 400.0))
        .with_window_icon(load_icon())
        .build(&event_loop)
        .expect("failed to create window");

    set_window_dark_mode(&window, true);

    let window = Arc::new(window);
    let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
    let mut surface =
        softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");

    let shared_fonts = SharedFonts::new();
    let math_font_ctx = shared_fonts.new_math_font_context();
    let mermaid_font_ctx = shared_fonts.new_math_font_context();
    let font_system = shared_fonts.into_font_system();

    let mut engine = DocumentLayoutEngine::new(font_system);
    let embed_measurer =
        RealEmbedMeasurer::new(math_font_ctx, mermaid_font_ctx, DiagramTheme::dark());
    engine.set_embed_measurer(embed_measurer.as_measurer());

    let now_init = Instant::now();
    let mut state = AppState {
        engine,
        embed_measurer,
        painter: Painter::new(),
        current_path: None,
        raw_markdown: String::new(),
        show_welcome: true,
        view_opts: {
            let mut opts = ViewOptions::default();
            opts.default_md_state = windows_assoc::check_default_status();
            opts
        },
        modifiers: ModifiersState::empty(),
        last_frame: now_init,
        was_animating: true,
        next_caret_blink: now_init + Duration::from_millis(500),
        cursor_pos: (-1000.0, -1000.0),
        toast_until: None,
        copied_until: None,
        last_surface_size: (0, 0),
        search_matches_list: Vec::new(),
        nav_back: Vec::new(),
        nav_forward: Vec::new(),
        selection_anchor: None,
        selection_focus: None,
        is_mouse_selecting: false,
        selected_text: String::new(),
        frame_pixmap: None,
    };

    let init_size = window.inner_size();
    let vw = init_size.width as f32;
    let vh = init_size.height as f32;

    let opened = if args.len() > 1 && !args[1].starts_with('-') {
        let clean_arg = args[1].trim_matches('"').trim();
        state.load_file(PathBuf::from(clean_arg), vw, vh)
    } else {
        false
    };
    if !opened {
        state
            .engine
            .set_layout_config(AppState::default_layout_config(1.0));
        state.sync_viewport(vw, vh);
        state.engine.load_markdown("");
        state.show_welcome = true;
        if args.len() > 1 && !args[1].starts_with('-') {
            state.show_toast("Unable to open file");
        }
    }
    window.set_title(&state.current_title());

    event_loop.run(move |event, _, control_flow| match event {
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                *control_flow = ControlFlow::Exit;
                std::process::exit(0);
            }

            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    if state.last_surface_size != (size.width, size.height) {
                        if let (Some(w), Some(h)) =
                            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                        {
                            let _ = surface.resize(w, h);
                        }
                        state.last_surface_size = (size.width, size.height);
                    }
                    state.sync_viewport(size.width as f32, size.height as f32);
                    window.request_redraw();
                }
            }

            WindowEvent::Focused(focused) => {
                if focused {
                    let latest = windows_assoc::check_default_status();
                    if state.view_opts.default_md_state != latest {
                        state.view_opts.default_md_state = latest;
                        window.request_redraw();
                    }
                } else if state.view_opts.file_drag_hover {
                    state.view_opts.file_drag_hover = false;
                    window.request_redraw();
                }
            }

            WindowEvent::ModifiersChanged(mods) => {
                state.modifiers = mods;
            }

            WindowEvent::HoveredFile(_) => {
                if !state.view_opts.file_drag_hover {
                    state.view_opts.file_drag_hover = true;
                    window.request_redraw();
                }
            }

            WindowEvent::HoveredFileCancelled => {
                if state.view_opts.file_drag_hover {
                    state.view_opts.file_drag_hover = false;
                    window.request_redraw();
                }
            }

            WindowEvent::DroppedFile(path) => {
                state.view_opts.file_drag_hover = false;
                let sz = window.inner_size();
                state.push_nav_history();
                if state.load_file(path.clone(), sz.width as f32, sz.height as f32) {
                    let fname = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Markdown");
                    state.show_toast(&format!("Opened {}", fname));
                } else {
                    state.show_toast("Failed to read dropped file");
                }
                window.set_title(&state.current_title());
                window.request_redraw();
            }

            WindowEvent::CursorLeft { .. } => {
                state.cursor_pos = (-1000.0, -1000.0);
                state.view_opts.cursor_pos = (-1000.0, -1000.0);
                state.view_opts.hovered_table_idx = None;
                if state.view_opts.hover != HoverTarget::None {
                    state.view_opts.hover = HoverTarget::None;
                    window.set_cursor_icon(CursorIcon::Default);
                }
                window.request_redraw();
            }

            WindowEvent::CursorMoved { position, .. } => {
                let pos = (position.x as f32, position.y as f32);
                state.cursor_pos = pos;
                state.view_opts.cursor_pos = pos;
                let sz = window.inner_size();
                let vh = sz.height as f32;

                if state.view_opts.diagram_lightbox.is_open && state.view_opts.diagram_lightbox.is_dragging {
                    let dx = pos.0 - state.view_opts.diagram_lightbox.drag_start_x;
                    let dy = pos.1 - state.view_opts.diagram_lightbox.drag_start_y;
                    state.view_opts.diagram_lightbox.pan_x = state.view_opts.diagram_lightbox.drag_start_pan_x + dx;
                    state.view_opts.diagram_lightbox.pan_y = state.view_opts.diagram_lightbox.drag_start_pan_y + dy;
                    window.request_redraw();
                    return;
                }

                if state.is_mouse_selecting && !state.show_welcome {
                    if let Some(b_idx) = state.find_block_at_y(pos.1) {
                        state.selection_focus = Some((b_idx, 0));
                        if let Some((start_b, _)) = state.selection_anchor {
                            let (min_b, max_b) = if start_b <= b_idx { (start_b, b_idx) } else { (b_idx, start_b) };
                            state.selected_text = state.extract_text_range(min_b, max_b);
                            state.view_opts.text_selection = Some(TextSelection {
                                is_active: true,
                                start_block: min_b,
                                start_char: 0,
                                end_block: max_b,
                                end_char: 999999,
                                selected_text: state.selected_text.clone(),
                            });
                            window.request_redraw();
                        }
                    }
                }

                if state.view_opts.scrollbar_dragging && !state.show_welcome {
                    state.scroll_from_scrollbar_y(pos.1, vh);
                    window.request_redraw();
                } else {
                    let new_hover = state.painter.hit_test(pos.0, pos.1, &state.view_opts, vh);
                    if new_hover != state.view_opts.hover {
                        state.view_opts.hover = new_hover;
                        window.set_cursor_icon(if state.view_opts.diagram_lightbox.is_open {
                            if state.view_opts.diagram_lightbox.is_dragging {
                                CursorIcon::Grabbing
                            } else if state.view_opts.hover.uses_hand_cursor() {
                                CursorIcon::Hand
                            } else {
                                CursorIcon::Grab
                            }
                        } else if state.view_opts.hover == HoverTarget::SearchInput {
                            CursorIcon::Text
                        } else if state.view_opts.hover.uses_hand_cursor() {
                            CursorIcon::Hand
                        } else {
                            CursorIcon::Default
                        });
                        window.request_redraw();
                    } else if !state.show_welcome || state.view_opts.hover.tooltip().is_some() {
                        // Redraw so floating tooltip tracks cursor & table row / heading '#' hover responds immediately
                        window.request_redraw();
                    }
                }
            }

            WindowEvent::MouseInput {
                state: btn_state,
                button,
                ..
            } => {
                let sz = window.inner_size();
                let vw = sz.width as f32;
                let vh = sz.height as f32;

                match button {
                    MouseButton::Left => {
                        if btn_state == ElementState::Released {
                            if state.view_opts.diagram_lightbox.is_dragging {
                                state.view_opts.diagram_lightbox.is_dragging = false;
                                window.request_redraw();
                            }
                            if state.view_opts.scrollbar_dragging {
                                state.view_opts.scrollbar_dragging = false;
                                window.request_redraw();
                            }
                            if state.is_mouse_selecting {
                                state.is_mouse_selecting = false;
                                if state.selection_anchor == state.selection_focus {
                                    state.clear_selection();
                                    window.request_redraw();
                                }
                            }
                        } else if btn_state == ElementState::Pressed {
                            let (mx, my) = state.cursor_pos;
                            let hit = state.painter.hit_test(mx, my, &state.view_opts, vh);
                            if state.view_opts.diagram_lightbox.is_open {
                                if hit == HoverTarget::DiagramCanvas {
                                    state.view_opts.diagram_lightbox.is_dragging = true;
                                    state.view_opts.diagram_lightbox.drag_start_x = mx;
                                    state.view_opts.diagram_lightbox.drag_start_y = my;
                                    state.view_opts.diagram_lightbox.drag_start_pan_x = state.view_opts.diagram_lightbox.pan_x;
                                    state.view_opts.diagram_lightbox.drag_start_pan_y = state.view_opts.diagram_lightbox.pan_y;
                                    window.request_redraw();
                                    return;
                                }
                            }
                            if hit == HoverTarget::None && my > HEADER_HEIGHT && my < vh - STATUS_BAR_HEIGHT && !state.show_welcome && !state.view_opts.diagram_lightbox.is_open {
                                if let Some(b_idx) = state.find_block_at_y(my) {
                                    state.selection_anchor = Some((b_idx, 0));
                                    state.selection_focus = Some((b_idx, 0));
                                    state.is_mouse_selecting = true;
                                    state.selected_text.clear();
                                    state.view_opts.text_selection = None;
                                    window.request_redraw();
                                }
                            }
                            handle_mouse_click(&mut state, &window);
                        }
                    }
                    MouseButton::Other(1) if btn_state == ElementState::Pressed => {
                        state.go_back(vw, vh);
                        window.set_title(&state.current_title());
                        window.request_redraw();
                    }
                    MouseButton::Other(2) if btn_state == ElementState::Pressed => {
                        state.go_forward(vw, vh);
                        window.set_title(&state.current_title());
                        window.request_redraw();
                    }
                    _ => {}
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 48.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    _ => 0.0,
                };
                if state.view_opts.diagram_lightbox.is_open {
                    let sz = window.inner_size();
                    state.view_opts.diagram_lightbox.zoom_at(
                        state.cursor_pos.0,
                        state.cursor_pos.1,
                        dy,
                        sz.width as f32,
                        sz.height as f32,
                    );
                    window.request_redraw();
                    return;
                }
                if !state.show_welcome
                    && !state.view_opts.stats_open
                    && !state.view_opts.about_open
                {
                    if state.modifiers.control_key() {
                        let step = if dy > 0.0 { 10 } else { -10 };
                        let next = (state.view_opts.zoom_pct as i32 + step).clamp(60, 200) as u32;
                        let sz = window.inner_size();
                        state.apply_zoom(next, sz.width as f32, sz.height as f32);
                    } else {
                        let (mx, my) = state.cursor_pos;
                        let sz = window.inner_size();
                        if state.view_opts.outline_open
                            && mx <= painter::OUTLINE_WIDTH
                            && my > HEADER_HEIGHT
                            && my < sz.height as f32 - STATUS_BAR_HEIGHT
                        {
                            let headings_len = state.view_opts.cached_headings.len();
                            if headings_len > 0 {
                                let cur = state.view_opts.active_heading_ordinal.unwrap_or(0);
                                let step = if dy > 0.0 { -2 } else { 2 };
                                let next = (cur as i32 + step).clamp(0, (headings_len - 1) as i32) as usize;
                                state.view_opts.active_heading_ordinal = Some(next);
                                if let Some(h) = state.view_opts.cached_headings.get(next) {
                                    let y = state
                                        .engine
                                        .layout()
                                        .geometries
                                        .get(h.block_idx)
                                        .map(|g| g.y_offset)
                                        .unwrap_or(0.0);
                                    state.engine.scroll_to((y - 36.0).max(0.0));
                                }
                            }
                        } else {
                            state.engine.smooth_scroll_by(-dy * 1.55);
                        }
                    }
                    window.request_redraw();
                }
            }

            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if key_event.state == ElementState::Pressed {
                    handle_key(
                        &mut state,
                        &window,
                        key_event.physical_key,
                        &key_event.logical_key,
                    );
                }
            }

            _ => {}
        },

        Event::MainEventsCleared => {
            if *control_flow == ControlFlow::Exit {
                return;
            }

            let now = Instant::now();
            let raw_dt = (now - state.last_frame).as_secs_f32();
            state.last_frame = now;
            // Guard against idle-wakeup dt spikes skipping 80%+ of animations on Frame 1
            let dt = if state.was_animating {
                raw_dt.clamp(0.0005, 0.033)
            } else {
                1.0 / 144.0
            };

            let mut timer_fired = false;

            if let Some(until) = state.toast_until {
                if now >= until {
                    state.view_opts.toast_msg = None;
                    state.toast_until = None;
                    timer_fired = true;
                }
            }

            if let Some(until) = state.copied_until {
                if now >= until {
                    state.view_opts.copied_block_idx = None;
                    state.copied_until = None;
                    timer_fired = true;
                }
            }

            let sz = window.inner_size();
            state.view_opts.last_viewport_w = sz.width as f32;

            let total_h = state.engine.layout().total_height;
            let max_scroll = (total_h - state.engine.viewport_height()).max(1.0);
            let target_prog = if state.show_welcome {
                0.0
            } else {
                (state.engine.scroll_y() / max_scroll).clamp(0.0, 1.0)
            };

            let scroll_anim = state.engine.tick(dt);
            let ui_anim = state.view_opts.tick_animations(dt, target_prog);

            if !state.show_welcome {
                state
                    .view_opts
                    .update_scroll_spy(state.engine.layout(), state.engine.scroll_y());
            }

            let animating = scroll_anim || ui_anim;
            let actively_moving = animating || state.was_animating;
            state.was_animating = animating;

            // Background progressive layout measurement for large documents (>250 blocks):
            // Only perform batch measurement when user is NOT actively scrolling or animating UI.
            // This reserves 100% of the frame budget for fluid 144Hz scroll physics.
            if !state.show_welcome && !actively_moving && state.engine.has_unmeasured_blocks() {
                if state.engine.step_idle_measurement(120) {
                    timer_fired = true;
                }
            }
            if !scroll_anim && !ui_anim && state.view_opts.search_open && now >= state.next_caret_blink
            {
                state.view_opts.anim_clock = (state.view_opts.anim_clock + 0.5) % 3600.0;
                state.next_caret_blink = now + Duration::from_millis(500);
                timer_fired = true;
            } else if scroll_anim || ui_anim {
                state.next_caret_blink = now + Duration::from_millis(500);
            }

            let animating = scroll_anim || ui_anim;
            state.was_animating = animating;

            if animating || timer_fired {
                window.request_redraw();
            }

            if animating {
                // High-precision ~144Hz frame pacing while springs/scroll are in flight
                *control_flow = ControlFlow::WaitUntil(now + Duration::from_micros(6944));
            } else {
                // 0.0% CPU sleep until next user input or earliest timer deadline
                let mut earliest: Option<Instant> = None;
                let mut consider = |deadline: Instant| {
                    earliest = Some(match earliest {
                        Some(cur) if cur <= deadline => cur,
                        _ => deadline,
                    });
                };
                if let Some(t) = state.toast_until {
                    consider(t);
                }
                if let Some(t) = state.copied_until {
                    consider(t);
                }
                if state.view_opts.search_open {
                    consider(state.next_caret_blink);
                }

                if let Some(wake_at) = earliest {
                    *control_flow = ControlFlow::WaitUntil(wake_at);
                } else {
                    *control_flow = ControlFlow::Wait;
                }
            }
        }

        Event::RedrawRequested(_) => {
            let size = window.inner_size();
            if size.width == 0 || size.height == 0 {
                return;
            }

            if state.last_surface_size != (size.width, size.height) {
                if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    let _ = surface.resize(w, h);
                }
                state.last_surface_size = (size.width, size.height);
            }

            let mut buffer = match surface.buffer_mut() {
                Ok(b) => b,
                Err(_) => return,
            };

            let need_new = match &state.frame_pixmap {
                Some(p) => p.width() != size.width || p.height() != size.height,
                None => true,
            };
            if need_new {
                state.frame_pixmap = Pixmap::new(size.width, size.height);
            }
            let pixmap = match state.frame_pixmap.as_mut() {
                Some(p) => p,
                None => return,
            };
            let mut pixmap_mut = pixmap.as_mut();

            let file_path = state.current_path.as_ref().and_then(|p| p.to_str());
            painter::paint_document_with_options(
                &mut state.painter,
                &mut pixmap_mut,
                &mut state.engine,
                &state.embed_measurer,
                size.width,
                size.height,
                file_path,
                state.show_welcome,
                &state.view_opts,
            );

            bridge::blit_pixmap_to_softbuffer(
                pixmap,
                &mut buffer,
                size.width as usize,
                size.height as usize,
            );

            let _ = buffer.present();

            // Post-paint hover synchronization: keeps hover & cursor icon 100% accurate
            // when scrolling or panel animations move elements underneath a stationary cursor.
            let (mx, my) = state.cursor_pos;
            let vh = size.height as f32;
            let mut post_changed = false;

            if state.view_opts.hovered_table_idx != state.painter.hovered_table_under_cursor {
                state.view_opts.hovered_table_idx = state.painter.hovered_table_under_cursor;
                post_changed = true;
            }

            if mx >= 0.0 && my >= 0.0 && !state.view_opts.scrollbar_dragging {
                let refreshed = state.painter.hit_test(mx, my, &state.view_opts, vh);
                if refreshed != state.view_opts.hover {
                    state.view_opts.hover = refreshed;
                    window.set_cursor_icon(if state.view_opts.hover == HoverTarget::SearchInput {
                        CursorIcon::Text
                    } else if state.view_opts.hover.uses_hand_cursor() {
                        CursorIcon::Hand
                    } else {
                        CursorIcon::Default
                    });
                    post_changed = true;
                }
            }

            if post_changed {
                state.was_animating = true;
            }
        }

        _ => {}
    });
}

// =============================================================================
// UNIFIED PIXEL-EXACT MOUSE INTERACTION ROUTER
// =============================================================================

fn handle_mouse_click(state: &mut AppState, window: &tao::window::Window) {
    let (mx, my) = state.cursor_pos;
    let size = window.inner_size();
    let vw = size.width as f32;
    let vh = size.height as f32;

    let hit = state.painter.hit_test(mx, my, &state.view_opts, vh);

    // 0. Diagram Lightbox handling
    if state.view_opts.diagram_lightbox.is_open {
        match hit {
            HoverTarget::DiagramZoomIn => {
                state.view_opts.diagram_lightbox.zoom_in();
                window.request_redraw();
                return;
            }
            HoverTarget::DiagramZoomOut => {
                state.view_opts.diagram_lightbox.zoom_out();
                window.request_redraw();
                return;
            }
            HoverTarget::DiagramZoomReset => {
                state.view_opts.diagram_lightbox.reset_zoom();
                window.request_redraw();
                return;
            }
            HoverTarget::DiagramFit => {
                state.view_opts.diagram_lightbox.fit_to_window(vw, vh);
                window.request_redraw();
                return;
            }
            HoverTarget::DiagramClose | HoverTarget::DiagramLightboxBackdrop => {
                state.view_opts.diagram_lightbox.close();
                window.request_redraw();
                return;
            }
            HoverTarget::DiagramCanvas => {
                return;
            }
            _ => {
                state.view_opts.diagram_lightbox.close();
                window.request_redraw();
                return;
            }
        }
    }

    // 0. Modal backdrop & interactive modal buttons (using exact painted modal_card_rect)
    if (state.view_opts.stats_open || state.view_opts.about_open) && my > HEADER_HEIGHT {
        if hit == HoverTarget::ModalClose {
            state.view_opts.stats_open = false;
            state.view_opts.about_open = false;
            window.request_redraw();
            return;
        }
        if hit == HoverTarget::SetDefaultMdBtn && state.view_opts.about_open {
            state.register_as_default_md();
            window.request_redraw();
            return;
        }
        if let Some((cx, cy, cw, ch)) = state.painter.modal_card_rect {
            let outside_card = mx < cx || mx > cx + cw || my < cy || my > cy + ch;
            if outside_card {
                state.view_opts.stats_open = false;
                state.view_opts.about_open = false;
                window.request_redraw();
            }
        }
        return;
    }

    match hit {
        HoverTarget::NavBack => {
            state.go_back(vw, vh);
            window.set_title(&state.current_title());
            window.request_redraw();
        }

        HoverTarget::NavForward => {
            state.go_forward(vw, vh);
            window.set_title(&state.current_title());
            window.request_redraw();
        }

        HoverTarget::BrandLogo => {}

        HoverTarget::ToolbarBtn(idx) => {
            match idx {
                0 => pick_and_load_file(state, window, vw, vh),
                1 => {
                    if state.show_welcome {
                        state.show_toast("Open a Markdown document to view outline");
                    } else {
                        state.view_opts.outline_open = !state.view_opts.outline_open;
                        state.sync_viewport(vw, vh);
                        window.set_title(&state.current_title());
                    }
                }
                2 => {
                    if state.show_welcome {
                        state.show_toast("Open a Markdown document to search");
                    } else {
                        state.view_opts.search_open = !state.view_opts.search_open;
                        state.sync_viewport(vw, vh);
                        if state.view_opts.search_open {
                            state.view_opts.anim_clock = 0.0;
                            state.next_caret_blink = Instant::now() + Duration::from_millis(500);
                        }
                        window.set_title(&state.current_title());
                    }
                }
                3 => {
                    if state.view_opts.about_open {
                        state.view_opts.about_open = false;
                    } else {
                        state.view_opts.default_md_state = windows_assoc::check_default_status();
                        state.view_opts.trigger_modal_open(true);
                    }
                    window.set_title(&state.current_title());
                }
                4 => {
                    if state.view_opts.stats_open {
                        state.view_opts.stats_open = false;
                    } else {
                        state.view_opts.trigger_modal_open(false);
                    }
                    window.set_title(&state.current_title());
                }
                5 => {
                    state.view_opts.width_mode = state.view_opts.width_mode.toggle();
                    state.sync_viewport(vw, vh);
                }
                6 => {
                    state.cycle_theme(window);
                }
                7 => {
                    let next = state.view_opts.zoom_pct.saturating_sub(10);
                    state.apply_zoom(next, vw, vh);
                }
                8 => {
                    state.apply_zoom(100, vw, vh);
                }
                9 => {
                    let next = state.view_opts.zoom_pct + 10;
                    state.apply_zoom(next, vw, vh);
                }
                10 => {
                    state.copy_raw_to_clipboard();
                }
                11 => {
                    export_snapshot_dialog(state, size.width, size.height);
                }
                _ => {}
            }
            window.request_redraw();
        }

        HoverTarget::WelcomeLogo => {}

        HoverTarget::WelcomeDropzone | HoverTarget::WelcomeChooseBtn => {
            pick_and_load_file(state, window, vw, vh);
        }

        HoverTarget::SetDefaultMdBtn => {
            state.register_as_default_md();
            window.request_redraw();
        }

        HoverTarget::WelcomeShortcut(_) => {}

        HoverTarget::SearchInput => {
            state.view_opts.anim_clock = 0.0;
            state.next_caret_blink = Instant::now() + Duration::from_millis(500);
            window.request_redraw();
        }

        HoverTarget::SearchPrev => {
            state.step_search_match(false);
            window.request_redraw();
        }

        HoverTarget::SearchNext => {
            state.step_search_match(true);
            window.request_redraw();
        }

        HoverTarget::SearchClose => {
            state.view_opts.search_open = false;
            state.view_opts.search_active_block = None;
            state.sync_viewport(vw, vh);
            window.request_redraw();
        }

        HoverTarget::OutlineItem(block_idx) => {
            if let Some(y) = state
                .engine
                .layout()
                .geometries
                .get(block_idx)
                .map(|g| g.y_offset)
            {
                state.push_nav_history();
                state.engine.scroll_to((y - 16.0).max(0.0));
                state.view_opts.update_scroll_spy(state.engine.layout(), (y - 16.0).max(0.0));
                window.request_redraw();
            }
        }

        HoverTarget::HeadingAnchor(block_idx) => {
            let anchor_info = state
                .engine
                .layout()
                .blocks
                .get(block_idx)
                .and_then(|b| match b {
                    MarkdownBlock::Heading { id, .. } => Some(id.clone()),
                    _ => None,
                });
            if let Some(y) = state
                .engine
                .layout()
                .geometries
                .get(block_idx)
                .map(|g| g.y_offset)
            {
                state.engine.scroll_to((y - 16.0).max(0.0));
            }
            if let Some(id) = anchor_info {
                let link_frag = format!("#{}", id);
                state.copy_text_to_clipboard(
                    &link_frag,
                    &format!("Copied Anchor {}", link_frag),
                    None,
                );
            }
            window.request_redraw();
        }

        HoverTarget::CodeCopyBtn(block_idx) => {
            if let Some(MarkdownBlock::CodeBlock { code, .. }) =
                state.engine.layout().blocks.get(block_idx).cloned()
            {
                state.copy_text_to_clipboard(
                    &code,
                    "Copied Code Block to Clipboard",
                    Some(block_idx),
                );
                window.request_redraw();
            }
        }

        HoverTarget::TableCopyBtn(block_idx) => {
            if let Some(MarkdownBlock::Table(table)) =
                state.engine.layout().blocks.get(block_idx).cloned()
            {
                let mut tsv = String::new();
                let hdr_line: Vec<String> = table
                    .headers
                    .iter()
                    .map(|cell| {
                        cell.spans
                            .iter()
                            .map(|s| s.text())
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .collect();
                tsv.push_str(&hdr_line.join("\t"));
                tsv.push('\n');
                for row in &table.rows {
                    let row_line: Vec<String> = row
                        .iter()
                        .map(|cell| {
                            cell.spans
                                .iter()
                                .map(|s| s.text())
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .collect();
                    tsv.push_str(&row_line.join("\t"));
                    tsv.push('\n');
                }
                state.copy_text_to_clipboard(
                    &tsv,
                    "Copied Table (TSV) to Clipboard",
                    Some(block_idx),
                );
                window.request_redraw();
            }
        }

        HoverTarget::Scrollbar => {
            if !state.show_welcome {
                state.view_opts.scrollbar_dragging = true;
                state.scroll_from_scrollbar_y(my, vh);
                window.request_redraw();
            }
        }

        HoverTarget::DocLink(url) => {
            handle_link_click(state, window, &url, vw, vh);
        }

        HoverTarget::ModalClose => {
            state.view_opts.stats_open = false;
            state.view_opts.about_open = false;
            window.request_redraw();
        }

        HoverTarget::MermaidCard(source) => {
            let (nat_w, nat_h) = if let Some(pm) = state.embed_measurer.render_unscaled_mermaid(&source) {
                (pm.width() as f32, pm.height() as f32)
            } else {
                (1200.0, 600.0)
            };
            state.view_opts.diagram_lightbox.open(source, nat_w, nat_h, vw, vh);
            state.view_opts.retained_tooltip.clear();
            state.view_opts.tooltip_anim = 0.0;
            state.view_opts.hover = HoverTarget::DiagramCanvas;
            window.request_redraw();
        }

        _ => {}
    }
}

fn pick_and_load_file(state: &mut AppState, window: &tao::window::Window, vw: f32, vh: f32) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("Markdown", &["md", "markdown", "txt"])
        .pick_file()
    {
        state.push_nav_history();
        let _ = state.load_file(path, vw, vh);
        window.set_title(&state.current_title());
        window.request_redraw();
    }
}

fn export_snapshot_dialog(state: &mut AppState, vw: u32, vh: u32) {
    if let Some(path) = rfd::FileDialog::new()
        .set_file_name("QuickMD_Snapshot.png")
        .add_filter("PNG Image", &["png"])
        .save_file()
    {
        if let Some(mut pixmap) = Pixmap::new(vw.max(1), vh.max(1)) {
            let file_path = state.current_path.as_ref().and_then(|p| p.to_str());
            painter::paint_document_with_options(
                &mut state.painter,
                &mut pixmap.as_mut(),
                &mut state.engine,
                &state.embed_measurer,
                vw,
                vh,
                file_path,
                state.show_welcome,
                &state.view_opts,
            );
            if pixmap.save_png(&path).is_ok() {
                state.show_toast("Saved PNG Snapshot");
            }
        }
    }
}

fn percent_decode_path(input: &str) -> String {
    quickmd_lib::bridge::percent_decode_path(input)
}

fn decode_text_bytes(bytes: &[u8]) -> Option<(String, &'static str)> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(rest).ok().map(|s| (s.to_owned(), "UTF-8"));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (text, _, had_errors) = encoding_rs::UTF_16LE.decode(rest);
        return (!had_errors).then(|| (text.into_owned(), "UTF-16 LE"));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (text, _, had_errors) = encoding_rs::UTF_16BE.decode(rest);
        return (!had_errors).then(|| (text.into_owned(), "UTF-16 BE"));
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Some((text.to_owned(), "UTF-8"));
    }
    let (text, _, had_errors) = encoding_rs::WINDOWS_874.decode(bytes);
    (!had_errors).then(|| (text.into_owned(), "TIS-620"))
}

fn is_safe_external_extension(path: &std::path::Path) -> bool {
    const SAFE: &[&str] = &[
        "png", "jpg", "jpeg", "gif", "webp", "bmp", "pdf", "mp3", "mp4", "wav", "txt", "csv",
        "json", "xml", "yaml", "yml", "toml",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SAFE.iter().any(|s| s.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

fn scroll_to_fragment(state: &mut AppState, fragment: &str) -> bool {
    let clean = percent_decode_path(fragment.trim_start_matches('#'));
    if clean.is_empty() {
        return false;
    }
    if state.engine.scroll_to_heading(&clean) {
        return true;
    }
    let slug = slugify(&clean);
    if !slug.is_empty() && state.engine.scroll_to_heading(&slug) {
        return true;
    }
    let collapsed = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    if !collapsed.is_empty() && state.engine.scroll_to_heading(&collapsed) {
        return true;
    }
    // Partial suffix match for section names like "profile-session-info"
    let matched_id = state.engine.layout().blocks.iter().find_map(|b| {
        if let doc_layout::MarkdownBlock::Heading { id, .. } = b {
            if id.ends_with(&collapsed) || id.ends_with(&slug) {
                return Some(id.clone());
            }
        }
        None
    });
    if let Some(id) = matched_id {
        return state.engine.scroll_to_heading(&id);
    }
    false
}

fn handle_link_click(
    state: &mut AppState,
    window: &tao::window::Window,
    url: &str,
    vw: f32,
    vh: f32,
) {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return;
    }

    // 1. In-document #anchor navigation
    if let Some(frag) = trimmed.strip_prefix('#') {
        state.push_nav_history();
        if scroll_to_fragment(state, frag) {
            window.request_redraw();
        } else {
            state.show_toast(&format!("Section not found: #{}", frag));
            window.request_redraw();
        }
        return;
    }

    // 2. External http(s) URL — launched via rundll32 FileProtocolHandler (zero cmd.exe shell injection)
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let unsafe_char = trimmed
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ','));
        if unsafe_char || trimmed.len() > 2048 {
            state.show_toast("Blocked malformed external URL");
            window.request_redraw();
            return;
        }
        #[cfg(target_os = "windows")]
        {
            let rundll32 = bridge::windows_system_binary(r"System32\rundll32.exe");
            let _ = std::process::Command::new(rundll32)
                .args(["url.dll,FileProtocolHandler", trimmed])
                .spawn();
        }
        state.show_toast("Opening Web Link in Browser");
        window.request_redraw();
        return;
    }

    // 3. Local file / Obsidian Wikilink (with optional #Heading section anchor)
    let (raw_path_part, opt_anchor) = {
        let as_whole_file = std::path::Path::new(trimmed);
        if as_whole_file.is_file() {
            (trimmed, None)
        } else {
            match trimmed.rsplit_once('#') {
                Some((p, a)) if !p.is_empty() && !a.contains(['/', '\\']) => (p, Some(a)),
                _ => (trimmed, None),
            }
        }
    };

    if raw_path_part.is_empty() {
        if let Some(frag) = opt_anchor {
            let _ = scroll_to_fragment(state, frag);
            window.request_redraw();
        }
        return;
    }

    let base_dir = state
        .current_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let (mut file_path, is_absolute_file) = match bridge::resolve_safe_local_path(&base_dir, raw_path_part) {
        Ok(res) => res,
        Err(reason) => {
            state.show_toast(reason);
            window.request_redraw();
            return;
        }
    };

    if !is_absolute_file && !file_path.exists() {
        let with_md = file_path.with_extension("md");
        if !bridge::has_unsafe_windows_prefix(&with_md) && with_md.exists() {
            file_path = with_md;
        }
    }

    if file_path.is_file() {
        state.push_nav_history();
        let target_clone = file_path.clone();
        if state.load_file(file_path, vw, vh) {
            if let Some(frag) = opt_anchor {
                let _ = scroll_to_fragment(state, frag);
            }
            window.set_title(&state.current_title());
        } else if is_safe_external_extension(&target_clone) {
            // Only open passive media/document types through the Windows default handler.
            // Executables, scripts, shortcuts and installers are never launched from a link.
            #[cfg(target_os = "windows")]
            {
                let path_str = target_clone.to_str().unwrap_or("");
                let unsafe_arg = path_str.is_empty()
                    || path_str
                        .chars()
                        .any(|c| c.is_control() || matches!(c, '"' | '\'' | '`' | ','));
                if unsafe_arg {
                    state.show_toast("Blocked unsafe external file path");
                } else {
                    let rundll32 = bridge::windows_system_binary(r"System32\rundll32.exe");
                    let _ = std::process::Command::new(rundll32)
                        .args(["url.dll,FileProtocolHandler", path_str])
                        .spawn();
                    state.show_toast(&format!(
                        "Opened external file: {}",
                        target_clone.file_name().and_then(|n| n.to_str()).unwrap_or("")
                    ));
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                state.show_toast("Unable to read target Markdown file");
            }
        } else {
            state.show_toast("Blocked: this file type cannot be opened from a link");
        }
        window.request_redraw();
    } else {
        state.show_toast(&format!("File not found: {}", raw_path_part));
        window.request_redraw();
    }
}

// =============================================================================
// KEYBOARD SHORTCUTS & SEARCH INPUT
// =============================================================================

fn handle_key(
    state: &mut AppState,
    window: &tao::window::Window,
    key_code: KeyCode,
    logical_key: &Key<'_>,
) {
    let ctrl = state.modifiers.control_key();
    let shift = state.modifiers.shift_key();
    let alt = state.modifiers.alt_key();
    let sz = window.inner_size();
    let vw = sz.width as f32;
    let vh = sz.height as f32;

    if state.view_opts.diagram_lightbox.is_open {
        if key_code == KeyCode::Escape || (ctrl && key_code == KeyCode::KeyW) {
            state.view_opts.diagram_lightbox.close();
            window.request_redraw();
            return;
        }
        match key_code {
            KeyCode::Equal | KeyCode::NumpadAdd => {
                state.view_opts.diagram_lightbox.zoom_in();
                window.request_redraw();
                return;
            }
            KeyCode::Minus | KeyCode::NumpadSubtract => {
                state.view_opts.diagram_lightbox.zoom_out();
                window.request_redraw();
                return;
            }
            KeyCode::Digit0 | KeyCode::Numpad0 => {
                state.view_opts.diagram_lightbox.reset_zoom();
                window.request_redraw();
                return;
            }
            KeyCode::KeyF => {
                state.view_opts.diagram_lightbox.fit_to_window(vw, vh);
                window.request_redraw();
                return;
            }
            _ => {}
        }
        return;
    }

    if key_code == KeyCode::Escape {
        if state.view_opts.stats_open || state.view_opts.about_open {
            state.view_opts.stats_open = false;
            state.view_opts.about_open = false;
            window.request_redraw();
            return;
        }
        if state.view_opts.search_open {
            state.view_opts.search_open = false;
            state.view_opts.search_active_block = None;
            state.sync_viewport(vw, vh);
            window.request_redraw();
            return;
        }
    }

    // Active Search Bar typing & clipboard integration
    if state.view_opts.search_open {
        if ctrl {
            match key_code {
                KeyCode::KeyV => {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        if let Ok(text) = cb.get_text() {
                            let clean: String = text
                                .chars()
                                .filter(|c| !c.is_control() || *c == ' ')
                                .take(120)
                                .collect();
                            if !clean.is_empty() {
                                state.view_opts.search_query.push_str(&clean);
                                state.view_opts.anim_clock = 0.0;
                                state.next_caret_blink = Instant::now() + Duration::from_millis(500);
                                state.update_search();
                                window.request_redraw();
                                return;
                            }
                        }
                    }
                }
                KeyCode::KeyC => {
                    if !state.view_opts.search_query.is_empty() {
                        let query = state.view_opts.search_query.clone();
                        state.copy_text_to_clipboard(
                            &query,
                            "Copied Search Query to Clipboard",
                            None,
                        );
                        window.request_redraw();
                        return;
                    }
                }
                KeyCode::KeyA => {
                    state.view_opts.search_query.clear();
                    state.view_opts.anim_clock = 0.0;
                    state.next_caret_blink = Instant::now() + Duration::from_millis(500);
                    state.update_search();
                    window.request_redraw();
                    return;
                }
                KeyCode::Backspace => {
                    let trimmed = state.view_opts.search_query.trim_end();
                    if let Some(pos) = trimmed.rfind(' ') {
                        state.view_opts.search_query.truncate(pos);
                    } else {
                        state.view_opts.search_query.clear();
                    }
                    state.view_opts.anim_clock = 0.0;
                    state.next_caret_blink = Instant::now() + Duration::from_millis(500);
                    state.update_search();
                    window.request_redraw();
                    return;
                }
                _ => {}
            }
        } else {
            match key_code {
                KeyCode::Enter => {
                    state.step_search_match(!shift);
                    window.request_redraw();
                    return;
                }
                KeyCode::Backspace => {
                    state.view_opts.search_query.pop();
                    state.view_opts.anim_clock = 0.0;
                    state.next_caret_blink = Instant::now() + Duration::from_millis(500);
                    state.update_search();
                    window.request_redraw();
                    return;
                }
                KeyCode::Delete => {
                    state.view_opts.search_query.clear();
                    state.view_opts.anim_clock = 0.0;
                    state.next_caret_blink = Instant::now() + Duration::from_millis(500);
                    state.update_search();
                    window.request_redraw();
                    return;
                }
                _ => {
                    if let Key::Character(ch) = logical_key {
                        if !ch.chars().any(|c| c.is_control()) {
                            state.view_opts.search_query.push_str(ch);
                            state.view_opts.anim_clock = 0.0;
                            state.next_caret_blink = Instant::now() + Duration::from_millis(500);
                            state.update_search();
                            window.request_redraw();
                            return;
                        }
                    }
                }
            }
        }
    }

    match key_code {
        KeyCode::KeyO if ctrl => {
            pick_and_load_file(state, window, vw, vh);
        }
        KeyCode::KeyC if ctrl => {
            if !state.selected_text.trim().is_empty() {
                let sel = state.selected_text.clone();
                state.copy_text_to_clipboard(&sel, "Copied Selected Text", None);
                window.request_redraw();
                return;
            } else if !state.raw_markdown.is_empty() {
                state.copy_raw_to_clipboard();
                window.request_redraw();
                return;
            }
        }
        KeyCode::KeyA if ctrl && !state.view_opts.search_open && !state.show_welcome => {
            state.select_all();
            window.request_redraw();
            return;
        }
        KeyCode::ArrowLeft if alt => {
            state.go_back(vw, vh);
            window.set_title(&state.current_title());
            window.request_redraw();
        }
        KeyCode::ArrowRight if alt => {
            state.go_forward(vw, vh);
            window.set_title(&state.current_title());
            window.request_redraw();
        }
        // Note: KeyCode::Backspace navigation removed to prevent accidental page jumps while reading
        KeyCode::KeyF if ctrl => {
            if state.show_welcome {
                state.show_toast("Open a document to search");
            } else {
                state.view_opts.search_open = !state.view_opts.search_open;
                if state.view_opts.search_open {
                    state.view_opts.anim_clock = 0.0;
                    state.next_caret_blink = Instant::now() + Duration::from_millis(500);
                }
                state.sync_viewport(vw, vh);
                window.set_title(&state.current_title());
            }
            window.request_redraw();
        }
        KeyCode::KeyW if ctrl && shift => {
            state.view_opts.width_mode = state.view_opts.width_mode.toggle();
            state.sync_viewport(vw, vh);
            window.request_redraw();
        }
        KeyCode::KeyW if ctrl => {
            if state.view_opts.stats_open || state.view_opts.about_open {
                state.view_opts.stats_open = false;
                state.view_opts.about_open = false;
            } else if state.view_opts.search_open {
                state.view_opts.search_open = false;
            } else if !state.show_welcome {
                state.show_welcome = true;
                state.current_path = None;
                state.raw_markdown.clear();
                state.engine.load_markdown("");
                state.clear_selection();
                state.show_toast("Closed Document");
            }
            window.set_title(&state.current_title());
            window.request_redraw();
        }
        KeyCode::KeyQ if ctrl => {
            std::process::exit(0);
        }
        KeyCode::F11 => {
            let is_fullscreen = window.fullscreen().is_some();
            if is_fullscreen {
                window.set_fullscreen(None);
            } else {
                window.set_fullscreen(Some(tao::window::Fullscreen::Borderless(None)));
            }
            window.request_redraw();
        }
        KeyCode::KeyT if ctrl && shift => {
            state.cycle_theme(window);
            window.request_redraw();
        }
        KeyCode::KeyT if ctrl => {
            if state.show_welcome {
                state.show_toast("Open a document to view outline");
            } else {
                state.view_opts.outline_open = !state.view_opts.outline_open;
                state.sync_viewport(vw, vh);
                window.set_title(&state.current_title());
            }
            window.request_redraw();
        }
        KeyCode::KeyI if ctrl => {
            if state.view_opts.about_open {
                state.view_opts.about_open = false;
            } else {
                state.view_opts.default_md_state = windows_assoc::check_default_status();
                state.view_opts.trigger_modal_open(true);
            }
            window.set_title(&state.current_title());
            window.request_redraw();
        }
        KeyCode::KeyP if ctrl => {
            export_snapshot_dialog(state, sz.width, sz.height);
            window.request_redraw();
        }
        KeyCode::Equal | KeyCode::NumpadAdd if ctrl => {
            state.apply_zoom(state.view_opts.zoom_pct + 10, vw, vh);
            window.request_redraw();
        }
        KeyCode::Minus | KeyCode::NumpadSubtract if ctrl => {
            state.apply_zoom(state.view_opts.zoom_pct.saturating_sub(10), vw, vh);
            window.request_redraw();
        }
        KeyCode::Digit0 | KeyCode::Numpad0 if ctrl => {
            state.apply_zoom(100, vw, vh);
            window.request_redraw();
        }
        KeyCode::KeyR if ctrl => {
            if let Some(p) = state.current_path.clone() {
                let _ = state.load_file(p, vw, vh);
                window.set_title(&state.current_title());
                window.request_redraw();
            }
        }
        KeyCode::F5 => {
            if let Some(p) = state.current_path.clone() {
                let _ = state.load_file(p, vw, vh);
                window.set_title(&state.current_title());
                window.request_redraw();
            }
        }
        KeyCode::F1 => {
            state.view_opts.about_open = !state.view_opts.about_open;
            state.view_opts.stats_open = false;
            window.request_redraw();
        }
        KeyCode::PageUp => {
            let viewport_h = state.engine.viewport_height();
            state.engine.smooth_scroll_by(-viewport_h * 0.85);
            window.request_redraw();
        }
        KeyCode::PageDown => {
            let viewport_h = state.engine.viewport_height();
            state.engine.smooth_scroll_by(viewport_h * 0.85);
            window.request_redraw();
        }
        KeyCode::ArrowDown => {
            state.engine.smooth_scroll_by(64.0);
            window.request_redraw();
        }
        KeyCode::ArrowUp => {
            state.engine.smooth_scroll_by(-64.0);
            window.request_redraw();
        }
        KeyCode::Home => {
            state.engine.scroll_to(0.0);
            window.request_redraw();
        }
        KeyCode::End => {
            let max_y =
                (state.engine.layout().total_height - state.engine.viewport_height()).max(0.0);
            state.engine.scroll_to(max_y);
            window.request_redraw();
        }
        _ => {}
    }
}
