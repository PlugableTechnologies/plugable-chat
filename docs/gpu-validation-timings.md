# GPU validation: measured time and cost

The point of this page is to judge time and cost from measurements, not guesses.
Numbers marked **~** were noted during the session and not timed to the second.
Anything not yet measured is marked **unmeasured** rather than estimated.

Update it after every run. CI durations can be regenerated with
`python3 scripts/ci/run-timings.py`; box costs come from the instance's launch time
in `aws ec2 describe-instances`.

## Prices used

| Item | Price | Source |
|---|---|---|
| g4dn.xlarge Windows, on demand | $0.526 per hour | AWS price list, us-east-1 |
| 100 GB gp3 root disk | about $0.011 per hour (only while the instance exists) | $0.08/GB-month |
| GitHub-hosted runners | $0 (public repository) | |
| Everything else in the AWS setup | $0 (VPC, security group, budget) | |

## The GPU box, first hands-on run (2026-09-29)

| Step | Time | Note |
|---|---|---|
| Launch to reachable by SSM | ~3 min | not timed precisely |
| NVIDIA driver download | 13 s | after switching off the progress bar |
| NVIDIA driver install | 110 s | no reboot needed for the T4 to appear |
| WebView2 runtime | 67 s | includes download |
| Node LTS | ~10 s | |
| ffmpeg download, unzip, capture test | ~1 min | |
| Auto-logon reboot until a desktop exists | ~2 min | |
| **Provisioning total, if done in one pass** | **~9 to 10 min** | driver + installs + one reboot |
| Wasted on the first driver download attempt | ~16 min | progress bar left on; never finished |
| Waiting for CI to produce an installer | see below | the dominant cost so far |

Cost so far for this box (launched 03:04 UTC, still running): read the meter with
`aws ec2 describe-instances --instance-ids <id> --query 'Reservations[0].Instances[0].LaunchTime'`
and multiply by $0.537/hour (instance plus disk). About 2 hours in, that is roughly **$1.10**.
Most of that is time spent waiting for CI, not testing.

## CI (GitHub-hosted, free), 2026-09-29

| Run | Windows job | Linux job | Result |
|---|---|---|---|
| 1 | 8.3 min | 3.8 min | both stopped early: `ethnum` does not compile on newest Rust |
| 2 | 7.9 min | 6.1 min | `protoc` missing |
| 3 | 21.1 min | 5.2 min | Windows compiled (19.6 min) then the test program crashed at start-up; Linux: `protoc` includes missing |
| 4 | 22.4 min | 5.6 min | Windows: still crashed (DLL); Linux: `rfd` backend conflict |
| 5 | 16.4 min | 8.0 min | Windows: still crashed (DLL); Linux: `rfd` backend conflict |
| 6 | 18.8+ min (running) | 10.5 min | Linux compiled and ran 295 tests in 52 s: 291 passed, 4 failed (they need a live Foundry) |

What this says about repeatability:

- A cold Windows compile is **14 to 20 minutes**. A cold Linux compile is **4 to 9 minutes**.
- Seven distinct problems stood between the first push and the first working test run,
  and every one of them needed a full round trip (about 10 to 25 minutes each). The
  fixes are recorded in `gpu-validation.md`; once fixed they should not recur.
- The Rust cache should cut warm runs sharply; **unmeasured** until a run has saved a cache.

## Estimated cost of one production GPU run (not yet measured end to end)

| Part | Time | Basis |
|---|---|---|
| Launch and provisioning | ~10 min | measured above |
| Install the app, run GPU tests, drive the UI, record | **unmeasured** | the whole point of the next run |
| Model download from Microsoft's catalog | **unmeasured** | depends on model size |
| Collect results and tear down | ~1 min | |

At $0.537 per hour every 10 minutes costs about **$0.09**. If the unmeasured parts total
20 to 30 minutes, one run is about **$0.30 to $0.40**. That is a hypothesis to check, not a number.

## How to keep this honest

- Record the start and end time of every box run from the instance's `LaunchTime` and
  `teardown.sh` output, and write the cost here.
- Do not leave the box up while waiting for CI. Today's box sat idle for about 1 hour
  40 minutes waiting for Windows builds; in production the box should not be launched
  until the installer artifact exists.
