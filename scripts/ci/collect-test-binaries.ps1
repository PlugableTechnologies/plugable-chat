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

# The test programs load ONNX Runtime. Windows ships an older onnxruntime.dll in
# System32, and the program finds that one first and dies at start-up with
# STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139). The build downloads the right DLLs;
# put them next to the test programs, both here (for `cargo test`, which runs
# from target/debug/deps) and in the output folder (for the GPU test box).
$dlls = Get-ChildItem -Path target -Recurse -Include "onnxruntime*.dll", "DirectML*.dll" -File -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch '\\deps\\' } |
    Sort-Object FullName -Unique
if (-not $dlls) { throw "no onnxruntime DLL found under target/" }
foreach ($dll in $dlls) {
    Copy-Item $dll.FullName -Destination "target/debug/deps" -Force
    Copy-Item $dll.FullName -Destination $Out -Force
    Write-Host "runtime DLL $($dll.FullName)"
}
