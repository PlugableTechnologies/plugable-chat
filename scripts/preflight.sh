#!/usr/bin/env bash
# Everything that can be checked on a developer machine before a push, so the paid box and the
# 20-minute GitHub Windows build only see problems that need them. See docs/testing-ladder.md.
#
#   scripts/preflight.sh          # all checks
#   scripts/preflight.sh --quick  # skip the Rust unit tests (about 1 minute faster)
#
# Exits non-zero if any check fails. Optional tools (actionlint, zizmor, cargo-deny, osv-scanner)
# are used when installed and reported as skipped otherwise.
set -uo pipefail
cd "$(dirname "$0")/.."
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-1.94.1}"
QUICK=0; [ "${1:-}" = "--quick" ] && QUICK=1

FAILED=()
SKIPPED=()
step() { printf '\n=== %s\n' "$1"; }
run() { # run <label> <command...>
  local label="$1"; shift
  step "$label"
  if "$@"; then echo "PASS  $label"; else echo "FAIL  $label"; FAILED+=("$label"); fi
}
optional() { # optional <tool> <label> <command...>
  local tool="$1" label="$2"; shift 2
  if command -v "$tool" >/dev/null 2>&1; then run "$label" "$@"; else
    step "$label"; echo "SKIP  $label ($tool not installed)"; SKIPPED+=("$label ($tool)"); fi
}

# 1. Versions and dependencies that broke a Windows build only at its very last step.
run "tauri crate and npm packages agree" node scripts/ci/check-tauri-versions.mjs
run "version-check tests (LF and CRLF lockfiles)" node --test scripts/ci/check-tauri-versions.test.mjs
run "npm audit (0 vulnerabilities)" npm audit
run "cargo lockfile is consistent" bash -c "cargo metadata --locked --format-version 1 --manifest-path src-tauri/Cargo.toml --no-deps >/dev/null"

# 2. Frontend.
# `npm run build` also regenerates the tracked icons; run its two real steps so the tree stays clean.
run "TypeScript check" npx tsc
run "frontend production build" npx vite build
run "frontend unit tests" npm run test:unit

# 3. Signing script (fake smctl, no Windows or DigiCert needed).
run "signing script tests" node --test scripts/sign-windows.test.mjs

# 4. Rust: everything compiles, including tests that are ignored by default.
run "cargo check (all targets, locked)" cargo check --locked --all-targets --manifest-path src-tauri/Cargo.toml
if [ "$QUICK" = 0 ]; then
  run "Rust unit tests" cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
fi

# 5. Workflows and scripts.
run "workflow YAML parses" python3 - <<'PY'
import glob, sys, yaml
bad = 0
for p in sorted(glob.glob('.github/workflows/*.yml')):
    try: yaml.safe_load(open(p))
    except Exception as e: print(p, e); bad = 1
sys.exit(bad)
PY
run "verifier takes several files (argument binding)" pwsh -NoProfile -Command '
  $out = (& ./scripts/verify-windows-signatures.ps1 -Path a.exe, b.dll, c.msi 2>&1 | Out-String)
  if ($out -match "positional parameter") { Write-Host $out; exit 1 }
  exit 0'
optional actionlint "actionlint (workflow semantics)" actionlint
optional zizmor "zizmor (workflow security)" zizmor --offline .github/workflows
run "PowerShell scripts parse" pwsh -NoProfile -Command '
  $bad = 0
  Get-ChildItem infra, scripts -Recurse -Filter *.ps1 | ForEach-Object {
    $e = $null; [void][System.Management.Automation.Language.Parser]::ParseFile($_.FullName, [ref]$null, [ref]$e)
    if ($e.Count) { Write-Host "$($_.FullName): $($e[0].Message)"; $bad = 1 }
  }
  exit $bad'
run "shell scripts parse" bash -c 'for f in infra/aws-gpu/*.sh scripts/*.sh; do bash -n "$f" || exit 1; done'

# 6. Supply chain (only when the tools exist).
optional cargo-deny "cargo-deny advisories and licenses" cargo deny --manifest-path src-tauri/Cargo.toml check
optional osv-scanner "osv-scanner (same scan as the weekly job)" osv-scanner scan --recursive .

step "Summary"
[ ${#SKIPPED[@]} -gt 0 ] && printf 'skipped: %s\n' "${SKIPPED[@]}"
if [ ${#FAILED[@]} -gt 0 ]; then printf 'FAILED: %s\n' "${FAILED[@]}"; exit 1; fi
echo "preflight passed. Next rung: docs/testing-ladder.md (EC2 box), then push."
