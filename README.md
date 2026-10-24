# QuickMD — Modern High-Performance Markdown Reader for Windows

<p align="center">
  <img src="assets/icons/logo.svg" alt="QuickMD Logo" width="128" height="128">
</p>

<p align="center">
  <strong>Fast, Secure, Offline & Native Markdown Reader for Windows</strong><br>
  Engineered with 100% Native Rust (Tao + Wry WebView2) with Zero-Localhost Daemon
</p>

<p align="center">
  <a href="https://github.com/koonnae741/QuickMD/releases"><img src="https://img.shields.io/badge/Release-v2.0.0-blue.svg?style=flat-square" alt="Version"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-green.svg?style=flat-square" alt="License"></a>
  <img src="https://img.shields.io/badge/Platform-Windows%20x64-0078D6.svg?style=flat-square&logo=windows" alt="Platform">
  <img src="https://img.shields.io/badge/Rust-2024%20Edition-DEA584.svg?style=flat-square&logo=rust" alt="Rust Edition">
  <img src="https://img.shields.io/badge/WebView2-Zero--Localhost-orange.svg?style=flat-square" alt="WebView2">
</p>

---

## Overview

**QuickMD** is an ultra-fast, lightweight standalone Markdown document reader built for Windows 10/11. Unlike traditional Electron or Node.js readers, QuickMD is compiled with 100% native Rust using Tao and Wry (Edge WebView2), booting in sub-milliseconds with minimal memory overhead and zero background daemon processes.

> **Credits & Authors:** Made by **Antigravity : Gemini 3.8 Flash** and **YoRHa2N**

---

## Key Features

* **Instant Startup & Zero Daemon:** Native Rust execution. Boots in milliseconds without Node.js, Python, or lingering background services.
* **Embedded Offline Assets (Zero-Localhost):** All stylesheets, icons, fonts, and scripts are directly embedded into the binary and served via secure custom URI scheme (`app://`), eliminating local web servers and network ports.
* **Full GitHub Flavored Markdown (GFM):** Complete support for tables (with 1-click "Copy as Markdown"), task checklists, footnotes, strikethroughs, and autolinks.
* **Offline LaTeX Math (KaTeX):** High-fidelity rendering for inline `$x$` and display block `$$\sum$$` equations with embedded offline KaTeX fonts.
* **Dynamic Mermaid Diagrams:** Interactive flowcharts, sequence diagrams, class diagrams, and state machines with automatic Light/Dark theme sync.
* **Syntax Highlighting:** Syntax highlighting for 100+ programming languages with 1-click clipboard copy.
* **Safe In-Page Search (Ctrl + F):** Native DOM text-node traversal search with instant match highlighting, completely resilient to XSS.
* **Outline & Table of Contents (Ctrl + T):** Dynamic document structure sidebar with smooth ScrollSpy navigation and full Unicode / Thai heading support.
* **Detailed Document Statistics (Ctrl + I):** Live calculation of words, characters, paragraphs, headings, tables, links, and math blocks.
* **Windows Shell Integration:** Native right-click `"Open with QuickMD"` context menu and default `.md` file handler via clean HKCU registry.

---

## Keyboard Shortcuts

| Shortcut | Description |
| :--- | :--- |
| `Ctrl + O` | Open Markdown file (`.md`, `.markdown`, `.txt`) |
| `Ctrl + F` | Toggle In-Page Search |
| `Ctrl + T` | Toggle Outline / Table of Contents sidebar |
| `Ctrl + Shift + T` | Cycle Themes (Dark, Light, Tokyo Night, Sepia, OLED) |
| `Ctrl + +` / `Ctrl + -` | Zoom In / Zoom Out text size |
| `Ctrl + 0` | Reset Zoom to 100% |
| `Ctrl + I` | View Document Statistics & Metadata |
| `Ctrl + P` | Print or Export to PDF |

---

## Installation

### Method 1: Official Windows Installer (Recommended)
Download the latest `QuickMD-2.0.0-windows-x64.msi` from [Releases](https://github.com/koonnae741/QuickMD/releases) and run the setup wizard.

### Method 2: Command Line Usage
```cmd
quickmd "C:\path\to\document.md"
```

To toggle Windows Explorer context menu association:
```cmd
QuickMD.exe --register
QuickMD.exe --unregister
```

---

## Building from Source

### Prerequisites
* [Rust](https://rustup.rs/) (2024 edition, 1.85+)
* Windows 10/11 x64
* LLVM / MinGW windres (for embedded PE icon and version resource compilation)
* [WiX Toolset v3.11](https://wixtoolset.org/) (optional, for building the MSI installer)

### Build Executable
```cmd
git clone https://github.com/koonnae741/QuickMD.git
cd QuickMD
cargo build --release
```
The optimized executable will be generated at `target/release/QuickMD.exe`.

### Build MSI Package
```cmd
candle.exe -nologo -arch x64 -out installer\quickmd.wixobj installer\quickmd.wxs
light.exe -nologo -ext WixUIExtension -sval -out installer\QuickMD-2.0.0-windows-x64.msi installer\quickmd.wixobj
```

---

## License

This project is licensed under the [MIT License](LICENSE) - see the LICENSE file for details.
