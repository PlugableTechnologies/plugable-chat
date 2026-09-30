---
name: release-signing
description: How Windows/Linux release signing works for plugable-chat (DigiCert KeyLocker EV cert, GitHub environment, smoke test, tag workflow) and what to do when signing fails. Use for any signing, release tag, smctl, or release.yml work.
---

# Release signing

Operator guide: `docs/release-signing.md` (rotation, renewal, credentials). Never handle secrets yourself: the DigiCert API
token, client `.p12` and password stay with the user (ingested by brix-cli `ingest_digicert_signing_secrets.py`).

## Setup facts
- EV certificate for **LEANCODE, INC.**, keypair alias in secret `SM_KEYPAIR_ALIAS`, expires 2027-08-11 (org validation to
  2027-09-11; client certificate to 2027-09-28). One designated signer per certificate.
- GitHub Environment **`release-signing`**: secrets `SM_HOST`, `SM_API_KEY`, `SM_CLIENT_CERT_FILE_B64`,
  `SM_CLIENT_CERT_PASSWORD`, `SM_KEYPAIR_ALIAS`. Required reviewers: `bernieplug` or `dnuzum`. Deployment branches: `main`
  and `v*` tags (the first smoke run failed because `main` was not allowed yet).
- **Never approve your own signing run.** The person or agent that dispatched it must not approve; the auto-mode safety
  check blocks it, and the two-person gate is the point.
- Tag ruleset "Restrict v* release tags". `release.yml` (verify-tag -> build-windows in the environment -> build-linux with
  cosign/attestations -> publish) is an ask-first file; Windows job timeout is 180 min.

## The signing wrapper (`scripts/sign-windows.mjs`, ask-first)
Used for bundled DLLs (`--skip-signed`) and as Tauri's `signCommand` for the exe, NSIS setup and MSI. It now:
captures `smctl` output; treats `FAILED`/`Error :` as failure **even when smctl exits 0**; verifies every file with
`Get-AuthenticodeSignature` (Valid, signer contains `LEANCODE, INC.`, countersigned timestamp); retries twice; supports
`SIGN_ROUTE=simple|signtool`. Tests: `node --test scripts/sign-windows.test.mjs` (fake smctl, runs in CI and preflight).
`scripts/verify-windows-signatures.ps1` is the release-time verifier.

## What broke and why (2026-09-30)
- Default `smctl sign` on Windows runs **`signtool` with the DigiCert KSP** (`/csp "DigiCert Signing Manager KSP"`). The DigiCert
  action in `simple-signing-mode: true` installs only `smctl`: no KSP, no certificate sync, `Signtool: Mapped: No`, and
  `signtool` is not on PATH on `windows-latest`. Result: `smctl` printed "signCommand command ... FAILED", **exited 0**, and
  the file stayed unsigned. Credentials were fine (`smctl healthcheck`: Connected, Can sign: Yes, keypair ONLINE).
- Two candidate routes, decided by the two-route smoke test (`signing-smoke-test.yml`, matrix `simple` / `signtool`, each
  signs an exe, a DLL and an MSI with the release script, then verifies): **simple** = `smctl sign --simple` (no signtool, no
  KSP, timestamp `timestamp.digicert.com`); **signtool** = action in normal mode plus `signtool` on PATH, `smksp_registrar`,
  `smctl windows certsync`. Use one route everywhere so the release path equals the tested path. Because the MSI must be
  kept, the winner must sign MSI too. **Result of the two-route run: not yet known** (run
  36681616841 awaiting approval); update this file and `release.yml` once it is.

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
