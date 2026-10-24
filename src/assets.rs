pub struct AssetData {
    pub bytes: &'static [u8],
    pub mime: &'static str,
}

pub fn get_asset(path: &str) -> Option<AssetData> {
    let clean = path.trim_start_matches('/').replace('\\', "/");

    match clean.as_str() {
        // Main Web Assets
        "index.html" | "web/index.html" => Some(AssetData {
            bytes: include_str!("../assets/web/index.html").as_bytes(),
            mime: "text/html; charset=utf-8",
        }),
        "app.css" | "web/app.css" => Some(AssetData {
            bytes: include_str!("../assets/web/app.css").as_bytes(),
            mime: "text/css; charset=utf-8",
        }),
        "app.js" | "web/app.js" => Some(AssetData {
            bytes: include_str!("../assets/web/app.js").as_bytes(),
            mime: "application/javascript; charset=utf-8",
        }),

        // CSS Assets
        "css/fonts.css" => Some(AssetData {
            bytes: include_str!("../assets/css/fonts.css").as_bytes(),
            mime: "text/css; charset=utf-8",
        }),
        "css/katex.min.css" => Some(AssetData {
            bytes: include_str!("../assets/css/katex.min.css").as_bytes(),
            mime: "text/css; charset=utf-8",
        }),
        "css/highlight-dark.min.css" => Some(AssetData {
            bytes: include_str!("../assets/css/highlight-dark.min.css").as_bytes(),
            mime: "text/css; charset=utf-8",
        }),
        "css/highlight-light.min.css" => Some(AssetData {
            bytes: include_str!("../assets/css/highlight-light.min.css").as_bytes(),
            mime: "text/css; charset=utf-8",
        }),

        // JS Dependencies
        "js/lucide.min.js" => Some(AssetData {
            bytes: include_str!("../assets/js/lucide.min.js").as_bytes(),
            mime: "application/javascript; charset=utf-8",
        }),
        "js/marked.min.js" => Some(AssetData {
            bytes: include_str!("../assets/js/marked.min.js").as_bytes(),
            mime: "application/javascript; charset=utf-8",
        }),
        "js/katex.min.js" => Some(AssetData {
            bytes: include_str!("../assets/js/katex.min.js").as_bytes(),
            mime: "application/javascript; charset=utf-8",
        }),
        "js/auto-render.min.js" => Some(AssetData {
            bytes: include_str!("../assets/js/auto-render.min.js").as_bytes(),
            mime: "application/javascript; charset=utf-8",
        }),
        "js/mermaid.min.js" => Some(AssetData {
            bytes: include_str!("../assets/js/mermaid.min.js").as_bytes(),
            mime: "application/javascript; charset=utf-8",
        }),
        "js/highlight.min.js" => Some(AssetData {
            bytes: include_str!("../assets/js/highlight.min.js").as_bytes(),
            mime: "application/javascript; charset=utf-8",
        }),
        "js/js-yaml.min.js" => Some(AssetData {
            bytes: include_str!("../assets/js/js-yaml.min.js").as_bytes(),
            mime: "application/javascript; charset=utf-8",
        }),

        // Fonts
        "fonts/KaTeX_AMS-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_AMS-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Caligraphic-Bold.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Caligraphic-Bold.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Caligraphic-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Caligraphic-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Fraktur-Bold.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Fraktur-Bold.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Fraktur-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Fraktur-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Main-Bold.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Main-Bold.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Main-BoldItalic.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Main-BoldItalic.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Main-Italic.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Main-Italic.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Main-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Main-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Math-BoldItalic.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Math-BoldItalic.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Math-Italic.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Math-Italic.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_SansSerif-Bold.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_SansSerif-Bold.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_SansSerif-Italic.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_SansSerif-Italic.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_SansSerif-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_SansSerif-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Script-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Script-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Size1-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Size1-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Size2-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Size2-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Size3-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Size3-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Size4-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Size4-Regular.woff2"),
            mime: "font/woff2",
        }),
        "fonts/KaTeX_Typewriter-Regular.woff2" => Some(AssetData {
            bytes: include_bytes!("../assets/fonts/KaTeX_Typewriter-Regular.woff2"),
            mime: "font/woff2",
        }),

        // App Icon & Logos
        "icons/app_icon.ico" => Some(AssetData {
            bytes: include_bytes!("../assets/icons/app_icon.ico"),
            mime: "image/x-icon",
        }),
        "web/logo.svg" => Some(AssetData {
            bytes: include_bytes!("../assets/web/logo.svg"),
            mime: "image/svg+xml",
        }),
        "icons/logo.svg" => Some(AssetData {
            bytes: include_bytes!("../assets/icons/logo.svg"),
            mime: "image/svg+xml",
        }),
        "icons/logo_32.png" => Some(AssetData {
            bytes: include_bytes!("../assets/icons/logo_32.png"),
            mime: "image/png",
        }),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_assets_exist() {
        let index = get_asset("web/index.html").expect("index.html must exist");
        assert!(index.bytes.len() > 1000);
        assert_eq!(index.mime, "text/html; charset=utf-8");

        let app_js = get_asset("web/app.js").expect("app.js must exist");
        assert!(app_js.bytes.len() > 5000);

        let marked = get_asset("js/marked.min.js").expect("marked.min.js must exist");
        assert!(marked.bytes.len() > 10000);

        let mermaid = get_asset("js/mermaid.min.js").expect("mermaid.min.js must exist");
        assert!(mermaid.bytes.len() > 500000);

        let katex_font = get_asset("fonts/KaTeX_Main-Regular.woff2").expect("KaTeX font must exist");
        assert!(katex_font.bytes.len() > 1000);
        assert_eq!(katex_font.mime, "font/woff2");

        let logo = get_asset("web/logo.svg").expect("web/logo.svg must exist");
        assert!(logo.bytes.len() > 500);
        assert_eq!(logo.mime, "image/svg+xml");
    }
}
