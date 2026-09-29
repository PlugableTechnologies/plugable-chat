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

## Rules that keep the cost at zero

- Every AWS resource carries the tag `Project=plugable-chat-gpu`.
- The box has no IAM role in production, no Elastic IP, no NAT gateway, no S3 bucket
  that outlives the run, no saved image and no snapshot.
- The disk is deleted on termination and the instance terminates when it shuts down.
- The failsafe powers the box off after `DeadlineMinutes` (default 90) whatever happens.
- Every run ends with `teardown.sh`, which lists anything tagged for this project that
  still exists and fails if it finds any.
