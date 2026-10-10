---
name: release-signing
description: How Windows/Linux release signing works for plugable-chat (DigiCert KeyLocker EV cert, GitHub environment, smoke test, tag workflow) and what to do when signing fails. Use for any signing, release tag, smctl, or release.yml work.
---

# Release signing

Operator guide: `docs/release-signing.md` (rotation, renewal, credentials). Never handle secrets yourself: the DigiCert API
token, client `.p12` and password stay with the user (ingested by brix-cli `ingest_digicert_signing_secrets.py`).

## Before you push a release tag
All of these, in order; a tag starts signing and publishing immediately, so none can be done afterwards:
1. `scripts/preflight.sh` and CI green on the exact commit (Linux and Windows).
2. `clean-host.yml` green (CPU scenarios, free runners).
3. **GPU box run green on the signed-to-be installer's commit** ([gpu-validation](../gpu-validation/SKILL.md)): always,
   no exceptions; installer lifecycle, `gpu-baseline`, Chicago questions, screenshots read.
4. Check `release-signing` environment protection (below): Required reviewers must be ON.
5. Check the release notes and the download-page status labels still match the results in `docs/gpu-validation.md`.

## Setup facts
- EV certificate for **LEANCODE, INC.**, keypair alias in secret `SM_KEYPAIR_ALIAS`, expires 2027-08-11 (org validation to
  2027-09-11; client certificate to 2027-09-28). One designated signer per certificate.
- GitHub Environment **`release-signing`**: secrets `SM_HOST`, `SM_API_KEY`, `SM_CLIENT_CERT_FILE_B64`,
  `SM_CLIENT_CERT_PASSWORD`, `SM_KEYPAIR_ALIAS`. Required reviewers: `bernieplug` or `dnuzum`. Deployment branches: `main`
  and `v*` tags (the first smoke run failed because `main` was not allowed yet).
- **Never approve your own signing run.** The person or agent that dispatched it must not approve; the auto-mode safety
  check blocks it, and the two-person gate is the point. **Current state (2026-09-30): Required reviewers is switched OFF on
  `release-signing`, by the user, to get the pipeline working. "Allow administrators to bypass configured protection rules"
  is still ticked. Before the first real release turn Required reviewers back on (`bernieplug`, `dnuzum`) and untick the
  bypass box; with the bypass ticked any repo admin can skip the gate.** A smoke job can get stuck in "waiting" if its gate was
  created just before the setting changed: cancel the run and dispatch again.
- Tag ruleset "Restrict v* release tags". `release.yml` (verify-tag -> build-windows in the environment -> build-linux with
  cosign/attestations -> publish) is an ask-first file; Windows job timeout is 180 min.

## The signing wrapper (`scripts/sign-windows.mjs`, ask-first)
Used for bundled DLLs (`--skip-signed`) and as Tauri's `signCommand` for the exe, NSIS setup and MSI. It now:
captures `smctl` output; treats `FAILED`/`Error :` as failure **even when smctl exits 0**; verifies every file with
`Get-AuthenticodeSignature` (Valid, signer contains `LEANCODE, INC.`, countersigned timestamp); retries twice; supports
`SIGN_ROUTE=simple|signtool`. Tests: `node --test scripts/sign-windows.test.mjs` (fake smctl, runs in CI and preflight).
`scripts/verify-windows-signatures.ps1` is the release-time verifier.

## What broke and why (2026-09-30), and the decision
- Default `smctl sign` on Windows runs **`signtool` with the DigiCert KSP**. The DigiCert action in `simple-signing-mode: true`
  installs only `smctl` (no KSP, no certificate sync, `Signtool: Mapped: No`, and `signtool` is not on PATH on `windows-latest`),
  so the first run printed "signCommand command ... FAILED", **exited 0**, and left the file unsigned. Credentials were fine
  (`smctl healthcheck`: Connected, Can sign: Yes, keypair ONLINE).
- **Two-route smoke test result (run 36768597470, green):** both routes sign an **exe, a DLL and an MSI**; the verifier shows
  `CN="LEANCODE, INC."` (EV, Delaware, serial 7376809) with a DigiCert timestamp on all six.
  - `simple` (`smctl sign --simple`): no signtool, no KSP, about 3 s for three files. **Chosen and the script default**;
    `release.yml` also sets `SIGN_ROUTE: simple` on both signing steps.
  - `signtool`: action in normal mode + `signtool` on PATH + `smksp_registrar register` + `smctl windows certsync`; kept as the
    fallback route in the smoke matrix (`SIGN_ROUTE=signtool`).
- Bugs in our own tooling found on the way (all fixed, each became a test or check): (1) `powershell -Command "<script>" <file>`
  never gives the script `$args`: pass the path in `$env:SIGN_TARGET_FILE` and use `-EncodedCommand`; (2) a Windows PowerShell 5.1
  child of a PowerShell 7 step inherits `PSModulePath` and cannot load `Microsoft.PowerShell.Security`: drop it from the
  child's environment; (3) `pwsh script.ps1 -Path a, b, c` from a step splits the list into positional arguments
  (`A positional parameter cannot be found`): call the verifier in-process (`./scripts/verify-windows-signatures.ps1 -Path ...`),
  which `release.yml`'s final "verify every shipped file" step also needed; (4) building a test MSI with PowerShell 7 COM calls
  fails with `DISP_E_TYPEMISMATCH`: use Windows PowerShell 5.1 (`scripts/ci/make-test-msi.ps1`, tested on a real box).

## Sequence to the first signed build
1. `scripts/preflight.sh`, CI green on the exact commit, GPU validation of its installer ([gpu-validation](../gpu-validation/SKILL.md)).
2. Smoke test green for the chosen route on exe, DLL and MSI (one reviewer approval per dispatch).
3. Add an early verify step after "Sign bundled native libraries" in `release.yml`, keep the final verify step.
4. Tag `v0.1.0-rc1` (measure the optimized build time), verify the artifacts, install the **signed release-profile**
   installer on a fresh box and rerun the lifecycle and Chicago checks.

## Troubleshooting
- `exec: "signtool": executable file not found` + exit 0: see above; use `smctl healthcheck` and `smctl keypair ls` (secrets are
  masked in Actions logs) to separate a credentials problem from a tooling problem.
- Authorization error: designated signer changed, or token/client certificate expired. Signature valid but no timestamp:
  the release job fails on purpose.
- Never re-run a release with a step disabled; fix and re-tag.
