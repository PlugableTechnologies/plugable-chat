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

### Unattended runs (rung 1b, scripted)

A harness can run the app, ask questions and read back what it did, without screenshots. All of these are
command-line flags with an environment twin (`--reply-file` / `PLUGABLE_REPLY_FILE`, and so on). They are for
testing; the public README and user guide do not mention them.

| Flag | What it does |
|---|---|
| `--initial-prompt`, `--prompts-file <json>` | The first prompt, then more prompts (a JSON array of strings), each in a fresh chat once the previous one finished. With no `--initial-prompt` the file's first prompt starts the run. One launch pays for one model load, however many questions. |
| `--same-chat` | Run the `--prompts-file` prompts as follow-ups in ONE chat (history grows each turn) instead of a fresh chat each. Reproduces a long conversation unattended: `infra/aws-gpu/chicago-followups.json` is ten linked questions on the Chicago data, the scenario that crashed rc14 inside the WebGPU provider. |
| `--reply-file <path>` | Appends one JSON line per finished turn: `prompt_index`, `question`, `reply`, `outcome` (`ok`, `error`, `cancelled`, `max_iterations`, `approval_timeout`, `launch_model_error`, `startup_error`, `timeout`), `detail`, `model`, `chat_id`, `duration_ms`, and `tool_calls` (server, tool, arguments, a short `result_summary`, `result_chars`, `is_error`, `duration_ms`). Anything that stops the run before a turn can finish also writes a record, so a missing line never has to be guessed at. |
| `--exit-after-reply` | Exit once every prompt has a record. Exit code 0 all `ok`, 1 a turn failed, 2 the `--model` could not be used, 3 startup failed or the arguments were not understood, 4 timed out. |
| `--reply-timeout-secs <n>` | Overall limit for a reply run (default 1800). |
| `--mcp-server <json or @file>` | Replace the MCP server list. Each flag value is **one server object**, not an array; repeat the flag for more servers (an array fails with "Failed to parse MCP server" and the run then waits for a reply that never comes). Set `auto_approve_tools: true` (otherwise a call waits up to 5 minutes for a click) and `defer_tools: false`; set `request_timeout_secs` for servers whose tools run longer than 30 s. A server at a URL uses `"transport": {"type": "http", "url": "http://host:8787/mcp"}` (no `command`); the dev test server's `POST /mcp` is a stand-in for one. With tool search on, a server's own `defer_tools` is honoured (the old forced deferral is gone), so `--tools <server_id>` and `defer_tools: false` both work without `--tool-search false`. `--mcp-timeout-secs <n>` sets it for every server that does not set its own. |
| `--tools <server_id>`, `--tool-search false` | Expose only that server's tools directly, so a small model does not have to search for them. |
| `--chat-home <dir>` | Keep config, chat history and cache under this directory instead of the platform locations. Downloaded Foundry models are still found. |
| `--ephemeral` | Never write the saved config file. Use it with `--chat-home` so a test run leaves nothing behind. |
| `--dump-chats <file>` | Write every saved chat (id, title, messages, and whether the saved messages parse) to a JSON file and exit; 1 when the chat store cannot be read. The soak test runs it after the app was killed. |
| `--hidden` | Start with the window hidden. A debug build loads its UI from the Vite dev server, so `npm run dev` must be running on port 1420 or the run stalls at "AwaitingFrontend" until its reply timeout. The page still runs, but check that a hidden webview keeps its timers on your OS before relying on it. |

Example (Mac):

```bash
PLUGABLE_CHAT_HOME=/tmp/pc-home plugable-chat --ephemeral --hidden \
  --mcp-server @/tmp/server.json --tools myserver --tool-search false \
  --prompts-file /tmp/questions.json --reply-file /tmp/replies.jsonl --exit-after-reply
```

A bad flag with `--reply-file` set is fatal (a `startup_error` record, exit 3) instead of silently running with defaults.

## Rung 2: only what needs real Windows or a GPU

This rung is **mandatory before any release tag**: always test Plugable Chat with a real NVIDIA GPU before signing. Do build
and install testing without a GPU wherever possible (rung 3's CI and `clean-host.yml`), and use the GPU box for the rest.

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
| A tester's eGPU stayed on the CPU and the support details could not say why: registration output went to `println!` (lost in release builds) and an empty refusal from the engine recorded no failure | Pat's machine (rc12) | rung 1: `sdk::r17_*`, `r18_*`, `gpu_assessment::r15_*`/`r16_*`; clean-host scenario `engine-declined` stages the engine's answer with `PLUGABLE_CHAT_TEST_REGISTRATION`. A consumer card or eGPU still has no runner |
| End-of-registration re-assessment ran before the adapter scan was stored and said "no GPU" on an NVIDIA machine | clean-host (`nvidia-present-no-providers`) | rung 1: `gpu_assessment::r15_an_unfinished_adapter_scan_*`; only clean-host shows the real start-up order |
| A start-up report never appeared: PowerShell CIM probes had no time limit, and the free-disk probes held the observation lock | clean-host (`egpu-thunderbolt-hint`) | rung 1: `process_utils::timeout_tests`; host probes give up after 45 s and run outside the lock |
| The report started during GPU setup finished last, so the newest report said "Setting up" (two reports also shared one file name within a second) | clean-host (`engine-declined`) | the report reads the GPU state after its probes; file names are per millisecond. Caught by the clean-host scenario |


| All three release candidates (rc1 to rc3) hung for the full 180 minutes. `src-tauri/build.rs` starts a nested `cargo build` for the Python sandbox (WebAssembly) when the gitignored `src-tauri/wasm/python-sandbox.wasm` is missing, as it always is on CI. In a release build the outer cargo holds `target/release/.cargo-lock` and the nested build needs it for its own build scripts: a circular wait. Debug CI used `target/debug`, so it never hung. The earlier "77 minutes without finishing" was the same deadlock, not slow compilation | GitHub release (3 times, 9 hours of runner time) | rung 1: reproduced on the Mac by hiding the .wasm and running `cargo build --release --bins --features tauri/custom-protocol` (process tree: build-script-build -> cargo at 0% CPU, both cargos holding target/release/.cargo-lock). Fixed first by giving the nested build `CARGO_TARGET_DIR` in `OUT_DIR` (5 min 30 s), then removed entirely (2026-10-01): the app never loaded the `.wasm` (no wasmtime dependency; `python_sandbox::execute` runs RustPython in-process), and the nested build had also started failing (`CLOCK_PROCESS_CPUTIME_ID` missing from libc for wasm32-wasip1 in rustpython-vm 0.4.0). The same release build now takes about 1m 40s with no `.wasm`. A fresh-checkout release build is still the first thing to run locally before tagging; preflight checks that `build.rs` starts no nested cargo. |
| v0.1.0-rc4 built and signed everything, then failed its last step: it verified `target/release/plugable-chat.exe`, but tauri signs patched copies of the exe that go into the installers and leaves that file unsigned. The signed MSI and NSIS setup both verified | GitHub release (after a 63-minute build) | rung 2: `scripts/ci/verify-installers.ps1` installs both installers silently and checks the installed exe; run on a Windows box against CI's unsigned installers (it found and checked all four files and failed them as NotSigned, as it should). Lesson: verify what ships (the installed app), not intermediate build outputs |
| v0.1.0-rc5 verified all four shipped files as signed, then failed at "Attest build provenance": the Windows job lacked `id-token: write` and `attestations: write` (the Linux job had them) | GitHub release (after another 65-minute build) | rung 1: `scripts/ci/check-workflow-parity.py` fails when a job that attests provenance lacks either permission (verified against rc5's release.yml). Lesson: every step of a tag-only workflow runs for the first time on a real tag, so read each unexercised step against its action's documented requirements before tagging |
| The signed rc6 MSI's first Chicago questions answered "I don't have access to real-time databases" (504-character prompt, no tables): launch-time schema indexing gave up after 120 s, but on a fresh box GPU provider registration alone took 153 s and the embedding model is only created after it ("CPU embedding model not initialized") | EC2 box, signed release build | the waits are now 10 minutes. The harness itself needed three fixes on its first real use (`APP_DIR` quoting for `C:\Program Files`, a race that reported a timeout at 0 s, `ask.sh` not passing the install path). Lesson: provider registration being 50x slower in the installed app than in the compiled tests (153 s vs 3 s) is still unexplained and sets the pace for every first launch |
| rc10 was tagged, signed and published after CPU-only runs; the NVIDIA box was not used first, and an announcement overstated what was tested | release | process: GPU run is mandatory before any tag (`release-signing`, `gpu-validation`, `test-ladder` skills; `AGENTS.md`) |
| rc14 testers (Wash, Pat, Bob): switching to `gpt-oss-20b` or `phi-4` failed with "requires 17537 total tokens … exceeds … 8192" and the model was marked incompatible; "Operation was cancelled" did the same to `qwen3.5-4b`. `max_tokens` was a fixed 16384 and every SDK error counted as a model fault | EC2-class hardware (TBT5-AI, eGPU) | rung 1: `context_budget` unit tests, `BackendError::classify_*` tests built from the exact strings, and `backend/fake_tests.rs` (a scriptable fake engine with a real context window) |
| The "This model isn't compatible… Switching…" text was saved as an assistant message and replayed to the next model, which reasoned about it | Wash (rc14) | rung 1: notices travel on the `chat-notice` event; `history.test.ts` strips legacy notices; `fake_tests` asserts no notice reaches a request |
| A 2.2 GB fallback model downloaded with no prompt; its progress bar stayed at 100%; a second download hid the first | Wash, Pat (rc14) | rung 1: `fallback.test.ts` (never downloads unasked), `downloads.test.ts` (one entry per model, every path ends done/error/cancelled), `variant_preference` test (an NPU build is not chosen on a PC without one) |
| The app died inside `onnxruntime_providers_webgpu.dll` (0xc0000409) after several follow-up questions on the Chicago data | Wash (rc14), Event Viewer | rung 1: `run_state` tests (a mid-answer death is detected next launch; the second one switches the model off). The cause is not proven (hypothesis: unbounded history) and a native crash capture with a minidump is not built: it needs a Windows build to check. Repro is clean-host case 9 plus the multi-turn Chicago scenario on the GPU box |
| `main.chicago_ crimes` (a space inside the table name, written by the model) was read as the table `main.chicago_`; the error listed sources, not tables, so the model repeated itself | Bob (rc14) | rung 1: `sql_tables` tests (garbled, quoted, aliased, ambiguous and near-miss names; SQL is rewritten when one real table matches) |
| After the toolbox finished installing, the red "Schema refresh failed" alert stayed until the app was restarted | Wash (rc14) | rung 1: `database-errors.test.ts`; the tab now re-runs a refresh that was waiting on the toolbox |
| Unplugging the network crashed the app in `onnxruntime_providers_webgpu.dll` (0xc0000409); the damaged dll then survived reinstall, every prompt crashed, and every model was switched off ("No compatible model available") | Wash (rc15), Event Viewer | rung 1: `engine_health` tests (marker, quarantine, GPU-off ladder, attribution from the Application log), `gpu_registration` watcher gate, `startup_actor` verdict. rc16 fault injection: see "Engine-health fault injection" below; the real check is the GPU box (truncate an `ep\webgpu-ep` file, kill the app twice) |
| `PLUGABLE_FOUNDRY_BACKEND=cli_http` stopped the app from starting where the `foundry` CLI was absent | Pat (rc14) | rung 1: `backend_choice_tests` (falls back to the SDK with a log line). The Windows launch itself is clean-host case 3 |

## Escapes with a named test (enforced by scripts/ci/check-escapes-have-tests.mjs)

From rc18 on, an escape is only recorded here, and only with a test that exists: the last cell names `path#test name`. `scripts/preflight.sh` and CI fail when a row names no test, a file that is not there, or a test name the file does not contain. The table above predates the rule. The browser tests (`e2e/`) render the real Settings screens against a mocked backend (`e2e/support/tauri-mock.ts`), so they show what a screen does with an answer; whether the real backend gives that answer is the Rust tests' job. Run them with `npx playwright test` (`PW_CHANNEL=chrome` uses an installed Chrome instead of downloading Chromium).

| Escape | Found at | Now caught at | Test |
|---|---|---|---|
| Settings > Network > Test connection showed an HTTP 404 or 400 as plain "reachable", with nothing saying that only proves the name resolved and TLS worked | Bob (rc18) | rung 1 | `src/lib/network-settings.test.ts#a 4xx answer is reachable` `e2e/settings.spec.ts#a 404 on the home page is reported as the expected answer` `src-tauri/src/commands/diagnostics.rs#a_4xx_answer_to_the_home_page_is_reachable` |
| The probe list named `cdn-lfs.huggingface.co`, which Hugging Face no longer serves large files from (Xet hosts now), so a healthy network showed a DNS failure; the chat-model catalog host was not probed at all | Pat (rc18) | rung 1 | `src-tauri/src/commands/diagnostics.rs#the_hosts_checked_are_the_ones_hugging_face_serves_large_files_from_now` `src-tauri/src/commands/diagnostics.rs#the_chat_model_catalog_host_is_probed` |
| "The search model loaded." in green sat beside a red host row and read as a verdict on it | Bob (rc18) | rung 1 | `e2e/settings.spec.ts#a loaded search model is not shown as a green success beside a failing host` |
| A fresh release install listed a dev-only MCP server that runs through `cargo`; enabling it failed with "cargo: program not found", and the server stayed saved as enabled | Pat (rc18) | rung 1 | `src-tauri/src/settings.rs#release_defaults_have_no_dev_servers` `src-tauri/src/actors/mcp_host_actor.rs#a_missing_program_says_what_to_install` `src-tauri/src/commands/settings.rs#a_server_that_failed_to_start_is_saved_as_off_and_the_error_is_returned` `e2e/settings.spec.ts#a server that cannot start stays off and the error says what to install` |
| Red "Startup failed (1/2)" while the first-run chat model download was still moving: the engine gave up connecting after about 45 s and nothing counted download progress as start-up progress | Bob, Pat (rc18) | rung 1 | `src-tauri/src/actors/startup_actor.rs#a_model_failure_is_not_a_start_up_failure_while_a_download_is_still_reporting_progress` `src/store/chat/startup-display.test.ts#a failed model is not a red failure while a model download or GPU setup is still moving` `e2e/settings.spec.ts#no red "Startup failed" while the chat model download is moving` |
| Refresh worked with the Database Toolbox switched off (the check looked only at the source's own box), and refreshed the saved settings rather than the ones on screen | Bob (rc18) | rung 1 | `src-tauri/src/commands/database.rs#refresh_is_refused_while_the_toolbox_is_off_even_when_a_source_is_on` `e2e/settings.spec.ts#Refresh is not offered while the Database Toolbox is off` `e2e/settings.spec.ts#Bob's flow` |
| "No database tables available" gave no cause (no source, toolbox off, nothing switched on, or a cache never read) and no way to fix it | Bob (rc18) | rung 1 | `src/lib/empty-tables.test.ts#no sources, toolbox off, nothing switched on and an unread cache each get their own explanation` `e2e/settings.spec.ts#an empty list says why and offers Refresh, which fills it` |
| The Save button showed (disabled) when there was nothing to save, and the Settings tab strip scrolled sideways so some tabs were out of sight | Bob (rc18) | rung 1 | `e2e/settings.spec.ts#Save appears only while there is something to save` `e2e/settings.spec.ts#every tab fits inside the window without a sideways scroll bar` |
| A network drop in the middle of a turn ended it as a short "answer" (or took the app down): the words written so far and the tool results of that turn were not all on disk, and a half-written tool call could be replayed to the model | David (rc15, again in the rc18 report) | rung 1 for the pieces: a confirmed drop stops the stream and any wait on a network tool (`turn_failure.rs`), the turn ends as a failed turn with Retry, the saved chat pairs every tool call with a result. Written and not yet run on Windows: fault scenarios `network-drop-mid-turn` and `network-drop-mid-tool-call` (`scripts/ci/faults.ps1`), and the 20-prompt soak `infra/aws-gpu/soak-run.ps1` | `src-tauri/src/turn_failure.rs#a_failed_turn_keeps_the_partial_words_and_every_tool_result_and_invents_no_reply` `src-tauri/src/turn_failure.rs#the_watch_returns_only_after_the_network_goes_away` `src-tauri/src/chat_persistence.rs#a_turn_cut_off_by_a_network_drop_saves_the_words_and_every_tool_result` `src/store/chat/failed-turn.test.ts#a chat saved after a network drop reloads with every message and no raw tool wrapper in a visible bubble` `e2e/failed-turn.spec.ts#a network drop keeps the words so far, says what happened, and sends nothing until Retry is pressed` `scripts/ci/check-chat-store-integrity.test.mjs#a prompt that vanished from the store is reported` |
| After the crash every later prompt crashed too, and the only way out was a reinstall; nothing recorded where the app died | David (rc15) | rung 1: a second death in a provider dll switches that GPU path off by itself, Repair engine sets the bad cache aside and clears every switch, and the last crash is recorded with its Windows minidump. Written and not yet run on Windows: scenario `crash-capture` (needs the installer's WER setting from `windows/hooks.nsh`) | `src-tauri/src/engine_health.rs#repeated_deaths_in_a_provider_dll_quarantine_once_then_leave_the_gpu_path_and_stay_off` `src-tauri/src/engine_health.rs#repair_sets_the_bad_cache_aside_and_clears_every_switch_a_crash_set` `src-tauri/src/crash_capture.rs#a_death_the_ep_fault_seam_names_leaves_a_last_run_crashed_in_ep_marker` `src-tauri/src/crash_capture.rs#the_marker_carries_the_dump_that_belongs_to_the_dead_run_not_an_older_one` `src/lib/engine-health.test.ts#a crash inside a GPU provider is named, with where its dump is` `e2e/failed-turn.spec.ts#an engine error offers Repair engine, which runs the one-click repair` |
| A chat-model download cut off by a network drop gave up at once, and the error did not say which host failed | rc18 report | rung 1: the download is called again with waits of 5, 20, 60 and 180 s, never for a full disk or a cancel, and the final error names the host when the engine names one. Whether a second call continues from bytes already fetched is up to Foundry Local Core (the download runs inside it); not shown here. Written and not yet run: scenario `foundry-download-resume` | `src-tauri/src/actors/foundry/download_retry.rs#a_download_cut_off_twice_completes_on_the_third_try_and_each_wait_names_the_host` `src-tauri/src/actors/foundry/download_retry.rs#when_every_attempt_fails_the_error_names_the_host_and_the_attempts` `src-tauri/src/actors/foundry/download_retry.rs#a_permanent_error_is_returned_at_once_with_no_wait` |
| Release candidates were tagged, signed and published on CPU-only runs or with no fresh-host run at all (rc10) | release | rung 3: `release.yml` runs `scripts/ci/check-release-evidence.mjs` before any build; a tag with no `release-evidence/<tag>.json` for the tested commit, or with a failed, skipped or missing scenario, or with code changed after the run, stops there | `scripts/ci/check-release-evidence.test.mjs#a tag with no evidence is blocked` `scripts/ci/check-release-evidence.test.mjs#a failed, skipped or absent scenario blocks the tag` `scripts/ci/check-release-evidence.test.mjs#release.yml makes every build and the publish step wait for the evidence check` |

Not covered by a test, so not in the table: the native crash itself (`onnxruntime_providers_webgpu.dll`, cause unproven; out of scope for these steps). Whether a real network drop on a Windows GPU host now ends the turn cleanly instead of crashing is only shown by the written-but-not-run scenarios above. The CLI download path used to treat a broken connection mid-stream as the end of the stream and report a half-downloaded model as finished; that is fixed in `model_gateway_actor.rs` but no test drives it (it needs the local `foundry` service).

## Release gate (from rc19)

`release.yml` starts with `scripts/ci/check-release-evidence.mjs --tag <tag>`; every build and the publish job wait for it. It needs `release-evidence/<tag>.json` with a passing result for every scenario tagged `gpu` in `scripts/ci/faults.ps1` (the GPU matrix, run on the AWS box with `infra/aws-gpu/run-matrix.sh gpu`) and for every scenario in the default list of `.github/workflows/clean-host.yml` (the Windows fresh-host CPU set), for a commit that the tagged commit has not changed since except under `docs/` and `release-evidence/`, no older than 14 days. Record each run with `node scripts/ci/record-release-evidence.mjs --tag <tag> --commit <tested sha> --suite gpu_matrix|clean_host_cpu --junit <clean-host-junit.xml>`, then commit the file. The check proves the record is complete, fresh and matches the code; it cannot prove the run happened, so the evidence file is reviewed like any other change (add `/release-evidence/` to `.github/CODEOWNERS` to require a named reviewer).

## Engine-health fault injection (rc16)

The app sets damaged GPU components aside and switches GPU acceleration off on its own (`src-tauri/src/engine_health.rs`). To exercise that without waiting for a real crash:

| Variable / flag | Effect |
|---|---|
| `PLUGABLE_CHAT_TEST_FAULT=offline` | the connectivity probe says there is no network (watcher and retries stand down; the catalog falls back to the last-known models) |
| `PLUGABLE_CHAT_TEST_FAULT=ep-fault` | if the previous run died, attribute it to `onnxruntime_providers_webgpu.dll` under `ep\` (quarantine that provider) |
| `PLUGABLE_CHAT_TEST_FAULT=ep-mismatch` | treat the verified marker as mismatched (quarantine the whole `ep` folder) |
| `PLUGABLE_CHAT_TEST_FAULT=registration-hang` | provider registration never returns (use with `PLUGABLE_CHAT_EP_REGISTER_TIMEOUT_SECS=20` to see start-up carry on) |
| `PLUGABLE_CHAT_TEST_FAULT=empty-catalog` | the model list call fails (the last-known list is shown) |
| `PLUGABLE_CHAT_TEST_FAULT=crash-at-launch` | die at once like the rc15 crash (a fast-fail, 0xc0000409 on Windows, so Windows Error Reporting takes a minidump and `run-state.json` stays behind) |
| `PLUGABLE_CHAT_TEST_FAULT=crash-mid-turn` | the same, just before the engine is asked to answer |
| `PLUGABLE_CHAT_TEST_OFFLINE_FILE=<path>` | while this file exists the connectivity probe says there is no network; creating and deleting it mid-run is a drop and a restore the app can see without unplugging anything (a turn that began online ends as a failed turn when it appears) |
| `PLUGABLE_CHAT_EP_DIR=<dir>` | use this folder instead of `~/.plugable-chat/ep` |
| `--repair-engine` / `PLUGABLE_REPAIR_ENGINE=1` | what Settings > Models > Repair engine does, before start-up |

Values of `PLUGABLE_CHAT_TEST_FAULT` combine with commas. A real check on the GPU box: after a successful chat turn truncate a file in `ep\webgpu-ep\`, kill the app, relaunch: the folder is renamed to `ep.quarantine-<unix time>` and Core downloads it again. Two `taskkill /f` in a row switch GPU acceleration off (`engine-health.json` in the config folder, `gpu_off`). Windows names the faulting module in the Application log and the app reads it with `wevtutil` at launch; a fast-fail crash (0xc0000409) skips in-process handlers, so there is no in-process hook.
