use std::env;
use std::path::Path;
use winreg::enums::*;
use winreg::RegKey;

const PROG_ID: &str = "QuickMD.Document";
const LEGACY_PROG_ID: &str = "md_reader.file";
const LEGACY_PROG_ID_V2: &str = "QuickMD.file";
const APP_NAME: &str = "Markdown Document";
const EXTENSIONS: [&str; 4] = [".md", ".markdown", ".mdown", ".mkd"];

/// Effective Windows file-association status for `.md` files under the current user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DefaultAssociationState {
    /// HKCU `.md` resolves to `QuickMD.Document` pointing at this binary, and `UserChoice` does not override it.
    DefaultActive,
    /// HKCU keys are registered, but Windows `UserChoice` for `.md` is locked to a different ProgId.
    NeedsWindowsConfirmation,
    /// QuickMD is not registered as the HKCU handler for `.md` (or points to another command).
    #[default]
    NotRegistered,
}

/// Pure status classifier separated from registry I/O for deterministic unit testing.
pub fn classify_association(
    classes_progid: Option<&str>,
    command_matches_exe: bool,
    user_choice_progid: Option<&str>,
    app_progid: Option<&str>,
) -> DefaultAssociationState {
    let hkcu_ok = classes_progid
        .map(|s| s.eq_ignore_ascii_case(PROG_ID))
        .unwrap_or(false)
        && command_matches_exe;

    if let Some(uc) = user_choice_progid.map(str::trim).filter(|s| !s.is_empty()) {
        let matches_app_progid = app_progid
            .map(|ap| uc.eq_ignore_ascii_case(ap))
            .unwrap_or(false);
        if (uc.eq_ignore_ascii_case(PROG_ID) && command_matches_exe) || matches_app_progid {
            return DefaultAssociationState::DefaultActive;
        }
        if uc.eq_ignore_ascii_case(PROG_ID) && !command_matches_exe {
            return DefaultAssociationState::NotRegistered;
        }
        return if hkcu_ok {
            DefaultAssociationState::NeedsWindowsConfirmation
        } else {
            DefaultAssociationState::NotRegistered
        };
    }

    if hkcu_ok {
        DefaultAssociationState::DefaultActive
    } else {
        DefaultAssociationState::NotRegistered
    }
}

/// Returns true if a shell command string invokes `exe_path` (case-insensitive on Windows).
pub fn command_targets_exe(command: &str, exe_path: &Path) -> bool {
    let exe_str = exe_path.to_string_lossy();
    let trimmed = command.trim();
    if trimmed.is_empty() || exe_str.is_empty() {
        return false;
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        return rest
            .split('"')
            .next()
            .unwrap_or("")
            .eq_ignore_ascii_case(&exe_str);
    }
    if trimmed.len() >= exe_str.len() && trimmed.is_char_boundary(exe_str.len()) {
        let (head, tail) = trimmed.split_at(exe_str.len());
        if head.eq_ignore_ascii_case(&exe_str)
            && (tail.is_empty() || tail.starts_with(char::is_whitespace))
        {
            return true;
        }
    }
    false
}

/// Checks whether the current executable is effectively the default `.md` viewer.
pub fn check_default_status() -> DefaultAssociationState {
    let Ok(current_exe) = env::current_exe() else {
        return DefaultAssociationState::NotRegistered;
    };
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);

    let classes_progid = hkcu
        .open_subkey(r"Software\Classes\.md")
        .ok()
        .and_then(|k| k.get_value::<String, _>("").ok())
        .or_else(|| {
            hkcr.open_subkey(".md")
                .ok()
                .and_then(|k| k.get_value::<String, _>("").ok())
        });

    let cmd_matches = hkcu
        .open_subkey(format!(r"Software\Classes\{}\shell\open\command", PROG_ID))
        .ok()
        .and_then(|k| k.get_value::<String, _>("").ok())
        .or_else(|| {
            hkcr.open_subkey(format!(r"{}\shell\open\command", PROG_ID))
                .ok()
                .and_then(|k| k.get_value::<String, _>("").ok())
        })
        .map(|cmd| command_targets_exe(&cmd, &current_exe))
        .unwrap_or(false);

    // Windows 11 24H2+ stores the active choice in UserChoiceLatest\ProgId while leaving legacy
    // UserChoice untouched (which may hold a stale legacy ProgId like QuickMD.file).
    let user_choice_progid = hkcu
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.md\UserChoiceLatest\ProgId")
        .ok()
        .and_then(|k| k.get_value::<String, _>("ProgId").ok())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            hkcu.open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.md\UserChoice")
                .ok()
                .and_then(|k| k.get_value::<String, _>("ProgId").ok())
        });

    let exe_name = current_exe.file_name().and_then(|n| n.to_str());
    let app_progid = exe_name.and_then(|name| {
        let candidate = format!(r"Applications\{}", name);
        let app_cmd_matches = hkcu
            .open_subkey(format!(r"Software\Classes\{}\shell\open\command", candidate))
            .ok()
            .and_then(|k| k.get_value::<String, _>("").ok())
            .map(|cmd| command_targets_exe(&cmd, &current_exe))
            .unwrap_or(false);
        app_cmd_matches.then_some(candidate)
    });

    classify_association(
        classes_progid.as_deref(),
        cmd_matches,
        user_choice_progid.as_deref(),
        app_progid.as_deref(),
    )
}

/// Registers `.md`, `.markdown`, `.mdown`, `.mkd`, `QuickMD.Document`, `OpenWithProgids`,
/// `Applications\<exe>`, and Explorer context menu entries under `HKCU\Software\Classes`.
pub fn register_associations() -> Result<(), String> {
    let current_exe = env::current_exe().map_err(|e| e.to_string())?;
    let exe_path = current_exe.to_string_lossy().to_string();
    let exe_name = current_exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("QuickMD3.exe")
        .to_string();
    let cmd = format!("\"{}\" \"%1\"", exe_path);

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let classes = hkcu
        .open_subkey_with_flags(r"Software\Classes", KEY_ALL_ACCESS)
        .map_err(|e| format!("Cannot open Classes key: {}", e))?;

    remove_legacy_entries(&classes);

    // 1. Register Markdown extensions (.md, .markdown, .mdown, .mkd) + OpenWithProgids
    for ext in EXTENSIONS {
        let (ext_key, _) = classes.create_subkey(ext).map_err(|e| e.to_string())?;
        ext_key.set_value("", &PROG_ID).map_err(|e| e.to_string())?;
        let _ = ext_key.set_value("Content Type", &"text/markdown");
        if let Ok((owp_key, _)) = ext_key.create_subkey("OpenWithProgids") {
            let _ = owp_key.set_value(PROG_ID, &"");
        }
    }

    // 2. ProgID definition
    let (prog_key, _) = classes.create_subkey(PROG_ID).map_err(|e| e.to_string())?;
    prog_key.set_value("", &APP_NAME).map_err(|e| e.to_string())?;
    let _ = prog_key.set_value("FriendlyTypeName", &APP_NAME);

    let (icon_key, _) = prog_key.create_subkey("DefaultIcon").map_err(|e| e.to_string())?;
    icon_key
        .set_value("", &format!("\"{}\",0", exe_path))
        .map_err(|e| e.to_string())?;

    let (open_key, _) = prog_key.create_subkey(r"shell\open").map_err(|e| e.to_string())?;
    let _ = open_key.set_value("", &"Open with QuickMD");

    let (cmd_key, _) = prog_key.create_subkey(r"shell\open\command").map_err(|e| e.to_string())?;
    cmd_key.set_value("", &cmd).map_err(|e| e.to_string())?;

    // 3. Registered application entry so Windows "Default apps" / "Open with" discovers QuickMD
    if let Ok((app_key, _)) = classes.create_subkey(format!(r"Applications\{}", exe_name)) {
        let _ = app_key.set_value("FriendlyAppName", &"QuickMD");
        if let Ok((app_icon, _)) = app_key.create_subkey("DefaultIcon") {
            let _ = app_icon.set_value("", &format!("\"{}\",0", exe_path));
        }
        if let Ok((app_cmd, _)) = app_key.create_subkey(r"shell\open\command") {
            let _ = app_cmd.set_value("", &cmd);
        }
        if let Ok((sup_key, _)) = app_key.create_subkey("SupportedTypes") {
            for ext in EXTENSIONS {
                let _ = sup_key.set_value(ext, &"");
            }
        }
    }

    // 4. Context Menu: "Open with QuickMD" for all files (*\shell\OpenWithQuickMD)
    let (star_key, _) = classes
        .create_subkey(r"*\shell\OpenWithQuickMD")
        .map_err(|e| e.to_string())?;
    star_key.set_value("", &"Open with QuickMD").map_err(|e| e.to_string())?;
    star_key
        .set_value("Icon", &format!("\"{}\",0", exe_path))
        .map_err(|e| e.to_string())?;
    let (star_cmd, _) = star_key.create_subkey("command").map_err(|e| e.to_string())?;
    star_cmd.set_value("", &cmd).map_err(|e| e.to_string())?;

    Ok(())
}

/// Opens Windows Default Apps settings so the user can confirm `.md` UserChoice if locked.
pub fn open_windows_default_apps() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let explorer = crate::bridge::windows_system_binary("explorer.exe");
        std::process::Command::new(explorer)
            .arg("ms-settings:defaultapps")
            .spawn()
            .map_err(|e| format!("Failed to open Windows Settings: {}", e))?;
    }
    Ok(())
}

pub fn unregister_associations() -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(classes) = hkcu.open_subkey_with_flags(r"Software\Classes", KEY_ALL_ACCESS) {
        let _ = classes.delete_subkey_all(PROG_ID);
        let _ = classes.delete_subkey_all(r"*\shell\OpenWithQuickMD");
        remove_legacy_entries(&classes);
        for ext in EXTENSIONS {
            if let Ok(ext_key) = classes.open_subkey_with_flags(ext, KEY_ALL_ACCESS) {
                if let Ok(val) = ext_key.get_value::<String, _>("") {
                    if val == PROG_ID || val == LEGACY_PROG_ID || val == LEGACY_PROG_ID_V2 {
                        let _ = ext_key.delete_value("");
                    }
                }
                if let Ok(owp) = ext_key.open_subkey_with_flags("OpenWithProgids", KEY_ALL_ACCESS) {
                    let _ = owp.delete_value(PROG_ID);
                }
            }
        }
    }
    Ok(())
}

/// Removes the keys created by legacy builds (md.exe / v2) so they never linger.
fn remove_legacy_entries(classes: &RegKey) {
    let _ = classes.delete_subkey_all(LEGACY_PROG_ID);
    let _ = classes.delete_subkey_all(LEGACY_PROG_ID_V2);
    let _ = classes.delete_subkey_all(r"*\shell\OpenWithMdReader");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_matching_handles_quoted_and_unquoted_paths() {
        let exe = Path::new(r"C:\Program Files\QuickMD\QuickMD3.exe");
        assert!(command_targets_exe(
            r#""C:\Program Files\QuickMD\quickmd3.exe" "%1""#,
            exe
        ));
        assert!(command_targets_exe(
            r#"C:\Program Files\QuickMD\QuickMD3.exe %1"#,
            exe
        ));
        assert!(!command_targets_exe(
            r#""C:\Program Files\Other\app.exe" "%1""#,
            exe
        ));
        assert!(!command_targets_exe("", exe));
    }

    #[test]
    fn classifier_detects_active_overridden_and_unregistered_states() {
        assert_eq!(
            classify_association(Some("QuickMD.Document"), true, None, None),
            DefaultAssociationState::DefaultActive
        );
        assert_eq!(
            classify_association(Some("QuickMD.Document"), true, Some("QuickMD.Document"), None),
            DefaultAssociationState::DefaultActive
        );
        assert_eq!(
            classify_association(
                Some("QuickMD.Document"),
                false,
                Some(r"Applications\QuickMD3.exe"),
                Some(r"Applications\QuickMD3.exe")
            ),
            DefaultAssociationState::DefaultActive
        );
        assert_eq!(
            classify_association(Some("QuickMD.Document"), true, Some("VSCode.md"), None),
            DefaultAssociationState::NeedsWindowsConfirmation
        );
        assert_eq!(
            classify_association(Some("txtfile"), false, None, None),
            DefaultAssociationState::NotRegistered
        );
        assert_eq!(
            classify_association(Some("QuickMD.Document"), false, Some("QuickMD.Document"), None),
            DefaultAssociationState::NotRegistered
        );
    }
}
