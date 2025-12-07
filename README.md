# Plugable Chat

A high-performance, local-first chat application built with Tauri v2, React, and Rust.

## Why Plugable Chat

- Local-first desktop app with Tauri: fast startup, small footprint, no browser overhead.
- Multiple model families (OpenAI-compatible, Gemma, Granite, Phi, etc.) with model-specific tool-calling formats.
- Offline-friendly: state stored locally; LanceDB keeps chat history on disk.
- Built-in agentic loop: models can call Python code, search tools, or MCP tools with auto-approval for built-ins.
- Streaming by default: tokens arrive via Tauri events and append to the latest assistant message for smooth UI updates.
- Robust listener management: the store uses generation counters to avoid duplicated Tauri event handlers during hot reloads.

## How It Works (High Level)

- Frontend: React 19 + Tailwind v4 (via `@tailwindcss/postcss`) in a single-page app embedded by Tauri.
- State: Zustand store (`src/store/chat-store.ts`) manages chats, streaming message assembly, and listener lifecycle.
- Backend: Rust (Tauri v2) handles model requests, streaming, and tool execution; profiles per model family live in `src-tauri/src/model_profiles.rs`.
- Tool calling: the agentic loop (`src-tauri/src/lib.rs`) resolves tools (built-in Python sandbox and tool search, plus MCP servers), executes them, and streams formatted results back.
- Python sandbox: RustPython-based sandbox with a curated allowlist; validation happens in `code_execution.rs` before execution.
- Vector store: LanceDB at `src-tauri/data/lancedb/chats.lance`; schema defined in `src-tauri/src/actors/vector_actor.rs` (drops/recreates on schema mismatch).
- Desktop build artifacts: Tauri bundles platform installers; icon variants generated from a single source PNG.

## 🚀 Getting Started

### macOS

Open Terminal and run:

```bash
./requirements.sh
```

This script will:
- Install Xcode Command Line Tools (if needed)
- Install Homebrew (if needed)
- Install Node.js, Rust, Git, and Protocol Buffers
- Run `npm install` automatically
- Tell you exactly what to do next

Once complete, start the app:

```bash
npx tauri dev
```

### Windows

Double-click `requirements.bat` or run in PowerShell:

```powershell
.\requirements.ps1
```

This script will:
- Install Node.js, Rust, Git, Visual Studio Build Tools, and Protocol Buffers via winget
- Initialize the Rust toolchain
- Run `npm install` automatically
- Tell you exactly what to do next

> **Note:** After installing Visual Studio Build Tools, you must open Visual Studio Installer and add the "Desktop development with C++" workload.

Once complete, start the app:

```bash
npx tauri dev
```

---

## Development Commands

### Run in Development Mode

```bash
npx tauri dev
```

Or using Cargo from the workspace root:

```bash
cargo run
```

*Both commands will build the frontend and compile the Rust backend.*

### Build for Production

```bash
npx tauri build
```

Or:

```bash
cargo build --release
```

The binary will be located at: `target/release/plugable-chat`

---

## Build Automation & Bundling

For creating distributable installers (DMG, MSI, etc.), use the automation scripts in `scripts/`:

### macOS (Bundle .app/.dmg)
```bash
./scripts/bundle-macos.sh
```

### Windows (Bundle .msi/.exe)
```powershell
.\scripts\bundle-windows.ps1
```

---

## Icon Generation

The repo tracks only the highest-resolution PNG (`src-tauri/icons/icon.png`). All other sizes and formats are generated automatically:

```bash
npm run generate-icons
```

This runs automatically during builds, so you only need to run it manually if you replace the source icon.

---

## Project Structure

| Directory | Description |
|-----------|-------------|
| `/` | Cargo Workspace root |
| `src-tauri/` | Rust backend and Tauri configuration |
| `src/` | React frontend (TypeScript) |
| `scripts/` | Platform-specific build automation |

---

## Prerequisites (Manual Installation)

If you prefer to install dependencies manually instead of using the requirements scripts:

| Dependency | macOS | Windows |
|------------|-------|---------|
| **Node.js** (v18+) | `brew install node` | `winget install OpenJS.NodeJS.LTS` |
| **Rust** (Stable) | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` | `winget install Rustlang.Rustup` |
| **Protocol Buffers** | `brew install protobuf` | `winget install Google.Protobuf` |
| **Build Tools** | Xcode CLT: `xcode-select --install` | VS Build Tools + C++ workload |

After installing, run:

```bash
npm install
npx tauri dev
```
