use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecentFileItem {
    pub filename: String,
    pub path: String,
    pub size: String,
    pub mtime: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub theme: String,
    pub content_width: String,
    pub font_size: usize,
    pub sidebar_open: bool,
    pub recent_files: Vec<RecentFileItem>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            content_width: "normal".to_string(),
            font_size: 16,
            sidebar_open: true,
            recent_files: Vec::new(),
        }
    }
}

pub fn get_data_dir() -> PathBuf {
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let app_data = PathBuf::from(local_app_data).join("QuickMD").join("data");
        let _ = fs::create_dir_all(&app_data);
        return app_data;
    }

    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            let local_data = parent.join("data");
            let _ = fs::create_dir_all(&local_data);
            return local_data;
        }
    }

    PathBuf::from("data")
}

pub fn get_config_path() -> PathBuf {
    get_data_dir().join("config.json")
}

pub fn load_config() -> AppConfig {
    let path = get_config_path();
    if path.exists() {
        if let Ok(mut file) = File::open(&path) {
            let mut s = String::new();
            if file.read_to_string(&mut s).is_ok() {
                if let Ok(cfg) = serde_json::from_str::<AppConfig>(&s) {
                    return cfg;
                }
            }
        }
    }
    AppConfig::default()
}

pub fn save_config(config: &AppConfig) {
    let path = get_config_path();
    if let Ok(s) = serde_json::to_string_pretty(config) {
        let tmp_path = path.with_extension("tmp");
        if let Ok(mut file) = File::create(&tmp_path) {
            if file.write_all(s.as_bytes()).is_ok() {
                drop(file);
                if fs::rename(&tmp_path, &path).is_err() {
                    let _ = fs::copy(&tmp_path, &path);
                    let _ = fs::remove_file(&tmp_path);
                }
            } else {
                let _ = fs::remove_file(&tmp_path);
            }
        }
    }
}

pub fn clean_path_prefix(p: &str) -> String {
    let s = p.trim().replace('/', "\\");
    if let Some(stripped) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", stripped)
    } else if let Some(stripped) = s.strip_prefix(r"\\?\") {
        stripped.to_string()
    } else {
        s
    }
}

pub fn add_recent_file(config: &mut AppConfig, doc_path: &str, filename: &str, size_kb: f64) {
    let clean_path = clean_path_prefix(doc_path);

    // Remove duplicate case-insensitively and slash-independently
    config.recent_files.retain(|item| {
        !clean_path_prefix(&item.path).eq_ignore_ascii_case(&clean_path)
    });

    let size_formatted = if size_kb >= 1024.0 {
        format!("{:.1} MB", size_kb / 1024.0)
    } else {
        format!("{:.1} KB", size_kb)
    };

    let mtime = match fs::metadata(&clean_path).or_else(|_| {
        if !clean_path.starts_with(r"\\?\") {
            let extended = format!(r"\\?\{}", clean_path);
            fs::metadata(&extended)
        } else {
            fs::metadata(&clean_path)
        }
    }).and_then(|m| m.modified()) {
        Ok(mod_time) => {
            if let Ok(elapsed) = std::time::SystemTime::now().duration_since(mod_time) {
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
        Err(_) => "Just now".to_string(),
    };

    let new_item = RecentFileItem {
        filename: filename.to_string(),
        path: clean_path,
        size: size_formatted,
        mtime,
    };

    config.recent_files.insert(0, new_item);
    if config.recent_files.len() > 15 {
        config.recent_files.truncate(15);
    }
    save_config(config);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_path_prefix() {
        assert_eq!(clean_path_prefix(r"\\?\C:\test\doc.md"), r"C:\test\doc.md");
        assert_eq!(clean_path_prefix(r"\\?\UNC\server\share\file.md"), r"\\server\share\file.md");
        assert_eq!(clean_path_prefix("C:/docs/sub/readme.md"), r"C:\docs\sub\readme.md");
    }

    #[test]
    fn test_recent_file_deduplication() {
        let mut cfg = AppConfig::default();
        add_recent_file(&mut cfg, r"C:\test\file.md", "file.md", 12.0);
        assert_eq!(cfg.recent_files.len(), 1);
        // Add with \\?\ prefix and forward slashes -> should deduplicate
        add_recent_file(&mut cfg, r"\\?\c:/test/file.md", "file.md", 14.0);
        assert_eq!(cfg.recent_files.len(), 1);
        assert_eq!(cfg.recent_files[0].path, r"c:\test\file.md");
    }
}

