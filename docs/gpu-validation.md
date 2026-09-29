# GPU validation: how it works and what we learned

Plugable Chat is tested on a real Windows machine with an NVIDIA GPU before any
release is signed. The machine exists only for the length of a test run: it is
created fresh from Amazon's stock Windows image and destroyed at the end. Nothing
is kept on AWS between runs, so there is no cost between runs.

This page is the runbook and the record of what was learned building it. The
scripts are in [`infra/aws-gpu/`](../infra/aws-gpu/); the build and unit tests are
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml).

## The pieces

| Piece | What it does | Cost |
|---|---|---|
| `ci.yml` | Builds on Windows and Linux, runs unit tests, keeps the unsigned installer and the compiled GPU tests | Free (public repo) |
| `infra/aws-gpu/10_network.sh` | One isolated network with no inbound access | Free |
| `infra/aws-gpu/00_budget.sh` | Monthly cost alert ($25 by default) | Free |
| `infra/aws-gpu/launch.sh` | Starts one g4dn.xlarge (NVIDIA T4, 16 GB) | About $0.53/hour while running |
| `infra/aws-gpu/bootstrap-box.ps1` | Sets up the box: driver, WebView2, Node, ffmpeg, auto-logon, failsafe | (part of the run) |
| `infra/aws-gpu/capture.ps1`, `run-in-session.ps1` | Record the screen inside the desktop session | (part of the run) |
| `infra/aws-gpu/teardown.sh` | Destroys the box and proves nothing billable is left | Free |

## Running one by hand (debugging)

```bash
cd infra/aws-gpu
./10_network.sh                       # once
SPIKE_SSM=1 ./launch.sh               # prints the instance id; adds a temporary SSM role
./ssm.sh <id> 'C:\gpu\...'            # run PowerShell on the box (wait ~2 min after launch)
./teardown.sh                         # ALWAYS finish with this; it exits non-zero if anything is left
```

## What we learned (each of these cost real time)

**On the box**

1. **Turn the PowerShell progress bar off** (`$ProgressPreference = "SilentlyContinue"`)
   before any download. With it on, the 748 MB NVIDIA driver never finished in 15
   minutes; with it off it took 13 seconds.
2. **The NVIDIA driver AWS publishes for G-instances works with no Marketplace
   subscription.** It is free from the public bucket `ec2-windows-nvidia-drivers`.
   Silent install takes about 110 seconds and needs no reboot for the T4 to appear
   (Tesla T4, 15,360 MiB, driver 596.86, CUDA 13.2, Windows display mode).
3. **Auto-logon is what creates a desktop.** Commands from SSM or user-data run in
   session 0, which has no desktop, so no screen capture and no app window. Setting
   auto-logon and rebooting once gives a console session 1 with Explorer running. Use
   `run-in-session.ps1` (a scheduled task with `/it`) to run anything there.
4. **A one-shot `shutdown /t` timer is lost when the box reboots** and auto-logon
   needs a reboot. We found this when the original 2-hour failsafe silently vanished.
   The failsafe is now a scheduled task that checks a stored deadline every 5 minutes.
5. **Record with `gdigrab`, not `ddagrab`.** `ddagrab` fails to open on this driver;
   `gdigrab` gave video and screenshots that were real desktop content (not black).
6. **Windows Server has no WebView2**, which the Tauri app needs. Install it (67 s).
7. **SSM output stops at about 24 KB** and long commands outlive most tool timeouts,
   so write results to a file and run long steps in the background.

**In CI**

8. **Pin Rust 1.94.1.** `ethnum 1.5.2` (via lancedb, lance and jsonb) does not compile
   on the newest stable ("cannot transmute between types of different sizes"). Remove
   the pin when that dependency chain moves. The release workflow needs the same pin.
9. **Install `protoc`** (the protobuf compiler): `lance-encoding` needs it. On Linux
   also install `libprotobuf-dev` for the standard `.proto` includes. Windows uses a
   pinned, hash-checked download.
10. **Linux never built.** `rfd` (the crash dialog) enabled its `xdg-portal` backend
    by default while Tauri's dialog plugin enabled `gtk3`, and `rfd` refuses both.
    Fixed in `src-tauri/Cargo.toml` (`default-features = false, features = ["gtk3"]`).
    Linux also needs `libgtk-3-dev`.
11. **The Windows test program imports `directml.dll` directly.** Windows ships an
    older one in System32 (1.15.5 on the runner), so the program died at start-up
    with `STATUS_ENTRYPOINT_NOT_FOUND` (0xc0000139) before running a single test.
    The matching `DirectML.dll` comes with the ONNX Runtime download and must sit
    beside the program. `scripts/ci/collect-test-binaries.ps1` does this. The same
    is likely true of the installed app: check that the installer ships it.
12. **A cold Windows build takes about 19 minutes.** CI caches Rust builds on `main`,
    and keeps the cache even when a run fails, so iterating on a failure is quick.
    The release workflow deliberately uses no cache.
13. **Do not let a new push cancel a running build on `main`.** A documentation-only
    push cancelled a 19-minute Windows build. CI now ignores changes that only touch
    `docs/`, `infra/` or Markdown, and only cancels superseded pull-request runs.

**What the GPU box found about the app itself (2026-09-29)**

14. **The CUDA provider could not load because `onnxruntime_providers_shared.dll` was not
    bundled.** `onnxruntime_providers_cuda.dll` depends on it (Windows error 126), so
    Foundry offered only CPU models. `build.rs` now bundles it. Verified on the T4: the
    catalog went from 36 to 48 GPU variants and CUDA registered.
15. **GPU execution providers are registered per process and the SDK does not register
    them by itself.** The app's own log says "EP registration deferred. Call
    DownloadAndRegisterEpsAsync to begin" and runs with only `CPUExecutionProvider`.
    Nothing in the app calls `download_and_register_eps`; only the GPU tests do. Loading
    a CUDA model from the real app returned `404` on `/openai/load/...` and the GPU stayed
    idle. **Open product decision:** the app should register providers on start (a first-run
    download of about 1.5 GB that took 4 min 39 s).
16. **Registering providers for the first time is slow (279 s); afterwards it takes 3 s.**
    Every fresh box pays the 279 s, because nothing is kept between runs.
17. **The app wants `phi-4-mini-instruct`.** With another model cached it shows "No
    compatible model available. Please download phi-4-mini-instruct" and sits on
    "Connecting to Foundry". With `Phi-4-mini-instruct-cuda-gpu:5` cached it connects.
18. **Results of the ignored suite on the T4 (20 tests): 10 pass, 10 fail.** Failures are
    environmental: 4 call the `foundry` command-line tool that the box does not have, 3
    cannot find `test-data/` (not shipped beside the tests), 1 assumes a Mac WebGPU host.
    Two (`native_fallback_to_hermes`, `tool_search_discovers_deferred`) are not diagnosed.
21. **Microsoft's Rust SDK docs say Windows apps should use the `winml` feature** (`cargo add
    foundry-local-sdk --features winml`), which "integrates with the Windows ML runtime" and
    does "automatic download and registration of appropriate ONNX Runtime execution
    providers (CUDA, Vitis, QNN, OpenVINO, TensorRT)". `src-tauri/Cargo.toml` uses
    `foundry-local-sdk = "1.2.0"` without it, which matches what the GPU box saw: providers
    "deferred" and CPU only. Two ways to fix it, and a decision for the owner: enable
    `winml` (Microsoft's recommended path; needs a Windows 11 24H2 / Server 2025 class OS,
    so the test box would need a Server 2025 image), or keep the cross-platform crate and
    call `download_and_register_eps` at start-up (what the GPU tests do; works on Server 2022).
22. **The app warms models with the CLI's REST call, which the SDK's own web service does
    not serve.** `model_gateway_actor.rs:399` sends `GET /openai/load/{name}?ttl=0`; the
    REST reference documents that call as part of the Foundry Local CLI service, and the
    SDK-hosted service answers `404`. The SDK path is `model.load()` (already wrapped as
    `FoundryBackend::load` in `backend/sdk.rs`). `prewarm_model_in_background` should use
    it when the SDK backend is active. `SdkBackend` is not `Clone`, so this needs the actor
    to hold it in an `Arc` (or expose the static manager handle).

**Driving the box (mechanics)**

19. Run screenshots through a scheduled task in the desktop session and start `ffmpeg`
    from `wscript.exe` with a hidden window; otherwise a black `cmd` window appears in the
    picture. `ffmpeg.exe` must be at the path the script uses (`C:\gpu\ffmpeg.exe`).
20. Send files to the box through short-lived links, not through SSM. Get GitHub artifacts
    with `GET /repos/.../actions/artifacts/<id>/zip` (it answers 302 with a signed URL that
    the box can fetch in about a second; downloading 100 MB through a laptop connection
    kept resetting). Send results back with a presigned S3 `PUT` and `curl.exe -T`.

## Rules that keep the cost at zero

- Every AWS resource carries the tag `Project=plugable-chat-gpu`.
- The box has no IAM role in production, no Elastic IP, no NAT gateway, no S3 bucket
  that outlives the run, no saved image and no snapshot.
- The disk is deleted on termination and the instance terminates when it shuts down.
- The failsafe powers the box off after `DeadlineMinutes` (default 90) whatever happens.
- Every run ends with `teardown.sh`, which lists anything tagged for this project that
  still exists and fails if it finds any.
