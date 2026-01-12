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

Double-click `requirements.bat` or run in Command Prompt/PowerShell:

```powershell
.\requirements.bat
```

This script will:
- Validate prerequisites (Windows version, disk space, network connectivity)
- Install Visual Studio Build Tools with C++ workload, Microsoft Foundry Local, Node.js, Rust, Git, and Protocol Buffers via winget
- Verify each installation and provide clear error messages if something fails
- Initialize the Rust toolchain
- Run `npm install` automatically
- Tell you exactly what to do next

> **Tip:** Run `requirements.bat --check` to diagnose issues without installing anything.

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

## MCP Test Server (dev)

- The dev MCP test server stays **disabled by default**, but you can launch it from the workspace:

```bash
cargo mcp-test
```

Or run it directly from the backend binary (no model/agentic loop):

```bash
cargo run -p plugable-chat -- --run-mcp-test-server
```

- If port `43030` is in use, override it:

```bash
cargo run -p plugable-chat -- --run-mcp-test-server --mcp-test-port 3333
```

- On startup it will:
  - Serve a small status UI at `http://127.0.0.1:43030` by default (opens your browser automatically; disable with `--open-ui false` or `--serve-ui false`; override with `--mcp-test-port <PORT>`).
  - Print a ready-made prompt you can paste into chat to trigger the full red/green sweep.
  - Expose MCP tools including `run_all_tests`, `get_test_status`, and the existing echo/math/json/error helpers.
- Endpoints:
  - `GET /` — UI with live red/green board and logs
  - `POST /api/run-all` — trigger full test sweep (same as the MCP tool)
  - `GET /api/status` — JSON with counts, per-test results, and the recommended prompt
  - `GET /api/logs` — recent log lines for agentic debugging

To auto-connect the desktop app to the dev test server, launch it with:

```bash
PLUGABLE_ENABLE_MCP_TEST=1 npx tauri dev
```

Helper scripts (start server + app together):
- macOS/Linux: `./scripts/mcp-test.sh`
- Windows: `powershell -ExecutionPolicy Bypass -File scripts/mcp-test.ps1`

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

## Troubleshooting

### Diagnostic Mode

Before troubleshooting manually, run the diagnostic check to identify issues:

```powershell
# Windows
.\requirements.bat --check

# macOS/Linux
./requirements.sh --check
```

This reports system info, installed components, and network connectivity without making changes.

### Windows Installation Issues

#### ERR_WINGET_MISSING - winget not found

**Symptoms:** Script exits immediately with "winget is required but not installed"

**Solution:**
1. Open Microsoft Store
2. Search for "App Installer"
3. Install or update "App Installer" by Microsoft Corporation
4. Close and reopen your terminal
5. Re-run `requirements.bat`

#### ERR_WINGET_TIMEOUT - Installation hangs at "Checking Node.js LTS..."

**Symptoms:** Script appears frozen during package checks or initialization

**Solutions:**
1. **Check for UAC dialogs** - Look behind the terminal window or in your taskbar for a "User Account Control" prompt
2. **Install Foundry Local first** - Some users report this prevents hanging:
   ```powershell
   winget install Microsoft.FoundryLocal
   ```
3. **Reset winget sources** (run PowerShell as Administrator):
   ```powershell
   winget source reset --force
   ```
4. Restart your computer and try again

#### ERR_CPP_WORKLOAD_MISSING - "link.exe not found" during build

**Symptoms:** Rust compilation fails with linker errors

**Solution:**
1. Open "Visual Studio Installer" from the Start Menu
2. Click "Modify" next to "Build Tools 2022"
3. Check the box for "Desktop development with C++"
4. Ensure "MSVC v143 - VS 2022 C++ x64/x86 build tools" is selected
5. Click "Modify" and wait for installation
6. Open a new terminal and re-run the build

#### ERR_VS_INSTALLER_RUNNING - Visual Studio Installer conflict

**Symptoms:** Script fails because Visual Studio Installer is already running

**Solution:**
1. Close all Visual Studio Installer windows
2. Check the system tray for any VS Installer processes
3. Wait for any pending updates to complete
4. Re-run `requirements.bat`

#### PowerShell execution policy errors

**Symptoms:** "running scripts is disabled on this system"

**Solution:** Always use `requirements.bat` (not the `.ps1` file directly). The batch wrapper handles execution policy automatically. If issues persist:

```powershell
# Run in PowerShell as Administrator
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
```

#### wasm32-wasip1 target installation fails

**Symptoms:** Warning about "rust-std-wasm32-wasi" not available

**Impact:** WASM sandboxing disabled, but the app still works (Python sandbox uses RustPython directly)

**Solution:** This is usually a temporary network issue. You can manually install later:
```powershell
rustup target add wasm32-wasip1
```

### Git Pull Shows Conflicts in `src-tauri/Cargo.toml`

If `git pull` reports conflicts or shows `Cargo.toml` as modified when you haven't changed it, this is usually caused by line ending differences between platforms. To fix:

```bash
# Reset line endings for the affected file
git checkout -- src-tauri/Cargo.toml

# Then pull normally
git pull
```

For a complete line ending refresh (one-time fix for existing clones):

```bash
# Re-normalize all files according to .gitattributes
git rm --cached -r .
git reset --hard
```

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
