---
name: gpu-validation
description: Run, drive and read the on-demand AWS Windows GPU test box for plugable-chat, including costs, hard stops, harness scripts, and lessons. Use when validating the installed app, GPU providers, models, or the installer on real Windows.
---

# GPU validation on a fresh AWS box

Full runbook and lesson list: `docs/gpu-validation.md`; timings and cost: `docs/gpu-validation-timings.md`. Ladder and
when to use this rung: [test-ladder](../test-ladder/SKILL.md).

## Rules
- **Always test Plugable Chat on a GPU box before a release is signed or tagged.** Plugable Chat is a GPU app: GPU
  provider registration, CUDA/WebGPU model loading, model answers and the installed app on NVIDIA hardware are what users
  hit first, and no free runner can show them. The CPU-only checks (build, installer, `faults.ps1` clean-host matrix on
  `windows-latest`) are the cheap pre-filter, and install and build testing should run without a GPU wherever it can;
  they never replace the GPU run. Minimum before any `v*` tag: `installer-lifecycle.ps1`, the `gpu-baseline` scenario
  (`infra/aws-gpu/run-matrix.sh`), and the Chicago questions via `ask.sh`, with the screenshots read. If the GPU run has not
  been done for the commit being tagged, do not tag. (rc10 was tagged, signed and published on CPU-only results; the GPU run
  was done afterwards. Do not repeat that order.)
- Fresh Windows Server 2025 box per run from a stock AMI; **nothing kept or billed between runs**; no nightly tests.
- **GPU generation: Ampere or newer only** (decision 2026-09-30). Default `INSTANCE_TYPE` is `g5.xlarge` (A10G, 24 GB,
  about $1.0 to 1.4/h). Turing (T4) and older are not test targets any more.
- Three hard stops: AWS EventBridge one-time terminate schedule (`MAX_RUN_MINUTES`, default 180, proven to fire), an
  on-box deadline task, and `teardown.sh`, which fails if anything billable remains. **Max 3 GPU runs per task;
  always finish with `infra/aws-gpu/teardown.sh`; never keep a box up waiting for CI.** Extend with `extend.sh <id> <min>`.
- Never send signing credentials to the box. Approvals, AWS resources and money are the user's call.

## Flow (`infra/aws-gpu/`)
1. `10_network.sh`, `00_budget.sh`, `20_scheduler_role.sh` once per account.
2. `SPIKE_SSM=1 ./launch.sh` (about 100 s to reachable), provision from the repo **by commit hash**
   (`bootstrap-box.ps1`: driver 596.86 from the public AWS bucket, WebView2/Node/ffmpeg/toolbox, auto-logon), reboot for the
   desktop (~4.5 to 6.5 min total). **Then run `extend.sh <id> 150` at once:** `bootstrap-box.ps1` arms a 90-minute guest
   deadline, and lifecycle + matrix + Chicago took about 70 minutes after provisioning for rc13.
3. Deploy CI artifacts `windows-unsigned-<sha>` and `gpu-tests-<sha>` (presigned S3 or a short-lived GitHub artifact URL).
4. `installer-lifecycle.ps1` (as SYSTEM via `ssm.sh`), then `run-matrix.sh` with `gpu-baseline,baseline,quarantined-dll,upgrade-over-old`
   (`OLD_INSTALLER_URL` = the previous rc's setup.exe), then `APP_DIR='C:\Program Files\plugable-chat' OUT=./ask-out ./ask.sh <id>
   qwen3.5-4b-cuda-gpu:4 <bucket>` for the Chicago questions. Run the matrix before the questions (a cached model breaks
   `gpu-baseline`). `run-matrix.sh` exits 1 when `unzip` warns about backslashes in the results zip: the box's `matrix-exit=0`
   and the JUnit counts are the result.
5. **Read the screenshots**; expected answers are in `chicago-questions.json` (227,299; THEFT 52,813 / BATTERY 41,130 /
   CRIMINAL DAMAGE 25,135; 36,070 arrests; Austin 11,358; 407 homicides; 18,608 gun; July 22,561).
6. `teardown.sh`, then `aws s3 rb s3://<bucket> --force`: teardown only *reports* leftover `plugable-chat-gpu-*` buckets.
   Create the per-run bucket with the tag `Project=plugable-chat-gpu` and public access blocked.

## Harness lessons that were expensive
- `$ProgressPreference='SilentlyContinue'` or big downloads hang; SSM output is cut at ~24 KB; SSM runs in session 0, so
  anything needing the desktop goes through a scheduled task `/it` in session 1; `ddagrab` fails, `gdigrab` works.
- Drive the app with env vars (`PLUGABLE_MODEL`, `PLUGABLE_INITIAL_PROMPT`, `PLUGABLE_ENABLE_DEMO_DB`,
  `PLUGABLE_ALWAYS_ON_TABLES`), not arguments with spaces. An installer run without `/D=` as SYSTEM lands in the SYSTEM profile.
- Parallel `ask.sh` runs need per-instance S3 keys (fixed). `ask.sh` refuses to run when HEAD is not on origin (the box
  fetches scripts by hash), waits for `chat-finished` (ceiling `WAIT`, 900 s), does a warm-up launch first, and cleans up on exit.
- Model cache: SYSTEM vs Administrator (see [installer](../installer/SKILL.md)). Registration of GPU providers varied from
  10 s to 681 s in the interactive profile; unresolved, compare against the previous build before blaming a change.

## Diagnosing "Windows sees the GPU but the app runs on the CPU" (rc12/rc13, Pat's TBT5-AI)
- **Registration is explicit and per process.** The app calls `discover_eps` then `download_and_register_eps`
  (`backend/sdk.rs`, `register_execution_providers`) and **waits with no timeout** (a heartbeat line every 15 s); neither does the
  SDK time out. The catalog only lists GPU variants for providers registered in *this* process.
- **Read the `[gpu]` lines in `logs/plugable-chat.log`** (since rc13): `registering providers: ...`, `engine answered after N ms:
  success=..., status=..., registered=[..], failed=[..]`, `registration done`. Healthy A10G: `success=true` after about 61 s,
  "EP registration complete: 3 bootstrapper(s) succeeded", then `model list: 49 GPU`. Pat's rc12 eGPU: the call returned within
  about 9 ms, registered nothing and named no failure; rc13 records that as `EngineDeclined` with the engine's status text.
- **Foundry Core's own log** goes to `logs/foundry/` (Information level, set in `get_or_create_manager`); its provider, WinML,
  CUDA and warning lines are appended to "Copy details for support". It is the only place the engine says why it skipped a provider.
- **Two adapters** (iGPU + eGPU) made `discover_eps` list providers twice; the app now requests each name once.
- **Windows ML preconditions** (from the open-source `sdk_v2` port of Core; the shipping 1.2.3 Core is closed): Windows 11 24H2
  (build 26100+), `Microsoft.Windows.AI.MachineLearning.dll` loadable, and for unpackaged apps the Windows App SDK bootstrap
  (`AdditionalSettings["Bootstrap"]="true"`, default in SDK v2, not in 1.2.x; opt in with `PLUGABLE_FOUNDRY_WINML_BOOTSTRAP=1`).
  Foundry's own CUDA bootstrapper is gated on `nvidia-smi` compute capability >= 5.0.
- **Rule-out switches:** `PLUGABLE_FOUNDRY_BACKEND=cli_http` makes the app use the system `foundry` service (the CLI that worked
  on Pat's machine). `scripts/support/gpu-ground-truth.ps1` (read-only) writes every layer's view to one text file for a tester.
- `Win32_VideoController.AdapterRAM` is 32-bit: a 16 GB card reads 4095 MB. Thunderbolt 4/5 controllers may be named "USB4".
- **No runner has a consumer card or an eGPU**, so a tester's machine (Pat) is the only check for that case and stays a release gate.

## Results so far (2026-09-29/30)
Phi-4-mini (CUDA) answered 7/7 Chicago questions correctly on a T4 (build 5d3385a). qwen3.5-4b initially produced no
result on an A10G because its XML tool calls were not parsed (fixed, see [models-and-tools](../models-and-tools/SKILL.md));
**the A10G re-run on the fixed build was done on 2026-10-10 (rc10): 7/7 Chicago questions correct on `qwen3.5-4b` (CUDA).** SDK 1.2.3 T4 run: SDK tests pass; app first-run
registration slow; Chicago run incomplete (`docs/gpu-validation.md`). **rc13 (496f828, 2026-10-17, A10G):** lifecycle 30/30,
matrix 4/4, Chicago 7/7 on `qwen3.5-4b-cuda-gpu:4`; about 1.5 h of box time including provisioning.
