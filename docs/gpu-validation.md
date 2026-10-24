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

## rc10 on an A10G (2026-10-10)

Signed `v0.1.0-rc10` installer (Authenticode Valid, LEANCODE, INC.) on a g5.xlarge (NVIDIA A10G, driver 596.86,
Windows Server 2025), fetched from the public release. **Run after the release was tagged and published, which is the wrong
order**: the rule is now that a GPU run comes first (see the `release-signing` skill).

| Check | Result |
|---|---|
| `installer-lifecycle.ps1` (as SYSTEM, perMachine) | 0 failed: fresh install, HKLM entry, no SYSTEM-profile files, all-users shortcut, launch as admin and as a standard user, upgrade over a running copy, repair, uninstall, reinstall |
| clean-host `gpu-baseline`, `baseline`, `quarantined-dll`, `upgrade-over-old` | all pass (`upgrade-over-old` first failed on Windows PowerShell 5.1 because a single object has no `.Count`; fixed in `faults.ps1` and re-run green) |
| Chicago questions through the installed app, `qwen3.5-4b-cuda-gpu:4`, no pre-synced model cache | 7 of 7 correct: 227,299; THEFT 52,813 / BATTERY 41,130 / CRIMINAL DAMAGE 25,135; 36,070 arrests; Austin 11,358; 407 homicides; 18,608 gun; July 22,561. GPU peak 56 to 78% utilization, 7.6 to 8.6 GB VRAM |

This also closes the open item "re-measure `qwen3.5-4b` on the A10G through the app". Not covered: the T4 and other pre-Ampere
GPUs (no longer test targets), the first-run experience on a machine without the VC++ runtime (the box already had it), and a
second Windows user with a non-ASCII name (needs an interactive desktop).


## rc11 on an A10G (2026-10-11)

Commit `0a9973a` (the commit tagged `v0.1.0-rc11`), the CI-built unsigned installer (`windows-unsigned-0a9973a`, perMachine NSIS), g5.xlarge
(NVIDIA A10G, driver 596.86, Windows Server 2025). **Run before the tag**, as the rule requires. Box and per-run bucket torn down
afterwards (`teardown.sh`: nothing left).

| Check | Result |
|---|---|
| `installer-lifecycle.ps1` (as SYSTEM, perMachine) | 30 of 30 PASS |
| clean-host `gpu-baseline`, `baseline`, `quarantined-dll`, `upgrade-over-old` (against the rc10 setup.exe) | 4 of 4 as expected |
| Chicago questions through the installed app, `qwen3.5-4b-cuda-gpu:4` | 7 of 7 correct, screenshots read: 227,299; THEFT 52,813 / BATTERY 41,130 / CRIMINAL DAMAGE 25,135; 36,070 arrests; Austin 11,358; 407 homicides; 18,608 gun; July 22,561 |
| clean-host CPU scenarios on `windows-latest` (workflow dispatch) | 10 of 10 pass |

Notes for the next run:
- `gpu-baseline` failed the first time (`phase 'model-download' never reported`) because the Chicago run had already cached the model on that box. The app was
  healthy (CUDA, WebGPU and TensorRT-RTX registered, model ready, smoke passed). Clear `C:\Users\Administrator\.foundry` and the app's profile folders
  before `gpu-baseline`, or run it before the Chicago questions; it then passed in 103 s.
- `ask.sh` printed exit 1 on a correct run: two new `schtasks` "/ST is earlier than current time" warning lines push the `outcome=` line out of its `tail -4`.
  The screenshots are the evidence; fix the script to search the whole output.
- GPU provider registration took about 6 minutes in the interactive profile on the first launch (range seen so far: 10 s to 681 s).
- This release adds unattended-run flags (`--reply-file`, `--prompts-file`, `--chat-home`, `--ephemeral`, `--hidden`) used by test harnesses; none changes the
  installed app's normal behaviour, and the Chicago results above are from the same build.

## rc13 on an A10G (2026-10-17)

Commit `496f828` (the commit tagged `v0.1.0-rc13`), the CI-built unsigned installer (`windows-unsigned-496f828`, perMachine NSIS),
g5.xlarge (NVIDIA A10G, driver 596.86, Windows Server 2025). **Run before the tag.** Box and per-run bucket torn down afterwards.

| Check | Result |
|---|---|
| CI (Windows and Linux) on `496f828` | green |
| clean-host default list on `windows-latest` (15 scenarios, including the new `engine-declined`) | 15 of 15 pass |
| `installer-lifecycle.ps1` (as SYSTEM, perMachine) | 30 of 30 PASS |
| clean-host `gpu-baseline`, `baseline`, `quarantined-dll`, `upgrade-over-old` (against the rc12 setup.exe) | 4 of 4 as expected |
| Chicago questions through the installed app, `qwen3.5-4b-cuda-gpu:4` | 7 of 7 correct, screenshots read: 227,299; THEFT 52,813 / BATTERY 41,130 / CRIMINAL DAMAGE 25,135; 36,070 arrests; Austin 11,358; 407 homicides; 18,608 gun; July 22,561. GPU peak 59 to 87%, 7.6 to 8.6 GB |
| Signed release ([run 38999549730](https://github.com/PlugableTechnologies/plugable-chat/actions/runs/38999549730)), tagged after the checks above | all four jobs green; the downloaded `plugable-chat_0.1.0-rc13_x64-setup.exe` and `.msi` match `SHA256SUMS-windows.txt`; the setup file's Authenticode signer is `LEANCODE, INC.` (EV, DigiCert Trusted G4 chain) with a timestamp. The signing environment's Required reviewers was still off for this release |

What a working registration looks like, now that it is logged (`gpu-baseline`): the engine answered after 61.3 s with
`success=true`, status "EP registration complete: 3 bootstrapper(s) succeeded in 61248ms", registered CUDA, WebGPU and
NvTensorRTRTX, and the model list went to 49 GPU models. Compare Pat's rc12 machine, where the same call returned within 9 ms
with nothing registered.

Found on the way to green, all fixed before the tag (each was a timing race in the new diagnostics, seen only on Windows):
- Registration finishing before the adapter scan was stored made the end-of-registration assessment say NoHardware on an
  NVIDIA machine. An unfinished scan is now Checking.
- A start-up report never appeared: the report waited on PowerShell CIM probes with no limit, and the free-disk probes ran
  inside the shared observation lock. Host probes now give up after 45 s and run outside the lock.
- The report started during GPU setup read its assessment first and finished last, so the newest report said "Setting up".
  The assessment is read after the probes, and report files are named by millisecond.

Notes for the next run:
- `bootstrap-box.ps1` arms a 90-minute guest deadline; lifecycle + matrix + Chicago need more. Run `extend.sh <id> 150` right after
  provisioning.
- `teardown.sh` reports leftover `plugable-chat-gpu-*` buckets but does not delete them; remove the run's bucket with
  `aws s3 rb --force` once the results are copied.
- `run-matrix.sh` exits 1 when `unzip` warns that the results zip uses backslashes; the box's own `matrix-exit=0` and the JUnit
  counts are the result.

## rc15 on an A10G (2026-10-22)

Commit `00011bc` (the commit tagged `v0.1.0-rc15`), the CI-built unsigned installer (`windows-unsigned-00011bc`, perMachine NSIS),
g5.xlarge (NVIDIA A10G, driver 596.86, Windows Server 2025). **Run before the signed release was published** (the tag was pushed while
the last check ran, with the release's publish step as the gate). Box and per-run bucket torn down afterwards.

| Check | Result |
|---|---|
| CI (Linux, macOS, Windows) on `00011bc` | green (22 min; macOS had failed on the previous commit's timing test, widened in this one) |
| clean-host default list on `windows-latest` (16 scenarios) | 16 of 16 pass (`nvidia-present-no-providers` had failed on the previous main build) |
| `installer-lifecycle.ps1` (as SYSTEM, perMachine) | 30 of 30 PASS |
| clean-host `gpu-baseline`, `baseline`, `quarantined-dll`, `upgrade-over-old` (against the rc14 setup.exe) | 4 of 4 as expected |
| Chicago questions through the installed app, `qwen3.5-4b-cuda-gpu:4` | **3 of 7 asked** (stopped there on purpose, for time), all correct, screenshots read: 227,299; THEFT 52,813 / BATTERY 41,130 / CRIMINAL DAMAGE 25,135; 36,070 arrests |
| Long-conversation check (`followups-run.ps1`, first **4 of 10** prompts in one chat) | 4 of 4 `ok`, one tool call each, 32 to 41 s per reply, app log "exiting with code 0" |

Found on the way (none blocks the release):
- `exit_app` (the `--exit-after-reply` path) skips `run_state::clear()`, so a harness run leaves `run-state.json` behind (phase `idle`, which
  gives no notice at the next start). A normal window close clears it: checked on the box, present while the app ran, gone after
  `CloseMainWindow`. Until `exit_app` clears it too, the "run-state.json must be gone" check above can only be made by closing the window.
- Per-question time through `ask.sh` was 3, 6.6 and 9.7 minutes, but the same model answered the follow-ups in 32 to 41 s each in one chat. The cost is the fresh
  launch, provider setup and screenshot flow per question, not generation. Timings and options are in issue 36.
- `followups-run.ps1` could not read the app's exit code (a PowerShell quirk with redirected output); the log line is the evidence.

Signed release ([run 39738797697](https://github.com/PlugableTechnologies/plugable-chat/actions/runs/39738797697), tagged 07:11 UTC, 1 h 48 min to publish: Windows 80 min,
Linux 62 min, macOS 107 min, publish under a minute; no approval step, the environment still has no required reviewers): all jobs green; 15 assets as a pre-release.
The downloaded `plugable-chat_0.1.0-rc15_x64-setup.exe` matches `SHA256SUMS-windows.txt`; its Authenticode signer is `LEANCODE, INC.` (EV, serial 7376809) under the
DigiCert Trusted G4 chain (checked from a Mac with the certificate-table method in the `release-signing` skill). The macOS and Linux files were not run.

Step timings (UTC): CI 06:03 to 06:25; box launched 06:24:49, bootstrap 2 min 37 s; lifecycle 2 min 43 s; `gpu-baseline` 3 min 04 s; other three scenarios
2 min 05 s; Chicago questions 06:40 to 07:02; follow-ups 07:03 to 07:12.

## rc15 second pass: the published, signed files (2026-10-22)

The first pass above was cut short and ran the CI (debug, unsigned) build. This pass ran the **published signed `v0.1.0-rc15` files** (nothing rebuilt) on a fresh g5.xlarge (A10G, driver 596.86,
Windows Server 2025), and on an Apple-silicon Mac. Box and per-run bucket torn down afterwards.

**Windows, signed files**

| Check | Result |
|---|---|
| Downloaded `setup.exe` and `.msi` against `SHA256SUMS-windows.txt` | both match |
| `scripts/ci/verify-installers.ps1` (PowerShell 7) on the setup and the MSI | exit 0: both installers and both installed `plugable-chat.exe` signed by `LEANCODE, INC.` (EV 7376809) with a timestamp; the VC++ payload matches its pin and is signed by Microsoft; the installer hook ran; a first run left the install folder untouched (17 files) |
| `installer-lifecycle.ps1` (signed setup, as SYSTEM) | 0 failed |
| `msi-lifecycle.ps1 -ExpectSigned` (signed MSI), on a clean machine | 0 failed (22 checks) |
| clean-host `gpu-baseline`, `baseline`, `quarantined-dll`, `upgrade-over-old` | 4 of 4 as expected |
| Chicago questions, all 7, reply-file harness (`followups-run.ps1 -FreshChats`) | 7 of 7 `ok`, every expected answer found, 21 to 32 s each, 215 s in total |
| Chicago questions, all 7, screenshots through the installed app (`ask.sh`) | all 7 answered; screenshots read for total, top-types, area, homicides, guns, month (227,299; Austin 11,358; 407; 18,608; July 22,561); the window title shows `v0.1.0-rc15 (00011bc)`. About 70 s per question |
| Same-chat conversation, all 10 prompts | **9 of 10 ok; prompt 10 failed**: see issue 38 (about 18,200 input tokens, `bad allocation` in the engine; replies 8 and 9 took about 5 minutes each). No crash and no native fault |
| Crash notices, rows 8 and 9 (`crash-test.ps1`) | pass: a kill during start-up is reported once at the next start and not after a normal close; a kill mid-answer is reported with the model and request size (`qwen3.5-4b-cuda-gpu:4`, 72 input tokens, 16384 max); the second kill switches the model off |
| Row 3 (`PLUGABLE_FOUNDRY_BACKEND=cli_http`, no `foundry` on the machine) | pass: starts on the built-in engine, logs "The Foundry command-line tool is not installed, so Plugable Chat is using its built-in engine instead", answers |

**macOS, signed `.dmg`** (checksum and GitHub attestation verified; ad hoc signed, hardened runtime, three engine libraries inside)

| Check | Result |
|---|---|
| Launch the shipped app | **fails**: the engine library is rejected ("different Team IDs", library validation). Issue 37 |
| Same app re-signed locally with `disable-library-validation` (a diagnosis, not a shipped file) | starts; chat answers; model switch across restarts works; the HTTP (Streamable) MCP transport works against the local stub and the live Brix box (`status` 0.4 s, `ask` 12.9 s); crash rows 8 and 9 behave as on Windows |

**Run-order mistakes of mine, so nobody repeats them:** the MSI lifecycle and `upgrade-over-old` fail with "found 2 registrations" if an NSIS install is still on the machine (uninstall everything first); `verify-installers.ps1` needs PowerShell 7 (it fails on Windows PowerShell 5.1); it installs the setup with `/D=` into its own folder, and NSIS then reuses that folder for the next silent install, so reinstall with `/D=C:\Program Files\plugable-chat` after it; a JSON prompt file written by Windows PowerShell 5.1 with `-Encoding UTF8` has a byte-order mark that the app's `--prompts-file` reader rejects, so the run silently waits for its timer.

**Not run:** clean-host row 10 (`gpt-oss-20b`, 8192-token context; an 11.8 GB model), rows 1, 2, 5 and 7 as manual cases (the CI scenarios `driver-absent`, `quarantined-dll` and the matrix cover their automated forms), row 6 on a real offline machine (`offline-hosts-block` passes in CI), the new Settings screens by a person, the Gatekeeper first-open flow, and the Linux files.

**Timing note:** the release (optimized) build answered the screenshot run in about 70 s per question, against 3 to 10 minutes per question on the CI debug build, and the reply-file harness took 215 s for all seven. See issue 36.

## rc12 on Pat's TBT5-AI eGPU (2026-10-16)

Framework 13 (Intel Iris Xe) plus an RTX 5080 in a TBT5-AI over Thunderbolt, Windows 11 Home build 26300
(Insider), driver 617.42. The `foundry` CLI 0.11.0 on the same machine registers CUDA and WebGPU in 30-45 s
and loads models into VRAM. The app stays on the CPU.

| # | Finding | Status |
|---|---|---|
| E1 | `download_and_register_eps` returned within 9 ms (two "engine providers" lines 9 ms apart in the log), registered nothing and named no failure. Not a timeout: the engine never tried. | Open: cause unknown. The next build's support details carry the engine's answer and its own log |
| E2 | `success:false` with empty lists counted as "nothing to report": no failure, no retry, no log line. | Fixed: every requested provider is accounted for; the rest are `EngineDeclined` |
| E3 | Registration logged only with `println!`, which a release build discards; Foundry Core's own log went to the SDK default folder. | Fixed: `[gpu]` lines in the app log; Core logs to `logs/foundry/` at Information, and its provider lines are in "Copy details for support" |
| E4 | `discover_eps` lists a provider twice (`WebGpuExecutionProvider`) and the duplicates were passed back. The single-GPU A10G never did this. | **Not fixed by deduping** (rc13, four logs on two hosts, see "rc13 on David's Arc B60 and Bob's RTX 4080" below): the engine declines even when each name is sent once. Core 1.2.4 is the expected fix, not yet verified on a failing machine |
| E5 | The report beside start-up said "not using it" while registration could still be running. | Fixed: the card says "Setting up GPU acceleration..." until registration ends |
| E6 | "Provider libraries next to the app" checked the exe folder, which never holds them. | Fixed: checks the folder the engine was loaded from |

Causes still to separate, in order: the duplicate names (E4); Windows ML 2.1.1 in Core 1.2.3 on build 26300 or
with a second adapter, while the CLI uses a newer core; Foundry's CUDA bootstrapper gate on compute capability
12.0; the Windows App SDK bootstrap (SDK v2 sets `Bootstrap=true` for unpackaged apps; try
`PLUGABLE_FOUNDRY_WINML_BOOTSTRAP=1`). `PLUGABLE_FOUNDRY_BACKEND=cli_http` (rc15 onward: ignored, with a log line, when the `foundry` CLI is not installed) uses the system `foundry` service
instead and is the quickest test of whether the in-process engine is at fault.

## rc13 on David's Arc B60 and Bob's RTX 4080 (2026-10-19)

David (Intel Arc B60 in a TBT5-AI plus the laptop's Arc 140V) and Bob (RTX 4080 plus Iris Xe), both Windows
11 build 26300, rc13 installed fresh or over rc11. Both stay on the CPU. Four support files, all the same:
the engine answers `download_and_register_eps` in 11-17 ms with

    success=false, "Unknown EP bootstrapper name(s): . Call DiscoverEps() and retry with supported names only.
    Available EPs: WebGpuExecutionProvider, OpenVINOExecutionProvider, WebGpuExecutionProvider."

The unknown-name list is empty, `WebGpuExecutionProvider` is listed twice by the engine itself, and nothing is
registered. Our request already sent each name once (rc13's dedupe), so the request is not the cause.

**Upstream has the same bug.** `microsoft/Foundry-Local` issue 792 shows the same text from Snapdragon NPU
machines (`Available EPs: WebGpuExecutionProvider, WebGpuExecutionProvider, QNNExecutionProvider`), so it is not
specific to two adapters or to our code. A maintainer wrote on 2026-08-07 that it "should be fixed in Foundry
Local Core 1.2.4 and CLI 0.10.3", and two reporters confirmed. We shipped Core 1.2.3 (`foundry-local-sdk`
1.2.3). The Rust crate has no 1.2.4, but Core 1.2.4 is on NuGet with the same ONNX Runtime, GenAI and Windows ML
dependencies, so rc14 stages it with `FOUNDRY_NATIVE_OVERRIDE_DIR` (`scripts/ci/fetch-foundry-core.ps1`, version in
`src-tauri/foundry-core-version.txt`). **Not yet verified on a failing machine.**

### Hypotheses and the test for each

Each has a Rust test with a fake engine that models it (`ep_ladder.rs`, `ep_probe.rs`), and a real-engine
experiment in `plugable-chat --ep-probe` (run on a machine with `scripts/support/ep-hypotheses.ps1`, in CI by
`scripts/ci/run-ep-probe.ps1`). Verdicts are `confirmed`, `refuted` or `inconclusive`.

| ID | Hypothesis | Real-engine test | Result so far |
|---|---|---|---|
| H1 | Core 1.2.3 rejects a request when its own list repeats a name | each name once / raw list / no names / one name at a time / bogus-name control | Matches all four logs and upstream 792; the ladder step that works is not yet known (needs a failing machine) |
| H2 | Windows App SDK not bootstrapped | same matrix with `PLUGABLE_FOUNDRY_WINML_BOOTSTRAP=1` in another process | open |
| H3 | Core version | same matrix against another Core build (`--ep-probe-other-core`); CI compares 1.2.3 with the bundled Core | upstream says 1.2.4 fixes it; open |
| H4 | Stale or missing Windows execution-provider packages | `Get-AppxPackage *.EP.*`, Windows Update paused, restart pending | open (upstream: one OpenVINO case needed the packages removed) |
| H5 | Windows Insider build 26300 | OS build in the facts; needs a comparison across hosts | open: Pat, David and Bob are all on 26300 |
| H6 | Our manager configuration | stock SDK configuration in its own process | open |
| H7 | A second onnxruntime in the process | every onnxruntime.dll the app could load, with versions | Refuted on the A10G box (2026-10-20): Windows' System32 copy (1.17) sits beside ours (1.26) and registration works. rc14's probe wrongly said "confirmed" there; fixed afterwards so it only confirms when registration failed |
| H8 | Bundle incomplete | `foundry-libs` against the expected five files | open: the rc13 support details listed only three of them, which may only be what that line prints |
| H9 | Called before the engine is ready | retries after 0, 5 and 30 s | open (Bob's Retry button also failed, which argues against it) |

On macOS the probe ran end to end against the real engine (2026-10-19): all three configurations produced a
result, the control answered `Unknown EP bootstrapper name(s): NoSuchExecutionProvider`, which names the unknown
provider (so the empty list on Windows is abnormal), and WebGPU registered. No Windows result yet.

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
- ~~Re-measure `qwen3.5-4b` on the A10G through the app~~ done for rc10 (7 of 7 Chicago questions, see above).
- First-run provider download: the cancel command exists (`cancel_ep_registration`) and the status bar has a Cancel button for it (`src/components/StatusBar.tsx`); the button has not been exercised on a real machine.
- Undiagnosed ignored tests (A8), and porting the four `foundry`-CLI tests to the SDK.
- The agent loop (a script that dispatches a run, reads the artifacts and iterates) is not built; agents
  can follow the runbook above and `AGENTS.md`.
- Raise the release build's time limit, or relax LTO, before the first release.
- **Consumer cards and external GPUs are not covered by any runner.** The A10G is one data-center card on a
  single-adapter Server build. Pat's RTX 5080 in a TBT5-AI (Blackwell, a second adapter, Windows 11 Insider) is
  the only check, so his machine stays a release gate until a consumer-card runner exists. For a report from
  him, `scripts/support/gpu-ground-truth.ps1` collects every layer's view in one read-only text file.

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

## Long-conversation check (rc14 crash)

The Chicago questions in `ask.sh` each start a fresh app launch and chat, so they never build up history. The rc14
crash (`onnxruntime_providers_webgpu.dll`, 0xc0000409) happened on follow-up questions in one chat. Before tagging, on
the GPU box run the installed app once with the follow-ups in a single conversation and confirm it ends with exit code 0:

```
plugable-chat.exe --hidden --ephemeral --same-chat --prompts-file chicago-followups.json ^
  --reply-file replies.jsonl --exit-after-reply --reply-timeout-secs 3600
```

Every line of `replies.jsonl` must have `"outcome":"ok"`, and `run-state.json` must be gone after the exit (it is removed on a
normal exit; if it is left behind, the next start reports a crash).

## rc16 (2026-10-23, tag on 6c321f3)

Validated on an A10G on the same commit's CI build: installer lifecycle 0 failed, 10-prompt single chat 10 of 10 (the rc15 failure at prompt 10 is fixed), Chicago 7 of 7, crash rows 8 and 9, and `infra/aws-gpu/engine-recovery-test.ps1` (a half-size `onnxruntime_providers_cuda.dll` is set aside in `ep.quarantine-*`, re-downloaded, the app answers; two kills switch the GPU off; `--repair-engine` clears it).
On the signed files: checksums, attestation, Authenticode (LEANCODE, INC.), lifecycle 0 failed, Chicago 7 of 7. macOS signed app loads its engine.
`infra/aws-gpu/netloss-repro.ps1` (firewall block on the app, 90 s, while generating and while idle) did NOT crash the app on the A10G, so the Intel Arc / WebGPU network-loss crash is unreproduced. Not run on the signed files: 10-prompt, recovery test, MSI on a clean machine. One earlier Chicago 7 run on the CI build hung for 55 minutes with no replies and did not recur on rerun; cause unknown.

## 1dc9b88 (resilience commit) on an A10G (2026-10-24)

Commit `1dc9b88` (CI run 40042160938, green), the CI-built unsigned installer (`windows-unsigned-1dc9b88…`, 0.0.0 debug build, perMachine NSIS) and `gpu-tests` artifact, one g5.xlarge (A10G, driver 596.86, Windows Server 2025). One box run. Launched 00:33 UTC, torn down about 02:43 UTC (about 2 h 10 min, roughly $2.2 to 3.0 at $1.0 to 1.4/h). `teardown.sh`: terminated the instance, deleted the schedule and the temporary role; it reported the run's S3 bucket as LEFTOVER (as documented), the bucket was then removed with `aws s3 rb --force`, and a second `teardown.sh` listed nothing left.

| Check | Result |
|---|---|
| `installer-lifecycle.ps1` (as SYSTEM, perMachine) | 0 failed (all checks PASS; 2 min 54 s) |
| clean-host `gpu-baseline`, `baseline`, `quarantined-dll`, `upgrade-over-old` (against the rc16 setup.exe) | 4 of 4 as expected |
| new scenarios `offline-hosts-block` and `embedding-tls-reset-recovers` (the latter never run before) | both pass on the box (6 of 6 in 9 min 20 s for the whole matrix) |
| Chicago questions, `ask.sh`, `qwen3.5-4b-cuda-gpu:4`, all 7, screenshots read | 7 of 7 correct: 227,299; THEFT 52,813 / BATTERY 41,130 / CRIMINAL DAMAGE 25,135; 36,070 arrests; Austin 11,358; 407 homicides; 18,608 gun; July 22,561. 463 to 566 s per question on this debug build; GPU peak 44 to 100%, 7.6 to 7.9 GB |
| `phi-4-mini-instruct` fallback | not run (this commit does not touch the model chain; AGENTS.md asks for both models only when the default changes) |
| (a) huggingface.co blocked by hosts file: chat usable, amber note, not red | **PASS.** Fresh profile, hosts block from before launch. Input enabled 54 to 57 s after launch, the amber "Search features unavailable - retrying" row appeared 1 to 2 s later, the red "Startup failed" bar never appeared in 160 s of observation. A prompt typed into the UI while the search model was unavailable was answered ("OK", chat-finished at about 100 s). Screenshots `a-chip-seen`, `a-final-blocked` read (amber row with Retry, Copy details, Details) |
| (b) unblock: background retry loads the embedding with no user action | **PASS.** App log: five quick attempts failed, `search model unavailable; background retry 1 in 30 s`, retry 2 in 60 s, then `CPU embedding model loaded and verified` 29 to 31 s after the block was removed; the amber row went away 2 s later. `b-after-recovery` read: no banner, answer still on screen |
| (c) placeholder during slow GPU provider registration | **PASS.** The maximized-window screenshot at 35 s into a fresh start (provider cache removed, so registration re-downloaded) shows the disabled input reading "Setting up GPU acceleration..." beside the "Preparing GPU acceleration" row. "Streaming in another chat" was never shown. Note: Windows UI Automation kept reporting the stale text "Model loading..." for the disabled textarea's name during this window; the pixels were right, so this was the test reader, not the app |
| `embedding-tls-reset-recovers` | PASS on the box (see above) |

Timings (UTC): lifecycle 00:40 to 00:43; matrix 00:43 to 00:53; first-launch GPU provider registration with the cache cleared 55 to 56 s (twice), but 419.9 s once, in the warm-up launch of the first Chicago attempt (same build and box; log showed `CUDAExecutionProvider 0%` for 7 minutes, then success); Chicago 7 questions 01:36 to 02:37.

Bugs and observations (none changes a verdict above):
- **Wrong cause text for a refused connection (app bug, minor).** With huggingface.co mapped to 127.0.0.1 the OS error is "actively refused (os error 10061)", yet the row says "Could not reach huggingface.co: the name did not resolve." Root cause: `describe_embedding_error` (`src-tauri/src/actors/foundry/embedding_init.rs`) tests `l.contains("resolve")` in the DNS branch, and every raw error carries the download URL `https://huggingface.co/.../resolve/main/onnx/model.onnx`, so the word "resolve" is always present and the DNS message wins before the "connection refused / 10061" branch is reached. Any error that does not match an earlier branch (disk, reset, certificate, proxy) is reported as a DNS failure. Fix: match `dns error`, `failed to lookup`, `os error 11001`, `no such host` only. The unit test uses `dns error: failed to lookup address`, so it does not catch this.
- The GPU switch-off safety counted my harness's `Stop-Process` kills (two aborted launches) as crashes: the next launch logged `previous run did not exit cleanly: DiedIdle` and `GPU provider registration skipped: GPU acceleration is switched off`, and the app began a CPU model download. Clearing `%APPDATA%\plugable-chat` fixed it. Harness lesson: close the app with `CloseMainWindow` in new scripts, or reset the profile after any forced kill.
- The input textarea's placeholder cannot be read through UI Automation while it is disabled (stale); use a screenshot of a maximized window. WebView2 remote debugging through `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` does not open a port in this app (port 9222 never listened).
- `run-in-session.ps1 -Args "-PutUrl '<presigned>'"` loses the URL (curl exit 3); upload the results zip from an SSM command instead.
- Chicago on the CI debug build took 8 to 9 minutes per question (the release build took about 70 s in rc15), so seven questions need about an hour; set the box deadline with `extend.sh` before starting.

Not run: phi-4-mini fallback, 10-prompt same-chat check, crash rows, MSI lifecycle, macOS, signed files (this was the CI build, before any tag). The new-behaviour driver script was a throwaway (`nb.ps1`, UI Automation plus screenshots) and is not committed.
