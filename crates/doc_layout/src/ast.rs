//! Flat document AST. Every top-level Markdown construct maps to exactly one
//! [`MarkdownBlock`], giving one geometry record per block (sorted by Y), which
//! is what makes O(log N) viewport culling possible.

use tiny_skia::Color;

/// GitHub-style alerts: `> [!NOTE]`, `> [!TIP]`, ...
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CalloutKind {
    /// Blue
    Note,
    /// Green
    Tip,
    /// Purple
    Important,
    /// Amber
    Warning,
    /// Red
    Caution,
}

impl CalloutKind {
    pub const ALL: [CalloutKind; 5] = [
        CalloutKind::Note,
        CalloutKind::Tip,
        CalloutKind::Important,
        CalloutKind::Warning,
        CalloutKind::Caution,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            CalloutKind::Note => "Note",
            CalloutKind::Tip => "Tip",
            CalloutKind::Important => "Important",
            CalloutKind::Warning => "Warning",
            CalloutKind::Caution => "Caution",
        }
    }

    /// Parses `NOTE`, `!tip`, `Warning`, ... (case-insensitive, with aliases).
    pub fn from_marker(marker: &str) -> Option<Self> {
        let m = marker.trim().trim_start_matches('!').trim().to_ascii_uppercase();
        match m.as_str() {
            "NOTE" | "INFO" => Some(CalloutKind::Note),
            "TIP" | "HINT" => Some(CalloutKind::Tip),
            "IMPORTANT" => Some(CalloutKind::Important),
            "WARNING" | "WARN" => Some(CalloutKind::Warning),
            "CAUTION" | "DANGER" | "ERROR" => Some(CalloutKind::Caution),
            _ => None,
        }
    }

    /// Accent colour as straight RGBA8.
    pub const fn accent_rgba(self) -> [u8; 4] {
        match self {
            CalloutKind::Note => [9, 105, 218, 255],
            CalloutKind::Tip => [26, 127, 55, 255],
            CalloutKind::Important => [130, 80, 223, 255],
            CalloutKind::Warning => [191, 135, 0, 255],
            CalloutKind::Caution => [207, 34, 46, 255],
        }
    }

    pub fn accent_color(self) -> Color {
        let [r, g, b, a] = self.accent_rgba();
        Color::from_rgba8(r, g, b, a)
    }

    /// Translucent tint for the callout background.
    pub fn background_color(self) -> Color {
        let [r, g, b, _] = self.accent_rgba();
        Color::from_rgba8(r, g, b, 26)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InlineSpan {
    Text(String),
    Bold(String),
    Italic(String),
    BoldItalic(String),
    InlineCode(String),
    Link { text: String, url: String },
    InlineMath(String),
    Image { alt: String, url: String, link_url: Option<String> },
}

impl InlineSpan {
    /// The visible text of the span (link label for links, raw LaTeX for math).
    pub fn text(&self) -> &str {
        match self {
            InlineSpan::Text(s)
            | InlineSpan::Bold(s)
            | InlineSpan::Italic(s)
            | InlineSpan::BoldItalic(s)
            | InlineSpan::InlineCode(s)
            | InlineSpan::InlineMath(s) => s,
            InlineSpan::Link { text, .. } => text,
            InlineSpan::Image { alt, .. } => alt,
        }
    }

    pub fn is_blank(&self) -> bool {
        match self {
            InlineSpan::Image { .. } => false,
            _ => self.text().trim().is_empty(),
        }
    }
}

/// Concatenates the visible text of all spans.
pub fn plain_text(spans: &[InlineSpan]) -> String {
    let mut out = String::with_capacity(spans.iter().map(|s| s.text().len()).sum());
    for span in spans {
        out.push_str(span.text());
    }
    out
}

pub fn spans_are_blank(spans: &[InlineSpan]) -> bool {
    spans.iter().all(InlineSpan::is_blank)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub spans: Vec<InlineSpan>,
    pub alignment: Alignment,
}

impl TableCell {
    pub fn empty(alignment: Alignment) -> Self {
        Self { spans: Vec::new(), alignment }
    }

    pub fn plain_text(&self) -> String {
        plain_text(&self.spans)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableData {
    pub headers: Vec<TableCell>,
    pub rows: Vec<Vec<TableCell>>,
}

impl TableData {
    pub fn column_count(&self) -> usize {
        self.headers
            .len()
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0))
    }

    /// Header row followed by body rows.
    pub fn all_rows(&self) -> impl Iterator<Item = &Vec<TableCell>> {
        std::iter::once(&self.headers).chain(self.rows.iter())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListType {
    Unordered,
    Ordered { start: u64 },
    Check,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BadgeItem {
    pub label: String,
    pub value: String,
    pub bg_color: [u8; 4],
    pub badge_url: String,
    pub link_url: Option<String>,
}

pub fn parse_shields_badge_url(url: &str) -> Option<BadgeItem> {
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
    Some(BadgeItem {
        label,
        value,
        bg_color,
        badge_url: url.to_string(),
        link_url: None,
    })
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

#[derive(Debug, Clone, PartialEq)]
pub enum MarkdownBlock {
    Heading { level: u8, text: String, id: String },
    Paragraph(Vec<InlineSpan>),
    Blockquote(Vec<InlineSpan>),
    Callout { kind: CalloutKind, title: String, content: Vec<InlineSpan> },
    CodeBlock { lang: String, code: String },
    /// Raw LaTeX for `native_math`.
    MathBlock(String),
    /// Raw Mermaid source for `native_mermaid`.
    MermaidBlock(String),
    Table(TableData),
    List { ordered: bool, start: u64, items: Vec<Vec<InlineSpan>> },
    /// `(checked, spans)`
    CheckList(Vec<(bool, Vec<InlineSpan>)>),
    HorizontalRule,
    Image {
        url: String,
        alt: String,
        link_url: Option<String>,
        width_override: Option<f32>,
        align: Alignment,
    },
    BadgeRow {
        badges: Vec<BadgeItem>,
        align: Alignment,
    },
}

impl MarkdownBlock {
    pub fn list_type(&self) -> Option<ListType> {
        match self {
            MarkdownBlock::List { ordered: true, start, .. } => Some(ListType::Ordered { start: *start }),
            MarkdownBlock::List { ordered: false, .. } => Some(ListType::Unordered),
            MarkdownBlock::CheckList(_) => Some(ListType::Check),
            _ => None,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            MarkdownBlock::Heading { .. } => "heading",
            MarkdownBlock::Paragraph(_) => "paragraph",
            MarkdownBlock::Blockquote(_) => "blockquote",
            MarkdownBlock::Callout { .. } => "callout",
            MarkdownBlock::CodeBlock { .. } => "code",
            MarkdownBlock::MathBlock(_) => "math",
            MarkdownBlock::MermaidBlock(_) => "mermaid",
            MarkdownBlock::Table(_) => "table",
            MarkdownBlock::List { .. } => "list",
            MarkdownBlock::CheckList(_) => "checklist",
            MarkdownBlock::HorizontalRule => "rule",
            MarkdownBlock::Image { .. } => "image",
            MarkdownBlock::BadgeRow { .. } => "badges",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callout_markers_and_colours() {
        assert_eq!(CalloutKind::from_marker("NOTE"), Some(CalloutKind::Note));
        assert_eq!(CalloutKind::from_marker("!tip"), Some(CalloutKind::Tip));
        assert_eq!(CalloutKind::from_marker(" Warning "), Some(CalloutKind::Warning));
        assert_eq!(CalloutKind::from_marker("bogus"), None);
        for kind in CalloutKind::ALL {
            assert_eq!(kind.accent_rgba()[3], 255);
            assert!(!kind.label().is_empty());
        }
    }

    #[test]
    fn plain_text_concatenates() {
        let spans = vec![
            InlineSpan::Text("a ".into()),
            InlineSpan::Bold("b".into()),
            InlineSpan::Link { text: " c".into(), url: "u".into() },
        ];
        assert_eq!(plain_text(&spans), "a b c");
        assert!(!spans_are_blank(&spans));
        assert!(spans_are_blank(&[InlineSpan::Text("  ".into())]));
    }

    #[test]
    fn table_column_count_uses_widest_row() {
        let t = TableData {
            headers: vec![TableCell::empty(Alignment::Left)],
            rows: vec![vec![TableCell::empty(Alignment::Left), TableCell::empty(Alignment::Right)]],
        };
        assert_eq!(t.column_count(), 2);
        assert_eq!(t.all_rows().count(), 2);
    }
}