# Clean-host testing: failures first, then the full run

Run in this order on every release candidate. Stage 1 proves each failure shows the right message; only then install what was
missing (Stage 2) and run the intended tests (Stage 3). Never skip to Stage 3 on a machine that already has the dependencies:
that is how the "Foundry must be installed" report was missed.

## Stage 1: before dependencies (record screenshot + the copied details for each row)

| # | Case | How to create it | Expected result |
|---|---|---|---|
| 1 | Fresh Windows host, nothing installed, no Foundry Local | Clean VM or reset machine; install the signed setup `.exe` | App starts, downloads model on its own; no "install Foundry" text anywhere |
| 2 | AI runtime files missing | Delete `Microsoft.AI.Foundry.Local.Core.dll` from the install folder (Windows), (on macOS, `PLUGABLE_FOUNDRY_LIBRARY_DIR=<empty folder>` was tried on 2026-10-07 in a dev build and did **not** produce the failure: the SDK still loaded, so this row needs a packaged Windows install) | Red "Plugable Chat can't start" card: reinstall link, VC++ Redistributable link, raw error shown, **no Mac wording on Windows** |
| 3 | Legacy CLI path, CLI missing | `PLUGABLE_FOUNDRY_BACKEND=cli_http`, `foundry` not on PATH | App starts on the built-in engine (rc15 onward; rc14 would not start) and logs that the CLI is not installed; no card, no hang |
| 4 | CLI installed, service will not start | `cli_http`, make `foundry service start` fail (rename a required file or use a bad shim) | Card names the failed command and its exit code (not "Could not connect") |
| 5 | No NVIDIA driver / unsupported GPU | Windows host with no NVIDIA driver (or Intel/AMD only) | Card or status explains the GPU problem with the driver link; app does not look hung |
| 6 | No internet at first launch | Disconnect network before first start | Download failure text mentions internet and disk space, not a CLI command |
| 7 | Previously-installed Foundry Local + Plugable Chat (David's laptop) | Host with both already present | Same behaviour as case 1; bundled engine is used |
| 9 | App died while answering last time | Ask a question, kill the process (`taskkill /F /IM plugable-chat.exe`) while the answer streams; launch again | Notice names the model and the request size and says it will be switched off if it happens again; killing it mid-answer a second time on the same model switches that model off (Settings > Models shows why, with *Retry anyway*) |
| 10 | Model larger than its context window | Select `gpt-oss-20b` (8192-token context) and ask a question with a long history | The answer arrives; no "isn't compatible" banner; the model is not marked *May not run here* |
| 8 | App died during startup last time | Launch the app, then kill it (`taskkill /F /IM plugable-chat.exe`) before a model name appears; launch again | Amber "did not finish starting last time" notice with the expected-case explanation; a normal close or a successful start never shows it |

Pass criteria for every row: a human with no context can tell what is wrong and what to do from the screen alone.
Rows 3, 4 and 6 can be run on macOS; rows 1, 2, 5 and 7 need Windows.

## Stage 2: install the dependencies

Install only what the Stage 1 message asked for, using the link on the card (driver, VC++ Redistributable, reinstall).
Re-run the failing row after each install and confirm that exact message goes away.

## Stage 3: full intended tests

1. `scripts/preflight.sh` (Rust lib tests, frontend unit tests, typecheck).
2. `scripts/ci/verify-installers.ps1 -Nsis <exe> [-Msi <msi>]` (signatures, the AI runtime ships, the installer hook ran, the VC++ payload matches its pin, running the app writes nothing under the install folder).
3. Install the signed build on a Windows RTX host: first-run download, chat with the default model, switch model, restart the app.
4. Repeat case 7's host to confirm an existing Foundry Local does not interfere.

## Automated matrix (repro-first)

`scripts/ci/faults.ps1` automates Stage 1 and the plan's fault table. Every fix PR for a first-run issue
needs its scenario red before the fix and green after; `-ExpectFail` is how "red before" is proven.

Each scenario installs the setup `.exe` perMachine, injects one fault, runs `plugable-chat.exe --smoke`
with `PLUGABLE_CHAT_TEST_STATE=<file>`, and asserts on the JSON lines the app writes there
(`{ts, phase, status, detail}`; phases `ep-registration`, `embedding`, `model-download`, `toolbox`, `ready`,
`error`). Statuses counted as success or failure are listed in one place, the top of
`scripts/ci/CleanHost.psm1`; change them there if the backend uses other words. A build without the hook
writes no file, so it reads as red.

| Scenario | Fault | Issue | Green means | Runs on |
|---|---|---|---|---|
| `baseline` | none | #1 #2 | hook registry key present; embedding and ready ok | CI, box |
| `readonly-nonadmin` | standard user, read-only install dir, cwd = install dir | #1 | embedding ok, models under the user profile, install dir unchanged | CI, box |
| `offline-hosts-block` | huggingface hosts -> 127.0.0.1 | #1 #3 | embedding is an explicit error naming the network, never ok without model files | CI, box |
| `proxy-dead` | `HTTPS_PROXY` at a closed port | #1 #4 | error names the proxy/connection, no hang | CI, box |
| `proxy-tls-intercept` | mitmproxy, untrusted CA | #1 #4 | error names the certificate (skipped without `mitmdump`) | CI, box |
| `no-vcredist` | VC++ absent before install | #7 #3 | hook installs it (`installed`/`installed-reboot`), ep-registration ok | clean Server image only |
| `driver-absent` | no NVIDIA driver | #3 | ep-registration explains the driver, ready reached on CPU | CPU host |
| `kill-mid-download` | `taskkill /F /T` mid-download | #5 | relaunch completes, no corrupt-partial error | CI, box |
| `unicode-username` | user `Zo<e-diaeresis> M<u-diaeresis>ller` | data-dir bugs | embedding ok, data dir under that profile | CI, box |
| `low-disk` | data dir on a 300 MB VHD (junction) | download failures | embedding error names disk space | CI, box |
| `quarantined-dll` | delete `foundry-libs\onnxruntime.dll` | rc9 card | ready after self-repair, or an error naming the DLL | CI, box |
| `upgrade-over-old` | install `-OldInstaller`, then this build over it | stale files | stale `foundry-libs` file gone, user data kept, one uninstall entry | CI, box |
| `nvidia-present-no-providers` | staged: NVIDIA adapter + driver, engine reports only CPU (twice, cold relaunch) | Pat R1, R3 | `gpu-assessment` says "hardware not used" naming the NVIDIA adapter, never "no GPU"; a CPU model runs; the preflight report carries both views | CI, box |
| `egpu-thunderbolt-hint` | staged: NVIDIA adapter + Thunderbolt controller, engine reports only CPU | Pat R2 | the assessment tells the user to check the Thunderbolt connection | CI, box |
| `egpu-late-arrival` | staged: no adapter at launch, an NVIDIA adapter appears at 20 s, the engine lists CUDA after a retry | Pat R2, R4 | the app notices by itself, retries GPU setup, and the assessment moves on from "no GPU" | CI, box |
| `engine-declined` | staged: NVIDIA adapter, engine lists CUDA and WebGpu (twice) as not registered and answers registration with `success:false` and empty lists | Pat E1, E2, E4 (rc12) | the card names the engine's refusal, not the hardware; each provider reported once; a hint points to the engine log; a CPU model runs | CI, box |
| `embedding-first-run-contention` | huggingface unreachable for the first 25 s of a first start | Bob R6, R10 | the embedding model still loads (5 attempts over about 45 s); a final failure would carry attempts and cause | CI, box |
| `version-visible` | none | Bob R12, Pat R13 | `--version` prints the release label and commit, and matches the installer file name | CI, box |
| `gpu-baseline` | none, NVIDIA driver present | #3 | ep-registration and model-download ok | GPU box only |

Rows that cannot be set up on the current host are reported as skipped with the reason, never as passed.

### Run it

```powershell
# anywhere: list scenarios, run the pure-logic tests
pwsh scripts/ci/faults.ps1 -Installer x -List
pwsh scripts/ci/clean-host.tests.ps1

# on a DISPOSABLE Windows host (edits hosts file, creates users, mounts a VHD)
$env:CLEAN_HOST_DISPOSABLE = "1"
./scripts/ci/faults.ps1 -Installer C:\x\plugable-chat_setup.exe -Scenario cpu
./scripts/ci/faults.ps1 -Installer C:\x\rc9-setup.exe -Scenario all -ExpectFail   # prove each row is red on rc9
./scripts/ci/faults.ps1 -Installer new.exe -OldInstaller rc9.exe -Scenario upgrade-over-old
```

Output: `clean-host-out\clean-host-junit.xml` plus one folder per scenario (state file, app output, install
and VC++ logs). Exit code = number of failed scenarios.

**CI:** `.github/workflows/clean-host.yml` runs the `cpu` scenarios nightly on `windows-latest` against the newest
unsigned installer from `ci.yml` on main, and on demand (inputs: scenarios, `installer_release_tag`,
`old_release_tag`, `expect_fail`). `windows-latest` already has VC++ and no GPU, so `no-vcredist`,
`gpu-baseline` and (if a driver exists) `driver-absent` skip there.

**AWS GPU box** (`infra/aws-gpu`, needs `SPIKE_SSM=1 ./launch.sh` for SSM, HEAD pushed):

```bash
./run-matrix.sh <instance-id> <bucket> <presigned-installer-url> gpu           # g5 box with driver
OLD_INSTALLER_URL=<presigned-rc9-url> ./run-matrix.sh <iid> <bucket> <url> upgrade-over-old,quarantined-dll
# CPU-only / no-VC++ rows: INSTANCE_TYPE=m5.xlarge ./launch.sh, then bootstrap-box.ps1 -SkipGpuDriver
VCREDIST_URL=<presigned vc_redist url> ./run-matrix.sh <iid> <bucket> <url> no-vcredist,driver-absent
```

`bootstrap-box.ps1` reports (never installs) the VC++ runtime so you know whether `no-vcredist` can run.
"Driver rolled back" on the g5 box is manual: uninstall the NVIDIA driver, reboot, run `driver-absent`.
Still manual, no scenario yet: kill mid-chat, a full TLS-intercepting corporate proxy with a trusted CA, a
real third-party AV quarantine.

## The installer hook (NSIS)

`src-tauri/windows/hooks.nsh` (`bundle.windows.nsis.installerHooks`):

- **Before copying:** closes a running instance; deletes `foundry-libs` (5 retries) and any runtime DLLs older
  releases left beside the exe, so a mismatched onnxruntime can never load.
- **After copying:** checks `HKLM\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64` (`Installed=1`, `Bld` >=
  `PC_VCREDIST_MIN_BLD`); if not, runs the bundled `redist\vc_redist.x64.exe /install /quiet /norestart`.
  Exit 0 = installed, 1638 = newer already present, 3010/1641 = installed, restart needed (continue, flagged);
  anything else is logged with a MessageBox (interactive) and **not fatal**: the app's own can't-start card and
  first-launch repair take over. `redist\vc_redist.x64.exe` is left in place for that repair.
- **WebView2:** after Tauri's `embedBootstrapper` step, verifies the runtime is registered; if not (bootstrapper
  blocked by a proxy), logs and shows the Evergreen Standalone link. For networks that block the bootstrapper
  outright, switch `webviewInstallMode` to `offlineInstaller` (+~170 MB) rather than adding more hook code.
- **Records:** `%TEMP%\plugable-chat-install.log` and `HKLM\SOFTWARE\Plugable\plugable-chat\Installer`
  (`HookVersion`, `VCRedistStatus`, `VCRedistExitCode`, `VCRedistBld`, `RebootRequired`, `WebView2Status`). The key is removed on
  uninstall. User data in `%APPDATA%` / `%LOCALAPPDATA%\plugable-chat` is never touched on upgrade or uninstall.
- **Silent installs** (`/S`, Intune, SCCM) only log. Under SYSTEM `%TEMP%` is `C:\Windows\Temp`.

### MSI has no hook: recommendation

Tauri's WiX bundler has no installer-hook equivalent. The MSI still gets `redist\vc_redist.x64.exe` (shared
resource list) but nothing runs it at install; only the app's first-launch repair can. A WiX fragment with a
deferred custom action is possible but untestable without a Windows box and adds a second place for this logic
to rot. **Recommendation: ship NSIS only** (change `--bundles nsis,msi` to `--bundles nsis` in `release.yml` and
`ci.yml`, and the download page) unless a customer needs an MSI for Group Policy; if one does, deploy
`vc_redist.x64.exe` beside it. Not changed here because `release.yml` is owned elsewhere.

### The VC++ redistributable pin (one human step before the first release)

`scripts/ci/vcredist.pin.json` ships with `sha256: TODO-PIN-AFTER-REVIEW`; `scripts/ci/fetch-vcredist.ps1`
fails until it is set. To set it: run `./scripts/ci/fetch-vcredist.ps1 -UpdatePin` (enforces a valid Microsoft
signature), check the printed version and signer, confirm `minBld` (and `PC_VCREDIST_MIN_BLD` in `hooks.nsh`;
`clean-host.tests.ps1` fails if they differ), and commit the pin. Caveat: `aka.ms/vs/17/release/...` always
serves Microsoft's newest build, so the pin goes stale when Microsoft ships one and the build fails with a hash
mismatch. For a stable pin, host the reviewed file somewhere we control and set `url` to it.

`build.rs` runs the script when `CI` is set (or `PLUGABLE_STAGE_VCREDIST=1`), before any signing secret is
present. A failure is a warning on ordinary builds and a build error on tag builds or when
`PLUGABLE_REQUIRE_VCREDIST=1`. `verify-installers.ps1` fails if the payload is absent or its hash differs.

## Static CRT

`.cargo/config.toml` sets `+crt-static` for `x86_64-pc-windows-msvc` so `plugable-chat.exe` starts without the
VC++ runtime and can show its own card. The native libraries (onnxruntime, Foundry core) still need the runtime,
which is why the hook exists. Known risk: fastembed pulls `ort 2.0.0-rc.9`, whose prebuilt static ONNX Runtime
may be built against the dynamic runtime and fail to link (`LNK2038 RuntimeLibrary mismatch`). It could not be
built on macOS; the first Windows CI run decides. `scripts/ci/assert-static-crt.ps1` (run in `ci.yml`) proves the
exe has no `vcruntime140`/`msvcp140` import. If the link fails, remove the `[target...]` table and rely on the
hook plus the app's preflight, or move fastembed to `ort-load-dynamic`.

## Test seams (unattended runs only)

When `PLUGABLE_CHAT_TEST_STATE` is set, the app also reads these, so a scenario can stage what Windows and the
model engine "report" on a machine that has no such hardware. Without `PLUGABLE_CHAT_TEST_STATE` they are ignored,
so a stray variable cannot change what a shipped app says.

| Variable | Staged value |
|---|---|
| `PLUGABLE_CHAT_TEST_ADAPTERS` | JSON `{"adapters":[{"name","vendor","vramMb","pnpId","status","problemCode"}],"thunderboltDevices":[...]}` in place of the Windows adapter scan |
| `PLUGABLE_CHAT_TEST_ADAPTERS_LATE` + `..._LATE_AFTER_SECS` | the same JSON, used once the app has run that many seconds (an adapter that appears late) |
| `PLUGABLE_CHAT_TEST_EPS` | JSON `[{"name","isRegistered"}]` in place of the engine's provider list (nothing is downloaded for these) |
| `PLUGABLE_CHAT_TEST_EPS_AFTER_RETRY` | the provider list once a GPU setup retry has been requested ("works the second time") |
| `PLUGABLE_CHAT_TEST_NVIDIA_SMI` | the driver version `nvidia-smi` "reports" (empty = no answer) |

The state file gains a `gpu-assessment` phase: `detail` is `<State> | <headline>`. It never decides a `--smoke`
run. The app also writes `%APPDATA%\plugable-chat\logs\plugable-chat.log` (rotating) and
`preflight-<time>.json` (the last five), which is what "Copy details for support" attaches.

## Reported issues and the tests that reproduce them

Every row below failed on rc10/rc11 and is covered by a test that was written to fail there. `R` numbers match the plan.

| # | Reported by | Symptom | Test |
|---|---|---|---|
| R1 | Pat | NVIDIA GPU + driver present, a CPU model loads, no GPU models offered | `gpu_assessment::r1_*`; `model_chain::r1_*`; scenario `nvidia-present-no-providers`; `gpu-assessment.test.ts` r1 |
| R2 | Pat | the GPU is an external TBT5-AI box | `gpu_assessment::r2_*`; `gpu_registration::r2_*` (late adapter); scenarios `egpu-thunderbolt-hint`, `egpu-late-arrival` |
| R3 | Pat | a reboot changes nothing (an earlier failed look was reused) | `preflight::r3_every_refresh_asks_again_*`; scenario `nvidia-present-no-providers` (second, cold run) |
| R4 | Pat, Bob | "Retry GPU setup" does nothing visible | `retry.test.ts` r4 (the retry now waits for the whole attempt and refreshes the list); scenario `egpu-late-arrival` |
| R5 | Pat, Bob | a failed model-list read shown as "No GPU models are available" | `gpu_assessment::r5_*`; `gpu-assessment.test.ts` r5 |
| R6 | Bob | first start: red "Failed to load CPU embedding model" | `embedding_init::r6_*` (retry budget); scenario `embedding-first-run-contention` |
| R7 | Bob | Retry never clears the row | `embedding_init::r7_*`; `retry.test.ts` r7 |
| R8 | Bob | a native-library panic ends every retry | `embedding_init::r8_*` |
| R9 | Bob | no "Copy details for support" on that error | `support.test.ts` r9 |
| R10 | Bob | fails once, clean on the second start (partial download) | existing `kill-mid-download`; `embedding_init::a_corrupt_model_cache_*`, `stale_download_files_*` |
| R11 | Bob | upgrade over an older install | existing `upgrade-over-old` (no red/amber row; ready) |
| R12 | Bob | no version in the title bar | `version_info::*`; `version-info.test.ts`; scenario `version-visible` |
| R13 | Pat | rc10 and rc11 installers share one file name | `scripts/ci/release-names.test.mjs`; release.yml renames after signing; scenario `version-visible` |
| R14 | Pat | machine without the VC++ runtime | existing `no-vcredist`; `gpu_assessment::missing_vcpp_is_called_out` |
| R15 | Pat (rc12) | the card said "not using it" seconds into GPU setup | `gpu_assessment::r15_*` |
| R16 | Pat (rc12) | the engine refused with no reason and the card blamed the hardware | `gpu_assessment::r16_*`; `sdk::r18_*`; scenario `engine-declined` |
| R17 | Pat (rc12) | two adapters made the engine list providers twice | `sdk::r17_*`; scenario `engine-declined` |

Add the next report as R15 with its test in the same change as its fix.

What has not been exercised: the Windows-only code (the PowerShell adapter scan, free-disk and RAM probes) and the
scenarios above have been parsed and listed on macOS only; the staged-GPU behaviour was run end to end on a macOS build
of the real app (late adapter -> retry -> "ready"). They need a run on a disposable Windows host and on Pat's machine.
