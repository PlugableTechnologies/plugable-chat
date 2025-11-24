# Plugable Chat

A high-performance, local-first chat application built with Tauri v2, React, and Rust.

## Quick Start

This project is configured as a Rust Workspace. You can build and run the entire application (frontend + backend) from the top-level directory using standard Cargo commands.

### 1. Development & Running
To start the application in development mode:

```bash
cargo run
```
*This will automatically install Node dependencies, build the frontend, and compile the Rust backend.*

### 2. Production Binary
To build the optimized production binary:

```bash
cargo build --release
```
The binary will be located at: `target/release/plugable-chat`

## Build Automation & Bundling

For creating distributable installers (DMG, MSI, etc.), we provide automation scripts in the `scripts/` directory, similar to other Rust desktop projects.

### macOS (Bundle .app/.dmg)
```bash
./scripts/bundle-macos.sh
```

### Windows (Bundle .msi/.exe)
```powershell
.\scripts\bundle-windows.ps1
```

## Icon Generation

The repo only tracks the highest-resolution transparent PNG (`src-tauri/icons/icon.png`). All other sizes (`32x32`, `128x128`, retina variants, `.ico`, `.icns`, etc.) are generated on-demand via the `generate-icons` script powered by Tauri's `tauri-icon` CLI.

```bash
npm run generate-icons
```

This script runs automatically before every `npm run dev|build|preview` invocation (and via the Rust workspace `cargo run`/`cargo build --release` flow thanks to `src-tauri/build.rs`), so you rarely need to run it manually unless you replace the source logo file.

## Project Structure

- **`/` (Root)**: Cargo Workspace configuration.
- **`src-tauri/`**: Rust backend and Tauri configuration.
- **`src/`**: React frontend code.
- **`scripts/`**: Platform-specific build automation.

## Prerequisites

- Node.js (v18+)
- Rust (Stable)
- OS-specific build tools (Xcode for macOS, VS Build Tools for Windows)
