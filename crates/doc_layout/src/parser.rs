//! pulldown-cmark → flat AST adapter.
//!
//! The AST is flat by design. Nested constructs inside a top-level container
//! (lists inside quotes, nested lists, code inside list items, …) are folded
//! into the container's inline spans using explicit line breaks and textual
//! markers (`◦`, `▪`, `1.`), so ordered-list numbering stays correct and
//! document order is preserved.

use std::collections::HashMap;
use std::mem;

use pulldown_cmark::{
    Alignment as CmAlignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser,
    Tag, TagEnd,
};

use crate::ast::{
    parse_shields_badge_url, plain_text, spans_are_blank, Alignment, CalloutKind, InlineSpan, MarkdownBlock, TableCell,
    TableData,
};
use crate::shaper::thai;

/// GFM-flavoured option set used by QuickMD.
pub fn parser_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
}

/// Parses Markdown source into the flat block AST.
pub fn parse_markdown(source: &str) -> Vec<MarkdownBlock> {
    let mut builder = AstBuilder::default();
    for event in Parser::new_ext(source, parser_options()) {
        builder.handle(event);
    }
    builder.finish()
}

/// GitHub-compatible heading slug. Thai letters *and* their combining marks
/// are preserved so `สวัสดี ชาวโลก` → `สวัสดี-ชาวโลก`.
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_dash = false;
    for ch in text.trim().chars() {
        if ch.is_alphanumeric() || thai::is_thai(ch) || ch == '_' {
            out.extend(ch.to_lowercase());
            last_was_dash = false;
        } else if ch == '-' || ch.is_whitespace() {
            if !last_was_dash && !out.is_empty() {
                out.push('-');
                last_was_dash = true;
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

const CHECKED_GLYPH: &str = "☑ ";
const UNCHECKED_GLYPH: &str = "☐ ";

fn checkbox_glyph(checked: bool) -> &'static str {
    if checked { CHECKED_GLYPH } else { UNCHECKED_GLYPH }
}

struct LinkAcc {
    url: String,
    text: String,
}

#[derive(Default)]
struct ItemAcc {
    checked: Option<bool>,
    spans: Vec<InlineSpan>,
    sep: bool,
}

/// Outermost container block currently open.
enum Root {
    Quote { kind: Option<CalloutKind>, spans: Vec<InlineSpan>, sep: bool },
    List { ordered: bool, start: u64, items: Vec<ItemAcc>, current: Option<ItemAcc> },
}

/// Containers nested inside the root (folded into the root's spans).
enum Nest {
    Quote,
    List { next_number: Option<u64> },
}

struct TableAcc {
    aligns: Vec<Alignment>,
    headers: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
    row: Vec<TableCell>,
}

#[derive(Default)]
struct AstBuilder {
    blocks: Vec<MarkdownBlock>,
    root: Option<Root>,
    nest: Vec<Nest>,
    inline: Vec<InlineSpan>,
    strong: u32,
    emph: u32,
    link: Option<LinkAcc>,
    image: Option<LinkAcc>,
    code: Option<(String, String)>,
    heading: Option<u8>,
    table: Option<TableAcc>,
    html_depth: u32,
    align: Alignment,
    slug_counts: HashMap<String, usize>,
}

impl AstBuilder {
    fn handle(&mut self, event: Event<'_>) {
        #[allow(unreachable_patterns)]
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => self.push_atomic(InlineSpan::InlineCode(t.to_string())),
            Event::InlineMath(t) => self.push_atomic(InlineSpan::InlineMath(t.trim().to_string())),
            Event::DisplayMath(t) => self.display_math(&t),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => self.rule(),
            Event::TaskListMarker(checked) => self.task_marker(checked),
            Event::FootnoteReference(label) => {
                self.push_atomic(InlineSpan::Text(format!("[{}]", &*label)))
            }
            Event::InlineHtml(html) => {
                if html.trim_start().to_ascii_lowercase().starts_with("<br") {
                    self.text("\n");
                } else {
                    self.handle_inline_html(&html);
                }
            }
            Event::Html(html) => self.handle_html(&html),
            _ => {}
        }
    }

    // ---------------------------------------------------------------- start

    fn start(&mut self, tag: Tag<'_>) {
        #[allow(unreachable_patterns)]
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.heading = Some(heading_level(level));
            }
            Tag::BlockQuote(kind) => {
                self.flush_inline();
                if self.root.is_none() && self.table.is_none() {
                    self.root = Some(Root::Quote {
                        kind: kind.map(map_quote_kind),
                        spans: Vec::new(),
                        sep: false,
                    });
                } else {
                    self.nest.push(Nest::Quote);
                    self.mark_sep();
                }
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => fence_language(&info),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::HtmlBlock => self.html_depth += 1,
            Tag::List(start) => {
                self.flush_inline();
                if self.root.is_none() {
                    self.root = Some(Root::List {
                        ordered: start.is_some(),
                        start: start.unwrap_or(1),
                        items: Vec::new(),
                        current: None,
                    });
                } else {
                    self.nest.push(Nest::List { next_number: start });
                }
            }
            Tag::Item => self.start_item(),
            Tag::FootnoteDefinition(label) => {
                self.flush_inline();
                self.inline.push(InlineSpan::Text(format!("[{}] ", &*label)));
            }
            Tag::Table(aligns) => {
                self.flush_inline();
                self.table = Some(TableAcc {
                    aligns: aligns.iter().map(map_alignment).collect(),
                    headers: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.row.clear();
                }
            }
            Tag::TableCell => {}
            Tag::Emphasis => self.emph += 1,
            Tag::Strong => self.strong += 1,
            Tag::Link { dest_url, .. } => {
                if self.link.is_none() && self.image.is_none() {
                    self.link = Some(LinkAcc { url: dest_url.to_string(), text: String::new() });
                }
            }
            Tag::Image { dest_url, .. } => {
                if self.image.is_none() {
                    self.image = Some(LinkAcc { url: dest_url.to_string(), text: String::new() });
                }
            }
            _ => {}
        }
    }

    fn start_item(&mut self) {
        if matches!(self.nest.last(), Some(Nest::List { .. })) {
            self.flush_inline();
            let depth = self.nest.iter().filter(|n| matches!(n, Nest::List { .. })).count();
            let indent_level = if matches!(self.root, Some(Root::Quote { .. })) {
                depth.saturating_sub(1)
            } else {
                depth
            };
            let marker = match self.nest.last_mut() {
                Some(Nest::List { next_number: Some(n) }) => {
                    let m = format!("{n}. ");
                    *n += 1;
                    m
                }
                _ => ["• ", "◦ ", "▪ "][indent_level % 3].to_string(),
            };
            let indent = "\u{2003}".repeat(indent_level);
            self.push_marker_to_root(format!("{indent}{marker}"));
        } else if let Some(Root::List { items, current, .. }) = self.root.as_mut() {
            if let Some(prev) = current.take() {
                items.push(prev);
            }
            *current = Some(ItemAcc::default());
        }
    }

    // ------------------------------------------------------------------ end

    fn end(&mut self, tag: TagEnd) {
        #[allow(unreachable_patterns)]
        match tag {
            TagEnd::Paragraph => self.end_paragraph(),
            TagEnd::Heading { .. } => self.end_heading(),
            TagEnd::BlockQuote { .. } => self.end_blockquote(),
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::HtmlBlock => self.html_depth = self.html_depth.saturating_sub(1),
            TagEnd::List { .. } => self.end_list(),
            TagEnd::Item => self.end_item(),
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.headers = mem::take(&mut t.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            TagEnd::TableCell => self.end_table_cell(),
            TagEnd::Emphasis => self.emph = self.emph.saturating_sub(1),
            TagEnd::Strong => self.strong = self.strong.saturating_sub(1),
            TagEnd::Link => {
                let img_opt = self.image.take();
                if let Some(LinkAcc { url, text }) = self.link.take() {
                    if let Some(img) = img_opt {
                        let alt = if !img.text.trim().is_empty() { img.text } else { text };
                        self.inline.push(InlineSpan::Image {
                            alt,
                            url: img.url,
                            link_url: Some(url),
                        });
                    } else {
                        let text = if text.trim().is_empty() { url.clone() } else { text };
                        self.inline.push(InlineSpan::Link { text, url });
                    }
                }
            }
            TagEnd::Image => {
                if self.link.is_none() {
                    if let Some(LinkAcc { url, text }) = self.image.take() {
                        let alt = if text.trim().is_empty() { "image".to_string() } else { text };
                        self.inline.push(InlineSpan::Image { alt, url, link_url: None });
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_inline_html(&mut self, html: &str) {
        let lower = html.to_ascii_lowercase();
        if lower.contains("<div") || lower.contains("<p") {
            if lower.contains("align=\"center\"") || lower.contains("align='center'") || lower.contains("align=center") {
                self.align = Alignment::Center;
            } else if lower.contains("align=\"right\"") || lower.contains("align='right'") || lower.contains("align=right") {
                self.align = Alignment::Right;
            }
        }
        if lower.contains("</div>") || lower.contains("</p>") {
            self.align = Alignment::Left;
        }

        if lower.contains("<a ") && lower.contains("href=") {
            if let Some(href) = extract_attribute(html, "href") {
                self.link = Some(LinkAcc { url: href, text: String::new() });
            }
        }
        if lower.contains("<img") {
            if let Some(src) = extract_attribute(html, "src") {
                let alt = extract_attribute(html, "alt").unwrap_or_default();
                let width_override = extract_attribute(html, "width").and_then(|w| w.parse::<f32>().ok());
                let link_url = extract_attribute(html, "href").or_else(|| self.link.take().map(|l| l.url));
                let final_url = if let Some(w) = width_override {
                    format!("{}#width={}", src, w)
                } else {
                    src
                };
                self.inline.push(InlineSpan::Image {
                    alt,
                    url: final_url,
                    link_url,
                });
            }
        }
    }

    fn handle_html(&mut self, html: &str) {
        let lower = html.to_ascii_lowercase();
        if lower.contains("<div") || lower.contains("<p") {
            if lower.contains("align=\"center\"") || lower.contains("align='center'") || lower.contains("align=center") {
                self.align = Alignment::Center;
            } else if lower.contains("align=\"right\"") || lower.contains("align='right'") || lower.contains("align=right") {
                self.align = Alignment::Right;
            }
        }
        if lower.contains("</div>") || lower.contains("</p>") {
            self.align = Alignment::Left;
        }

        if lower.contains("<img") {
            if let Some(src) = extract_attribute(html, "src") {
                let alt = extract_attribute(html, "alt").unwrap_or_default();
                let width_override = extract_attribute(html, "width").and_then(|w| w.parse::<f32>().ok());
                let link_url = extract_attribute(html, "href");
                self.blocks.push(MarkdownBlock::Image {
                    url: src,
                    alt,
                    link_url,
                    width_override,
                    align: self.align,
                });
            }
        }
    }

    fn end_paragraph(&mut self) {
        if self.root.is_some() {
            self.flush_inline();
            self.mark_sep();
        } else {
            let mut spans = mem::take(&mut self.inline);
            trim_edges(&mut spans);
            if spans_are_blank(&spans) {
                return;
            }

            // 1. Detect if entire paragraph is a row of shields.io badges
            let non_blank: Vec<&InlineSpan> = spans.iter().filter(|s| !s.is_blank()).collect();
            let is_all_badges = !non_blank.is_empty() && non_blank.iter().all(|s| match s {
                InlineSpan::Image { url, .. } => url.contains("shields.io/badge/"),
                InlineSpan::Link { url, .. } => url.contains("shields.io/badge/"),
                _ => false,
            });
            if is_all_badges {
                let mut badges = Vec::with_capacity(non_blank.len());
                for s in &non_blank {
                    match s {
                        InlineSpan::Image { alt, url, link_url } => {
                            if let Some(mut b) = parse_shields_badge_url(url) {
                                if !alt.is_empty() && b.label.is_empty() {
                                    b.label = alt.clone();
                                }
                                b.link_url = link_url.clone();
                                badges.push(b);
                            }
                        }
                        InlineSpan::Link { text, url } => {
                            if let Some(mut b) = parse_shields_badge_url(url) {
                                if !text.is_empty() && b.label.is_empty() {
                                    b.label = text.clone();
                                }
                                badges.push(b);
                            }
                        }
                        _ => {}
                    }
                }
                if !badges.is_empty() {
                    self.blocks.push(MarkdownBlock::BadgeRow { badges, align: self.align });
                    return;
                }
            }

            // 2. Detect if entire paragraph is a single standalone image
            if non_blank.len() == 1 {
                if let InlineSpan::Image { alt, url, link_url } = non_blank[0] {
                    let (clean_url, width_override) = if let Some(idx) = url.find("#width=") {
                        let w_str = &url[idx + 7..];
                        let w = w_str.parse::<f32>().ok();
                        (url[..idx].to_string(), w)
                    } else {
                        (url.clone(), None)
                    };
                    self.blocks.push(MarkdownBlock::Image {
                        url: clean_url,
                        alt: alt.clone(),
                        link_url: link_url.clone(),
                        width_override,
                        align: self.align,
                    });
                    return;
                }
            }

            self.blocks.push(MarkdownBlock::Paragraph(spans));
        }
    }

    fn end_heading(&mut self) {
        let level = self.heading.take().unwrap_or(1);
        let spans = mem::take(&mut self.inline);
        let text = plain_text(&spans).trim().to_string();
        if self.root.is_some() {
            self.append_to_root(vec![InlineSpan::Bold(text)]);
            self.mark_sep();
        } else {
            let id = self.unique_slug(&text);
            self.blocks.push(MarkdownBlock::Heading { level, text, id });
        }
    }

    fn end_blockquote(&mut self) {
        if matches!(self.nest.last(), Some(Nest::Quote)) {
            self.nest.pop();
            self.flush_inline();
            self.mark_sep();
            return;
        }
        self.flush_inline();
        match self.root.take() {
            Some(Root::Quote { kind, spans, .. }) => self.emit_quote(kind, spans),
            other => self.root = other,
        }
    }

    fn emit_quote(&mut self, kind: Option<CalloutKind>, spans: Vec<InlineSpan>) {
        let (kind, custom_title, mut content) = match kind {
            Some(k) => {
                let (custom, rem) = extract_callout_title_from_spans(&spans);
                (Some(k), custom, rem)
            }
            None => detect_inline_callout(spans),
        };
        trim_edges(&mut content);
        match kind {
            Some(kind) => {
                let title = custom_title.unwrap_or_else(|| kind.label().to_string());
                self.blocks.push(MarkdownBlock::Callout {
                    kind,
                    title,
                    content,
                })
            }
            None => {
                if !spans_are_blank(&content) {
                    self.blocks.push(MarkdownBlock::Blockquote(content));
                }
            }
        }
    }

    fn end_code_block(&mut self) {
        let Some((lang, mut code)) = self.code.take() else { return };
        while code.ends_with('\n') || code.ends_with('\r') {
            code.pop();
        }
        if self.root.is_none() && self.table.is_none() {
            let block = match lang.to_ascii_lowercase().as_str() {
                "mermaid" => MarkdownBlock::MermaidBlock(code),
                "math" | "latex" | "tex" | "katex" => MarkdownBlock::MathBlock(code),
                _ => MarkdownBlock::CodeBlock { lang, code },
            };
            self.blocks.push(block);
        } else {
            self.mark_sep();
            self.append_to_root(vec![InlineSpan::InlineCode(code)]);
            self.mark_sep();
        }
    }

    fn end_list(&mut self) {
        if matches!(self.nest.last(), Some(Nest::List { .. })) {
            self.nest.pop();
            self.flush_inline();
            self.mark_sep();
            return;
        }
        self.flush_inline();
        match self.root.take() {
            Some(Root::List { ordered, start, mut items, current }) => {
                if let Some(c) = current {
                    items.push(c);
                }
                self.emit_list(ordered, start, items);
            }
            other => self.root = other,
        }
    }

    fn emit_list(&mut self, ordered: bool, start: u64, items: Vec<ItemAcc>) {
        if items.is_empty() {
            return;
        }
        if items.iter().all(|i| i.checked.is_some()) {
            let items = items
                .into_iter()
                .map(|mut i| {
                    trim_edges(&mut i.spans);
                    (i.checked.unwrap_or(false), i.spans)
                })
                .collect();
            self.blocks.push(MarkdownBlock::CheckList(items));
        } else {
            let items = items
                .into_iter()
                .map(|mut i| {
                    trim_edges(&mut i.spans);
                    if let Some(c) = i.checked {
                        i.spans.insert(0, InlineSpan::Text(checkbox_glyph(c).to_string()));
                    }
                    i.spans
                })
                .collect();
            self.blocks.push(MarkdownBlock::List { ordered, start, items });
        }
    }

    fn end_item(&mut self) {
        self.flush_inline();
        if self.nest.is_empty() {
            if let Some(Root::List { items, current, .. }) = self.root.as_mut() {
                if let Some(c) = current.take() {
                    items.push(c);
                }
            }
        } else {
            self.mark_sep();
        }
    }

    fn end_table_cell(&mut self) {
        let mut spans = mem::take(&mut self.inline);
        trim_edges(&mut spans);
        if let Some(t) = self.table.as_mut() {
            let alignment = t.aligns.get(t.row.len()).copied().unwrap_or_default();
            t.row.push(TableCell { spans, alignment });
        }
    }

    fn end_table(&mut self) {
        let Some(acc) = self.table.take() else { return };
        let TableAcc { aligns, mut headers, mut rows, .. } = acc;
        let cols = headers.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
        let pad = |row: &mut Vec<TableCell>| {
            row.truncate(cols);
            while row.len() < cols {
                let a = aligns.get(row.len()).copied().unwrap_or_default();
                row.push(TableCell::empty(a));
            }
        };
        pad(&mut headers);
        for r in rows.iter_mut() {
            pad(r);
        }

        if self.root.is_none() {
            if cols > 0 {
                self.blocks.push(MarkdownBlock::Table(TableData { headers, rows }));
            }
        } else {
            for row in std::iter::once(headers).chain(rows) {
                let mut spans = Vec::new();
                for (i, cell) in row.into_iter().enumerate() {
                    if i > 0 {
                        spans.push(InlineSpan::Text(" | ".into()));
                    }
                    spans.extend(cell.spans);
                }
                self.append_to_root(spans);
                self.mark_sep();
            }
        }
    }

    // --------------------------------------------------------------- inline

    fn text(&mut self, s: &str) {
        if self.html_depth > 0 {
            return;
        }
        if let Some((_, code)) = self.code.as_mut() {
            code.push_str(s);
            return;
        }
        if let Some(img) = self.image.as_mut() {
            img.text.push_str(s);
            return;
        }
        if let Some(link) = self.link.as_mut() {
            link.text.push_str(s);
            return;
        }
        self.push_styled(s);
    }

    fn push_styled(&mut self, s: &str) {
        let (b, i) = (self.strong > 0, self.emph > 0);
        if let Some(last) = self.inline.last_mut() {
            match (last, b, i) {
                (InlineSpan::Text(t), false, false)
                | (InlineSpan::Bold(t), true, false)
                | (InlineSpan::Italic(t), false, true)
                | (InlineSpan::BoldItalic(t), true, true) => {
                    t.push_str(s);
                    return;
                }
                _ => {}
            }
        }
        let owned = s.to_string();
        self.inline.push(match (b, i) {
            (false, false) => InlineSpan::Text(owned),
            (true, false) => InlineSpan::Bold(owned),
            (false, true) => InlineSpan::Italic(owned),
            (true, true) => InlineSpan::BoldItalic(owned),
        });
    }

    fn push_atomic(&mut self, span: InlineSpan) {
        if let Some(img) = self.image.as_mut() {
            img.text.push_str(span.text());
            return;
        }
        if let Some(link) = self.link.as_mut() {
            link.text.push_str(span.text());
            return;
        }
        self.inline.push(span);
    }

    fn display_math(&mut self, latex: &str) {
        let top_level = self.root.is_none()
            && self.table.is_none()
            && self.heading.is_none()
            && self.link.is_none()
            && self.image.is_none();
        if top_level {
            let mut before = mem::take(&mut self.inline);
            trim_edges(&mut before);
            if !spans_are_blank(&before) {
                self.blocks.push(MarkdownBlock::Paragraph(before));
            }
            self.blocks.push(MarkdownBlock::MathBlock(latex.trim().to_string()));
        } else {
            self.push_atomic(InlineSpan::InlineMath(latex.trim().to_string()));
        }
    }

    fn rule(&mut self) {
        self.flush_inline();
        if self.root.is_none() && self.table.is_none() {
            self.blocks.push(MarkdownBlock::HorizontalRule);
        } else {
            self.mark_sep();
        }
    }

    fn task_marker(&mut self, checked: bool) {
        if self.nest.is_empty() {
            if let Some(Root::List { current: Some(item), .. }) = self.root.as_mut() {
                item.checked = Some(checked);
                return;
            }
        }
        self.push_atomic(InlineSpan::Text(checkbox_glyph(checked).to_string()));
    }

    // ---------------------------------------------------------- root sinks

    fn sink_parts(&mut self) -> Option<(&mut Vec<InlineSpan>, &mut bool)> {
        match self.root.as_mut()? {
            Root::Quote { spans, sep, .. } => Some((spans, sep)),
            Root::List { current, .. } => {
                let item = current.get_or_insert_with(ItemAcc::default);
                Some((&mut item.spans, &mut item.sep))
            }
        }
    }

    /// Moves pending inline spans to the root container, or to a top-level
    /// paragraph when no container is open.
    fn flush_inline(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let spans = mem::take(&mut self.inline);
        self.append_to_root(spans);
    }

    fn append_to_root(&mut self, mut content: Vec<InlineSpan>) {
        if spans_are_blank(&content) {
            return;
        }
        if self.root.is_none() {
            trim_edges(&mut content);
            self.blocks.push(MarkdownBlock::Paragraph(content));
            return;
        }
        if let Some((sink, sep)) = self.sink_parts() {
            if *sep && !sink.is_empty() {
                sink.push(InlineSpan::Text("\n".into()));
            }
            *sep = false;
            sink.extend(content);
        }
    }

    fn push_marker_to_root(&mut self, marker: String) {
        if let Some((sink, sep)) = self.sink_parts() {
            if sink.is_empty() {
                sink.push(InlineSpan::Text(marker));
            } else {
                sink.push(InlineSpan::Text(format!("\n{marker}")));
            }
            *sep = false;
        }
    }

    fn mark_sep(&mut self) {
        match self.root.as_mut() {
            Some(Root::Quote { sep, .. }) => *sep = true,
            Some(Root::List { current: Some(item), .. }) => item.sep = true,
            _ => {}
        }
    }

    fn unique_slug(&mut self, text: &str) -> String {
        let mut base = slugify(text);
        if base.is_empty() {
            base = "section".to_string();
        }
        let count = self.slug_counts.entry(base.clone()).or_insert(0);
        let id = if *count == 0 { base.clone() } else { format!("{base}-{count}") };
        *count += 1;
        id
    }

    fn finish(mut self) -> Vec<MarkdownBlock> {
        if self.code.is_some() {
            self.end_code_block();
        }
        if self.table.is_some() {
            self.end_table();
        }
        self.flush_inline();
        self.nest.clear();
        match self.root.take() {
            Some(Root::Quote { kind, spans, .. }) => self.emit_quote(kind, spans),
            Some(Root::List { ordered, start, mut items, current }) => {
                if let Some(c) = current {
                    items.push(c);
                }
                self.emit_list(ordered, start, items);
            }
            None => {}
        }
        self.blocks
    }
}

// -------------------------------------------------------------------- helpers

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn map_quote_kind(kind: BlockQuoteKind) -> CalloutKind {
    match kind {
        BlockQuoteKind::Note => CalloutKind::Note,
        BlockQuoteKind::Tip => CalloutKind::Tip,
        BlockQuoteKind::Important => CalloutKind::Important,
        BlockQuoteKind::Warning => CalloutKind::Warning,
        BlockQuoteKind::Caution => CalloutKind::Caution,
    }
}

fn map_alignment(a: &CmAlignment) -> Alignment {
    match a {
        CmAlignment::Center => Alignment::Center,
        CmAlignment::Right => Alignment::Right,
        CmAlignment::Left | CmAlignment::None => Alignment::Left,
    }
}

fn fence_language(info: &str) -> String {
    info.split(|c: char| c.is_whitespace() || c == ',' || c == '{' || c == '}')
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .trim_start_matches('.')
        .to_string()
}

/// Trims leading whitespace of the first text span and trailing whitespace of
/// the last, then drops empty text spans.
fn trim_edges(spans: &mut Vec<InlineSpan>) {
    if let Some(InlineSpan::Text(t)) = spans.first_mut() {
        let owned = t.trim_start().to_string();
        *t = owned;
    }
    if let Some(InlineSpan::Text(t)) = spans.last_mut() {
        let len = t.trim_end().len();
        t.truncate(len);
    }
    spans.retain(|s| !matches!(s, InlineSpan::Text(t) if t.is_empty()));
}

/// Fallback `[!KIND]` detection (used when the GFM tag was not produced).
fn parse_callout_marker(spans: &[InlineSpan]) -> Option<(CalloutKind, String)> {
    let InlineSpan::Text(first) = spans.first()? else { return None };
    let trimmed = first.trim_start();
    let rest = trimmed.strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = CalloutKind::from_marker(&rest[..close])?;
    Some((kind, rest[close + 1..].trim_start().to_string()))
}

fn extract_callout_title_from_spans(spans: &[InlineSpan]) -> (Option<String>, Vec<InlineSpan>) {
    let mut content = spans.to_vec();
    while matches!(content.first(), Some(s) if s.is_blank()) {
        content.remove(0);
    }
    if let Some(InlineSpan::Bold(b)) = content.first() {
        let title = b.trim().to_string();
        content.remove(0);
        while matches!(content.first(), Some(s) if s.is_blank()) {
            content.remove(0);
        }
        if !title.is_empty() {
            return (Some(title), content);
        }
    }
    if let Some(InlineSpan::Text(t)) = content.first() {
        if let Some(nl) = t.find('\n') {
            let title = t[..nl].trim().trim_matches('*').trim().to_string();
            let rem = t[nl + 1..].trim_start().to_string();
            if rem.is_empty() {
                content.remove(0);
            } else {
                content[0] = InlineSpan::Text(rem);
            }
            while matches!(content.first(), Some(s) if s.is_blank()) {
                content.remove(0);
            }
            if !title.is_empty() {
                return (Some(title), content);
            }
        }
    }
    (None, content)
}

fn detect_inline_callout(
    mut spans: Vec<InlineSpan>,
) -> (Option<CalloutKind>, Option<String>, Vec<InlineSpan>) {
    match parse_callout_marker(&spans) {
        None => (None, None, spans),
        Some((kind, rest)) => {
            let (custom_title, remaining) = if let Some(nl) = rest.find('\n') {
                (rest[..nl].trim(), rest[nl + 1..].trim_start())
            } else {
                (rest.trim(), "")
            };
            let mut title = if custom_title.is_empty() {
                None
            } else {
                let clean = custom_title.trim_matches('*').trim();
                if clean.is_empty() {
                    None
                } else {
                    Some(clean.to_string())
                }
            };

            if remaining.is_empty() {
                spans.remove(0);
            } else {
                spans[0] = InlineSpan::Text(remaining.to_string());
            }
            while matches!(spans.first(), Some(s) if s.is_blank()) {
                spans.remove(0);
            }

            if title.is_none() {
                if let Some(InlineSpan::Bold(b)) = spans.first() {
                    let clean = b.trim().to_string();
                    if !clean.is_empty() {
                        title = Some(clean);
                        spans.remove(0);
                        while matches!(spans.first(), Some(s) if s.is_blank()) {
                            spans.remove(0);
                        }
                    }
                }
            }

            (Some(kind), title, spans)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(block: &MarkdownBlock) -> &[InlineSpan] {
        match block {
            MarkdownBlock::Paragraph(s) => s,
            other => panic!("expected paragraph, got {other:?}"),
        }
    }

    #[test]
    fn headings_get_unique_github_ids() {
        let blocks = parse_markdown("# Hello World\n\n## Hello World\n\n### สวัสดี ชาวโลก!\n");
        let ids: Vec<&str> = blocks
            .iter()
            .filter_map(|b| match b {
                MarkdownBlock::Heading { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["hello-world", "hello-world-1", "สวัสดี-ชาวโลก"]);
        assert!(matches!(&blocks[0], MarkdownBlock::Heading { level: 1, text, .. } if text == "Hello World"));
    }

    #[test]
    fn inline_styles_map_to_spans() {
        let blocks = parse_markdown(
            "Hello **bold** *it* ***both*** `code` [link](https://x.dev) $a^2$",
        );
        let spans = para(&blocks[0]);
        assert!(spans.contains(&InlineSpan::Bold("bold".into())));
        assert!(spans.contains(&InlineSpan::Italic("it".into())));
        assert!(spans.contains(&InlineSpan::BoldItalic("both".into())));
        assert!(spans.contains(&InlineSpan::InlineCode("code".into())));
        assert!(spans.contains(&InlineSpan::Link { text: "link".into(), url: "https://x.dev".into() }));
        assert!(spans.contains(&InlineSpan::InlineMath("a^2".into())));
    }

    #[test]
    fn fenced_code_mermaid_and_math_blocks() {
        let src = "```rust\nfn main() {}\n```\n\n```mermaid\ngraph TD; A-->B\n```\n\n```math\nE=mc^2\n```\n\n$$x^2 + y^2$$\n";
        let blocks = parse_markdown(src);
        assert_eq!(
            blocks,
            vec![
                MarkdownBlock::CodeBlock { lang: "rust".into(), code: "fn main() {}".into() },
                MarkdownBlock::MermaidBlock("graph TD; A-->B".into()),
                MarkdownBlock::MathBlock("E=mc^2".into()),
                MarkdownBlock::MathBlock("x^2 + y^2".into()),
            ]
        );
    }

    #[test]
    fn display_math_splits_paragraph() {
        let blocks = parse_markdown("before $$x$$ after\n");
        assert_eq!(blocks.len(), 3);
        assert_eq!(plain_text(para(&blocks[0])), "before");
        assert_eq!(blocks[1], MarkdownBlock::MathBlock("x".into()));
        assert_eq!(plain_text(para(&blocks[2])), "after");
    }

    #[test]
    fn tables_with_alignment() {
        let blocks = parse_markdown("| A | B | C |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n| x |\n");
        let MarkdownBlock::Table(t) = &blocks[0] else { panic!("expected table") };
        assert_eq!(t.headers.len(), 3);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.headers[0].alignment, Alignment::Left);
        assert_eq!(t.headers[1].alignment, Alignment::Center);
        assert_eq!(t.headers[2].alignment, Alignment::Right);
        assert!(t.rows.iter().all(|r| r.len() == 3));
        assert_eq!(t.rows[0][1].plain_text(), "2");
    }

    #[test]
    fn gfm_callouts() {
        let blocks = parse_markdown("> [!WARNING]\n> Be careful with ไม้เอก\n");
        match &blocks[0] {
            MarkdownBlock::Callout { kind, title, content } => {
                assert_eq!(*kind, CalloutKind::Warning);
                assert_eq!(title, "Warning");
                assert!(plain_text(content).contains("Be careful with ไม้เอก"));
                assert!(!plain_text(content).contains("[!"));
            }
            other => panic!("expected callout, got {other:?}"),
        }
    }

    #[test]
    fn plain_blockquote() {
        let blocks = parse_markdown("> quoted *text*\n>\n> second\n");
        let MarkdownBlock::Blockquote(spans) = &blocks[0] else { panic!() };
        let text = plain_text(spans);
        assert!(text.starts_with("quoted text"));
        assert!(text.contains('\n'));
        assert!(text.ends_with("second"));
    }

    #[test]
    fn task_lists_become_checklists() {
        let blocks = parse_markdown("- [x] done\n- [ ] todo\n");
        let MarkdownBlock::CheckList(items) = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(items.len(), 2);
        assert!(items[0].0);
        assert!(!items[1].0);
        assert_eq!(plain_text(&items[0].1).trim(), "done");
        assert_eq!(plain_text(&items[1].1).trim(), "todo");
    }

    #[test]
    fn ordered_list_start_and_nested_flattening() {
        let blocks = parse_markdown("3. a\n4. b\n");
        assert!(matches!(&blocks[0], MarkdownBlock::List { ordered: true, start: 3, items } if items.len() == 2));

        let blocks = parse_markdown("- a\n  - b\n- c\n");
        let MarkdownBlock::List { ordered: false, items, .. } = &blocks[0] else { panic!() };
        assert_eq!(items.len(), 2, "nested items fold into their parent");
        let first = plain_text(&items[0]);
        assert!(first.starts_with('a') && first.contains('\n') && first.ends_with('b'));
        assert_eq!(plain_text(&items[1]), "c");
    }

    #[test]
    fn rule_and_thai_paragraph_preserved() {
        let blocks = parse_markdown("ภาษาไทย ปั้นน้ำเป็นตัว\n\n---\n");
        assert_eq!(plain_text(para(&blocks[0])), "ภาษาไทย ปั้นน้ำเป็นตัว");
        assert_eq!(blocks[1], MarkdownBlock::HorizontalRule);
    }

    #[test]
    fn slugify_rules() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("ผู้ใหญ่ลี้"), "ผู้ใหญ่ลี้");
        assert_eq!(slugify("a_b-c"), "a_b-c");
    }

    #[test]
    fn custom_callout_title_with_bold_header() {
        let blocks = parse_markdown("> [!NOTE] **User Profile**\n> \u{2022} **User**: John\n");
        let MarkdownBlock::Callout { kind, title, content } = &blocks[0] else { panic!("{blocks:?}") };
        assert_eq!(*kind, CalloutKind::Note);
        assert_eq!(title, "User Profile");
        assert_eq!(content[0], InlineSpan::Text("• ".into()));
        assert_eq!(content[1], InlineSpan::Bold("User".into()));
    }
}


fn extract_attribute(html: &str, attr: &str) -> Option<String> {
    let lower_html = html.to_ascii_lowercase();
    let pat = format!("{}=", attr.to_ascii_lowercase());
    let pos = lower_html.find(&pat)?;
    let rem = &html[pos + pat.len()..];
    let rem = rem.trim_start();
    if rem.starts_with('"') {
        let inside = &rem[1..];
        let end = inside.find('"')?;
        Some(inside[..end].to_string())
    } else if rem.starts_with('\'') {
        let inside = &rem[1..];
        let end = inside.find('\'')?;
        Some(inside[..end].to_string())
    } else {
        let end = rem.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rem.len());
        Some(rem[..end].to_string())
    }
}
