pub mod bridge;
pub mod painter;
pub mod windows_assoc;

#[cfg(test)]
mod test_render_png {
    fn out_png(name: &str) -> String {
        let dir = std::env::temp_dir().join("quickmd-test-output");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name).to_string_lossy().into_owned()
    }

    const REPO_README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/README.md");
    use std::path::Path;
    use tiny_skia::Pixmap;
    use doc_layout::ast::MarkdownBlock;
    use doc_layout::DocumentLayoutEngine;
    use native_render::native_mermaid::DiagramTheme;
    use crate::bridge::{preprocess_markdown, DarkDiagramTheme, RealEmbedMeasurer, SharedFonts};
    use crate::painter::{
        self, HoverTarget, Painter, Spring, ThemeMode, ViewOptions, HEADER_HEIGHT,
        STATUS_BAR_HEIGHT,
    };

    const FULL_SHOWCASE_MD: &str = r##"# QuickMD 3.0 — Pure Native Rust Edition
> [!NOTE]
> **Native Rust Architecture:** 100% Pure Rust Native Canvas with **ZERO Chromium, ZERO WebView2, ZERO HTML/CSS, and ZERO JavaScript**.

---

## Architectural Benchmarks

| Metric | QuickMD 2.1 (WebView2) | QuickMD 3.0 (Pure Native) | Multiplier |
| :--- | :--- | :--- | :--- |
| **Cold Boot** | ~450 ms | **< 15 ms** | **30x Faster** |
| **Memory (Idle)** | 240 MB+ | **< 18 MB** | **92% Less RAM** |
| **Rendering** | DOM / Web Engine | **Direct Win32 Framebuffer** | **Zero Jitter 144Hz** |
| **Dependencies** | `WebView2Loader.dll` | **Single Standalone PE Binary** | **Zero DLL** |

---

## Pure Native LaTeX Math

$$f(x) = \int_{-\infty}^{\infty} \hat{f}(\xi)\,e^{2\pi i \xi x}\,d\xi$$

$$\begin{pmatrix} \alpha & \beta \\ \gamma & \delta \end{pmatrix} \times \vec{v} = \lambda \vec{v}$$

---

## Pure Native Mermaid Flowchart

```mermaid
flowchart TD
    A[Markdown Input .md] --> B[doc_layout O-log-N Culler]
    B --> C[native_render: Math + Mermaid]
    B --> D[tiny-skia 2D Canvas]
    C --> D
    D --> E[softbuffer Direct DIB Blit]
    E --> F([144 FPS Smooth Display])
```

---

## Complex Typography & Tone-Mark Safety Test
ทดสอบการจัดวางและเรนเดอร์อักขระภาษาไทยที่มีวรรณยุกต์ซ้อนสองชั้น: **ระบบทดสอบตัวอักษรไทยสถาปัตยกรรมเนทีฟ** วรรณยุกต์บน-ล่าง เช่น **ปั้, ผู้ใหญ่, ที่นี่, รู้สึก, กลิ่นกล้วยไม้, น้ำเปล่า** จะต้องไม่ถูกขอบกล่องข้อความตัดขาดอย่างเด็ดขาด ด้วยระยะป้องกันความปลอดภัย `1.45x + 8px` clearance

- [x] **100% Native Rust Pipeline:** `tao` + `softbuffer` + `tiny-skia` + `cosmic-text 0.12`
- [x] **Unified Rich-Text Shaper:** Multi-span paragraphs with `InlineCode` pills & Thai cluster breaks
- [x] **Interactive Workspace:** 5 Themes, Live Outline Sidebar, Search Bar, Zoom & Document Metrics

```rust
fn render_frame(engine: &mut DocumentLayoutEngine, pixmap: &mut PixmapMut) {
    for block in engine.visible_blocks() {
        // O(log N) zero-allocation viewport culling at 144 FPS
    }
}
```

> [!TIP]
> **Drag & Drop:** ลากไฟล์ `.md` ใดๆ เข้ามาในหน้าต่างนี้เพื่อเปิดอ่านได้ทันที หรือกด `Ctrl+O` เพื่อเปิดไฟล์
"##;

    #[test]
    fn test_spring_convergence_and_substepping() {
        let mut s = Spring { pos: 0.85, vel: 2.0 };
        let mut moving = true;
        for _ in 0..300 {
            moving = s.step(1.0, 340.0, 26.0, 1.0 / 144.0);
            assert!(s.pos.is_finite() && s.vel.is_finite());
        }
        assert!(!moving, "Spring must converge and report idle");
        assert!((s.pos - 1.0).abs() < 1e-4);

        // Coarse dt stability test (50ms frames)
        let mut coarse = Spring { pos: 0.0, vel: 0.0 };
        for _ in 0..60 {
            coarse.step(10.0, 320.0, 28.0, 0.05);
            assert!(coarse.pos.is_finite());
        }
        assert!((coarse.pos - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_theme_crossfade_and_animation_convergence() {
        let mut opts = ViewOptions::default();
        opts.outline_open = true;
        opts.search_open = true;
        opts.switch_theme(ThemeMode::SolarizedSepia);
        opts.trigger_modal_open(false);
        opts.trigger_toast("Test Toast".to_string());
        opts.trigger_copy_feedback(2);

        // Mid-transition palette should blend between DarkNoir and SolarizedSepia
        let _ = opts.tick_animations(1.0 / 60.0, 0.5);
        let mid_pal = opts.effective_palette();
        assert!(mid_pal.bg_primary.red() > 0.05 && mid_pal.bg_primary.red() < 0.96);

        // Clear toast & run 2.5 seconds of frames -> must converge to 0% idle CPU (`active == false`)
        opts.toast_msg = None;
        opts.stats_open = false;
        let mut active = true;
        for _ in 0..360 {
            active = opts.tick_animations(1.0 / 144.0, 0.5);
        }
        assert!(!active, "All UI channels & springs must settle to idle (0% CPU)");
        assert!((opts.sidebar_anim - 1.0).abs() < 1e-4);
        assert!((opts.search_anim - 1.0).abs() < 1e-4);
        assert!(opts.modal_anim.abs() < 1e-4);
        assert!(opts.toast_anim.abs() < 1e-4);
        assert!((opts.theme_anim - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_preprocess_markdown_inline_code_and_fences() {
        let src = concat!(
            "Link [[docs/session_recap|Session Recap]] and [[README]]\n",
            "Inline code `[[docs/do_not_touch]]` and ``[[also|literal]]``\n",
            "```md\n[[inside_fence|literal]]\n```\n"
        );
        let out = preprocess_markdown(src);
        assert!(out.contains("[Session Recap](docs/session_recap)"));
        assert!(out.contains("[README](README)"));
        assert!(out.contains("`[[docs/do_not_touch]]`"));
        assert!(out.contains("``[[also|literal]]``"));
        assert!(out.contains("[[inside_fence|literal]]"));
    }

    #[test]
    fn test_unified_hitboxes_pixel_exact() {
        let shared_fonts = SharedFonts::new();
        let math_ctx = shared_fonts.new_math_font_context();
        let mermaid_ctx = shared_fonts.new_math_font_context();
        let font_system = shared_fonts.into_font_system();

        let mut engine = DocumentLayoutEngine::new(font_system);
        let measurer = RealEmbedMeasurer::new(math_ctx, mermaid_ctx, DiagramTheme::dark());
        engine.set_embed_measurer(measurer.as_measurer());

        let mut cfg = doc_layout::LayoutConfig::default();
        cfg.code_padding = 22.0;
        engine.set_layout_config(cfg);

        let w = 1280u32;
        let h = 900u32;
        let mut opts = ViewOptions::default();
        let mut painter = Painter::new();
        let mut pixmap = Pixmap::new(w, h).unwrap();

        // A. Welcome Screen Hitboxes (Clean static brand badge, static shortcuts)
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            None,
            true,
            &opts,
        );
        // Brand logo and shortcuts are strictly non-interactive
        assert!(!painter.ui_boxes.iter().any(|hb| hb.target == HoverTarget::BrandLogo));
        assert!(!painter.ui_boxes.iter().any(|hb| matches!(hb.target, HoverTarget::WelcomeShortcut(_))));
        assert_eq!(painter.hit_test(240.0, 26.0, &opts, h as f32), HoverTarget::ToolbarBtn(0));
        assert!(painter.ui_boxes.iter().any(|hb| hb.target == HoverTarget::WelcomeChooseBtn));

        // B. Document + Outline + Search + Copy Buttons + Links
        let md = "# Heading One\n\nParagraph with [TestLink](https://example.com) and match word.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```rust\nlet x = 1;\n```\n\n## Heading Two\n";
        opts.outline_open = true;
        opts.search_open = true;
        opts.search_query = "match".to_string();
        opts.search_matches = 1;
        opts.search_active_block = Some((1, 0));
        opts.cursor_pos = (640.0, 260.0);
        let cw = opts.target_content_width(w as f32);
        engine.set_viewport(cw, h as f32 - HEADER_HEIGHT - STATUS_BAR_HEIGHT - 44.0);
        engine.load_markdown(md);

        // Identify table block index and set hovered_table_idx so [Copy Table] pill registers
        let table_idx = engine
            .layout()
            .blocks
            .iter()
            .position(|b| matches!(b, MarkdownBlock::Table(_)))
            .unwrap();
        let code_idx = engine
            .layout()
            .blocks
            .iter()
            .position(|b| matches!(b, MarkdownBlock::CodeBlock { .. }))
            .unwrap();
        let h2_idx = engine
            .layout()
            .blocks
            .iter()
            .rposition(|b| matches!(b, MarkdownBlock::Heading { .. }))
            .unwrap();
        opts.hovered_table_idx = Some(table_idx);

        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some("test.md"),
            false,
            &opts,
        );

        for target in [
            HoverTarget::SearchInput,
            HoverTarget::SearchPrev,
            HoverTarget::SearchNext,
            HoverTarget::SearchClose,
            HoverTarget::OutlineItem(0),
            HoverTarget::OutlineItem(h2_idx),
            HoverTarget::TableCopyBtn(table_idx),
            HoverTarget::CodeCopyBtn(code_idx),
            HoverTarget::HeadingAnchor(0),
            HoverTarget::Scrollbar,
        ] {
            let hb = painter
                .ui_boxes
                .iter()
                .find(|b| b.target == target)
                .unwrap_or_else(|| panic!("Missing hitbox for {:?}", target));
            assert_eq!(
                painter.hit_test(hb.x + hb.w * 0.5, hb.y + hb.h * 0.5, &opts, h as f32),
                target
            );
        }

        let link_hb = painter.link_boxes.first().expect("Link hitbox registered");
        assert_eq!(
            painter.hit_test(
                link_hb.x + link_hb.w * 0.5,
                link_hb.y + link_hb.h * 0.5,
                &opts,
                h as f32
            ),
            HoverTarget::DocLink("https://example.com".to_string())
        );

        // C. Stats Modal & About Modal [×] Close Hitbox + Backdrop Rect
        opts.stats_open = true;
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some("test.md"),
            false,
            &opts,
        );
        assert!(painter.modal_card_rect.is_some());
        let stats_close = painter
            .ui_boxes
            .iter()
            .find(|b| b.target == HoverTarget::ModalClose)
            .expect("Stats ModalClose registered");
        assert_eq!(
            painter.hit_test(
                stats_close.x + stats_close.w * 0.5,
                stats_close.y + stats_close.h * 0.5,
                &opts,
                h as f32
            ),
            HoverTarget::ModalClose
        );

        opts.stats_open = false;
        opts.about_open = true;
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some("test.md"),
            false,
            &opts,
        );
        assert!(painter.modal_card_rect.is_some());
        let about_close = painter
            .ui_boxes
            .iter()
            .find(|b| b.target == HoverTarget::ModalClose)
            .expect("About ModalClose registered");
        assert_eq!(
            painter.hit_test(
                about_close.x + about_close.w * 0.5,
                about_close.y + about_close.h * 0.5,
                &opts,
                h as f32
            ),
            HoverTarget::ModalClose
        );
    }

    #[test]
    fn test_capture_png() {
        let shared_fonts = SharedFonts::new();
        let math_ctx = shared_fonts.new_math_font_context();
        let mermaid_ctx = shared_fonts.new_math_font_context();
        let font_system = shared_fonts.into_font_system();

        let mut engine = DocumentLayoutEngine::new(font_system);
        let measurer = RealEmbedMeasurer::new(math_ctx, mermaid_ctx, DiagramTheme::dark());
        engine.set_embed_measurer(measurer.as_measurer());

        let mut cfg = doc_layout::LayoutConfig::default();
        cfg.code_padding = 22.0;
        engine.set_layout_config(cfg);

        let w = 1280;
        let h = 860;
        let mut opts = ViewOptions::default();
        opts.raw_char_count = FULL_SHOWCASE_MD.chars().count();
        opts.raw_word_count = FULL_SHOWCASE_MD.split_whitespace().count();
        opts.raw_line_count = FULL_SHOWCASE_MD.lines().count();

        let content_w = opts.target_content_width(w as f32);
        engine.set_viewport(content_w, h as f32 - HEADER_HEIGHT - STATUS_BAR_HEIGHT);

        let mut painter = Painter::new();

        // 1. Render Welcome Screen (with hover highlight on Shortcut #0)
        opts.hover = HoverTarget::WelcomeShortcut(0);
        let mut pixmap_welcome = Pixmap::new(w, 800).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_welcome.as_mut(),
            &mut engine,
            &measurer,
            w,
            800,
            None,
            true,
            &opts,
        );
        let out_welcome = out_png("welcome_screen.png");
        pixmap_welcome.save_png(Path::new(&out_welcome)).unwrap();
        opts.hover = HoverTarget::None;

        // 2. Render Document Screen (Top: Callout, Table with InlineCode pills & hover Copy Table + row highlight, Math integral & matrix, Bottom Status Bar)
        engine.load_markdown(FULL_SHOWCASE_MD);
        let table_idx = engine
            .layout()
            .blocks
            .iter()
            .position(|b| matches!(b, MarkdownBlock::Table(_)));
        opts.hovered_table_idx = table_idx;
        if let Some(tidx) = table_idx {
            let ty = HEADER_HEIGHT
                + painter::CONTENT_TOP_PADDING
                + engine.layout().geometries[tidx].y_offset
                + 54.0;
            opts.cursor_pos = (500.0, ty);
        }
        let mut pixmap_doc = Pixmap::new(w, h).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_doc.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some(REPO_README),
            false,
            &opts,
        );
        let out_doc = out_png("doc_screen.png");
        pixmap_doc.save_png(Path::new(&out_doc)).unwrap();
        opts.cursor_pos = (-100.0, -100.0);
        opts.hovered_table_idx = None;

        // 3. Render Scrolled Document Screen (Lower: Framed Mermaid Flowchart Card, Thai Typography)
        engine.scroll_by(650.0);
        opts.progress_anim = 0.45;
        let mut pixmap_lower = Pixmap::new(w, 1080).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_lower.as_mut(),
            &mut engine,
            &measurer,
            w,
            1080,
            Some(REPO_README),
            false,
            &opts,
        );
        let out_lower = out_png("doc_mermaid_thai.png");
        pixmap_lower.save_png(Path::new(&out_lower)).unwrap();

        // 3b. Render Bottom Section (Thai Tone-Mark Safety, CheckList, CodeBlock with macOS dots + RUST header + Copy button, Tip Callout)
        engine.set_viewport(content_w, 300.0);
        engine.scroll_by(900.0);
        opts.progress_anim = 0.92;
        let mut pixmap_bottom = Pixmap::new(w, 860).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_bottom.as_mut(),
            &mut engine,
            &measurer,
            w,
            860,
            Some(REPO_README),
            false,
            &opts,
        );
        let out_bottom = out_png("doc_thai_code.png");
        pixmap_bottom.save_png(Path::new(&out_bottom)).unwrap();

        // 4. Render Interactive Workspace (Outline Sidebar w/ ScrollSpy + Search Bar w/ Active Amber Match + Centered Stats Modal + Toast)
        engine.scroll_by(-2000.0);
        opts.outline_open = true;
        opts.sidebar_anim = 1.0;
        opts.update_scroll_spy(engine.layout(), 0.0);
        opts.stats_open = true;
        opts.modal_anim = 1.0;
        opts.search_open = true;
        opts.search_anim = 1.0;
        opts.search_query = "Native".to_string();
        opts.search_matches = 6;
        opts.search_current_idx = 0;
        opts.search_active_block = Some((0, 0));
        opts.theme = ThemeMode::TokyoNight;
        opts.toast_msg = Some("Theme: Tokyo Night".to_string());
        opts.toast_anim = 1.0;
        let cw_outline = opts.target_content_width(w as f32);
        engine.set_viewport(cw_outline, h as f32 - HEADER_HEIGHT - 44.0);

        let mut pixmap_ui = Pixmap::new(w, h).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_ui.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some(REPO_README),
            false,
            &opts,
        );
        let out_ui = out_png("doc_interactive_ui.png");
        pixmap_ui.save_png(Path::new(&out_ui)).unwrap();

        // 5. Render Editorial Sepia / Light Theme Verification (Theme-adaptive Callouts + Inked Math + Studio Mermaid)
        let mut light_opts = ViewOptions::default();
        light_opts.theme = ThemeMode::SolarizedSepia;
        light_opts.outline_open = true;
        light_opts.sidebar_anim = 1.0;
        light_opts.raw_char_count = FULL_SHOWCASE_MD.chars().count();
        light_opts.raw_word_count = FULL_SHOWCASE_MD.split_whitespace().count();
        light_opts.raw_line_count = FULL_SHOWCASE_MD.lines().count();
        light_opts.update_scroll_spy(engine.layout(), 0.0);
        let mut pixmap_sepia = Pixmap::new(w, 960).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_sepia.as_mut(),
            &mut engine,
            &measurer,
            w,
            960,
            Some(REPO_README),
            false,
            &light_opts,
        );
        let out_sepia = out_png("doc_light_sepia.png");
        pixmap_sepia.save_png(Path::new(&out_sepia)).unwrap();

        // 6. Render Drag & Drop Overlay Verification
        let mut drag_opts = ViewOptions::default();
        drag_opts.file_drag_hover = true;
        let mut pixmap_drag = Pixmap::new(w, h).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_drag.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some(REPO_README),
            false,
            &drag_opts,
        );
        let out_drag = out_png("doc_drag_overlay.png");
        pixmap_drag.save_png(Path::new(&out_drag)).unwrap();
    }

    #[test]
    fn test_dashboard_render_png() {
        let shared_fonts = SharedFonts::new();
        let math_ctx = shared_fonts.new_math_font_context();
        let mermaid_ctx = shared_fonts.new_math_font_context();
        let font_system = shared_fonts.into_font_system();

        let mut engine = DocumentLayoutEngine::new(font_system);
        let measurer = RealEmbedMeasurer::new(math_ctx, mermaid_ctx, DiagramTheme::dark());
        engine.set_embed_measurer(measurer.as_measurer());

        let mut cfg = doc_layout::LayoutConfig::default();
        cfg.code_padding = 22.0;
        engine.set_layout_config(cfg);

        let w = 1280;
        let h = 920;
        let mut opts = ViewOptions::default();

        let fallback_md = r##"# 👤 1. Profile & Document Sample

> [!NOTE] **Sample Profile**
> • **User**: [Sample User](docs/user_profile.md) (Engineering)
> • **Roadmap**: [[docs/Roadmap|Roadmap]]

---

## Documentation

> [!TIP]
> [[docs/Architecture|Architecture Guide]] — sample content for render tests.

• Inline code literal check: `[[notes/session_recap]]`
"##;

        let real_path = Path::new(REPO_README);
        let raw = std::fs::read_to_string(real_path).unwrap_or_else(|_| fallback_md.to_string());
        let processed = preprocess_markdown(&raw);

        opts.raw_char_count = raw.chars().count();
        opts.raw_word_count = raw.split_whitespace().count();
        opts.raw_line_count = raw.lines().count();
        opts.outline_open = true;
        opts.sidebar_anim = 1.0;

        let content_w = opts.target_content_width(w as f32);
        engine.set_viewport(content_w, h as f32 - HEADER_HEIGHT - STATUS_BAR_HEIGHT);
        engine.load_markdown(&processed);
        opts.update_scroll_spy(engine.layout(), 0.0);

        let mut painter = Painter::new();
        let mut pixmap = Pixmap::new(w, h).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some(REPO_README),
            false,
            &opts,
        );
        let out_path = out_png("dashboard_fixed.png");
        pixmap.save_png(Path::new(&out_path)).unwrap();
    }
    #[test]
    fn test_narrow_and_about_render_png() {
        use crate::painter::TextSelection;
        let shared_fonts = SharedFonts::new();
        let math_ctx = shared_fonts.new_math_font_context();
        let mermaid_ctx = shared_fonts.new_math_font_context();
        let font_system = shared_fonts.into_font_system();

        let mut engine = DocumentLayoutEngine::new(font_system);
        let measurer = RealEmbedMeasurer::new(math_ctx, mermaid_ctx, DiagramTheme::dark());
        engine.set_embed_measurer(measurer.as_measurer());

        let fallback_md = "# QuickMD Workspace Overview\n\nWelcome to your workspace.\n\n## Document Info\n\nTesting text selection and responsive header.\n";
        let real_path = Path::new(REPO_README);
        let raw = std::fs::read_to_string(real_path).unwrap_or_else(|_| fallback_md.to_string());
        let processed = preprocess_markdown(&raw);

        // 1. Narrow Mobile Screen (480px width, 850px height)
        let narrow_w = 480u32;
        let narrow_h = 850u32;
        let mut narrow_opts = ViewOptions::default();
        narrow_opts.raw_char_count = raw.chars().count();
        narrow_opts.raw_word_count = raw.split_whitespace().count();
        narrow_opts.raw_line_count = raw.lines().count();

        let cw = narrow_opts.target_content_width(narrow_w as f32);
        engine.set_viewport(cw, narrow_h as f32 - HEADER_HEIGHT - STATUS_BAR_HEIGHT);
        engine.load_markdown(&processed);

        let mut painter = Painter::new();
        let mut pixmap_narrow = Pixmap::new(narrow_w, narrow_h).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_narrow.as_mut(),
            &mut engine,
            &measurer,
            narrow_w,
            narrow_h,
            Some("Dashboard.md"),
            false,
            &narrow_opts,
        );

        // Mathematically verify zero header button collision at 480px!
        let header_boxes: Vec<_> = painter.ui_boxes.iter().filter(|b| b.y < HEADER_HEIGHT).collect();
        for (i, b1) in header_boxes.iter().enumerate() {
            for b2 in header_boxes.iter().skip(i + 1) {
                let overlap_x = b1.x < b2.x + b2.w && b1.x + b1.w > b2.x;
                let overlap_y = b1.y < b2.y + b2.h && b1.y + b1.h > b2.y;
                assert!(
                    !(overlap_x && overlap_y),
                    "Header button collision detected at 480px: {:?} vs {:?}",
                    b1.target,
                    b2.target
                );
            }
        }

        let out_narrow = out_png("narrow_mobile_screen.png");
        pixmap_narrow.save_png(Path::new(&out_narrow)).unwrap();

        // 2. Clean V2.0 About Modal
        let mut about_opts = ViewOptions::default();
        about_opts.about_open = true;
        about_opts.modal_anim = 1.0;
        let w = 1000u32;
        let h = 750u32;
        let mut pixmap_about = Pixmap::new(w, h).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_about.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some("Dashboard.md"),
            false,
            &about_opts,
        );
        let out_about = out_png("about_v2_modal.png");
        pixmap_about.save_png(Path::new(&out_about)).unwrap();

        // 3. Text Selection Highlight
        let mut sel_opts = ViewOptions::default();
        sel_opts.text_selection = Some(TextSelection {
            is_active: true,
            start_block: 0,
            start_char: 0,
            end_block: 1,
            end_char: 50,
            selected_text: "Selection verification".to_string(),
        });
        let mut pixmap_sel = Pixmap::new(w, h).unwrap();
        painter::paint_document_with_options(
            &mut painter,
            &mut pixmap_sel.as_mut(),
            &mut engine,
            &measurer,
            w,
            h,
            Some("Dashboard.md"),
            false,
            &sel_opts,
        );
        let out_sel = out_png("text_selection_screen.png");
        pixmap_sel.save_png(Path::new(&out_sel)).unwrap();
    }

    #[test]
    #[ignore]
    fn bench_scroll_frame_time() {
        let shared_fonts = SharedFonts::new();
        let math_ctx = shared_fonts.new_math_font_context();
        let mermaid_ctx = shared_fonts.new_math_font_context();
        let font_system = shared_fonts.into_font_system();

        let mut engine = DocumentLayoutEngine::new(font_system);
        let measurer = RealEmbedMeasurer::new(math_ctx, mermaid_ctx, DiagramTheme::dark());
        engine.set_embed_measurer(measurer.as_measurer());

        let w: u32 = 1280;
        let h: u32 = 860;
        let mut opts = ViewOptions::default();
        opts.raw_char_count = FULL_SHOWCASE_MD.chars().count();
        opts.raw_word_count = FULL_SHOWCASE_MD.split_whitespace().count();
        opts.raw_line_count = FULL_SHOWCASE_MD.lines().count();

        let content_w = opts.target_content_width(w as f32);
        engine.set_viewport(content_w, h as f32 - HEADER_HEIGHT - STATUS_BAR_HEIGHT);
        engine.load_markdown(FULL_SHOWCASE_MD);

        let mut painter = Painter::new();
        let mut pixmap = Pixmap::new(w, h).unwrap();
        let frames = 300;

        let mut scroll_total = std::time::Duration::ZERO;
        let mut still_total = std::time::Duration::ZERO;
        for i in 0..frames {
            engine.scroll_by(if i % 2 == 0 { 6.0 } else { -6.0 });
            let t = std::time::Instant::now();
            painter::paint_document_with_options(
                &mut painter,
                &mut pixmap.as_mut(),
                &mut engine,
                &measurer,
                w,
                h,
                Some(REPO_README),
                false,
                &opts,
            );
            scroll_total += t.elapsed();
        }
        for _ in 0..frames {
            let t = std::time::Instant::now();
            painter::paint_document_with_options(
                &mut painter,
                &mut pixmap.as_mut(),
                &mut engine,
                &measurer,
                w,
                h,
                Some(REPO_README),
                false,
                &opts,
            );
            still_total += t.elapsed();
        }

        eprintln!(
            "BENCH scroll avg {:.3} ms/frame, still avg {:.3} ms/frame ({} frames each)",
            scroll_total.as_secs_f64() * 1000.0 / frames as f64,
            still_total.as_secs_f64() * 1000.0 / frames as f64,
            frames
        );
    }

    #[ignore]
    #[test]
    fn generate_docs_screenshots() {
        use tiny_skia::{PixmapPaint, Transform};
        let docs_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/images"));
        let _ = std::fs::create_dir_all(docs_dir);

        let arch_doc = r##"# QuickMD Core Architecture

> [!NOTE] **Pure Native Rust Pipeline**
> Rendered directly through `tao`, `softbuffer`, `tiny-skia`, and `cosmic-text` — zero WebView2, zero Electron, and zero background services.

## Execution & Layout Pipeline

$$\mathcal{L}_{\text{render}} = \sum_{t=1}^{T} \alpha^t \cdot \mathbb{E}\!\left[\Delta\tau_{\text{frame}}\right] < 15\,\text{ms}$$

```mermaid
graph LR
    A[Markdown .md] --> B[AST Parser]
    B --> C[2-Tier Layout]
    C --> D[TeX & Sugiyama]
    D --> E[tiny-skia 144Hz]
```

## Key Properties

| Component | Engine | Memory Footprint |
| :--- | :--- | :--- |
| **Windowing** | `tao` Win32 Loop | < 4 MB |
| **Typography** | `cosmic-text` + ICU | ~12 MB |
| **Rasterizer** | `tiny-skia` DIB | ~25 MB Total |

```rust
pub fn render_frame(surface: &mut [u32], layout: &DocumentLayout) {
    // Progressive viewport hydration guarantees smooth 144Hz scroll
    painter::paint_viewport(surface, layout);
}
```
"##;

        let doc_showcase_md = r##"# QuickMD 3.0 — Feature Overview

> [!TIP] **Air-Gapped & Instant Cold Boot**
> Opens standalone `.md`, `.markdown`, `.mdown`, and `.mkd` files in under **15 ms** with strict 32 MB bounds and UNC path blocking.

## Thai ICU Word Segmentation

การเรนเดอร์ภาษาไทยรองรับการตัดคำด้วยพจนานุกรม ICU พร้อมระยะเผื่อสระบน-ล่างและวรรณยุกต์อย่างแม่นยำ ไม่ตัดคำกลางพยางค์

## Mathematical Typesetting

$$f(x) = \int_{-\infty}^{\infty} \hat{f}(\xi)\,e^{2\pi i \xi x}\,d\xi$$

| Metric | QuickMD 3.0 | Electron Apps |
| :--- | :--- | :--- |
| **Boot Time** | **< 15 ms** | 1.5–4.0 s |
| **Idle RAM** | **~25 MB** | 250–600 MB |
| **Rendering** | **Native Rust** | Chromium DOM |
"##;

        let render_scene = |md: Option<&str>, theme: ThemeMode, outline: bool, stats: bool, virtual_path: Option<&str>| -> Pixmap {
            let shared_fonts = SharedFonts::new();
            let math_ctx = shared_fonts.new_math_font_context();
            let mermaid_ctx = shared_fonts.new_math_font_context();
            let font_system = shared_fonts.into_font_system();

            let mut engine = DocumentLayoutEngine::new(font_system);
            let measurer = RealEmbedMeasurer::new(math_ctx, mermaid_ctx, DiagramTheme::dark());
            engine.set_embed_measurer(measurer.as_measurer());

            let mut cfg = doc_layout::LayoutConfig::default();
            cfg.wrap_code = true;
            cfg.code_padding = 22.0;
            engine.set_layout_config(cfg);

            let w = 1280u32;
            let h = 800u32;
            let mut opts = ViewOptions::default();
            opts.theme = theme;
            opts.outline_open = outline;
            opts.sidebar_anim = if outline { 1.0 } else { 0.0 };
            opts.stats_open = stats;
            opts.modal_anim = if stats { 1.0 } else { 0.0 };
            opts.last_modal_is_about = false;
            opts.modal_scale = Spring::new(1.0);
            opts.default_md_state = crate::windows_assoc::DefaultAssociationState::NotRegistered;

            let is_welcome = md.is_none();
            if let Some(raw) = md {
                let processed = preprocess_markdown(raw);
                opts.raw_char_count = raw.chars().count();
                opts.raw_word_count = raw.split_whitespace().count();
                opts.raw_line_count = raw.lines().count();
                let content_w = opts.target_content_width(w as f32);
                engine.set_viewport(content_w, h as f32 - HEADER_HEIGHT - STATUS_BAR_HEIGHT);
                engine.load_markdown(&processed);
                opts.cached_headings = engine
                    .layout()
                    .blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, b)| match b {
                        MarkdownBlock::Heading { level, text, .. } => Some(crate::painter::HeadingItem {
                            block_idx: idx,
                            level: *level,
                            text: text.clone(),
                        }),
                        _ => None,
                    })
                    .collect();
                opts.update_scroll_spy(engine.layout(), 0.0);
            } else {
                engine.set_viewport(880.0, h as f32 - HEADER_HEIGHT - STATUS_BAR_HEIGHT);
                engine.load_markdown("");
            }

            let mut painter = Painter::new();
            let mut pixmap = Pixmap::new(w, h).unwrap();
            painter::paint_document_with_options(
                &mut painter,
                &mut pixmap.as_mut(),
                &mut engine,
                &measurer,
                w,
                h,
                virtual_path,
                is_welcome,
                &opts,
            );
            pixmap
        };

        let hero = render_scene(Some(arch_doc), ThemeMode::TokyoNight, true, false, Some("QuickMD/Architecture.md"));
        hero.save_png(docs_dir.join("quickmd-hero.png")).unwrap();

        let doc_render = render_scene(Some(doc_showcase_md), ThemeMode::DarkNoir, true, false, Some("QuickMD/Features.md"));
        doc_render.save_png(docs_dir.join("quickmd-doc-render.png")).unwrap();

        let stats_modal = render_scene(Some(arch_doc), ThemeMode::DarkNoir, false, true, Some("QuickMD/Architecture.md"));
        stats_modal.save_png(docs_dir.join("quickmd-stats-modal.png")).unwrap();

        let light_welcome = render_scene(None, ThemeMode::GithubLight, false, false, None);
        light_welcome.save_png(docs_dir.join("quickmd-theme-light.png")).unwrap();

        let oled_doc = render_scene(Some(arch_doc), ThemeMode::ObsidianOled, true, false, Some("QuickMD/Architecture.md"));
        oled_doc.save_png(docs_dir.join("quickmd-theme-oled.png")).unwrap();

        // Build crisp 2x2 showcase composite (1920x1200)
        let mut grid = Pixmap::new(1920, 1200).unwrap();
        let paint = PixmapPaint {
            quality: tiny_skia::FilterQuality::Bilinear,
            ..Default::default()
        };
        let sx = 960.0 / 1280.0;
        let sy = 600.0 / 800.0;
        let dark_welcome = render_scene(None, ThemeMode::DarkNoir, false, false, None);
        grid.draw_pixmap(0, 0, dark_welcome.as_ref(), &paint, Transform::from_scale(sx, sy), None);
        grid.draw_pixmap(0, 0, light_welcome.as_ref(), &paint, Transform::from_scale(sx, sy).post_translate(960.0, 0.0), None);
        grid.draw_pixmap(0, 0, oled_doc.as_ref(), &paint, Transform::from_scale(sx, sy).post_translate(0.0, 600.0), None);
        grid.draw_pixmap(0, 0, stats_modal.as_ref(), &paint, Transform::from_scale(sx, sy).post_translate(960.0, 600.0), None);
        grid.save_png(docs_dir.join("quickmd-showcase-grid.png")).unwrap();
    }
}

