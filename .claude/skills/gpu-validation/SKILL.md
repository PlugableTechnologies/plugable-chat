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
   desktop (~4.5 to 6.5 min total).
3. Deploy CI artifacts `windows-unsigned-<sha>` and `gpu-tests-<sha>` (presigned S3 or a short-lived GitHub artifact URL).
4. `installer-lifecycle.ps1`, then `OUT=./ask-out ./ask.sh <id> <model-id> <bucket>` for the Chicago questions.
5. **Read the screenshots**; expected answers are in `chicago-questions.json` (227,299; THEFT 52,813 / BATTERY 41,130 /
   CRIMINAL DAMAGE 25,135; 36,070 arrests; Austin 11,358; 407 homicides; 18,608 gun; July 22,561).
6. `teardown.sh`.

## Harness lessons that were expensive
- `$ProgressPreference='SilentlyContinue'` or big downloads hang; SSM output is cut at ~24 KB; SSM runs in session 0, so
  anything needing the desktop goes through a scheduled task `/it` in session 1; `ddagrab` fails, `gdigrab` works.
- Drive the app with env vars (`PLUGABLE_MODEL`, `PLUGABLE_INITIAL_PROMPT`, `PLUGABLE_ENABLE_DEMO_DB`,
  `PLUGABLE_ALWAYS_ON_TABLES`), not arguments with spaces. An installer run without `/D=` as SYSTEM lands in the SYSTEM profile.
- Parallel `ask.sh` runs need per-instance S3 keys (fixed). `ask.sh` refuses to run when HEAD is not on origin (the box
  fetches scripts by hash), waits for `chat-finished` (ceiling `WAIT`, 900 s), does a warm-up launch first, and cleans up on exit.
- Model cache: SYSTEM vs Administrator (see [installer](../installer/SKILL.md)). Registration of GPU providers varied from
  10 s to 681 s in the interactive profile; unresolved, compare against the previous build before blaming a change.

## Results so far (2026-09-29/30)
Phi-4-mini (CUDA) answered 7/7 Chicago questions correctly on a T4 (build 5d3385a). qwen3.5-4b initially produced no
result on an A10G because its XML tool calls were not parsed (fixed, see [models-and-tools](../models-and-tools/SKILL.md));
**the A10G re-run on the fixed build has not been done yet.** SDK 1.2.3 T4 run: SDK tests pass; app first-run
registration slow; Chicago run incomplete (`docs/gpu-validation.md`).
