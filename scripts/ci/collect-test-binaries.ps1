# Compile the Rust tests without running them and copy the test executables into
# an output folder, so the GPU test box can run them without a compiler.
#
#   pwsh scripts/ci/collect-test-binaries.ps1 -Out gpu-tests
param([string]$Out = "gpu-tests")

$ErrorActionPreference = "Stop"
New-Item -ItemType Directory -Force -Path $Out | Out-Null

$lines = cargo test --locked --manifest-path src-tauri/Cargo.toml --lib --no-run --message-format=json
if ($LASTEXITCODE -ne 0) { throw "cargo test --no-run failed" }

$copied = 0
foreach ($line in $lines) {
    if ($line -notmatch '^\{') { continue }
    $msg = $line | ConvertFrom-Json
    if ($msg.reason -eq "compiler-artifact" -and $msg.profile.test -and $msg.executable) {
        Copy-Item $msg.executable -Destination $Out
        Write-Host "collected $($msg.executable)"
        $copied++
    }
}
if ($copied -eq 0) { throw "no test executables found" }
