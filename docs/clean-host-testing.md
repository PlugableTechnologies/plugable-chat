# Clean-host testing: failures first, then the full run

Run in this order on every release candidate. Stage 1 proves each failure shows the right message; only then install what was
missing (Stage 2) and run the intended tests (Stage 3). Never skip to Stage 3 on a machine that already has the dependencies:
that is how the "Foundry must be installed" report was missed.

## Stage 1: before dependencies (record screenshot + the copied details for each row)

| # | Case | How to create it | Expected result |
|---|---|---|---|
| 1 | Fresh Windows host, nothing installed, no Foundry Local | Clean VM or reset machine; install the signed setup `.exe` | App starts, downloads model on its own; no "install Foundry" text anywhere |
| 2 | AI runtime files missing | Delete `Microsoft.AI.Foundry.Local.Core.dll` from the install folder (Windows), (on macOS, `PLUGABLE_FOUNDRY_LIBRARY_DIR=<empty folder>` was tried on 2026-10-07 in a dev build and did **not** produce the failure: the SDK still loaded, so this row needs a packaged Windows install) | Red "Plugable Chat can't start" card: reinstall link, VC++ Redistributable link, raw error shown, **no Mac wording on Windows** |
| 3 | Legacy CLI path, CLI missing | `PLUGABLE_FOUNDRY_BACKEND=cli_http`, `foundry` not on PATH | Same card, raw error shown, no hang |
| 4 | CLI installed, service will not start | `cli_http`, make `foundry service start` fail (rename a required file or use a bad shim) | Card names the failed command and its exit code (not "Could not connect") |
| 5 | No NVIDIA driver / unsupported GPU | Windows host with no NVIDIA driver (or Intel/AMD only) | Card or status explains the GPU problem with the driver link; app does not look hung |
| 6 | No internet at first launch | Disconnect network before first start | Download failure text mentions internet and disk space, not a CLI command |
| 7 | Previously-installed Foundry Local + Plugable Chat (David's laptop) | Host with both already present | Same behaviour as case 1; bundled engine is used |

Pass criteria for every row: a human with no context can tell what is wrong and what to do from the screen alone.
Rows 3, 4 and 6 can be run on macOS; rows 1, 2, 5 and 7 need Windows.

## Stage 2: install the dependencies

Install only what the Stage 1 message asked for, using the link on the card (driver, VC++ Redistributable, reinstall).
Re-run the failing row after each install and confirm that exact message goes away.

## Stage 3: full intended tests

1. `scripts/preflight.sh` (Rust lib tests, frontend unit tests, typecheck).
2. `scripts/ci/verify-installers.ps1 -Msi <msi> -Nsis <exe>` (signatures and the AI runtime ships).
3. Install the signed build on a Windows RTX host: first-run download, chat with the default model, switch model, restart the app.
4. Repeat case 7's host to confirm an existing Foundry Local does not interfere.
