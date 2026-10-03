# GPU validation: how it works and what we learned

Plugable Chat is tested on a real Windows machine with an NVIDIA GPU before any release is
signed. The machine exists only for the length of a test run: it is created fresh from
Amazon's stock Windows image and destroyed at the end. Nothing is kept on AWS between runs, so
there is no cost between runs. This page is the runbook and the record of what was learned;
timings and cost are in [gpu-validation-timings.md](gpu-validation-timings.md).

## The pieces

| Piece | What it does | Cost |
|---|---|---|
| [`ci.yml`](../.github/workflows/ci.yml) | Builds on Windows and Linux, runs unit tests, keeps the unsigned (debug) installer and the compiled GPU tests | Free (public repo) |
| `infra/aws-gpu/10_network.sh` | One isolated network with no inbound access | Free |
| `infra/aws-gpu/00_budget.sh` | Monthly cost alert ($25 by default) | Free |
| `infra/aws-gpu/20_scheduler_role.sh` | The role and group behind the AWS-side hard stop (below) | Free |
| `infra/aws-gpu/launch.sh` | Starts one GPU box (default g4dn.xlarge, T4, 16 GB; `INSTANCE_TYPE=g5.xlarge` for an A10G) on Windows Server 2025 and sets its hard stop | T4 $0.526/h, A10G ~$1.0-1.4/h while running |
| `infra/aws-gpu/bootstrap-box.ps1` | Sets up the box: failsafe, NVIDIA driver, WebView2, Node, ffmpeg, auto-logon | (part of the run) |
| `infra/aws-gpu/ssm.sh` | Runs PowerShell on the box (debugging; needs `SPIKE_SSM=1`) | Free |
| `infra/aws-gpu/run-in-session.ps1`, `capture.ps1` | Run things and record the screen inside the desktop session | (part of the run) |
| `infra/aws-gpu/ask.sh` + `ask-app.ps1` + `chicago-questions.json` | Ask the installed app the Chicago crimes questions, one fresh launch each, and bring back screenshots | (part of the run) |
| `infra/aws-gpu/extend.sh` | Moves a box's hard stop to N minutes from now | Free |
| `infra/aws-gpu/teardown.sh` | Destroys the box(es) and proves nothing billable is left | Free |

## Hard time limits (three layers, so a hung run cannot keep costing)

1. **AWS terminates the box at a fixed time**, whatever the machine is doing. `launch.sh` creates a
   one-time EventBridge Scheduler entry (`MAX_RUN_MINUTES`, default 180) that calls
   `ec2:TerminateInstances`; it deletes itself after running. The role behind it can only terminate
   instances tagged `Project=plugable-chat-gpu`. Use `extend.sh <id> <minutes>` to move it; nothing
   extends it automatically.
2. **The guest powers itself off** at a stored deadline (`C:\gpu\deadline.txt`, default 150 minutes
   from bootstrap). It is a scheduled task that checks every 5 minutes, not a `shutdown /t` timer,
   because a one-shot timer is lost when the box reboots (auto-logon needs a reboot).
3. **The disk is deleted on termination and the instance terminates on shutdown.** A monthly budget
   alerts by e-mail, and `teardown.sh` fails if anything tagged for the project is still there.

## Running a test, start to finish

```bash
cd infra/aws-gpu
./10_network.sh && ./00_budget.sh && ./20_scheduler_role.sh     # once per AWS account

SPIKE_SSM=1 ./launch.sh                 # prints the instance id (also sets the AWS hard stop)
# ~100 s later the box answers. Provision it from the repo, by COMMIT HASH (not branch name):
./ssm.sh <id> "Invoke-WebRequest -UseBasicParsing 'https://raw.githubusercontent.com/PlugableTechnologies/plugable-chat/<sha>/infra/aws-gpu/bootstrap-box.ps1' -OutFile C:\gpu\bootstrap-box.ps1; powershell -File C:\gpu\bootstrap-box.ps1"
aws ec2 reboot-instances --instance-ids <id>                    # auto-logon creates the desktop
# deploy CI's gpu-tests / installer artifacts (lesson 9), then:
OUT=./ask-out ./ask.sh <id> <model-id> <bucket>                 # Chicago crimes questions
./teardown.sh                           # ALWAYS finish here; exits non-zero if anything is left
```

Expected answers for the Chicago questions are in `chicago-questions.json`, computed from
`test-data/demo.db` itself (for example 227,299 crimes; top types THEFT 52,813, BATTERY 41,130,
CRIMINAL DAMAGE 25,135; 36,070 arrests; Austin the busiest area with 11,358; 407 homicides).

## What the GPU box found about the app (2026-09-29)

| # | Finding | Status |
|---|---|---|
| A1 | The CUDA provider could not load: `onnxruntime_providers_cuda.dll` needs `onnxruntime_providers_shared.dll` (Windows error 126), which `build.rs` did not bundle, so Foundry offered CPU-only models. | Fixed (bundled) |
| A2 | Nothing in the app registered GPU execution providers, and registration is per process. The app's log said "EP registration deferred" and ran CPU-only. | Fixed: `register_execution_providers` at start-up, with progress events and a cancel command |
| A3 | Model warm-up called the CLI's `GET /openai/load/{name}`; the SDK-hosted service answers 404, so no model ever loaded and the GPU stayed idle. | Fixed: warm-up uses `model.load()`; a failed warm-up now shows in the UI |
| A4 | `--initial-prompt` failed: the launch-prompt call omitted the four attachment lists the `chat` command requires ("missing required key attachedFiles"). Found only by running the installed app. | Fixed |
| A5 | In SDK mode a short model name (`phi-4-mini-instruct`, `qwen3.5-4b`) could not be resolved, so the first-run download and the fallback download failed. | Fixed: short names resolve to the best variant for the machine |
| A6 | `winml` did not register providers by itself: `already_registered` was empty and the log showed Foundry's own CUDA/WebGPU bootstrappers. The explicit start-up step is needed with `winml` too. | Known; `winml` is the Windows default |
| A7 | `qwen3.5-4b` (CUDA) fails to generate on an NVIDIA T4 (Turing): `LinearAttention ... CUDA failure 1: invalid argument` at layer 0. On an A10G (Ampere) it runs but wrote 17,379 characters for a one-line prompt and hit a five-minute limit (measured without a reasoning-effort setting, which the app does send). | Default is `qwen3.5-4b`, fallback `phi-4-mini-instruct`; the fallback only triggers after a first failed chat (see open items) |
| A8 | Ignored suite on the T4: 10 of 20 pass. Failures are environmental: 4 call the `foundry` command-line tool the box lacks, 3 cannot find `test-data/`, 1 assumes a Mac WebGPU host; 2 (`native_fallback_to_hermes`, `tool_search_discovers_deferred`) are undiagnosed. | Open |

Measured with the production code on Windows Server 2025 (build 26100) with `winml`: CUDA and WebGPU
both registered in 45.5 s on a fresh box; 48 of 48 catalog models became GPU variants (all CUDA);
a Phi-4-mini chat ran at 91% GPU and 9,685 MiB; the real app registered its own providers on first
run (80 s) and its warm-up put the model on the GPU (VRAM 80 MiB to about 5 GB).

## Lessons: the box

1. **Turn the PowerShell progress bar off** (`$ProgressPreference = "SilentlyContinue"`) before any
   download. With it on, the 748 MB NVIDIA driver never finished in 15 minutes; with it off, 13 seconds.
2. **The NVIDIA driver AWS publishes for G instances needs no Marketplace subscription** (public bucket
   `ec2-windows-nvidia-drivers`). Silent install ~2 minutes; the T4 and A10G appear without a reboot.
3. **Auto-logon is what creates a desktop.** SSM and user-data run in session 0 with no desktop, so no
   screen capture and no app window. Set auto-logon (random one-run password), reboot once, and session 1
   exists. Run anything that needs the desktop through a scheduled task with `/it`
   (`run-in-session.ps1`).
4. **A one-shot `shutdown /t` is lost on reboot.** The guest failsafe is a scheduled task that checks a
   stored deadline; the AWS-side schedule is the backstop.
5. **Record with `gdigrab`, not `ddagrab`** (`ddagrab` fails to open on this driver). Take stills from a
   hidden `wscript.exe` launch, otherwise a black `cmd` window appears in the picture.
6. **Windows Server 2022 has no WebView2** (Server 2025 does). Install it if missing (67 s).
7. **SSM output stops at ~24 KB** and long commands outlive most tool timeouts. Write results to a file
   and run long steps in the background.
8. **Put scripts in the repo and fetch them by commit hash**, not by branch name (`raw.githubusercontent.com`
   serves a cached branch file for minutes) and not pasted inline (bash-to-PowerShell quoting broke
   several one-liners).
9. **Sending files.** Get CI artifacts with `GET /repos/.../actions/artifacts/<id>/zip`: it answers 302 with
   a short-lived signed URL the box fetches in about a second (100 MB through a laptop connection kept
   resetting). Send results back with a presigned S3 `PUT` and `curl.exe -T`.
10. **Pipelines in PowerShell can hand over a whole list as one item.**
    `Invoke-RestMethod ... | Where-Object {...}` gave one item containing all 288 Node versions and a
    400-error URL. Assign to a variable first, then filter. Retry every download.
11. **Pass prompts to the app through `PLUGABLE_INITIAL_PROMPT`** (and `PLUGABLE_MODEL`,
    `PLUGABLE_ENABLE_DEMO_DB`, `PLUGABLE_ALWAYS_ON_TABLES`), not the command line: `Start-Process
    -ArgumentList` does not quote array items, so a prompt with spaces is split into arguments.
12. **The first run of provider registration downloads ~1.5 GB.** It took 279 s on the first box and 45 to
    80 s on later ones; afterwards it takes ~3 s. Nothing is kept between runs, so every fresh box pays it.

## Lessons: CI

13. **Pin Rust 1.94.1.** `ethnum 1.5.2` (via lancedb, lance, jsonb) does not compile on the newest stable
    ("cannot transmute between types of different sizes"). Remove the pin when that chain moves. The
    release workflow needs the same pin.
14. **Install `protoc`** (`lance-encoding` needs it); on Linux also `libprotobuf-dev` and `libgtk-3-dev`.
    Windows uses a pinned, hash-checked download.
15. **Linux never built:** `rfd` enabled its `xdg-portal` backend by default while Tauri's dialog plugin
    enabled `gtk3`, and `rfd` refuses both. Fixed with `default-features = false, features = ["gtk3"]`.
16. **The Windows unit-test program died at start-up with `STATUS_ENTRYPOINT_NOT_FOUND` (0xc0000139).**
    The cause was **not** DirectML (that was a wrong first guess). `rfd` imports `TaskDialogIndirect`,
    which only Common Controls v6 exports; a test program has no application manifest, so Windows binds
    it to the old comctl32 v5.82 and it dies before any test runs. `scripts/ci/check-imports.ps1` found
    it by listing every import and the DLL that lacks it. Fix: `build.rs` delay-loads `comctl32.dll`.
    (A linker-embedded manifest would clash with Tauri's own; `rustc-link-arg-tests` does not reach unit
    tests inside the library.)
17. **A cold Windows build takes ~19 minutes; with the Rust cache ~5 to 6.** The cache is saved from `main`
    even when a run fails. The release workflow deliberately uses no cache.
18. **The optimized release build did not finish in 77 minutes** (fat LTO, one codegen unit, lancedb and
    datafusion) and hit the 90-minute job limit. CI builds the validation installer in the debug profile
    (~9 minutes). `release.yml` would hit the same limit: raise its timeout or relax LTO.
19. **Do not let a new push cancel a running build on `main`,** and skip docs- and infra-only pushes.
    CI now cancels only superseded pull-request runs and ignores `docs/`, `infra/` and Markdown.
20. **Upload the GPU tests right after building them,** not at the end of the job (~8 minutes earlier).
21. **A YAML edit that matched the wrong step duplicated part of a workflow** and GitHub rejected it with
    no jobs. Check with `python3 -c "import yaml..."` and read the job list after scripted edits.

## Open items

- **Proactive check of the default model.** On a T4 the first chat with `qwen3.5-4b` fails, then the app
  falls back to Phi-4-mini (a second ~4 GB download). A one-token generation at warm-up would find this
  before the user does.
- Re-measure `qwen3.5-4b` on the A10G through the app (with its reasoning-effort setting).
- First-run provider download: the cancel command exists (`cancel_ep_registration`), the button does not.
- Undiagnosed ignored tests (A8), and porting the four `foundry`-CLI tests to the SDK.
- The agent loop (a script that dispatches a run, reads the artifacts and iterates) is not built; agents
  can follow the runbook above and `AGENTS.md`.
- Raise the release build's time limit, or relax LTO, before the first release.

## Rules that keep the cost at zero

- Every AWS resource carries the tag `Project=plugable-chat-gpu`.
- Permanent, free pieces only: the network, the budget, the scheduler role and group. No Elastic IP, NAT
  gateway, saved image, snapshot, standing volume or bucket outlives a run.
- Every box has an AWS-side hard stop (default 180 minutes) and a guest-side deadline.
- Every run ends with `teardown.sh`, which lists anything tagged for this project that still exists
  (instances, volumes, snapshots, images, addresses, NAT gateways, buckets, schedules) and fails if it
  finds any.

## Chicago crimes run and installer lifecycle (2026-09-29, build 5d3385a)

Phi-4-mini (CUDA) on the T4 answered all seven questions correctly through the installed app, each with
a real SQL tool call: 227,299 crimes; THEFT 52,813 / BATTERY 41,130 / CRIMINAL DAMAGE 25,135; 36,070
arrests; Austin 11,358; 407 homicides; 18,608 with a gun; July 22,561. About 2 to 3 minutes per
question. qwen3.5-4b (CUDA) on the A10G loaded and reasoned, but had not produced a query result
within 200 s on any question it was given: it is too slow and verbose as the default model on this hardware.

| # | Finding | Status |
|---|---|---|
| A9 | The demo database is served by the external MCP Database Toolbox (`toolbox.exe`, 216 MB on Windows, 119 MB macOS arm64, 232 MB Linux), which the installer does not bundle; on a fresh machine the demo source failed with "No command specified for stdio transport". | Fixed in the app: Settings > Databases offers a one-click, SHA-256-verified download of the pinned v0.24.0 into `<data dir>/tools/toolbox/0.24.0/` (`src-tauri/src/toolbox_install.rs`), `find_toolbox_binary()` also checks that location and next to the app's resources, and enabling the demo without a toolbox now reports a plain "toolbox not installed" message. Bundling was rejected because it would add 100+ MB per installer. `bootstrap-box.ps1` keeps its toolbox install step on purpose: unattended `--initial-prompt` runs have no one to click Download. Not yet verified on a Windows box. |
| A10 | A launch with the demo database never indexed its schema, so the model was told "no tables cached" and `sql_select` was blocked. | Fixed (index at launch; launch prompt waits for it) |
| A11 | Models call the built-in tool `sql`; an unknown tool ended the reply silently. | Fixed (alias to `sql_select`; warning for unknown tools) |
| A12 | qwen3.5-4b too slow to answer within 200 s on an A10G. | Open |

Installer lifecycle (`infra/aws-gpu/installer-lifecycle.ps1`): fresh install 22 s, launch, install over a
running copy, uninstall, reinstall all pass. Lessons: the per-user installer run as SYSTEM (as IT tools
and SSM do) installs into the SYSTEM profile unless given `/D=`; run the ask scripts against `C:\gpu\app`.
Parallel `ask.sh` runs need per-instance S3 keys (fixed).

## SDK 1.2.3 on the T4 box (2026-09-30, build b597ad3, run 1 of 3)

One g4dn.xlarge (Tesla T4, driver 596.86, Windows Server 2025), about 1 h 45 min, roughly $1. CI artifacts
`windows-unsigned-b597ad3...` and `gpu-tests-b597ad3...` from run 36650701881; torn down cleanly.

**Passed (compiled GPU tests, run as SYSTEM through SSM):**
- `sdk_backend_gpu_execution_providers`: CUDA and WebGPU both registered (60 s the first time, 10 s afterwards); 48 of 48 catalog models are GPU variants.
- `sdk_backend_download_gpu_chat_model` (`qwen3-0.6b-cuda-gpu:2` in 5 s; phi-4-mini 61 s; qwen3.5-4b 68 s) and `sdk_backend_chat_roundtrip`.
- Sweep on `Phi-4-mini-instruct-cuda-gpu:5`: OK in 7 s.
- `qwen3.5-4b-cuda-gpu:4` fails at generation exactly as before (A7: `LinearAttention ... CUDA failure` at layer 0, Turing). No change from 1.2.0.
- `sdk_backend_catalog_has_device_type` fails on this box at the Mac-only WebGPU assertion (already noted in A8); not a 1.2.3 effect.

**Installed app, Chicago questions: not completed, and this is the open work.**
- The first-run provider registration in the interactive Administrator profile took 681 s, then 199 s and 82 s on the next launches, while the same registration in the test program took 10 to 12 s (and 379 s once as Administrator, before dropping to 12 s). The log is silent during it and the box is idle (4% CPU, no disk reads), so it is waiting, not working. Lesson 12's "about 3 s afterwards" did not hold for the app. Unknown whether this is 1.2.3 or the box; a run of the previous build (1.2.0) is the missing comparison.
- The harness (`ask-app.ps1`) restarts the app for every question and waits a fixed time, so a question whose launch registers slowly is screenshotted at "Connecting to Foundry...". Of the runs that got far enough, one (`top-types`) loaded phi-4-mini and answered with a generic "I don't have access to real-time databases", with no SQL tool call and a 264-character system prompt. A separate launch with the same model did make `sqlite-sql` calls. Not diagnosed; it may predate 1.2.3 (the earlier 5d3385a run had all 7 correct), so it needs a 1.2.0 run for comparison before it is attributed to anything.
- Harness problems found, all fixed afterwards (`ask.sh`, `ask-app.ps1`, `sync-model-cache.ps1`; **not yet exercised on a box**, only parse-checked and the log-watching logic tested against a real app log):
  - The tests download models into the SYSTEM cache but the app reads the Administrator cache: `ask.sh` now runs `sync-model-cache.ps1` first (`NO_SYNC=1` skips).
  - Killing `ask.sh` left `ask-app.ps1` and the app running on the box: `ask-app.ps1` removes any other copy of itself and the app at start, and `ask.sh` sends `-CleanupOnly` on exit and on Ctrl-C.
  - A fixed sleep before the screenshot: `ask-app.ps1` now watches the app's stdout for `chat-finished` and returns then (`WAIT`, default 900 s, is only a ceiling), takes the screenshot after a 6 s render pause, and prints one summary line (`outcome=`, `ep_registration=`, `model_prewarm=`, `mcp_tool_calls=`, and whether the launch model override was applied). `ask.sh` collects these into `summary.txt` and exits non-zero if any question did not finish.
  - Slow first launches: `ask.sh` does one `-Warm` launch (waits for the model to pre-warm) before the questions (`NO_WARM=1` skips), so that cost is paid once and shown, not inside a question.
  - `ask.sh` refuses to run when HEAD is not on origin, because the box fetches the scripts by commit hash.

**Next run (2 of 3):** build the previous commit's installer (or fetch 5d3385a's artifacts if they still exist) and repeat the app launch timing and one Chicago question on the same box type, then this build again, so the registration time and the tool-less answer can be compared.

## Installer scope: perMachine (decided 2026-09-29)

`bundle.windows.nsis.installMode` is `perMachine` (Program Files, HKLM uninstall entry, all-users Start
Menu shortcut). The per-user mode installed into `C:\Windows\System32\config\systemprofile\AppData\Local\plugable-chat`
when run silently as SYSTEM, which is how SSM, Intune and SCCM run installers, so managed fleets never
saw the app.

| | currentUser | perMachine (shipped) | both |
|---|---|---|---|
| `/S` as SYSTEM | Lands in the SYSTEM profile unless `/D=` is given; no shortcut or uninstall entry for real users | Correct: Program Files, HKLM, all-users shortcut | Per-machine (correct) |
| `/S` as standard user | Works, per user | Needs elevation; fails silently without it | Works, per user |
| Upgrade | Per user; each user updates | One copy for everyone; installing needs admin | Must match the original scope; mixed fleets can end up with two copies |
| Uninstall | Only that user's copy; IT cannot remove centrally | One HKLM entry, removed for all users | Depends on the scope used |
| Model cache | Per user | Still per user (`%USERPROFILE%`), so each user downloads their own multi-GB models | Same |

Cost of the choice: a person without admin rights cannot self-install. The app only reads next to its
exe (`onnxruntime.dll`, `foundry-libs`, `test-data`), so the read-only Program Files location is fine.

`infra/aws-gpu/installer-lifecycle.ps1` now defaults to `-Mode perMachine`: run it as SYSTEM (`ssm.sh`) with
no `/D=`. It checks the install lands in Program Files by itself, the uninstall entry is in HKLM and not
HKCU, nothing lands in the SYSTEM profile, the shortcut is all-users, then launches the app as a standard
non-admin user (batch logon, session 0: proves the exe runs under a limited token, not that a window
renders), and runs upgrade over a running copy, repair, silent uninstall and reinstall. `-Mode currentUser -Dir C:\gpu\life`
runs the old per-user flow.

Results (2026-09-29, T4 box, Server 2025, installer from CI build 85a7a8f, script at d81ad28): **30 of 30 checks
pass** when run as SYSTEM with no `/D=`. Fresh install 21 s to `C:\Program Files\plugable-chat`; HKLM uninstall
entry only; all-users shortcut; nothing in the SYSTEM profile; the app stays up 25 s as Administrator (desktop)
and as a non-admin local user; upgrade over a running copy leaves one uninstall entry; repair restores the exe and
a native library; silent uninstall removes the exe, entry and shortcut; reinstall works.

The first attempt (same box) had two failures, both bugs in the test, not the app: `schtasks /tr` split the
unquoted `C:\Program Files\...` path (now launched through `C:\gpu\life-launch.cmd`), and a new local user has no
"log on as batch job" right (the script now grants it with `secedit`). One GPU run, one box, torn down
with `teardown.sh` (all checks `ok`).

Not covered: a standard user launching with a rendered window (needs an interactive session for that user),
the auto-updater under a non-admin user (Tauri's NSIS updater relaunches the installer, which will ask for
UAC), and an in-place upgrade from a previous per-user install (that copy stays in `%LOCALAPPDATA%`; the
per-machine installer does not remove it).

## qwen3.5 WebGPU failure on macOS and the Foundry SDK upgrade options (2026-09-29)

**Symptom.** On macOS arm64 (Foundry CLI 0.8.119, `foundry-local-sdk` 1.2.0) the first chat with
`qwen3.5-4b-generic-gpu:4` fails in `OnnxChatGenerator.CreateOnnxChatGenerator` with `WebGPU validation
failed ... usage (Storage(read-write)|Storage(read-only)) includes writable usage and another usage in the
same synchronization scope`. The app blocklists the model for that SDK version and falls back to phi-4-mini.
The CPU build `qwen3.5-4b-generic-cpu:3` works but decodes at about 3 tokens/s.

**It is a known bug, already fixed upstream.**
- Cause: ONNX Runtime GenAI's `RecurrentState` (qwen3.5's linear-attention layers) used one buffer for both
  past and present state. Under WebGPU that buffer is bound read-write and read-only in one compute pass.
  Some drivers tolerate it; Metal (via Dawn) and Intel Arc reject it.
- Tracked as [microsoft/foundry-local#779](https://github.com/microsoft/foundry-local/issues/779) and
  [#799](https://github.com/microsoft/foundry-local/issues/799) (same error, `qwen3.5-4b-generic-gpu:2`, Intel Arc).
  A maintainer's reply on #799 says the CPU variant is the right workaround until the CLI is rebuilt.
- Fix: [onnxruntime-genai#2191](https://github.com/microsoft/onnxruntime-genai/pull/2191) (merged 2026-06-02) uses
  separate past/present buffers on WebGPU. Per the #799 maintainer reply it shipped in Foundry Local **SDK 1.2.1**.
  SDK 1.2.0 (our pin) bundles GenAI 0.14.0, which was released 2026-05-29, before the fix.
- Follow-ups that do **not** address our error: #2244 (graph-capture aliasing, in 0.15.x) and #2564 (WebGPU
  graph-capture variants, merged 2026-09-17, in 0.16/0.17). Neither matters until graph capture is enabled.
- Brew's `microsoft/foundrylocal/foundrylocal` now offers CLI **0.10.3**, built on SDK 1.2.4, so it should
  include the fix. Untested (see "Not yet verified").

**What each SDK version bundles** (from the crates' `deps_versions*.json`):

| Crate | Foundry Core | ONNX Runtime | GenAI | WinML | Source change vs 1.2.0 |
|---|---|---|---|---|---|
| 1.2.0 (pinned now) | 1.2.0 | 1.26.0 | 0.14.0 | 2.1.1 | none |
| **1.2.3** | 1.2.3 | 1.26.0 | 0.14.1 (on nuget) | 2.1.1 | `catalog.rs` only: re-scans the cache when an alias or id is unknown (BYOM self-heal) |
| 2.0.1 | (new `foundry_local` runtime) | 1.28.0 | 0.15.2 | 2.1.70, bundled, no feature | Session API, new native library, `winml` feature removed |
| 2.1.0 | not assessed (published today) | | | | |

**Recommendation: bump to `foundry-local-sdk = "1.2.3"` first; do not jump to 2.x for this bug.**
- 1.2.1 to 1.2.3 keep ORT 1.26.0, the same native file set, the same `winml` feature and the same public API,
  so `build.rs` staging, `library_path` handling and the Windows CUDA/WinML setup stay valid.
  I diffed `src/` of 1.2.0 against 1.2.3: only `catalog.rs` changed.
- Also required: `async-openai = "=0.33.1"` still matches (`Cargo.toml` of 1.2.3 differs only in the version line).
- The unverified part is that 0.14.1 contains #2191. There is no public git tag for 0.14.1; the evidence is the
  maintainer statement plus the fact that 1.2.1+ pin it. Only a test on the failing Mac settles it.

**What SDK 2.0.1 would change** (assessed from the published crate; nothing built):
- Inference moves to `ChatSession` / `Request` / `Response` items. The old `ChatClient` still exists but is
  deprecated and scheduled for removal at the end of 2026. `ChatToolChoice` and `DeviceType` are still exported,
  so `backend/sdk.rs` compiles in principle, but streaming, tool calls and reasoning effort would be re-plumbed
  onto sessions to avoid building on a deprecated API.
- Native library: 2.x loads `foundry_local` (`libfoundry_local.dylib`, `foundry_local.dll`) through `libloading`,
  next to ORT and GenAI. Our `build.rs` (`copy foundry-local-sdk native runtime libraries`, around line 564)
  stages `Microsoft.AI.Foundry.Local.Core.*` and lists `onnxruntime_providers_shared` and the WinML DLL by name,
  so the staged list, the tauri `frameworks`/`resources` entries and the runtime `library_path` all need rework
  (see the `foundry-native-lib-bundling` memory note).
- Windows: the `winml` feature is gone and one runtime bundles WinML 2.x, so `default = ["winml"]` in
  `src-tauri/Cargo.toml` must go. The SDK now picks WinML, WebGPU, CPU or CUDA itself; our explicit
  `register_execution_providers` step and the CUDA provider workarounds (A7, `onnxruntime_providers_shared`)
  must be re-validated. ORT moves 1.26 to 1.28, which is the biggest risk for the CUDA T4/A10G results.
- Cost: a multi-day change touching the backend, the packaging and the Windows GPU results. It does buy GenAI
  0.15.2 and the maintained API, but none of that is needed for this bug.

**Test plan** (each step gates the next; GPU runs are capped at three per task):
1. Free, on the Mac: install CLI 0.10.3 (needs `brew trust` of the tap) or build a scratch crate against
   `foundry-local-sdk =1.2.3` and load `qwen3.5-4b-generic-gpu:4`. Pass: a 50-token reply with no WebGPU error.
   Compare tokens/s with the CPU build (about 3/s).
2. Free, CI: after the user approves the upgrade, change only the pin to 1.2.3 on `main` (no release tag, no
   signing-workflow changes), build all three platforms, run the unit tests. The gpu-tests artifact shows whether the native staging still works.
3. Box run 1, A10G: `PLUGABLE_MODEL=phi-4-mini-instruct`, then `qwen3.5-4b`, ask the Chicago questions with
   `ask.sh`, compare answers with `chicago-questions.json` and the earlier results in this file. Pass: no
   regression versus build 5d3385a, and read the screenshots.
4. Box run 2, T4: the same two models. A7 (`qwen3.5-4b` fails on Turing) is expected to persist; confirm it does not get worse.
5. Only after 1.2.3 is proven: decide on 2.x as its own task, with the same plan plus a `build.rs`/packaging rework and an ORT 1.28 regression pass.

**Verified on the failing Mac (2026-09-29, M5 Max, macOS arm64).** After `brew trust microsoft/foundrylocal` and
`brew upgrade` (CLI 0.8.119 to **0.10.3**, SDK 1.2.4), `foundry chat qwen3.5-4b-generic-gpu:4` loads and answers.
The daemon log shows `Using WebGPU EP for model: qwen3.5-4b-generic-gpu:4` then `Model loaded successfully`, with no
validation error. A 200-token completion through the local service took 2.1 s (about **95 tokens/s**, against about 3
tokens/s for the CPU build). So the fix is in the 1.2.x line, and the CLI test used SDK 1.2.4, not 1.2.3.

**SDK 1.2.3 adopted (2026-09-29).** `src-tauri/Cargo.toml` now pins `foundry-local-sdk` 1.2.3. On the Mac,
`cargo test --lib backend::sdk::tests::sdk_backend_prompts_every_cached_model -- --ignored` (10 cached models)
gave 8 responding, 0 unexpected. `qwen3.5-4b-generic-gpu:4`, the variant that failed on 1.2.0, generated
48,318 characters on WebGPU with no validation error. The other qwen3.5 rows (`gpu:2`, `cpu:3`) were cancelled
by the test's time limit while still reasoning (the test sets no token cap), which is not a runtime error.
One change was needed: **1.2.3's `Microsoft.AI.Foundry.Local.Core.WinML` package has no `osx-arm64` binary**, so with
the old default `winml` feature a macOS build ended without `Microsoft.AI.Foundry.Local.Core.dylib` ("Could not
locate native library"). The `winml` feature is now enabled only for Windows through a
`[target.'cfg(windows)'.dependencies]` entry, and the app's own `winml` feature is removed (nothing referenced it).
Windows builds are unchanged: same crate, same `winml` variant.

**Still not verified.** Windows: CI must build the pinned crate and the A10G/T4 boxes still need the plan above
(steps 2 to 4). Nothing on Windows has run 1.2.3 yet. Also not exercised: the packaged macOS app (the tauri
bundle takes the dylibs from `foundry-libs/`, which the build now refreshes from 1.2.3).

## Release v0.1.0-rc7 on a fresh A10G (2026-10-01)

**Signed release build.** The tag run (36870139972) passed verify-tag, Windows build and signing, Linux build and
signing, and publish. The Windows exe, setup.exe and MSI carry the LEANCODE, INC. EV signature with a DigiCert timestamp.

**Signed MSI lifecycle: 21 of 21 checks passed** (`msi-lifecycle.ps1 -ExpectSigned`, run as SYSTEM): MSI and installed exe
signed and timestamped; install under Program Files; HKLM registration; all-users shortcut; nothing in the SYSTEM profile;
reinstall over a running copy; repair restores the exe and a native library; uninstall removes everything; reinstall.

**First-launch timing of the installed, signed app on a fresh box:** warm-up ready in 147 s, GPU provider registration
65 s, model pre-warm 7.8 s. Earlier debug-build installs took 153 to 383 s for registration, so that delay varies by box
and is not fixed; it is still unexplained.

**Chicago questions.** On the CI debug MSI (commit 13a0fd1) Phi-4-mini answered six of seven correctly with one tool call each
(227,299; THEFT 52,813 / BATTERY 41,130 / CRIMINAL DAMAGE 25,135; 36,070; Austin 11,358; 407; 18,608); the month question
was not captured. On the signed rc7 install the questions did **not** complete: the box had no Phi model cached (only qwen),
and the box stopped answering SSM after the first question, so only a "model not downloaded, using startup model" warning
was captured. qwen3.5-4b through the installed app is still unmeasured. The harness should download the requested model
before the warm-up, and `ask.sh` should stop on the first SSM failure instead of looping through every question.

**Open:** SHA256SUMS lacks the Windows files; the publish job uploads cargo-timing HTML files; the default-model decision
(qwen needs six of seven within 3 minutes, else Phi-4-mini) waits for a qwen measurement.

### qwen3.5-4b through the signed rc7 install (2026-10-03, fresh A10G)

Signed MSI lifecycle again 0 failed. First launch: warm-up ready in 144 s (GPU provider registration 61 s, pre-warm 7.3 s).
**All 7 Chicago questions answered correctly** (227,299; THEFT 52,813 / BATTERY 41,130 / CRIMINAL DAMAGE 25,135; 36,070;
Austin 11,358; 407; 18,608; July 22,561), each from a database query. Screenshot timestamps put the questions 1 to 4
minutes apart (8:21 to 8:35 box time) including app launch, so the "within 3 minutes" criterion is probably met but was not
timed per question; add a per-question duration to `ask-app.ps1` output. Quality issue seen: qwen's visible answer often
begins with its own reasoning ("The user asked ... I should provide ...") and ends with long follow-up menus; Phi-4-mini
answers were plainer. The earlier failure of the 2026-10-01 signed run was the guest failsafe (default deadline) terminating
the box; run `extend.sh <id> 150` right after bootstrap.
