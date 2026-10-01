# Testing ladder: local first, then EC2, then GitHub

Goal: few errors reach GitHub. Each rung is slower and costs more than the one below it, so a problem
should be caught on the lowest rung that can show it. When something does escape to a higher rung,
add a check to the lower rung (see "Escapes so far").

| Rung | Where | Cost | Time | Catches |
|---|---|---|---|---|
| 1 | Your Mac / dev machine | free | about 1 min (`scripts/preflight.sh`) | version mismatches, type errors, Rust compile errors and unit tests, signing-script logic, workflow syntax, script syntax, npm advisories |
| 1b | Mac, app against Foundry | free | minutes | app logic: prompts, tool calls, parsing, schema indexing, UI flows (CPU model builds, e.g. `qwen3.5-4b-generic-cpu:3`) |
| 2 | Fresh AWS Windows GPU box | about $0.5 to $1.4 per hour | 30 to 90 min | anything that needs real Windows or an NVIDIA GPU: the installed app, installer lifecycle, GPU providers, model answers on CUDA, screenshots |
| 3 | GitHub Actions | free (public repo) | about 20 min Windows build | the compile and unit-test gate on Windows and Linux; signing (needs the protected environment and a reviewer); release |

## Rung 1: before every push

```bash
scripts/preflight.sh          # about 1 minute; --quick skips the Rust unit tests
```

It runs: tauri crate/npm version match, `npm audit`, TypeScript check and frontend build, frontend tests,
signing-script tests (fake `smctl`), `cargo check --all-targets --locked` and the Rust unit tests, workflow YAML
parsing, PowerShell and shell syntax checks. When installed it also runs `actionlint`, `zizmor`, `cargo-deny`
and `osv-scanner` (`brew install actionlint zizmor cargo-deny osv-scanner`); these are reported as skipped
otherwise, and CI runs the same scanners.

Rung 1b, when the change touches app behaviour: run the app on the Mac against Foundry with the same
environment variables the box uses (`PLUGABLE_MODEL`, `PLUGABLE_INITIAL_PROMPT`, `PLUGABLE_ENABLE_DEMO_DB`,
`PLUGABLE_ALWAYS_ON_TABLES`) and read the log. Back up `~/Library/Application Support/plugable-chat/config.json`
first and restore it afterwards. The Mac cannot show Windows-only code paths, NSIS bundling, CUDA or the
winml providers.

## Rung 2: only what needs real Windows or a GPU

Start it only when rung 1 is green and the installer for the exact commit exists (CI artifacts
`windows-unsigned-<sha>` and `gpu-tests-<sha>`). Follow [gpu-validation.md](gpu-validation.md): at most 3 GPU
runs per task, always finish with `infra/aws-gpu/teardown.sh`, never leave a box up waiting for CI. Run the
installer lifecycle (`installer-lifecycle.ps1`) and the Chicago questions (`ask.sh`) and read the screenshots.
Signing credentials never go to the box.

## Rung 3: GitHub

CI is the compile gate for Windows and Linux. Do not push to `main` while a CI run you need is still in
flight (a newer push can cancel or replace it). Signing runs only in the `release-signing` environment and
needs a reviewer other than the person who started the run; the signing smoke test proves it before any tag.

## Escapes so far (each became a lower-rung check)

| Escape | Found at | Now caught at |
|---|---|---|
| tauri crate 2.12 with `@tauri-apps/api` 2.11 failed the Windows build at its last step (25 min in) | GitHub | rung 1: `scripts/ci/check-tauri-versions.mjs` (first check in preflight and CI) |
| `smctl` printed FAILED, exited 0, and the signing step looked successful | GitHub smoke test | rung 1: `scripts/sign-windows.test.mjs`; the script now verifies every signature itself |
| Weekly OSV scan failed for weeks on real advisories | GitHub (schedule) | rung 1: `npm audit` and `osv-scanner` in preflight |
| Launch prompt raced the demo-database schema index | EC2 box | rung 1b: run the Chicago question on the Mac with `PLUGABLE_ENABLE_DEMO_DB=true` |
| Qwen 3.5 XML tool calls were not parsed | EC2 box | rung 1: parser unit tests (`hermes_parser.rs`); rung 1b: Qwen CPU build on the Mac |
| Per-user installer landed in the SYSTEM profile when run as SYSTEM | EC2 box | rung 2: `installer-lifecycle.ps1` (perMachine, run as SYSTEM) |
| The new version check itself failed on Windows only: `Cargo.lock` is checked out with CRLF line endings there and the pattern expected LF | GitHub | rung 1: `scripts/ci/check-tauri-versions.test.mjs` parses both LF and CRLF text. Windows-only differences (line endings, path separators, quoting) cannot all be seen on a Mac, so scripts that read files should be tested with CRLF input |
| Signature check read "Unknown" on GitHub: `powershell -Command "<script>" <file>` never passes `$args`, and a Windows PowerShell 5.1 child of a PowerShell 7 step could not load its Security module (inherited `PSModulePath`) | GitHub smoke test | rung 2: the encoded command and the signed/unsigned results were run on a real Windows box (`notepad.exe` Valid, unsigned MSI NotSigned, path with spaces); rung 1: unit tests for the encoded script and the environment |
| `pwsh script.ps1 -Path a, b, c` failed with "positional parameter cannot be found" (also in `release.yml`'s final verify) | GitHub smoke test | rung 1: preflight runs the verifier with several files and fails on a binding error |
| Test MSI built with PowerShell 7 COM calls failed (`DISP_E_TYPEMISMATCH`) | GitHub smoke test | rung 2: `scripts/ci/make-test-msi.ps1` run on a small Windows box (about a cent) before use |
| The first release-candidate Linux build (v0.1.0-rc1) failed: `release.yml` lacked `libprotobuf-dev` and `libgtk-3-dev`, which `ci.yml` had needed since its first green run | GitHub release | rung 1: `scripts/ci/check-workflow-parity.py` (preflight and CI) fails when release.yml installs fewer apt packages than ci.yml, or pins a different Rust toolchain or protoc. Lesson: a workflow that only runs on a tag is never tested by CI; keep its environment identical to the one CI proves |


| All three release candidates (rc1 to rc3) hung for the full 180 minutes. `src-tauri/build.rs` starts a nested `cargo build` for the Python sandbox (WebAssembly) when the gitignored `src-tauri/wasm/python-sandbox.wasm` is missing, as it always is on CI. In a release build the outer cargo holds `target/release/.cargo-lock` and the nested build needs it for its own build scripts: a circular wait. Debug CI used `target/debug`, so it never hung. The earlier "77 minutes without finishing" was the same deadlock, not slow compilation | GitHub release (3 times, 9 hours of runner time) | rung 1: reproduced on the Mac by hiding the .wasm and running `cargo build --release --bins --features tauri/custom-protocol` (process tree: build-script-build -> cargo at 0% CPU, both cargos holding target/release/.cargo-lock). Fixed by giving the nested build `CARGO_TARGET_DIR` in `OUT_DIR`; the same build then finished in 5 min 30 s. A fresh-checkout release build is now the first thing to run locally before tagging; preflight checks that the fix is still in `build.rs`. |

