use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

const MAX_FILE_SIZE: u64 = 30 * 1024 * 1024; // 30 MB

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HeadingItem {
    pub level: usize,
    pub text: String,
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileStats {
    pub words: usize,
    pub chars: usize,
    pub chars_no_spaces: usize,
    pub lines: usize,
    pub reading_time_minutes: usize,
    pub read_time_min: usize,
    pub headings: usize,
    pub headings_count: usize,
    pub code_blocks: usize,
    pub code_blocks_count: usize,
    pub tables: usize,
    pub images: usize,
    pub links: usize,
    pub math_equations: usize,
}

impl Default for FileStats {
    fn default() -> Self {
        Self {
            words: 0,
            chars: 0,
            chars_no_spaces: 0,
            lines: 0,
            reading_time_minutes: 0,
            read_time_min: 0,
            headings: 0,
            headings_count: 0,
            code_blocks: 0,
            code_blocks_count: 0,
            tables: 0,
            images: 0,
            links: 0,
            math_equations: 0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DocumentData {
    pub success: bool,
    pub path: String,
    pub filename: String,
    pub dirname: String,
    pub dirpath: String,
    pub content: String,
    pub raw_content: String,
    pub frontmatter: Option<String>,
    pub size_formatted: String,
    pub size_kb: f64,
    pub encoding: String,
    pub last_modified: String,
    pub mtime: String,
    pub stats: FileStats,
    pub headings: Vec<HeadingItem>,
    pub recent_files: Vec<String>,
    pub error: Option<String>,
}

impl DocumentData {
    pub fn error(msg: &str) -> Self {
        Self {
            success: false,
            path: String::new(),
            filename: String::new(),
            dirname: String::new(),
            dirpath: String::new(),
            content: String::new(),
            raw_content: String::new(),
            frontmatter: None,
            size_formatted: "0.0 KB".to_string(),
            size_kb: 0.0,
            encoding: "unknown".to_string(),
            last_modified: String::new(),
            mtime: String::new(),
            stats: FileStats::default(),
            headings: Vec::new(),
            recent_files: Vec::new(),
            error: Some(msg.to_string()),
        }
    }
}

pub fn parse_frontmatter(raw: &str) -> (Option<String>, String) {
    let mut lines = raw.lines().peekable();

    // Skip leading blank lines
    while let Some(line) = lines.peek() {
        if line.trim().is_empty() {
            lines.next();
        } else {
            break;
        }
    }

    let remaining_lines: Vec<&str> = lines.collect();
    if !remaining_lines.is_empty() && remaining_lines[0].trim() == "---" {
        let mut end_idx = None;
        for (i, line) in remaining_lines.iter().enumerate().skip(1) {
            let t = line.trim();
            if t == "---" || t == "..." {
                end_idx = Some(i);
                break;
            }
        }
        if let Some(end) = end_idx {
            let fm = remaining_lines[1..end].join("\n").trim().to_string();
            let body = remaining_lines[end + 1..].join("\n");
            return (Some(fm), body);
        }
    }
    (None, raw.to_string())
}

pub fn clean_prefix(p: &str) -> String {
    let s = p.trim().replace('/', "\\");
    if let Some(stripped) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", stripped)
    } else if let Some(stripped) = s.strip_prefix(r"\\?\") {
        stripped.to_string()
    } else {
        s
    }
}

pub fn load_markdown_file(path_str: &str) -> DocumentData {
    let clean_str = clean_prefix(path_str);
    let path = PathBuf::from(&clean_str);
    if !path.exists() {
        return DocumentData::error(&format!("File not found: {}", clean_str));
    }

    let metadata = match path.metadata() {
        Ok(m) => m,
        Err(e) => return DocumentData::error(&format!("Cannot read metadata: {}", e)),
    };

    if metadata.len() > MAX_FILE_SIZE {
        return DocumentData::error(&format!(
            "File exceeds 30MB limit (size: {:.1} MB)",
            metadata.len() as f64 / (1024.0 * 1024.0)
        ));
    }

    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => return DocumentData::error(&format!("Cannot open file: {}", e)),
    };

    let mut buffer = Vec::new();
    if let Err(e) = file.read_to_end(&mut buffer) {
        return DocumentData::error(&format!("Error reading file: {}", e));
    }

    // Strip UTF-8 BOM if present
    if buffer.starts_with(&[0xEF, 0xBB, 0xBF]) {
        buffer.drain(0..3);
    }

    // Binary file guard: check for null bytes in the first 8KB
    let check_len = buffer.len().min(8192);
    if buffer[..check_len].contains(&0) {
        return DocumentData::error("Binary file detected. QuickMD only opens text/markdown files.");
    }

    // Decode UTF-8 with Thai Windows-874 / TIS-620 fallback
    let (raw_text, encoding) = match std::str::from_utf8(&buffer) {
        Ok(valid_str) => (valid_str.to_string(), "UTF-8".to_string()),
        Err(_) => {
            // Check for Windows-874 / TIS-620 Thai bytes (0xA1..=0xFB)
            let mut thai_candidate = String::with_capacity(buffer.len());
            let mut is_thai_cp874 = false;
            for &b in &buffer {
                if b < 0x80 {
                    thai_candidate.push(b as char);
                } else if (0xA1..=0xFB).contains(&b) {
                    if let Some(ch) = char::from_u32(0x0E00 + (b - 0xA0) as u32) {
                        thai_candidate.push(ch);
                        is_thai_cp874 = true;
                    } else {
                        thai_candidate.push('\u{FFFD}');
                    }
                } else {
                    thai_candidate.push('\u{FFFD}');
                }
            }
            if is_thai_cp874 {
                (thai_candidate, "TIS-620 / CP874".to_string())
            } else {
                let lossy = String::from_utf8_lossy(&buffer).to_string();
                (lossy, "UTF-8 (Lossy)".to_string())
            }
        }
    };

    let size_kb = (buffer.len() as f64) / 1024.0;
    let size_formatted = if size_kb >= 1024.0 {
        format!("{:.1} MB", size_kb / 1024.0)
    } else {
        format!("{:.1} KB", size_kb)
    };

    let now = std::time::SystemTime::now();
    let mtime = match metadata.modified() {
        Ok(mod_time) => {
            if let Ok(elapsed) = now.duration_since(mod_time) {
                let secs = elapsed.as_secs();
                if secs < 60 {
                    "Just now".to_string()
                } else if secs < 3600 {
                    format!("{}m ago", secs / 60)
                } else if secs < 86400 {
                    format!("{}h ago", secs / 3600)
                } else {
                    format!("{}d ago", secs / 86400)
                }
            } else {
                "Just now".to_string()
            }
        }
        Err(_) => "Unknown".to_string(),
    };

    let filename = path.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled.md".to_string());

    let dirpath = path.parent()
        .map(|p| clean_prefix(&p.to_string_lossy()))
        .unwrap_or_default();

    let (frontmatter, body) = parse_frontmatter(&raw_text);
    let stats = calculate_stats(&raw_text);
    let headings = extract_headings(&body);
    let last_modified = mtime.clone();

    DocumentData {
        success: true,
        path: clean_str,
        filename,
        dirname: dirpath.clone(),
        dirpath,
        content: body,
        raw_content: raw_text,
        frontmatter,
        size_formatted,
        size_kb: (size_kb * 10.0).round() / 10.0,
        encoding,
        last_modified,
        mtime,
        stats,
        headings,
        recent_files: Vec::new(),
        error: None,
    }
}

fn calculate_stats(content: &str) -> FileStats {
    let chars = content.chars().count();
    let chars_no_spaces = content.chars().filter(|c| !c.is_whitespace()).count();
    let lines = content.lines().count();

    let mut en_words = 0;
    let mut thai_chars = 0;
    let mut in_codeblock = false;
    let mut code_blocks_count = 0;
    let mut headings_count = 0;
    let mut tables_count = 0;
    let mut in_table = false;
    let mut images_count = 0;
    let mut links_count = 0;
    let mut math_count = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if !in_codeblock {
                code_blocks_count += 1;
            }
            in_codeblock = !in_codeblock;
            in_table = false;
            continue;
        }

        if !in_codeblock {
            if trimmed.starts_with('#') {
                let lvl = trimmed.chars().take_while(|&c| c == '#').count();
                if lvl >= 1 && lvl <= 6 && trimmed[lvl..].starts_with(' ') {
                    headings_count += 1;
                }
            }

            // Accurate table block detection
            let is_table_row = trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() > 1;
            if is_table_row {
                if !in_table {
                    tables_count += 1;
                    in_table = true;
                }
            } else {
                in_table = false;
            }

            let img_matches = trimmed.matches("![").count();
            images_count += img_matches;

            let bracket_links = trimmed.matches("](").count();
            // Don't count images (![...](...)) as links
            links_count += bracket_links.saturating_sub(img_matches);

            if trimmed.contains("$$") {
                math_count += trimmed.matches("$$").count();
            } else if trimmed.matches('$').count() >= 2 {
                math_count += trimmed.matches('$').count();
            }

            for word in trimmed.split_whitespace() {
                let mut has_thai = false;
                for ch in word.chars() {
                    // Thai Unicode block: U+0E00 to U+0E7F
                    if ('\u{0E00}'..='\u{0E7F}').contains(&ch) {
                        thai_chars += 1;
                        has_thai = true;
                    }
                }
                if !has_thai && !word.is_empty() {
                    en_words += 1;
                }
            }
        }
    }

    // Thai syllable-level word estimation: approx 4 characters per Thai word
    let thai_word_est = thai_chars / 4;
    let total_words = en_words + thai_word_est;
    let read_time = if total_words == 0 {
        0
    } else {
        ((total_words as f64) / 200.0).ceil() as usize
    };

    FileStats {
        words: total_words,
        chars,
        chars_no_spaces,
        lines,
        reading_time_minutes: if total_words == 0 { 0 } else { read_time.max(1) },
        read_time_min: if total_words == 0 { 0 } else { read_time.max(1) },
        headings: headings_count,
        headings_count,
        code_blocks: code_blocks_count,
        code_blocks_count,
        tables: tables_count,
        images: images_count,
        links: links_count,
        math_equations: (math_count + 1) / 2,
    }
}

fn extract_headings(content: &str) -> Vec<HeadingItem> {
    let mut headings = Vec::new();
    let mut in_codeblock = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_codeblock = !in_codeblock;
            continue;
        }
        if in_codeblock {
            continue;
        }

        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|&c| c == '#').count();
            if level >= 1 && level <= 6 && trimmed[level..].starts_with(' ') {
                let mut text = trimmed[level..].trim();
                // Strip optional closing hashes in CommonMark (e.g. '## Title ##')
                if text.ends_with('#') {
                    text = text.trim_end_matches('#').trim();
                }
                if !text.is_empty() {
                    let mut id = String::new();
                    let mut last_was_dash = false;
                    for c in text.to_lowercase().chars() {
                        if c.is_alphanumeric() || ('\u{0E00}'..='\u{0E7F}').contains(&c) || c == '_' {
                            id.push(c);
                            last_was_dash = false;
                        } else if !last_was_dash {
                            id.push('-');
                            last_was_dash = true;
                        }
                    }
                    let id = id.trim_matches('-').to_string();

                    headings.push(HeadingItem {
                        level,
                        text: text.to_string(),
                        id,
                    });
                }
            }
        }
    }

    headings
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_calculate_stats_english_and_thai() {
        let content = "# Header\nHello world from Rust. สวัสดีชาวโลก\n```rust\nlet x = 1;\n```";
        let stats = calculate_stats(content);
        assert!(stats.words > 0);
        assert_eq!(stats.headings_count, 1);
        assert_eq!(stats.code_blocks_count, 1);
    }

    #[test]
    fn test_extract_headings() {
        let content = "# Title 1\nSome text\n## Section 1.1\n### Sub-section\n```\n# Not a heading\n```\n## Section 1.2";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 4);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].text, "Title 1");
        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[2].level, 3);
        assert_eq!(headings[3].level, 2);
        assert_eq!(headings[3].text, "Section 1.2");
    }

    #[test]
    fn test_binary_file_guard() {
        let td = std::env::temp_dir();
        let bin_file = td.join("test_bin_guard.dat");
        let mut f = File::create(&bin_file).unwrap();
        f.write_all(b"Hello\x00Binary\x00World").unwrap();
        drop(f);

        let doc = load_markdown_file(&bin_file.to_string_lossy());
        assert!(!doc.success);
        assert!(doc.error.unwrap().contains("Binary file detected"));

        let _ = std::fs::remove_file(bin_file);
    }

    #[test]
    fn test_load_valid_markdown() {
        let td = std::env::temp_dir();
        let md_file = td.join("test_valid.md");
        let mut f = File::create(&md_file).unwrap();
        f.write_all(b"# Test Doc\n\nThis is a sample markdown file for testing.\n").unwrap();
        drop(f);

        let doc = load_markdown_file(&md_file.to_string_lossy());
        assert!(doc.success);
        assert_eq!(doc.filename, "test_valid.md");
        assert_eq!(doc.encoding, "UTF-8");
        assert_eq!(doc.headings.len(), 1);

        let _ = std::fs::remove_file(md_file);
    }

    #[test]
    fn test_empty_document_stats() {
        let stats = calculate_stats("");
        assert_eq!(stats.words, 0);
        assert_eq!(stats.read_time_min, 0);
        assert_eq!(stats.chars, 0);
        assert_eq!(stats.lines, 0);
    }

    #[test]
    fn test_code_block_prose_exclusion() {
        let content = "Hello world\n```\nignore these multiple words inside code block\n```\nGoodbye";
        let stats = calculate_stats(content);
        // "Hello", "world", "Goodbye" = 3 prose words
        assert_eq!(stats.words, 3);
        assert_eq!(stats.code_blocks_count, 1);
    }

    #[test]
    fn test_parse_frontmatter() {
        let raw = "---\ntitle: Rust Markdown\nauthor: Antigravity\n---\n# Main Header\nBody text";
        let (fm, body) = parse_frontmatter(raw);
        assert!(fm.is_some());
        assert!(fm.unwrap().contains("title: Rust Markdown"));
        assert!(body.contains("# Main Header"));
    }

    #[test]
    fn test_thai_heading_slug_and_closing_hashes() {
        let content = "## ข้อมูลสำคัญ ##\n#NotHeadingBecauseNoSpace\n### Normal Heading";
        let headings = extract_headings(content);
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0].text, "ข้อมูลสำคัญ");
        assert_eq!(headings[0].id, "ข้อมูลสำคัญ");
        assert_eq!(headings[1].text, "Normal Heading");
    }

    #[test]
    fn test_table_and_image_stats_accuracy() {
        let content = "| Col1 | Col2 |\n|---|---|\n| A | B |\n| C | D |\n\n![Image](pic.png)\n[Link](url.com)";
        let stats = calculate_stats(content);
        assert_eq!(stats.tables, 1); // 1 table, not 4 rows
        assert_eq!(stats.images, 1);
        assert_eq!(stats.links, 1);  // 1 real link, not 2
    }
}
