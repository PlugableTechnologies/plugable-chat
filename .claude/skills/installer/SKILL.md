---
name: installer
description: How the Windows installer is configured, installed silently, upgraded, repaired and uninstalled, and what the first run of the installed app needs. Use for installer, deployment, first-run, or "why doesn't the installed app work" questions.
---

# Windows installer and first run

## Configuration (`src-tauri/tauri.conf.json`, ask-first file)
- NSIS `installMode: perMachine` (Program Files, HKLM uninstall entry, all-users Start Menu shortcut), lzma compression,
  `minimumWebview2Version 110.0.1587.0`, WebView2 via `embedBootstrapper` (silent). MSI is also built by `release.yml`.
- **Why perMachine:** the per-user mode installs into `C:\Windows\System32\config\systemprofile\AppData\Local\plugable-chat`
  when run silently as SYSTEM, which is how SSM, Intune and SCCM run installers, so managed users never saw the app.
  Cost: a user without admin rights cannot self-install. See `docs/gpu-validation.md` ("Installer scope").
- Silent install: `plugable-chat_<ver>_x64-setup.exe /S` (add `/D=C:\path` only for test boxes; it must be the last argument).
  Uninstall: `<install dir>\uninstall.exe /S`.

## Tested lifecycle (`infra/aws-gpu/installer-lifecycle.ps1`, run as SYSTEM on a box)
Fresh install (~21 s), uninstall entry in HKLM only, nothing in the SYSTEM profile, all-users shortcut, app stays up as
Administrator and as a non-admin user, upgrade over a running copy leaves one entry, repair restores exe and native
library, silent uninstall removes exe/entry/shortcut, reinstall. Result 2026-09-29: **30/30**. Script lessons: `schtasks /tr`
splits unquoted paths with spaces (launch through a `.cmd`), a new local user needs the "log on as batch job" right,
`Stop-Process` then wait before deleting a locked DLL.

## What the installed app needs on a fresh machine
- **Bundled next to the exe:** `foundry-libs\*` (Foundry Local Core, ONNX Runtime, GenAI, providers_shared, Windows ML DLL),
  `test-data\demo.db` (Chicago crimes, 227,299 rows).
- **Downloads at first run:** GPU execution providers (~1.5 GB for CUDA/WebGPU; 45 s to 11 min observed, silent in the log, a
  Cancel button and a 15 s heartbeat log exist), the chat model (Phi-4-mini ~4 GB, qwen3.5-4b ~2.5 to 4 GB), and the MCP
  Database Toolbox `toolbox.exe` on demand for the demo database (the app does not bundle it).
- **Per-user model cache:** models live under the user's profile (under the profile, e.g. `~/.foundry/cache/models` on the Mac; check the exact Windows path on a box), so each user downloads their own.
  Tests run as SYSTEM download into the SYSTEM cache; `infra/aws-gpu/sync-model-cache.ps1` copies to the Administrator cache.
- Unsigned CI installers trigger SmartScreen; only the tagged release build is signed ([release-signing](../release-signing/SKILL.md)).
