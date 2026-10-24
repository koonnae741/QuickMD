use std::env;
use winreg::enums::*;
use winreg::RegKey;

const PROG_ID: &str = "QuickMD.file";
const LEGACY_PROG_ID: &str = "md_reader.file";
const APP_NAME: &str = "QuickMD";

pub fn is_registered() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey(format!(r"Software\Classes\{}\shell\open\command", PROG_ID)) {
        if let Ok(val) = key.get_value::<String, _>("") {
            if let Ok(current_exe) = env::current_exe() {
                let current_str = current_exe.to_string_lossy().to_lowercase();
                let val_lower = val.to_lowercase();
                return val_lower.contains(&current_str);
            }
            return !val.is_empty();
        }
    }
    false
}

pub fn register_associations() -> Result<(), String> {
    let current_exe = env::current_exe().map_err(|e| e.to_string())?;
    let exe_path = current_exe.to_string_lossy().to_string();
    let cmd = format!("\"{}\" \"%1\"", exe_path);

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let classes = hkcu.open_subkey_with_flags(r"Software\Classes", KEY_ALL_ACCESS)
        .map_err(|e| format!("Cannot open Classes key: {}", e))?;

    remove_legacy_entries(&classes);

    // 1. .md extension
    let (md_key, _) = classes.create_subkey(".md").map_err(|e| e.to_string())?;
    let _ = md_key.set_value("", &PROG_ID);

    // 2. .markdown extension
    let (markdown_key, _) = classes.create_subkey(".markdown").map_err(|e| e.to_string())?;
    let _ = markdown_key.set_value("", &PROG_ID);

    // 3. ProgID definition
    let (prog_key, _) = classes.create_subkey(PROG_ID).map_err(|e| e.to_string())?;
    let _ = prog_key.set_value("", &APP_NAME);

    let (icon_key, _) = prog_key.create_subkey("DefaultIcon").map_err(|e| e.to_string())?;
    let _ = icon_key.set_value("", &format!("\"{}\",0", exe_path));

    let (cmd_key, _) = prog_key.create_subkey(r"shell\open\command").map_err(|e| e.to_string())?;
    let _ = cmd_key.set_value("", &cmd);

    // 4. Context Menu: "Open with QuickMD" for all files (*\shell\OpenWithQuickMD)
    if let Ok((star_key, _)) = classes.create_subkey(r"*\shell\OpenWithQuickMD") {
        let _ = star_key.set_value("", &"Open with QuickMD");
        let _ = star_key.set_value("Icon", &format!("\"{}\",0", exe_path));
        if let Ok((star_cmd, _)) = star_key.create_subkey("command") {
            let _ = star_cmd.set_value("", &cmd);
        }
    }

    Ok(())
}

pub fn unregister_associations() -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(classes) = hkcu.open_subkey_with_flags(r"Software\Classes", KEY_ALL_ACCESS) {
        let _ = classes.delete_subkey_all(PROG_ID);
        let _ = classes.delete_subkey_all(r"*\shell\OpenWithQuickMD");
        remove_legacy_entries(&classes);
        for ext in [".md", ".markdown"] {
            if let Ok(ext_key) = classes.open_subkey_with_flags(ext, KEY_ALL_ACCESS) {
                if let Ok(val) = ext_key.get_value::<String, _>("") {
                    if val == PROG_ID || val == LEGACY_PROG_ID {
                        let _ = ext_key.delete_value("");
                    }
                }
            }
        }
    }
    Ok(())
}

/// Removes the keys created by the pre-rename build (md.exe) so they never linger.
fn remove_legacy_entries(classes: &RegKey) {
    let _ = classes.delete_subkey_all(LEGACY_PROG_ID);
    let _ = classes.delete_subkey_all(r"*\shell\OpenWithMdReader");
}
