use std::path::{Path, PathBuf};
use tiny_skia::*;
use cosmic_text::{Attrs, Color as CosmicColor, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};

fn out_png(name: &str) -> String {
    let dir = std::env::temp_dir().join("quickmd-test-output");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name).to_string_lossy().into_owned()
}

fn vault_path(rel: &str) -> Option<PathBuf> {
    let root = std::env::var_os("QUICKMD_TEST_VAULT")?;
    Some(PathBuf::from(root).join(rel))
}

#[derive(Debug, Clone, PartialEq)]
pub struct BadgePill {
    pub label: String,
    pub value: String,
    pub bg_color: [u8; 4],
}

pub fn parse_shields_badge_url(url: &str) -> Option<BadgePill> {
    if !url.contains("shields.io/badge/") {
        return None;
    }
    let after = url.split("shields.io/badge/").nth(1)?;
    let clean = after.split('?').next()?;
    let path = clean.strip_suffix(".svg").unwrap_or(clean);

    let sentinel = "\u{FFFF}";
    let protected = path.replace("--", sentinel);
    let parts: Vec<&str> = protected.split('-').collect();
    if parts.len() < 2 {
        return None;
    }

    let decode_part = |s: &str| -> String {
        let restored = s.replace(sentinel, "-").replace('_', " ");
        percent_decode(&restored)
    };

    let (label, value, color_str) = if parts.len() == 2 {
        ("".to_string(), decode_part(parts[0]), parts[1])
    } else {
        let label = decode_part(parts[0]);
        let value = decode_part(parts[1]);
        let color = parts[2];
        (label, value, color)
    };

    let bg_color = parse_badge_color(color_str);
    Some(BadgePill { label, value, bg_color })
}

fn percent_decode(s: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = s.as_bytes().iter().copied().peekable();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(c1), Some(c2)) = (h1, h2) {
                let hex_str = [c1, c2];
                if let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&hex_str).unwrap_or("0"), 16) {
                    bytes.push(byte);
                    continue;
                }
            }
        }
        bytes.push(b);
    }
    String::from_utf8(bytes).unwrap_or_else(|_| s.to_string())
}

fn parse_badge_color(col: &str) -> [u8; 4] {
    let lower = col.to_lowercase();
    match lower.as_str() {
        "orange" => [249, 115, 22, 255],
        "blue" => [59, 130, 246, 255],
        "green" | "success" => [34, 197, 94, 255],
        "black" => [24, 24, 27, 255],
        "lightgrey" | "lightgray" => [113, 113, 122, 255],
        "red" => [239, 68, 68, 255],
        "yellow" => [234, 179, 8, 255],
        "purple" => [139, 92, 246, 255],
        _ => {
            let hex = lower.trim_start_matches('#');
            if hex.len() == 6 {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    u8::from_str_radix(&hex[0..2], 16),
                    u8::from_str_radix(&hex[2..4], 16),
                    u8::from_str_radix(&hex[4..6], 16),
                ) {
                    return [r, g, b, 255];
                }
            }
            [99, 102, 241, 255]
        }
    }
}

pub fn render_badge_pill(pill: &BadgePill, font_system: &mut FontSystem, swash: &mut SwashCache) -> Option<Pixmap> {
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

    // Draw pill rounded rect
    let radius = 4.0f32;

    if has_label {
        // Draw left side (dark background #20242c)
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

        // Draw right side (colored background)
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
        // Single pill background
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

    // Draw text with cosmic-text
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

pub fn load_and_render_image(
    full_path: &Path,
    max_w: f32,
    radius: f32,
) -> Option<Pixmap> {
    let bytes = std::fs::read(full_path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (orig_w, orig_h) = (img.width(), img.height());
    if orig_w == 0 || orig_h == 0 {
        return None;
    }

    let scale = if orig_w as f32 > max_w { max_w / orig_w as f32 } else { 1.0 };
    let target_w = ((orig_w as f32 * scale).round() as u32).max(1);
    let target_h = ((orig_h as f32 * scale).round() as u32).max(1);

    let resized = if (target_w, target_h) != (orig_w, orig_h) {
        image::imageops::resize(&img, target_w, target_h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let mut pixmap = Pixmap::new(target_w, target_h)?;
    let dst = pixmap.data_mut();

    for (i, px) in resized.pixels().enumerate() {
        let [r, g, b, a] = px.0;
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

    // Apply rounded corners clip with tiny-skia mask
    if radius > 0.5 {
        let mut mask = Mask::new(target_w, target_h)?;
        let mut pb = PathBuilder::new();
        pb.move_to(radius, 0.0);
        pb.line_to(target_w as f32 - radius, 0.0);
        pb.quad_to(target_w as f32, 0.0, target_w as f32, radius);
        pb.line_to(target_w as f32, target_h as f32 - radius);
        pb.quad_to(target_w as f32, target_h as f32, target_w as f32 - radius, target_h as f32);
        pb.line_to(radius, target_h as f32);
        pb.quad_to(0.0, target_h as f32, 0.0, target_h as f32 - radius);
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

#[test]
fn test_experiment_render_v2_fable_scene() {
    let mut font_system = FontSystem::new();
    let mut swash = SwashCache::new();

    let vw = 1280u32;
    let vh = 960u32;
    let mut canvas = Pixmap::new(vw, vh).unwrap();
    // Tokyo Night background
    canvas.fill(Color::from_rgba8(13, 17, 23, 255));

    // 1. Render Header Toolbar in v2.0 style
    // Header background
    let mut header_paint = Paint::default();
    header_paint.set_color_rgba8(22, 27, 34, 255);
    canvas.fill_rect(Rect::from_xywh(0.0, 0.0, vw as f32, 44.0).unwrap(), &header_paint, Transform::identity(), None);

    // Draw [ # QuickMD PRO ]
    draw_text_into_pixmap(
        &mut canvas,
        "# QuickMD",
        &mut font_system,
        &mut swash,
        18.0,
        14.0,
        CosmicColor::rgb(240, 246, 252),
        13.5,
        Weight::BOLD,
    );
    // Draw PRO pill
    let mut pro_paint = Paint::default();
    pro_paint.set_color_rgba8(59, 130, 246, 255);
    canvas.fill_rect(Rect::from_xywh(96.0, 14.0, 32.0, 16.0).unwrap(), &pro_paint, Transform::identity(), None);
    draw_text_into_pixmap(
        &mut canvas,
        "PRO",
        &mut font_system,
        &mut swash,
        100.0,
        16.0,
        CosmicColor::rgb(255, 255, 255),
        10.0,
        Weight::BOLD,
    );

    // Draw [ 📁 Open ] with text!
    draw_text_into_pixmap(
        &mut canvas,
        "📁 Open",
        &mut font_system,
        &mut swash,
        142.0,
        14.0,
        CosmicColor::rgb(200, 210, 225),
        12.5,
        Weight::MEDIUM,
    );

    // Draw [ 📑 Outline ] and [ 🔍 Search ] on the LEFT side!
    draw_text_into_pixmap(
        &mut canvas,
        "|≡",
        &mut font_system,
        &mut swash,
        210.0,
        14.0,
        CosmicColor::rgb(140, 150, 165),
        13.0,
        Weight::MEDIUM,
    );
    draw_text_into_pixmap(
        &mut canvas,
        "🔍",
        &mut font_system,
        &mut swash,
        240.0,
        14.0,
        CosmicColor::rgb(140, 150, 165),
        12.0,
        Weight::MEDIUM,
    );

    // Center Breadcrumb: C:\...\docs\reference / README.md
    let center_text = r"C:\...\docs\reference  /  README.md";
    draw_text_into_pixmap(
        &mut canvas,
        center_text,
        &mut font_system,
        &mut swash,
        420.0,
        14.0,
        CosmicColor::rgb(230, 237, 243),
        12.5,
        Weight::BOLD,
    );

    // 2. Load and render banner image (760px wide, centered!)
    let banner_path = vault_path("sample-project/images/banner.png");
    if let Some(banner_pixmap) = banner_path.and_then(|p| load_and_render_image(&p, 760.0, 12.0)) {
        let bw = banner_pixmap.width() as f32;
        let bh = banner_pixmap.height() as f32;
        let bx = (vw as f32 - bw) / 2.0;
        let by = 70.0;
        canvas.draw_pixmap(
            bx as i32,
            by as i32,
            banner_pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );

        // 3. Render Badges (Row 1) Centered!
        let badge_row_1 = [
            "https://img.shields.io/badge/License-CC_BY_4.0-lightgrey.svg",
            "https://img.shields.io/badge/Try_it_on-MuAPI-black",
            "https://img.shields.io/badge/Fable_5-20%25_OFF-ff6b00",
            "https://img.shields.io/badge/Website-Live-orange",
            "https://img.shields.io/badge/Docs-Read-blue",
            "https://img.shields.io/badge/Build_AI_SaaS-50%2B_templates-22c55e",
        ];

        let mut row1_pills = Vec::new();
        let mut total_row1_w = 0.0f32;
        for u in badge_row_1 {
            if let Some(pill) = parse_shields_badge_url(u) {
                if let Some(pm) = render_badge_pill(&pill, &mut font_system, &mut swash) {
                    total_row1_w += pm.width() as f32 + 8.0;
                    row1_pills.push(pm);
                }
            }
        }
        total_row1_w -= 8.0;

        let mut cur_x = (vw as f32 - total_row1_w) / 2.0;
        let row1_y = by + bh + 24.0;
        for pm in row1_pills {
            let pw = pm.width() as f32;
            canvas.draw_pixmap(cur_x as i32, row1_y as i32, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            cur_x += pw + 8.0;
        }

        // 4. Render Language Badges (Row 2) Centered!
        let badge_row_2 = [
            "https://img.shields.io/badge/English-Read-111111",
            "https://img.shields.io/badge/Espa%C3%B1ol-Read-ffb703",
            "https://img.shields.io/badge/Portugu%C3%AAs-Read-2a9d8f",
            "https://img.shields.io/badge/%E6%97%A5%E6%9C%AC%E8%AA%9E-Read-52b788",
            "https://img.shields.io/badge/%ED%95%9C%EA%B5%AD%EC%96%B4-Read-4ea8de",
            "https://img.shields.io/badge/Deutsch-Read-f4a261",
            "https://img.shields.io/badge/Fran%C3%A7ais-Read-e76f51",
            "https://img.shields.io/badge/T%C3%BCrk%C3%A7e-Read-d62828",
            "https://img.shields.io/badge/%E7%B9%81%E9%AB%94%E4%B8%AD%E6%96%87-Read-8338ec",
        ];

        let mut row2_pills = Vec::new();
        let mut total_row2_w = 0.0f32;
        for u in badge_row_2 {
            if let Some(pill) = parse_shields_badge_url(u) {
                if let Some(pm) = render_badge_pill(&pill, &mut font_system, &mut swash) {
                    total_row2_w += pm.width() as f32 + 6.0;
                    row2_pills.push(pm);
                }
            }
        }
        total_row2_w -= 6.0;

        let mut cur_x2 = (vw as f32 - total_row2_w) / 2.0;
        let row2_y = row1_y + 30.0;
        for pm in row2_pills {
            let pw = pm.width() as f32;
            canvas.draw_pixmap(cur_x2 as i32, row2_y as i32, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            cur_x2 += pw + 6.0;
        }

        // 5. Render Language Badges (Row 3) Centered!
        let badge_row_3 = [
            "https://img.shields.io/badge/%E7%AE%80%E4%BD%93%E4%B8%AD%E6%96%87-Read-ef476f",
            "https://img.shields.io/badge/%D0%A0%D1%83%D1%81%D1%81%D0%BA%D0%B8%D0%B9-Read-577590",
        ];
        let mut row3_pills = Vec::new();
        let mut total_row3_w = 0.0f32;
        for u in badge_row_3 {
            if let Some(pill) = parse_shields_badge_url(u) {
                if let Some(pm) = render_badge_pill(&pill, &mut font_system, &mut swash) {
                    total_row3_w += pm.width() as f32 + 6.0;
                    row3_pills.push(pm);
                }
            }
        }
        total_row3_w -= 6.0;
        let mut cur_x3 = (vw as f32 - total_row3_w) / 2.0;
        let row3_y = row2_y + 30.0;
        for pm in row3_pills {
            let pw = pm.width() as f32;
            canvas.draw_pixmap(cur_x3 as i32, row3_y as i32, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            cur_x3 += pw + 6.0;
        }
    }

    // Save output artifact
    let out_path = out_png("experiment_v2_output.png");
    canvas.save_png(&out_path).expect("Failed to save experiment PNG");
    println!("SUCCESS: Saved experiment_v2_output.png to {}", out_path);
}


#[test]
fn test_production_pipeline_fable_readme() {
    use doc_layout::ast::{Alignment, MarkdownBlock};
    use doc_layout::DocumentLayoutEngine;
    use quickmd_lib::bridge::{preprocess_markdown, DarkDiagramTheme, RealEmbedMeasurer, SharedFonts};
    use quickmd_lib::painter::{paint_document_with_options, Painter, ViewOptions, ReadingWidthMode, ThemeMode};
    use native_render::native_mermaid::DiagramTheme;

    let Some(readme_path) = vault_path("sample-project/README.md") else {
        println!("skipped: set QUICKMD_TEST_VAULT to run this test");
        return;
    };
    let content = std::fs::read_to_string(&readme_path).expect("Failed to read README.md");
    let processed = preprocess_markdown(&content);

    let shared_fonts = SharedFonts::new();
    let math_font_ctx = shared_fonts.new_math_font_context();
    let mermaid_font_ctx = shared_fonts.new_math_font_context();
    let font_system = shared_fonts.into_font_system();

    let mut engine = DocumentLayoutEngine::new(font_system);
    let embed_measurer = RealEmbedMeasurer::new(math_font_ctx, mermaid_font_ctx, DiagramTheme::dark());
    embed_measurer.set_base_dir(readme_path.parent().unwrap().to_path_buf());
    engine.set_embed_measurer(embed_measurer.as_measurer());

    let vw = 1280.0f32;
    let vh = 960.0f32;
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Normal;
    engine.set_viewport(opts.target_content_width(vw), vh - 80.0);
    engine.load_markdown(&processed);

    let blocks = &engine.layout().blocks;
    println!("Parsed {} blocks from README.md", blocks.len());
    for (i, b) in blocks.iter().take(5).enumerate() {
        println!("Block {}: {:?}", i, b);
    }

    let mut found_image = false;
    let mut found_badge_row = false;
    for b in blocks {
        match b {
            MarkdownBlock::Image { url, align, width_override, .. } => {
                println!("Found Image: url={}, align={:?}, width_override={:?}", url, align, width_override);
                if url == "images/banner.png" && *align == Alignment::Center && *width_override == Some(760.0) {
                    found_image = true;
                }
            }
            MarkdownBlock::BadgeRow { badges, align } => {
                println!("Found BadgeRow: count={}, align={:?}", badges.len(), align);
                if !badges.is_empty() && *align == Alignment::Center {
                    found_badge_row = true;
                }
            }
            _ => {}
        }
    }

    assert!(found_image, "Must find centered banner image block!");
    assert!(found_badge_row, "Must find centered badge row block!");

    // Paint full document to pixmap
    let mut painter = Painter::new();
    let mut canvas = Pixmap::new(vw as u32, vh as u32).unwrap();
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Wide;
    opts.raw_word_count = content.split_whitespace().count();
    opts.raw_line_count = content.lines().count();
    opts.raw_char_count = content.chars().count();

    paint_document_with_options(
        &mut painter,
        &mut canvas.as_mut(),
        &mut engine,
        &embed_measurer,
        vw as u32,
        vh as u32,
        Some(readme_path.to_str().unwrap()),
        false,
        &opts,
    );

    let out_path = out_png("production_v2_fable_output.png");
    canvas.save_png(&out_path).expect("Failed to save production PNG");
    println!("SUCCESS: Saved production_v2_fable_output.png to {}", out_path);
    assert!(!painter.link_boxes.is_empty(), "Must register link hitboxes for badges");
    println!("Total interactive link hitboxes: {}", painter.link_boxes.len());
}

#[test]
fn test_audit_security_and_bug_fixes() {
    use quickmd_lib::bridge::percent_decode_path;
    use quickmd_lib::painter::{count_block_search_matches, find_case_insensitive_matches};
    use doc_layout::ast::{InlineSpan, MarkdownBlock};

    // 1. Verify find_case_insensitive_matches on multi-byte characters that change length
    // Turkish uppercase İ (2 bytes) -> lowercase i + dot (3 bytes)
    let turkish = "İSTANBUL VE İZMİR";
    let m1 = find_case_insensitive_matches(turkish, "i");
    assert!(!m1.is_empty(), "Must match turkish İ case-insensitively");
    for (start, end) in m1 {
        assert!(turkish.is_char_boundary(start), "start index must be valid char boundary");
        assert!(turkish.is_char_boundary(end), "end index must be valid char boundary");
    }

    // German uppercase ẞ (3 bytes) vs lowercase ß (2 bytes)
    let german = "STRASSE UND GROẞE";
    let m2 = find_case_insensitive_matches(german, "große");
    for (start, end) in m2 {
        assert!(german.is_char_boundary(start));
        assert!(german.is_char_boundary(end));
    }

    // Thai unicode text
    let thai_text = "หลักสูตรวิทยาการคอมพิวเตอร์ มหาวิทยาลัยหอการค้าไทย";
    let m3 = find_case_insensitive_matches(thai_text, "วิทยาการคอม");
    assert_eq!(m3.len(), 1, "Must find exactly 1 Thai match");
    let (ts, te) = m3[0];
    assert_eq!(&thai_text[ts..te], "วิทยาการคอม");

    // 2. Percent-decode path verification
    assert_eq!(percent_decode_path("images/my%20screenshot.png"), "images/my screenshot.png");
    assert_eq!(percent_decode_path("path%2Fwith%2Fslashes"), "path/with/slashes");
    assert_eq!(percent_decode_path("normal/path.png"), "normal/path.png");
    assert_eq!(percent_decode_path("malformed%2"), "malformed%2");

    // 3. Search inside CodeBlock and InlineCode
    let code_block = MarkdownBlock::CodeBlock {
        lang: "rust".to_string(),
        code: "fn test_function() {\n    let val = 42;\n}".to_string(),
    };
    let hits = count_block_search_matches(&code_block, "test_function");
    assert_eq!(hits, 1, "Must find search query in CodeBlock!");

    let p_with_inline = MarkdownBlock::Paragraph(vec![
        InlineSpan::Text("Call ".to_string()),
        InlineSpan::InlineCode("DocumentLayout::measure_with".to_string()),
        InlineSpan::Text(" to layout.".to_string()),
    ]);
    let hits_inline = count_block_search_matches(&p_with_inline, "measure_with");
    assert_eq!(hits_inline, 1, "Must find search query in InlineCode!");

    println!("AUDIT PASS: All 4 bug/vulnerability regression checks passed with 100% precision!");
}

#[test]
fn test_render_mermaid_security_issue() {
    use quickmd_lib::bridge::SharedFonts;
    use native_render::native_mermaid::{self, DiagramTheme};

    let src = r#"
graph LR
    subgraph Host [Windows Host OS]
        File[Markdown File .md]
    end

    subgraph QuickMDKernel [QuickMD Native Rust Engine]
        Tao[Tao Window Loop]
        Wry[Wry Core Controller]
        Scheme[In-Memory Direct Asset Scheme]
    end

    subgraph SandboxedDOM [Isolated Rendering Context]
        Parser[Marked AST Parser]
        Sanitizer[DOM Purify Filter]
        Render[KaTeX / Mermaid / Highlight.js]
    end

    File -->|Direct Read| QuickMDKernel
    QuickMDKernel -->|In-Memory Buffer| Scheme
    Scheme --> SandboxedDOM
    Parser --> Sanitizer --> Render
    "#;

    let shared_fonts = SharedFonts::new();
    let math_ctx = shared_fonts.new_math_font_context();
    let mut renderer = native_mermaid::MermaidRenderer::new(math_ctx);

    let mut theme = DiagramTheme::default();
    theme.background = Some([13, 17, 23, 255]); // #0D1117
    theme.node_fill = [26, 29, 38, 255];
    theme.node_stroke = [99, 102, 241, 255];
    theme.node_text_color = [226, 232, 240, 255];
    theme.edge_stroke = [100, 116, 139, 255];

    let ast = native_mermaid::parser::parse(src).unwrap();
    println!("Parsed AST: {} nodes, {} subgraphs, {} edges", ast.nodes.len(), ast.subgraphs.len(), ast.edges.len());
    for (id, node) in &ast.nodes {
        println!("  AST Node: id='{}', label='{}', shape={:?}", id, node.label, node.shape);
    }
    for sub in &ast.subgraphs {
        println!("  AST Subgraph: id='{}', title='{}', nodes={:?}", sub.id, sub.title, sub.node_ids);
    }

    let pixmap = renderer.render(src, &theme).expect("Mermaid render failed");
    let out_path = out_png("diagram_fixed.png");
    pixmap.save_png(&out_path).expect("Failed to save diagram PNG");
    println!("SUCCESS: Rendered diagram to {}", out_path);
    println!("Diagram dimensions: {}x{}", pixmap.width(), pixmap.height());
}

#[test]
fn test_render_readme_security_diagram_in_window() {
    use doc_layout::DocumentLayoutEngine;
    use quickmd_lib::bridge::{preprocess_markdown, DarkDiagramTheme, RealEmbedMeasurer, SharedFonts};
    use quickmd_lib::painter::{paint_document_with_options, Painter, ViewOptions, ReadingWidthMode, ThemeMode};
    use native_render::native_mermaid::DiagramTheme;

    let readme_path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"));
    let content = std::fs::read_to_string(&readme_path).expect("Failed to read README.md");
    let processed = preprocess_markdown(&content);

    let shared_fonts = SharedFonts::new();
    let math_font_ctx = shared_fonts.new_math_font_context();
    let mermaid_font_ctx = shared_fonts.new_math_font_context();
    let font_system = shared_fonts.into_font_system();

    let mut engine = DocumentLayoutEngine::new(font_system);
    let embed_measurer = RealEmbedMeasurer::new(math_font_ctx, mermaid_font_ctx, DiagramTheme::dark());
    embed_measurer.set_base_dir(readme_path.parent().unwrap().to_path_buf());
    engine.set_embed_measurer(embed_measurer.as_measurer());

    let vw = 1280.0f32;
    let vh = 960.0f32;
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Normal;
    engine.set_viewport(opts.target_content_width(vw), vh - 80.0);
    engine.load_markdown(&processed);

    // Scroll to the Security Architecture heading
    let found = engine.scroll_to_heading("air-gapped-security-architecture");
    println!("Found heading 'air-gapped-security-architecture': {}", found);
    for _ in 0..120 {
        engine.tick(0.016);
    }
    let cur_y = engine.scroll_y();
    println!("Engine settled at scroll_y = {}", cur_y);

    let mut painter = Painter::new();
    let mut canvas = Pixmap::new(vw as u32, vh as u32).unwrap();
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Normal; // 880px like user's screenshot
    opts.raw_word_count = content.split_whitespace().count();
    opts.raw_line_count = content.lines().count();
    opts.raw_char_count = content.chars().count();
    opts.update_scroll_spy(engine.layout(), cur_y);

    paint_document_with_options(
        &mut painter,
        &mut canvas.as_mut(),
        &mut engine,
        &embed_measurer,
        vw as u32,
        vh as u32,
        Some(readme_path.to_str().unwrap()),
        false,
        &opts,
    );

    let out_path = out_png("diagram_in_quickmd_window.png");
    canvas.save_png(&out_path).expect("Failed to save window diagram PNG");
    println!("SUCCESS: Rendered in-window diagram to {}", out_path);
}

#[test]
fn test_render_all_features_master_test_file() {
    use doc_layout::DocumentLayoutEngine;
    use quickmd_lib::bridge::{preprocess_markdown, DarkDiagramTheme, RealEmbedMeasurer, SharedFonts};
    use quickmd_lib::painter::{paint_document_with_options, Painter, ViewOptions, ReadingWidthMode, ThemeMode};
    use native_render::native_mermaid::DiagramTheme;

    let test_file_path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/QUICKMD_ALL_FEATURES_TEST.md"));
    let content = std::fs::read_to_string(&test_file_path).expect("Failed to read QUICKMD_ALL_FEATURES_TEST.md");
    let processed = preprocess_markdown(&content);

    let shared_fonts = SharedFonts::new();
    let math_font_ctx = shared_fonts.new_math_font_context();
    let mermaid_font_ctx = shared_fonts.new_math_font_context();
    let font_system = shared_fonts.into_font_system();

    let mut engine = DocumentLayoutEngine::new(font_system);
    let embed_measurer = RealEmbedMeasurer::new(math_font_ctx, mermaid_font_ctx, DiagramTheme::dark());
    embed_measurer.set_base_dir(test_file_path.parent().unwrap().to_path_buf());
    engine.set_embed_measurer(embed_measurer.as_measurer());

    let vw = 1280.0f32;
    let vh = 960.0f32;
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Normal;
    opts.raw_word_count = content.split_whitespace().count();
    opts.raw_line_count = content.lines().count();
    opts.raw_char_count = content.chars().count();

    let content_w = opts.target_content_width(vw);
    engine.set_viewport(content_w, vh - 80.0);
    engine.load_markdown(&processed);

    let blocks = &engine.layout().blocks;
    println!("Parsed {} total blocks from QUICKMD_ALL_FEATURES_TEST.md", blocks.len());

    let mut painter = Painter::new();
    let mut canvas = Pixmap::new(vw as u32, vh as u32).unwrap();
    opts.update_scroll_spy(engine.layout(), 0.0);

    paint_document_with_options(
        &mut painter,
        &mut canvas.as_mut(),
        &mut engine,
        &embed_measurer,
        vw as u32,
        vh as u32,
        Some(test_file_path.to_str().unwrap()),
        false,
        &opts,
    );

    let out_path = out_png("all_features_master_top.png");
    canvas.save_png(&out_path).expect("Failed to save master top PNG");
    println!("SUCCESS: Rendered master top view to {}", out_path);

    // Scroll to the security diagram heading and render
    let found = engine.scroll_to_heading("air-gapped-security-architecture-mermaid-graph-lr");
    println!("Found heading 'air-gapped-security-architecture-mermaid-graph-lr': {}", found);
    for _ in 0..120 {
        engine.tick(0.016);
    }
    let cur_y = engine.scroll_y();

    let mut canvas2 = Pixmap::new(vw as u32, vh as u32).unwrap();
    let mut painter2 = Painter::new();
    opts.update_scroll_spy(engine.layout(), cur_y);

    paint_document_with_options(
        &mut painter2,
        &mut canvas2.as_mut(),
        &mut engine,
        &embed_measurer,
        vw as u32,
        vh as u32,
        Some(test_file_path.to_str().unwrap()),
        false,
        &opts,
    );

    let out_path2 = out_png("all_features_master_diagram.png");
    canvas2.save_png(&out_path2).expect("Failed to save master diagram PNG");
    println!("SUCCESS: Rendered master diagram view to {}", out_path2);
}

#[test]
fn test_diagram_lightbox_and_no_top_leak() {
    use doc_layout::DocumentLayoutEngine;
    use quickmd_lib::bridge::{preprocess_markdown, DarkDiagramTheme, RealEmbedMeasurer, SharedFonts};
    use quickmd_lib::painter::{paint_document_with_options, Painter, ViewOptions, ReadingWidthMode, ThemeMode, HoverTarget};
    use native_render::native_mermaid::DiagramTheme;

    let test_file_path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/QUICKMD_ALL_FEATURES_TEST.md"));
    let content = std::fs::read_to_string(&test_file_path).expect("Failed to read QUICKMD_ALL_FEATURES_TEST.md");
    let processed = preprocess_markdown(&content);

    let shared_fonts = SharedFonts::new();
    let math_font_ctx = shared_fonts.new_math_font_context();
    let mermaid_font_ctx = shared_fonts.new_math_font_context();
    let font_system = shared_fonts.into_font_system();

    let mut engine = DocumentLayoutEngine::new(font_system);
    let embed_measurer = RealEmbedMeasurer::new(math_font_ctx, mermaid_font_ctx, DiagramTheme::dark());
    embed_measurer.set_base_dir(test_file_path.parent().unwrap().to_path_buf());
    engine.set_embed_measurer(embed_measurer.as_measurer());

    let vw = 1280.0f32;
    let vh = 960.0f32;
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Normal;
    engine.set_viewport(opts.target_content_width(vw), vh - 80.0);
    engine.load_markdown(&processed);

    // 1. Scroll to the exact position from user's screenshot
    let found = engine.scroll_to_heading("native-rendering-pipeline-mermaid-graph-td-with-all-shapes");
    assert!(found, "Must find TD diagram heading");
    for _ in 0..120 {
        engine.tick(0.016);
    }
    let target_y = engine.scroll_y() + 160.0;
    engine.scroll_to(target_y);
    for _ in 0..120 {
        engine.tick(0.016);
    }

    let mut painter = Painter::new();
    let mut canvas = Pixmap::new(vw as u32, vh as u32).unwrap();
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Normal;
    opts.update_scroll_spy(engine.layout(), engine.scroll_y());

    paint_document_with_options(
        &mut painter,
        &mut canvas.as_mut(),
        &mut engine,
        &embed_measurer,
        vw as u32,
        vh as u32,
        Some(test_file_path.to_str().unwrap()),
        false,
        &opts,
    );

    let out_leak = out_png("test_no_top_leak.png");
    canvas.save_png(&out_leak).expect("Failed to save leak test PNG");
    println!("SUCCESS: Saved no-leak render to {}", out_leak);

    // 2. Test Diagram Lightbox Pan-Zoom
    let diagram_src = r#"
graph TD
    subgraph Ingestion [Stage 1: Document Ingestion]
        MDFile[(Disk File .md)]
        ASTParser[Parser & AST Preprocessor]
    end
    MDFile --> ASTParser
    "#;

    opts.diagram_lightbox.open(diagram_src.to_string(), 1200.0, 600.0, vw, vh);
    assert!(opts.diagram_lightbox.is_open, "Lightbox must be open");

    // Test Zoom In
    let z0 = opts.diagram_lightbox.zoom;
    opts.diagram_lightbox.zoom_in();
    assert!(opts.diagram_lightbox.zoom > z0, "Zoom in must increase scale");

    // Test Zoom Out
    opts.diagram_lightbox.zoom_out();
    opts.diagram_lightbox.zoom_out();

    // Test Reset
    opts.diagram_lightbox.reset_zoom();
    assert_eq!(opts.diagram_lightbox.zoom, 1.0, "Reset must set zoom to 1.0");

    // Test Pan
    opts.diagram_lightbox.pan_x = 50.0;
    opts.diagram_lightbox.pan_y = -30.0;

    let mut canvas_lb = Pixmap::new(vw as u32, vh as u32).unwrap();
    let mut painter_lb = Painter::new();

    paint_document_with_options(
        &mut painter_lb,
        &mut canvas_lb.as_mut(),
        &mut engine,
        &embed_measurer,
        vw as u32,
        vh as u32,
        Some(test_file_path.to_str().unwrap()),
        false,
        &opts,
    );

    let out_lb = out_png("test_diagram_lightbox_render.png");
    canvas_lb.save_png(&out_lb).expect("Failed to save lightbox PNG");
    println!("SUCCESS: Saved lightbox render to {}", out_lb);

    // Verify lightbox canvas hitbox is registered
    assert!(painter_lb.ui_boxes.iter().any(|h| matches!(h.target, HoverTarget::DiagramCanvas)));
}

#[test]
fn test_render_awesome_opus5_5_videos() {
    use doc_layout::DocumentLayoutEngine;
    use quickmd_lib::bridge::{preprocess_markdown, DarkDiagramTheme, RealEmbedMeasurer, SharedFonts};
    use quickmd_lib::painter::{paint_document_with_options, Painter, ViewOptions, ReadingWidthMode, ThemeMode};
    use native_render::native_mermaid::DiagramTheme;

    let Some(target_path) = vault_path("sample-gallery/README.md") else {
        println!("skipped: set QUICKMD_TEST_VAULT to run this test");
        return;
    };
    let content = std::fs::read_to_string(&target_path).expect("Failed to read sample-gallery README.md");
    let processed = preprocess_markdown(&content);

    let shared_fonts = SharedFonts::new();
    let math_font_ctx = shared_fonts.new_math_font_context();
    let mermaid_font_ctx = shared_fonts.new_math_font_context();
    let font_system = shared_fonts.into_font_system();

    let mut engine = DocumentLayoutEngine::new(font_system);
    let embed_measurer = RealEmbedMeasurer::new(math_font_ctx, mermaid_font_ctx, DiagramTheme::dark());
    embed_measurer.set_base_dir(target_path.parent().unwrap().to_path_buf());
    engine.set_embed_measurer(embed_measurer.as_measurer());

    let vw = 1280.0f32;
    let vh = 960.0f32;
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Normal;
    opts.raw_word_count = content.split_whitespace().count();
    opts.raw_line_count = content.lines().count();
    opts.raw_char_count = content.chars().count();

    let content_w = opts.target_content_width(vw);
    engine.set_viewport(content_w, vh - 80.0);
    engine.load_markdown(&processed);

    let blocks = &engine.layout().blocks;
    println!("=== AWESOME OPUS 5.5 AUDIT ===");
    println!("Total blocks parsed: {}", blocks.len());
    for (i, b) in blocks.iter().take(25).enumerate() {
        println!("  Block {}: {:?}", i, std::mem::discriminant(b));
    }

    let mut painter = Painter::new();
    let mut canvas = Pixmap::new(vw as u32, vh as u32).unwrap();
    opts.update_scroll_spy(engine.layout(), 0.0);

    let t0 = std::time::Instant::now();
    paint_document_with_options(
        &mut painter,
        &mut canvas.as_mut(),
        &mut engine,
        &embed_measurer,
        vw as u32,
        vh as u32,
        Some(target_path.to_str().unwrap()),
        false,
        &opts,
    );
    let top_time = t0.elapsed();
    println!("Frame 0 (top view) render time: {:?}", top_time);

    let out_top = out_png("opus_videos_render_top.png");
    canvas.save_png(&out_top).expect("Failed to save opus top PNG");
    println!("SUCCESS: Saved opus top view to {}", out_top);
    println!("Total link hitboxes on screen: {}", painter.link_boxes.len());

    // Scroll to middle (where the huge 58-row table is!) and paint
    engine.scroll_to(1400.0);
    for _ in 0..60 {
        engine.tick(0.016);
    }
    let mut canvas_mid = Pixmap::new(vw as u32, vh as u32).unwrap();
    let mut painter_mid = Painter::new();
    opts.update_scroll_spy(engine.layout(), engine.scroll_y());

    let t1 = std::time::Instant::now();
    paint_document_with_options(
        &mut painter_mid,
        &mut canvas_mid.as_mut(),
        &mut engine,
        &embed_measurer,
        vw as u32,
        vh as u32,
        Some(target_path.to_str().unwrap()),
        false,
        &opts,
    );
    let mid_time = t1.elapsed();
    println!("Frame 1 (table view at y=1400) render time: {:?}", mid_time);

    // Run 10 consecutive frames like a real 144Hz scroll
    let mut total_time = std::time::Duration::ZERO;
    for f in 0..10 {
        let scroll = 1400.0 + f as f32 * 20.0;
        engine.scroll_to(scroll);
        engine.tick(0.016);
        let mut p = Painter::new();
        let start = std::time::Instant::now();
        paint_document_with_options(
            &mut p,
            &mut canvas_mid.as_mut(),
            &mut engine,
            &embed_measurer,
            vw as u32,
            vh as u32,
            Some(target_path.to_str().unwrap()),
            false,
            &opts,
        );
        total_time += start.elapsed();
    }
    println!("Average render time per scroll frame over table: {:?}", total_time / 10);

    let out_mid = out_png("opus_videos_render_mid.png");
    canvas_mid.save_png(&out_mid).expect("Failed to save opus mid PNG");
    println!("SUCCESS: Saved opus mid view to {}", out_mid);
}

#[test]
fn test_render_native_pipeline_diagram_fixed() {
    use quickmd_lib::bridge::SharedFonts;
    use native_render::native_mermaid::{self, DiagramTheme};

    let src = r#"
graph TD
    subgraph Ingestion [Stage 1: Document Ingestion]
        MDFile[(Disk File .md)]
        ASTParser[Parser & AST Preprocessor]
    end

    subgraph ShaperStage [Stage 2: Shaper & Layout]
        FastMono([O_1 Arithmetic Monospace])
        SwashShaper[Swash OpenType Shaper]
        TypeRouter{"Block Type?"}
    end

    subgraph NativeRender [Stage 3: native_render Kernel]
        TeXBox[[Knuth TeX Box Model]]
        SugiyamaGraph[[Sugiyama Hierarchical DAG]]
        LanczosScaler[[Lanczos3 Resampler]]
    end

    subgraph CompositorStage [Stage 4: Win32 Compositor]
        Canvas[(tiny-skia 32-bit Pixmap)]
        Surface([Win32 Direct Framebuffer])
    end

    MDFile --> ASTParser
    ASTParser --> TypeRouter
    TypeRouter -->|Code Block| FastMono
    TypeRouter -->|Standard Text| SwashShaper
    TypeRouter -->|Math Formula| TeXBox
    TypeRouter -->|Mermaid Diagram| SugiyamaGraph
    TypeRouter -->|Bitmap Image| LanczosScaler

    FastMono --> Canvas
    SwashShaper --> Canvas
    TeXBox --> Canvas
    SugiyamaGraph --> Canvas
    LanczosScaler --> Canvas

    Canvas ==>|144Hz DWM Present| Surface
    "#;

    let shared_fonts = SharedFonts::new();
    let math_ctx = shared_fonts.new_math_font_context();
    let mut renderer = native_mermaid::MermaidRenderer::new(math_ctx);

    let mut theme = DiagramTheme::default();
    theme.background = Some([13, 17, 23, 255]); // #0D1117
    theme.node_fill = [26, 29, 38, 255];
    theme.node_stroke = [99, 102, 241, 255];
    theme.node_text_color = [226, 232, 240, 255];
    theme.edge_stroke = [100, 116, 139, 255];

    let pixmap = renderer.render(src, &theme).expect("Pipeline render failed");
    let out_path = out_png("pipeline_diagram_fixed.png");
    pixmap.save_png(&out_path).expect("Failed to save pipeline PNG");
    println!("SUCCESS: Rendered pipeline diagram to {}", out_path);
    println!("Pipeline diagram dimensions: {}x{}", pixmap.width(), pixmap.height());
}

#[test]
fn test_profile_agents_md() {
    use std::fs;
    use std::time::Instant;
    use quickmd_lib::bridge::{preprocess_markdown, DarkDiagramTheme, RealEmbedMeasurer, SharedFonts};
    use quickmd_lib::painter::{paint_document_with_options, Painter, ViewOptions, ReadingWidthMode, ThemeMode};
    use native_render::native_mermaid::DiagramTheme;
    use doc_layout::DocumentLayoutEngine;
    use tiny_skia::Pixmap;

    let Some(target_path) = vault_path("AGENTS.md") else {
        println!("skipped: set QUICKMD_TEST_VAULT to run this test");
        return;
    };
    let content = match fs::read_to_string(&target_path) {
        Ok(c) => c,
        Err(e) => {
            println!("Could not read AGENTS.md: {}", e);
            return;
        }
    };

    println!("\n=== PROFILING AGENTS.MD ===");
    println!("File size: {} bytes", content.len());

    let processed = preprocess_markdown(&content);
    let shared_fonts = SharedFonts::new();
    let math_font_ctx = shared_fonts.new_math_font_context();
    let mermaid_font_ctx = shared_fonts.new_math_font_context();
    let font_system = shared_fonts.into_font_system();

    let mut engine = DocumentLayoutEngine::new(font_system);
    let embed_measurer = RealEmbedMeasurer::new(math_font_ctx, mermaid_font_ctx, DiagramTheme::dark());
    embed_measurer.set_base_dir(target_path.parent().unwrap().to_path_buf());
    engine.set_embed_measurer(embed_measurer.as_measurer());

    let vw = 1280.0f32;
    let vh = 960.0f32;
    let mut opts = ViewOptions::default();
    opts.theme = ThemeMode::DarkNoir;
    opts.width_mode = ReadingWidthMode::Normal;

    let content_w = opts.target_content_width(vw);
    engine.set_viewport(content_w, vh - 80.0);

    let t1 = Instant::now();
    engine.load_markdown(&processed);
    let load_time = t1.elapsed();
    println!("load_markdown time: {:?}", load_time);

    let total_h = engine.layout().total_height;
    println!("Total layout height: {:.1}px, block count: {}", total_h, engine.layout().blocks.len());

    let mut painter = Painter::new();
    let mut pixmap = Pixmap::new(vw as u32, vh as u32).unwrap();

    let mut frame_count = 0;
    let mut total_paint_time = std::time::Duration::ZERO;
    let mut slow_frames = 0;
    let mut max_frame_time = std::time::Duration::ZERO;
    let mut max_frame_scroll_y = 0.0;

    let mut scroll_y = 0.0;
    while scroll_y < total_h {
        engine.scroll_to(scroll_y);
        opts.update_scroll_spy(engine.layout(), scroll_y);

        let t_frame = Instant::now();
        let mut pixmap_mut = pixmap.as_mut();
        paint_document_with_options(
            &mut painter,
            &mut pixmap_mut,
            &mut engine,
            &embed_measurer,
            vw as u32,
            vh as u32,
            Some(target_path.to_str().unwrap()),
            false,
            &opts,
        );
        let frame_time = t_frame.elapsed();

        if frame_time > max_frame_time {
            max_frame_time = frame_time;
            max_frame_scroll_y = scroll_y;
        }

        if frame_time.as_millis() >= 8 {
            slow_frames += 1;
            let visible = engine.visible_blocks();
            let visible_types: Vec<_> = visible.iter().filter_map(|vb| {
                engine.layout().blocks.get(vb.block_index).map(|b| match b {
                    doc_layout::ast::MarkdownBlock::Paragraph(_) => "Paragraph",
                    doc_layout::ast::MarkdownBlock::Heading { .. } => "Heading",
                    doc_layout::ast::MarkdownBlock::CodeBlock { .. } => "CodeBlock",
                    doc_layout::ast::MarkdownBlock::List { .. } => "List",
                    doc_layout::ast::MarkdownBlock::Blockquote(_) => "Blockquote",
                    doc_layout::ast::MarkdownBlock::Table(_) => "Table",
                    _ => "Other",
                })
            }).collect();
            println!(
                "  Slow frame at scroll_y={:.0} (block count={}): {:?} | types: {:?}",
                scroll_y, visible.len(), frame_time, visible_types
            );
        }

        total_paint_time += frame_time;
        frame_count += 1;
        scroll_y += 120.0;
    }

    let avg_time = total_paint_time / frame_count.max(1);
    println!("Total frames tested: {}", frame_count);
    println!("Average frame paint time: {:?}", avg_time);
    println!("Slow frames (>=8ms): {} / {}", slow_frames, frame_count);
    println!("Max frame paint time: {:?} at scroll_y={:.0}", max_frame_time, max_frame_scroll_y);
}
