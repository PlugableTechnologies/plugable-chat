---
name: build-and-ci
description: How plugable-chat builds and what CI does. Use when a build, CI run, dependency update, or Tauri/Rust/npm version change is involved, or when a Windows/Linux/macOS build fails.
---

# Building plugable-chat and reading CI

Ladder first: run `scripts/preflight.sh` before pushing ([test-ladder](../test-ladder/SKILL.md)).

## Pins and why (each one cost a CI round trip)
- **Rust 1.94.1** (`RUSTUP_TOOLCHAIN`): `ethnum` does not compile on the newest stable.
- **protoc 30.2**, hash-checked in CI (`PROTOC` env). Linux also needs `libprotobuf-dev` and `libgtk-3-dev`.
- **rfd**: `default-features = false, features = ["gtk3"]` (the xdg-portal backend conflicts with tauri-plugin-dialog's gtk3).
- **Windows delay-loads comctl32** (`src-tauri/build.rs`, `delayimp` + `/DELAYLOAD:comctl32.dll`): test binaries have no
  manifest, so the loader picked comctl32 v5.82, which lacks `TaskDialogIndirect` (used by rfd), and the test exe died with
  `STATUS_ENTRYPOINT_NOT_FOUND`. DirectML was the wrong first guess.
- **Foundry Local**: `foundry-local-sdk` is pinned (`1.2.3`); `winml` is a default cargo feature and is Windows-only
  (Windows ML bundles `Microsoft.Windows.AI.MachineLearning.dll`). `build.rs` also stages `onnxruntime_providers_shared.dll`
  next to the exe; without it the CUDA provider fails to register (Win32 error 126).
- **Tauri**: the `tauri` crate and `@tauri-apps/api` + `@tauri-apps/cli` must share major.minor. `tauri build` checks this
  only at its last step, so `scripts/ci/check-tauri-versions.mjs` runs first (tests cover LF and CRLF `Cargo.lock`; Windows
  checkouts use CRLF). Upgrade the crate family and the npm packages together.

## CI (`.github/workflows/ci.yml`)
- Jobs: **Unit tests (Linux)** (~11 min) and **Build and unit tests (Windows)** (~20 min cold; cache saved on `main`).
- Windows uploads `gpu-tests-<sha>` early (~8 min: compiled test exes + ONNX Runtime/Foundry DLLs) and
  `windows-unsigned-<sha>` last (the **debug-profile** NSIS installer). Debug because the optimized release build did not
  finish in 77 minutes; `release.yml` now allows 180 minutes.
- `concurrency` is meant to cancel only PR runs, but runs on `main` were seen cancelled: **do not push to `main` while a CI
  run you need is still in flight**, and check the run page (`gh run view`) rather than assuming.
- Docs-only pushes are ignored by `paths-ignore`.
- Read a failure with `gh run view <id> --log-failed`; the failing step is often only the last symptom (a later
  `if: always()` step failing is usually fallout).

## Dependencies and advisories
- npm has an org **7-day cooldown** (`.npmrc` `min-release-age=7`). Bypass only with the user's explicit decision and only
  per command (`npm install pkg@x --min-release-age=0`); `.npmrc` stays unchanged. Done once, 2026-09-30, for
  `@tauri-apps/api` and `@tauri-apps/cli` 2.12.0.
- `npm audit fix` cleared all JS advisories (0 remaining). Rust: prefer targeted `cargo update -p <crate>`; bumping only
  `tauri` with `--precise` does not compile (the runtime crates must move together).
- Weekly `osv-scanner.yml` failed for weeks on real advisories; it now passes with justified ignores recorded in
  `osv-scanner.toml` and `deny.toml`. `cargo-deny.yml` runs weekly too.
- Dependabot cargo jobs sometimes show failures; check before ignoring.

## Ask-first files
`.github/workflows/release.yml`, `scripts/sign-windows.mjs`, `scripts/verify-windows-signatures.ps1`,
`.github/CODEOWNERS`, `src-tauri/tauri*.conf.json`.
