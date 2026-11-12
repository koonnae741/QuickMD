use std::env;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/app_icon.ico");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let out_dir = env::var("OUT_DIR").unwrap();
        let res_path = format!("{}/app_icon.o", out_dir);
        let rc_path = format!("{}/app_icon.rc", out_dir);
        
        let rc_content = r#"1 ICON "assets/icons/app_icon.ico"

1 VERSIONINFO
FILEVERSION 3,0,0,0
PRODUCTVERSION 3,0,0,0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "koonnae741\0"
            VALUE "FileDescription", "QuickMD 3.0 Native Markdown Reader\0"
            VALUE "FileVersion", "3.0.0.0\0"
            VALUE "InternalName", "QuickMD\0"
            VALUE "LegalCopyright", "Copyright (c) 2026 koonnae741\0"
            VALUE "OriginalFilename", "QuickMD3.exe\0"
            VALUE "ProductName", "QuickMD 3.0\0"
            VALUE "ProductVersion", "3.0.0.0\0"
            VALUE "Comments", "QuickMD 3.0 Pure Native Rust Markdown Reader\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#;
        std::fs::write(&rc_path, rc_content).expect("failed to write app_icon.rc");

        let mut embedded = false;
        for cmd in ["x86_64-w64-mingw32-windres", "llvm-windres", "windres"] {
            let status = Command::new(cmd)
                .args([rc_path.as_str(), "-O", "coff", "-o", res_path.as_str()])
                .status();
            if matches!(status, Ok(s) if s.success()) {
                println!("cargo:rustc-link-arg={}", res_path);
                embedded = true;
                break;
            }
        }
        if !embedded {
            println!(
                "cargo:warning=no windres found; the app icon and version info will not be embedded"
            );
        }
    }
}
