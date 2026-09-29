# Release signing

Pushing a version tag builds, signs and publishes plugable-chat. This page says what
happens, who can approve it, and what to do when it breaks.

## What a release does

1. Push a tag from `main`: `v1.2.3`, or `v1.2.3-rc1` for a prerelease.
2. `verify-tag` checks the tag commit is on `main`. A tag on any other branch fails here,
   before anyone is asked to approve.
3. `build-windows` waits for approval in the `release-signing` GitHub Environment. One
   approval from any listed reviewer (Bernie, Pranav, Derek) releases it. After that it
   builds, signs the executables and installers (`.exe`, `.msi`) with the company EV
   certificate, and checks every file is signed with a timestamp.
4. `build-linux` builds `.deb`, `.rpm` and `.AppImage`. Linux has no Authenticode, so each
   file gets a Sigstore signature (`*.sigstore.json`), plus `SHA256SUMS`. No approval or
   stored key is needed.
5. `publish` creates the GitHub Release with everything above.

Both builds also record a GitHub build-provenance attestation.

## Where the certificate lives

The private key never leaves DigiCert KeyLocker. The workflow asks KeyLocker to sign
(`smctl sign`) using a service user's API token and client certificate. Those, plus the
signing key alias and host, are stored in the `release-signing` Environment:

| Name | Kind |
|---|---|
| `SM_API_KEY` | secret |
| `SM_CLIENT_CERT_FILE_B64` | secret (base64 of the .p12) |
| `SM_CLIENT_CERT_PASSWORD` | secret |
| `SM_KEYPAIR_ALIAS` | secret |
| `SM_HOST` | secret |

Nobody sets these by hand. They are written by
`skills/core/scripts/ingest_digicert_signing_secrets.py` in the brix-cli repo, which also
keeps the backup copy in GCP Secret Manager. To rotate them, re-run that script.

The workflow exposes them only to the steps that sign, after `npm ci` and the dependency
compile, so package install scripts and dependency build scripts never see them.

## Checking a download

Windows (PowerShell): `pwsh scripts/verify-windows-signatures.ps1 -Path .\plugable-chat_1.2.3_x64-setup.exe`
Expect status Valid, signer `LEANCODE, INC.`, and a timestamp.

Linux:

```bash
sha256sum -c SHA256SUMS
cosign verify-blob --bundle plugable-chat_1.2.3_amd64.deb.sigstore.json \
  --certificate-identity-regexp 'https://github.com/PlugableTechnologies/plugable-chat/.github/workflows/release.yml@.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  plugable-chat_1.2.3_amd64.deb
gh attestation verify plugable-chat_1.2.3_amd64.deb --repo PlugableTechnologies/plugable-chat
```

## Trying it out

Run the **Signing smoke test** workflow (Actions tab, manual). It signs a trivial program
and verifies it, using the same approval and credentials as a release. Do this after any
credential change, and before the first real tag.

## Renewal

The certificate itself expires **2027-08-11**, and the EV organisation validation
(DigiCert org 2168851) runs until 2027-09-11. Start renewal 60 days before the certificate
expires (by mid-June 2027). The API token and client certificate should be rotated yearly.

## If signing stops working

- `smctl` reports an authorization error: the designated signer on the certificate may
  have changed (DigiCert allows only one), or the API token or client certificate expired.
  Ask a KeyLocker lead to check the certificate's signer in DigiCert ONE.
- Signature verifies but has no timestamp: the release job fails on purpose. Check the
  `smctl` version the DigiCert action installed.
- Never re-run a release job to "just get it out" with a step disabled. Fix and re-tag.

## Known issues found by the first CI and GPU-box runs (2026-09-29)

- **The release build needs the same fixes CI got:** Rust is pinned to 1.94.1 (`ethnum` does not compile on
  the newest stable), `protoc` is installed, and Linux needs `libprotobuf-dev` and `libgtk-3-dev`. These are
  already in `release.yml`.
- **The optimized release build may not fit in the 90-minute job limit.** A release-profile build of the app
  (fat LTO, one codegen unit, lancedb and datafusion) ran 77 minutes in CI without finishing. The `release.yml`
  Windows job has `timeout-minutes: 90`; raise it (GitHub allows up to 6 hours on hosted runners) or relax LTO
  (`lto = "thin"`, more codegen units) before cutting a release. This has not been changed.
- **Validate before signing.** Run the GPU validation ([gpu-validation.md](gpu-validation.md)) on the tagged
  commit first: the installed app, not just the unit tests, is what found the bugs that mattered.
