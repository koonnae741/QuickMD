#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_config;
mod assets;
mod file_loader;
mod windows_assoc;

use std::borrow::Cow;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use serde::Deserialize;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::{Icon, WindowBuilder},
};
use wry::WebViewBuilder;

const WEBVIEW2_LOADER_BYTES: &[u8] = include_bytes!("../assets/dll/WebView2Loader.dll");
const ICON_RGBA_32: &[u8] = include_bytes!("../assets/icons/logo_32.rgba");

#[derive(Debug, Deserialize)]
struct IpcRequest {
    id: u64,
    action: String,
    payload: serde_json::Value,
}

#[derive(Clone, Debug)]
struct WatchedFile {
    path: PathBuf,
    mtime: std::time::SystemTime,
}

enum AppEvent {
    ShowWindow,
    Ipc(String),
    FileDialogResult(u64, Option<PathBuf>),
    FileDropped(PathBuf, usize),
    FileModified(PathBuf),
}

static DIALOG_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" {
    fn AttachConsole(dw_process_id: u32) -> i32;
    fn GetStdHandle(n_std_handle: u32) -> *mut std::ffi::c_void;
    fn WriteConsoleW(
        h_console_output: *mut std::ffi::c_void,
        lp_buffer: *const u16,
        n_number_of_chars_to_write: u32,
        lp_number_of_chars_written: *mut u32,
        lp_reserved: *mut std::ffi::c_void,
    ) -> i32;
    fn SetDllDirectoryW(lp_path_name: *const u16) -> i32;
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        lp_operation: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show_cmd: i32,
    ) -> isize;
    fn CoInitializeEx(pv_reserved: *mut std::ffi::c_void, dw_co_init: u32) -> i32;
    fn CoUninitialize();
}

fn print_console(msg: &str) {
    let wide: Vec<u16> = format!("{}\r\n", msg).encode_utf16().collect();
    unsafe {
        let handle = GetStdHandle(0xFFFFFFF5); // STD_OUTPUT_HANDLE = -11
        if !handle.is_null() {
            let mut written = 0;
            WriteConsoleW(handle, wide.as_ptr(), wide.len() as u32, &mut written, std::ptr::null_mut());
        }
    }
}

fn ensure_webview2_loader() {
    // 1. Primary: Extract to %LOCALAPPDATA%\QuickMD to maintain clean standalone exe folder
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let app_dir = PathBuf::from(local_app_data).join("QuickMD");
        let _ = std::fs::create_dir_all(&app_dir);
        let app_dll = app_dir.join("WebView2Loader.dll");

        // Symlink security guard: refuse to overwrite if target is a symlink
        if let Ok(symlink_meta) = std::fs::symlink_metadata(&app_dll) {
            if symlink_meta.file_type().is_symlink() {
                let _ = std::fs::remove_file(&app_dll);
            }
        }

        let needs_write = match std::fs::metadata(&app_dll) {
            Ok(m) => m.len() != WEBVIEW2_LOADER_BYTES.len() as u64,
            Err(_) => true,
        };
        if needs_write {
            let _ = std::fs::write(&app_dll, WEBVIEW2_LOADER_BYTES);
        }
        let wide: Vec<u16> = app_dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        unsafe { SetDllDirectoryW(wide.as_ptr()); }
        return;
    }

    // 2. Fallback: directory containing current executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let target_dll = exe_dir.join("WebView2Loader.dll");

            if let Ok(symlink_meta) = std::fs::symlink_metadata(&target_dll) {
                if symlink_meta.file_type().is_symlink() {
                    let _ = std::fs::remove_file(&target_dll);
                }
            }

            let needs_fallback_write = match std::fs::metadata(&target_dll) {
                Ok(m) => m.len() != WEBVIEW2_LOADER_BYTES.len() as u64,
                Err(_) => true,
            };
            if needs_fallback_write {
                let _ = std::fs::write(&target_dll, WEBVIEW2_LOADER_BYTES);
            }
            let wide: Vec<u16> = exe_dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            unsafe { SetDllDirectoryW(wide.as_ptr()); }
        }
    }
}

pub fn clean_windows_path(p: &Path) -> PathBuf {
    let path_str = p.to_string_lossy().to_string();
    if let Some(stripped) = path_str.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{}", stripped))
    } else if let Some(stripped) = path_str.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        p.to_path_buf()
    }
}

fn update_file_watch(
    path: &Path,
    watched_file: &Arc<Mutex<Option<WatchedFile>>>,
) {
    let clean = clean_windows_path(path);
    let mtime = std::fs::metadata(&clean)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| std::time::SystemTime::now());
    let mut guard = watched_file.lock().unwrap();
    *guard = Some(WatchedFile {
        path: clean,
        mtime,
    });
}

fn is_allowed_asset_ext(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" | "bmp" | "woff" | "woff2" | "ttf" | "otf"
    )
}

fn main() {
    unsafe {
        AttachConsole(0xFFFFFFFF);
    }

    ensure_webview2_loader();

    let args_os: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let args: Vec<String> = args_os.iter().map(|s| s.to_string_lossy().to_string()).collect();

    // 1. Process CLI Flags
    if args.len() > 1 {
        let flag = args[1].trim();
        if flag == "--version" || flag == "-v" {
            print_console("QuickMD 2.0.0");
            return;
        }
        if flag == "--register" {
            match windows_assoc::register_associations() {
                Ok(_) => print_console("Successfully registered QuickMD in Windows context menu & file associations."),
                Err(e) => print_console(&format!("Registration failed: {}", e)),
            }
            return;
        }
        if flag == "--unregister" {
            match windows_assoc::unregister_associations() {
                Ok(_) => print_console("Successfully unregistered QuickMD from Windows shell."),
                Err(e) => print_console(&format!("Unregistration failed: {}", e)),
            }
            return;
        }
    }

    // 2. Parse Initial File from arguments with lossless Unicode and \\?\ stripping
    let mut initial_file_path: Option<String> = None;
    if args_os.len() > 1 {
        let candidate_str = args_os[1].to_string_lossy().trim().trim_matches('"').trim_matches('\'').to_string();
        if !candidate_str.starts_with("--") && !candidate_str.is_empty() {
            let p = PathBuf::from(&args_os[1]);
            if p.exists() {
                let raw_path = p.canonicalize().unwrap_or(p);
                let path_str = raw_path.to_string_lossy().to_string();
                let clean_path = if let Some(stripped) = path_str.strip_prefix(r"\\?\UNC\") {
                    format!(r"\\{}", stripped)
                } else if let Some(stripped) = path_str.strip_prefix(r"\\?\") {
                    stripped.to_string()
                } else {
                    path_str
                };
                initial_file_path = Some(clean_path);
            } else {
                initial_file_path = Some(candidate_str);
            }
        }
    }

    // 3. Application State
    let mut config = app_config::load_config();
    let current_doc_dir: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let watched_file: Arc<Mutex<Option<WatchedFile>>> = Arc::new(Mutex::new(None));

    if let Some(ref path_str) = initial_file_path {
        let p = PathBuf::from(path_str);
        if let Some(parent) = p.parent() {
            *current_doc_dir.lock().unwrap() = Some(parent.to_path_buf());
        }
        update_file_watch(&p, &watched_file);
    }

    // 4. Create Window & Event Loop
    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Background Live File Watcher (Polls mtime every 800ms)
    let watcher_target = watched_file.clone();
    let proxy_for_watch = proxy.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(800));
            let current = { watcher_target.lock().unwrap().clone() };
            if let Some(watched) = current {
                if let Ok(meta) = std::fs::metadata(&watched.path) {
                    if let Ok(mtime) = meta.modified() {
                            let should_notify = {
                                let mut guard = watcher_target.lock().unwrap();
                                if let Some(ref mut target) = *guard {
                                    if target.path == watched.path {
                                        target.mtime = mtime;
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            };
                            if should_notify {
                                let _ = proxy_for_watch.send_event(AppEvent::FileModified(watched.path.clone()));
                            }
                    }
                }
            }
        }
    });

    // Pre-load initial file & Windows registration status BEFORE WebView creation for zero-latency 0ms hydration
    let is_registered_initial = windows_assoc::is_registered();
    let mut preloaded_file_data = None;

    if let Some(ref path_str) = initial_file_path {
        let mut doc = file_loader::load_markdown_file(path_str);
        if doc.success {
            app_config::add_recent_file(&mut config, &doc.path, &doc.filename, doc.size_kb);
            *current_doc_dir.lock().unwrap() = Some(PathBuf::from(&doc.dirpath));
            update_file_watch(Path::new(&doc.path), &watched_file);
            doc.recent_files = config.recent_files.iter().map(|i| i.path.clone()).collect();
        }
        preloaded_file_data = Some(doc);
    }

    let initial_title = if let Some(ref doc) = preloaded_file_data {
        if !doc.filename.is_empty() {
            format!("{} - QuickMD", doc.filename)
        } else {
            "QuickMD".to_string()
        }
    } else if let Some(ref p) = initial_file_path {
        let filename = Path::new(p).file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        format!("{} - QuickMD", filename)
    } else {
        "QuickMD".to_string()
    };

    let bg_rgba: (u8, u8, u8, u8) = match config.theme.as_str() {
        "light" => (255, 255, 255, 255),
        "tokyo-night" => (26, 27, 38, 255),
        "sepia" => (251, 240, 217, 255),
        "oled" => (0, 0, 0, 255),
        _ => (13, 17, 23, 255), // dark (#0d1117)
    };

    let app_window_icon = Icon::from_rgba(ICON_RGBA_32.to_vec(), 32, 32).ok();

    let window = WindowBuilder::new()
        .with_title(&initial_title)
        .with_window_icon(app_window_icon)
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 820.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(640.0, 480.0))
        .with_visible(false)
        .build(&event_loop)
        .expect("Failed to create native application window");

    // 5. Setup Custom Protocol Handler
    let doc_dir_for_protocol = current_doc_dir.clone();
    let protocol_handler = move |_id: wry::WebViewId, req: wry::http::Request<Vec<u8>>| -> wry::http::Response<Cow<'static, [u8]>> {
        let raw_path = req.uri().path().trim_start_matches('/');
        let path_clean = raw_path.replace('\\', "/");

        // (A) Check embedded asset catalog
        if let Some(asset) = assets::get_asset(&path_clean) {
            return wry::http::Response::builder()
                .header("Content-Type", asset.mime)
                .header("Access-Control-Allow-Origin", "*")
                .header("Cache-Control", "no-cache")
                .body(Cow::Borrowed(asset.bytes))
                .unwrap();
        }

        // (B) Relative local image / asset resolver with vault/parent containment check
        let guard = doc_dir_for_protocol.lock().unwrap();
        if let Some(ref base_dir) = *guard {
            let decoded = url_decode(&path_clean).replace('\\', "/");
            let mut rel_str = decoded.trim_start_matches('/').to_string();

            // Strip leading "web/" if browser resolved relative to web/index.html
            if rel_str.starts_with("web/") {
                rel_str = rel_str[4..].to_string();
            }

            if !rel_str.is_empty() {
                let candidate = base_dir.join(&rel_str);
                // Allow up to parent vault root containment check
                let vault_root = base_dir.parent().unwrap_or(base_dir);
                if let (Ok(canonical_candidate), Ok(canonical_vault)) = (candidate.canonicalize(), vault_root.canonicalize()) {
                    if canonical_candidate.is_file() && canonical_candidate.starts_with(&canonical_vault) {
                        let ext = canonical_candidate.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if is_allowed_asset_ext(ext) {
                            if let Ok(bytes) = std::fs::read(&canonical_candidate) {
                                let mime = get_mime_by_ext(ext);
                                return wry::http::Response::builder()
                                    .header("Content-Type", mime)
                                    .header("Access-Control-Allow-Origin", "*")
                                    .body(Cow::Owned(bytes))
                                    .unwrap();
                            }
                        }
                    }
                }
            }
        }

        // (C) 404 Fallback
        wry::http::Response::builder()
            .status(wry::http::StatusCode::NOT_FOUND)
            .header("Content-Type", "text/plain")
            .body(Cow::Borrowed(&b"404 Not Found"[..]))
            .unwrap()
    };

    // 6. JavaScript Bridge Injection Script
    let bridge_init_script = r#"
        (function() {
            let msgCounter = 0;
            const callbackMap = new Map();

            window.__rustCallback = function(id, result, error) {
                const cb = callbackMap.get(id);
                if (cb) {
                    callbackMap.delete(id);
                    if (error) {
                        cb.reject(new Error(error));
                    } else {
                        cb.resolve(result);
                    }
                }
            };

            window.__callRust = function(action, payload) {
                return new Promise((resolve, reject) => {
                    const id = ++msgCounter;
                    callbackMap.set(id, { resolve, reject });
                    try {
                        window.ipc.postMessage(JSON.stringify({ id, action, payload: payload || {} }));
                    } catch (e) {
                        callbackMap.delete(id);
                        reject(e);
                    }
                });
            };

            window.pywebview = {
                api: {
                    log_from_js: function(msg) {
                        return window.__callRust("log_from_js", { msg: msg });
                    },
                    show_window: function() {
                        return window.__callRust("show_window", {});
                    },
                    get_initial_data: function() {
                        return window.__callRust("get_initial_data", {});
                    },
                    load_file: function(path) {
                        return window.__callRust("load_file", { path: path });
                    },
                    save_settings: function(settings) {
                        return window.__callRust("save_settings", settings);
                    },
                    copy_to_clipboard: function(text) {
                        return window.__callRust("copy_to_clipboard", { text: text });
                    },
                    open_external_url: function(url) {
                        return window.__callRust("open_external_url", { url: url });
                    },
                    close_app: function() {
                        return window.__callRust("close_app", {});
                    },
                    open_file_dialog: function() {
                        return window.__callRust("open_file_dialog", {});
                    },
                    register_windows_integration: function() {
                        return window.__callRust("register_windows_integration", {});
                    },
                    load_wikilink: function(target) {
                        return window.__callRust("load_wikilink", { target: target });
                    }
                }
            };

            window.addEventListener('error', function(e) {
                try {
                    window.ipc.postMessage(JSON.stringify({ id: 0, action: "log_from_js", payload: { msg: "Global JS Error: " + (e.message || e.error) } }));
                } catch (err) {}
            });
            window.addEventListener('unhandledrejection', function(e) {
                try {
                    window.ipc.postMessage(JSON.stringify({ id: 0, action: "log_from_js", payload: { msg: "Unhandled Promise Rejection: " + (e.reason ? (e.reason.message || e.reason) : "unknown") } }));
                } catch (err) {}
            });

            if (document.readyState === 'loading') {
                document.addEventListener('DOMContentLoaded', function() {
                    window.dispatchEvent(new Event('pywebviewready'));
                });
            } else {
                window.dispatchEvent(new Event('pywebviewready'));
            }
        })();
    "#;

    // 7. IPC Dispatcher Handler
    let proxy_for_ipc = proxy.clone();
    let ipc_handler = move |req: wry::http::Request<String>| {
        let _ = proxy_for_ipc.send_event(AppEvent::Ipc(req.body().clone()));
    };

    // 8. Build WebView with isolated data directory, native background color, and native drag-drop
    let data_dir = app_config::get_data_dir();
    let webview_profile_dir = data_dir.join("webview_profile");
    let _ = std::fs::create_dir_all(&webview_profile_dir);
    let mut web_context = wry::WebContext::new(Some(webview_profile_dir));

    let proxy_for_drop = proxy.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut web_context)
        .with_background_color(bg_rgba)
        .with_custom_protocol("app".into(), protocol_handler)
        .with_initialization_script(bridge_init_script)
        .with_ipc_handler(ipc_handler)
        .with_drag_drop_handler(move |event| {
            if let wry::DragDropEvent::Drop { paths, .. } = event {
                let count = paths.len();
                if let Some(first_path) = paths.into_iter().next() {
                    let _ = proxy_for_drop.send_event(AppEvent::FileDropped(first_path, count));
                    return true;
                }
            }
            false
        })
        .with_url("app://localhost/web/index.html")
        .build(&window)
        .expect("Failed to initialize WebView2");

    let webview = Arc::new(Mutex::new(webview));
    let webview_clone = webview.clone();
    let window = Arc::new(window);
    let window_shown = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Safety fallback timer: ensure window becomes visible within 450ms even if WebView2 takes extra time on cold boot
    let proxy_for_show = proxy.clone();
    let shown_for_timer = window_shown.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(450));
        if !shown_for_timer.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = proxy_for_show.send_event(AppEvent::ShowWindow);
        }
    });

    // 9. Main Event Loop
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(AppEvent::ShowWindow) => {
                if !window_shown.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    window.set_visible(true);
                    window.set_focus();
                }
            }
            Event::UserEvent(AppEvent::Ipc(raw_msg)) => {
                if let Ok(ipc_req) = serde_json::from_str::<IpcRequest>(&raw_msg) {
                    let id = ipc_req.id;
                    let action = ipc_req.action.as_str();
                    let payload = ipc_req.payload;

                    match action {
                        "show_window" => {
                            if !window_shown.swap(true, std::sync::atomic::Ordering::SeqCst) {
                                window.set_visible(true);
                                window.set_focus();
                            }
                            if id != 0 {
                                send_js_callback(&webview_clone, id, &serde_json::json!({ "success": true }), None);
                            }
                        }
                        "log_from_js" => {
                            if let Some(msg) = payload.get("msg").and_then(|m| m.as_str()) {
                                print_console(&format!("[JS] {}", msg));
                            }
                            if id != 0 {
                                send_js_callback(&webview_clone, id, &serde_json::json!({ "success": true }), None);
                            }
                        }
                        "get_initial_data" => {
                            let is_registered = is_registered_initial;
                            let mut file_data = preloaded_file_data.take();
                            let has_file = initial_file_path.is_some();

                            if file_data.is_none() {
                                if let Some(ref path_str) = initial_file_path {
                                    let mut doc = file_loader::load_markdown_file(path_str);
                                    if doc.success {
                                        app_config::add_recent_file(&mut config, &doc.path, &doc.filename, doc.size_kb);
                                        let filename = doc.filename.clone();
                                        window.set_title(&format!("{} - QuickMD", filename));
                                        *current_doc_dir.lock().unwrap() = Some(PathBuf::from(&doc.dirpath));
                                        update_file_watch(Path::new(&doc.path), &watched_file);
                                        doc.recent_files = config.recent_files.iter().map(|i| i.path.clone()).collect();
                                    }
                                    file_data = Some(doc);
                                }
                            }

                            let resp = serde_json::json!({
                                "has_initial_file": has_file,
                                "file_data": file_data,
                                "config": config,
                                "is_windows_registered": is_registered,
                            });
                            send_js_callback(&webview_clone, id, &resp, None);
                        }
                        "load_file" => {
                            if let Some(path_str) = payload.get("path").and_then(|p| p.as_str()) {
                                let path = Path::new(path_str);
                                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                                if !matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkdn" | "txt") {
                                    send_js_callback(&webview_clone, id, &serde_json::Value::Null, Some("Only Markdown/Text files (.md, .markdown, .txt) can be opened"));
                                    return;
                                }

                                let mut doc = file_loader::load_markdown_file(path_str);
                                if doc.success {
                                    app_config::add_recent_file(&mut config, &doc.path, &doc.filename, doc.size_kb);
                                    window.set_title(&format!("{} - QuickMD", doc.filename));
                                    *current_doc_dir.lock().unwrap() = Some(PathBuf::from(&doc.dirpath));
                                    update_file_watch(Path::new(&doc.path), &watched_file);
                                    doc.recent_files = config.recent_files.iter().map(|i| i.path.clone()).collect();
                                }
                                let resp = serde_json::to_value(&doc).unwrap_or(serde_json::Value::Null);
                                send_js_callback(&webview_clone, id, &resp, None);
                            } else {
                                send_js_callback(&webview_clone, id, &serde_json::Value::Null, Some("Invalid path argument"));
                            }
                        }
                        "open_file_dialog" => {
                            if DIALOG_ACTIVE.compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok() {
                                let proxy_dialog = proxy.clone();
                                let window_for_dialog = window.clone();
                                std::thread::spawn(move || {
                                    unsafe { CoInitializeEx(std::ptr::null_mut(), 0x2); }
                                    let picked = rfd::FileDialog::new()
                                        .add_filter("Markdown Files", &["md", "markdown", "mdown", "mkdn", "txt"])
                                        .add_filter("All Files", &["*"])
                                        .set_parent(&*window_for_dialog)
                                        .pick_file();
                                    unsafe { CoUninitialize(); }
                                    DIALOG_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
                                    let _ = proxy_dialog.send_event(AppEvent::FileDialogResult(id, picked));
                                });
                            } else {
                                send_js_callback(&webview_clone, id, &serde_json::Value::Null, None);
                            }
                        }
                        "save_settings" => {
                            if let Some(theme) = payload.get("theme").and_then(|t| t.as_str()) {
                                config.theme = theme.to_string();
                            }
                            if let Some(width) = payload.get("content_width").and_then(|w| w.as_str()) {
                                config.content_width = width.to_string();
                            }
                            if let Some(size) = payload.get("font_size").and_then(|s| s.as_u64()) {
                                config.font_size = size as usize;
                            }
                            if let Some(sidebar) = payload.get("sidebar_open").and_then(|s| s.as_bool()) {
                                config.sidebar_open = sidebar;
                            }
                            app_config::save_config(&config);
                            send_js_callback(&webview_clone, id, &serde_json::json!({ "success": true }), None);
                        }
                        "copy_to_clipboard" => {
                            let text = payload.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            match copy_clipboard(text) {
                                Ok(_) => send_js_callback(&webview_clone, id, &serde_json::json!({ "success": true }), None),
                                Err(e) => send_js_callback(&webview_clone, id, &serde_json::Value::Null, Some(&e)),
                            }
                        }
                        "open_external_url" => {
                            if let Some(url) = payload.get("url").and_then(|u| u.as_str()) {
                                open_url(url);
                            }
                            send_js_callback(&webview_clone, id, &serde_json::json!({ "success": true }), None);
                        }
                        "close_app" => {
                            *control_flow = ControlFlow::Exit;
                            std::process::exit(0);
                        }
                        "register_windows_integration" => {
                            match windows_assoc::register_associations() {
                                Ok(_) => send_js_callback(&webview_clone, id, &serde_json::json!({ "success": true }), None),
                                Err(e) => send_js_callback(&webview_clone, id, &serde_json::Value::Null, Some(&e)),
                            }
                        }
                        "load_wikilink" => {
                            if let Some(target) = payload.get("target").and_then(|t| t.as_str()) {
                                let doc_guard = current_doc_dir.lock().unwrap();
                                let mut resolved_path: Option<PathBuf> = None;

                                if let Some(ref base_dir) = *doc_guard {
                                    let raw_target = target.trim().replace('\\', "/");
                                    let clean_target = raw_target.split('#').next().unwrap_or("").trim().to_string();

                                    if !clean_target.is_empty() {
                                        let mut search_dirs = vec![base_dir.clone()];
                                        if let Some(parent) = base_dir.parent() {
                                            search_dirs.push(parent.to_path_buf());
                                        }

                                        let vault_root = base_dir.parent().unwrap_or(base_dir);
                                        let check_candidate = |cand: &Path| -> Option<PathBuf> {
                                            if cand.is_file() {
                                                if let (Ok(can_cand), Ok(can_vault)) = (cand.canonicalize(), vault_root.canonicalize()) {
                                                    if can_cand.starts_with(&can_vault) {
                                                        let ext = can_cand.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                                                        if matches!(ext.as_str(), "md" | "markdown" | "mdown" | "mkdn" | "txt") {
                                                            return Some(clean_windows_path(&can_cand));
                                                        }
                                                    }
                                                }
                                            }
                                            None
                                        };

                                        // 1. Direct path search across current dir and parent vault
                                        for s_dir in &search_dirs {
                                            let cand1 = s_dir.join(&clean_target);
                                            if let Some(found) = check_candidate(&cand1) {
                                                resolved_path = Some(found);
                                                break;
                                            }
                                            if !clean_target.ends_with(".md") {
                                                let cand2 = s_dir.join(format!("{}.md", clean_target));
                                                if let Some(found) = check_candidate(&cand2) {
                                                    resolved_path = Some(found);
                                                    break;
                                                }
                                            }
                                        }

                                        // 2. Vault-wide shallow search for [[NoteName]] if not yet resolved
                                        if resolved_path.is_none() && !clean_target.contains('/') {
                                            let note_file = format!("{}.md", clean_target);
                                            let note_exact = clean_target.clone();
                                            if let Ok(entries) = std::fs::read_dir(vault_root) {
                                                for entry in entries.flatten() {
                                                    let p = entry.path();
                                                    if p.is_dir() {
                                                        let sub_cand1 = p.join(&note_file);
                                                        let sub_cand2 = p.join(&note_exact);
                                                        if let Some(found) = check_candidate(&sub_cand1).or_else(|| check_candidate(&sub_cand2)) {
                                                            resolved_path = Some(found);
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                if let Some(found) = resolved_path {
                                    let path_str = found.to_string_lossy().to_string();
                                    let mut doc = file_loader::load_markdown_file(&path_str);
                                    if doc.success {
                                        app_config::add_recent_file(&mut config, &doc.path, &doc.filename, doc.size_kb);
                                        window.set_title(&format!("{} - QuickMD", doc.filename));
                                        *current_doc_dir.lock().unwrap() = Some(PathBuf::from(&doc.dirpath));
                                        update_file_watch(Path::new(&doc.path), &watched_file);
                                        doc.recent_files = config.recent_files.iter().map(|i| i.path.clone()).collect();
                                    }
                                    let resp = serde_json::to_value(&doc).unwrap_or(serde_json::Value::Null);
                                    send_js_callback(&webview_clone, id, &resp, None);
                                } else {
                                    let err_doc = file_loader::DocumentData::error(&format!("Wikilink target not found or invalid: {}", target));
                                    let resp = serde_json::to_value(&err_doc).unwrap_or(serde_json::Value::Null);
                                    send_js_callback(&webview_clone, id, &resp, None);
                                }
                            }
                        }
                        _ => {
                            send_js_callback(&webview_clone, id, &serde_json::Value::Null, Some("Unknown IPC method"));
                        }
                    }
                }
            }
            Event::UserEvent(AppEvent::FileDialogResult(id, picked_path)) => {
                if let Some(picked) = picked_path {
                    let path_str = picked.to_string_lossy().to_string();
                    let mut doc = file_loader::load_markdown_file(&path_str);
                    if doc.success {
                        app_config::add_recent_file(&mut config, &doc.path, &doc.filename, doc.size_kb);
                        window.set_title(&format!("{} - QuickMD", doc.filename));
                        *current_doc_dir.lock().unwrap() = Some(PathBuf::from(&doc.dirpath));
                        update_file_watch(Path::new(&doc.path), &watched_file);
                        doc.recent_files = config.recent_files.iter().map(|i| i.path.clone()).collect();
                    }
                    let resp = serde_json::to_value(&doc).unwrap_or(serde_json::Value::Null);
                    send_js_callback(&webview_clone, id, &resp, None);
                } else {
                    send_js_callback(&webview_clone, id, &serde_json::Value::Null, None);
                }
            }
            Event::UserEvent(AppEvent::FileDropped(path, total_count)) => {
                let path_str = path.to_string_lossy().to_string();
                let mut doc = file_loader::load_markdown_file(&path_str);
                if doc.success {
                    app_config::add_recent_file(&mut config, &doc.path, &doc.filename, doc.size_kb);
                    window.set_title(&format!("{} - QuickMD", doc.filename));
                    *current_doc_dir.lock().unwrap() = Some(PathBuf::from(&doc.dirpath));
                    update_file_watch(Path::new(&doc.path), &watched_file);
                    doc.recent_files = config.recent_files.iter().map(|i| i.path.clone()).collect();
                    let resp = serde_json::to_value(&doc).unwrap_or(serde_json::Value::Null);
                    let res_json = serde_json::to_string(&resp).unwrap_or_else(|_| "null".to_string());
                    let script = if total_count > 1 {
                        format!(
                            "if (window.renderFile) {{ window.renderFile({}); }} if (window.showToast) {{ window.showToast('เปิดไฟล์แรกแล้ว (ลากมา {} ไฟล์)'); }}",
                            res_json, total_count
                        )
                    } else {
                        format!("if (window.renderFile) {{ window.renderFile({}); }}", res_json)
                    };
                    if let Ok(wv) = webview_clone.lock() {
                        let _ = wv.evaluate_script(&script);
                    }
                }
            }
            Event::UserEvent(AppEvent::FileModified(path)) => {
                let is_still_active = {
                    let guard = watched_file.lock().unwrap();
                    guard.as_ref().map(|w| w.path == path).unwrap_or(false)
                };
                if is_still_active {
                    let path_str = path.to_string_lossy().to_string();
                    let doc = file_loader::load_markdown_file(&path_str);
                    if doc.success {
                        let resp = serde_json::to_value(&doc).unwrap_or(serde_json::Value::Null);
                        let res_json = serde_json::to_string(&resp).unwrap_or_else(|_| "null".to_string());
                        let script = format!("if (window.onFileReloaded) {{ window.onFileReloaded({}); }}", res_json);
                        if let Ok(wv) = webview_clone.lock() {
                            let _ = wv.evaluate_script(&script);
                        }
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
                std::process::exit(0);
            }
            _ => (),
        }
    });
}

fn send_js_callback(
    webview: &Arc<Mutex<wry::WebView>>,
    id: u64,
    result: &serde_json::Value,
    error: Option<&str>,
) {
    let res_json = serde_json::to_string(result).unwrap_or_else(|_| "null".to_string());
    let err_json = match error {
        Some(e) => serde_json::to_string(e).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };

    let script = format!("window.__rustCallback({}, {}, {});", id, res_json, err_json);
    if let Ok(wv) = webview.lock() {
        let _ = wv.evaluate_script(&script);
    }
}

fn copy_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text).map_err(|e| e.to_string())?;
    Ok(())
}


fn open_url(url: &str) {
    let trimmed = url.trim();
    let is_safe = trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("mailto:");

    let contains_dangerous_chars = trimmed.chars().any(|c| c.is_control() || c == '"' || c == '\'');

    if is_safe && !contains_dangerous_chars {
        unsafe {
            let op: Vec<u16> = std::ffi::OsStr::new("open").encode_wide().chain(std::iter::once(0)).collect();
            let target: Vec<u16> = std::ffi::OsStr::new(trimmed).encode_wide().chain(std::iter::once(0)).collect();
            ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), target.as_ptr(), std::ptr::null(), std::ptr::null(), 1);
        }
    }
}

fn url_decode(s: &str) -> String {
    let mut result = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                result.push(b);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

fn get_mime_by_ext(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}
