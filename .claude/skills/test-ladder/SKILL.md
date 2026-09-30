---
name: test-ladder
description: Run and order plugable-chat checks so few errors reach GitHub. Use before pushing, before starting a paid EC2 GPU box, before cutting a release tag, or when a CI or signing run failed and you need to decide which lower rung should have caught it.
---

# Test ladder for plugable-chat

Order: **1. local (free) -> 2. EC2 GPU box (paid) -> 3. GitHub (compile gate, signing, release)**.
Full detail and the table of past escapes: [docs/testing-ladder.md](../../../docs/testing-ladder.md).

1. Before every push run `scripts/preflight.sh` (about 1 minute). Fix everything it reports; do not push on red.
   If the change touches app behaviour, also run the app on this Mac against Foundry with the `PLUGABLE_*`
   environment variables and read the log (back up and restore the app's `config.json`).
2. Only when preflight is green and the CI installer for the exact commit exists, use the EC2 box
   ([docs/gpu-validation.md](../../../docs/gpu-validation.md)): at most 3 GPU runs per task, always run
   `infra/aws-gpu/teardown.sh`, read the screenshots, never send signing credentials to the box.
3. GitHub last. Do not push to `main` while a CI run you need is in flight. Signing runs need a reviewer other
   than the person who started them; never approve your own signing run.
4. Ask-first files: `.github/workflows/release.yml`, `scripts/sign-windows.mjs`,
   `scripts/verify-windows-signatures.ps1`, `.github/CODEOWNERS`, `src-tauri/tauri*.conf.json`.
5. When something reaches a higher rung that a lower rung could have caught, add the check to the lower rung
   and record it under "Escapes so far" in `docs/testing-ladder.md`.
