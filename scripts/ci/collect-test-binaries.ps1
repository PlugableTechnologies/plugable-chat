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

# The test programs import directml.dll directly. Windows ships an older
# DirectML.dll in System32 (1.15.5 on the CI runner) and the program finds that
# one first, then dies at start-up with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139).
# The ONNX Runtime download that the build uses includes a matching DirectML.dll,
# but it stays in that download cache rather than under target/. Put it, and the
# ONNX Runtime files the build produces, next to the test programs: here (for
# `cargo test`, which runs from target/debug/deps) and in the output folder (for
# the GPU test box).
$roots = @("target", (Join-Path $env:LOCALAPPDATA "ort.pyke.io"), (Join-Path $env:USERPROFILE ".cache")) |
    Where-Object { Test-Path $_ }
$found = Get-ChildItem -Path $roots -Recurse -Include "onnxruntime*.dll", "DirectML*.dll" -File -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch '[\\/]deps[\\/]' }
Write-Host "candidate runtime DLLs:"
$found | ForEach-Object { Write-Host "  $($_.FullName)  $($_.VersionInfo.FileVersion)" }

$directml = $found | Where-Object { $_.Name -ieq "DirectML.dll" } | Select-Object -First 1
if (-not $directml) {
    # Not under target/ (the ONNX Runtime download cache is outside it and is not
    # restored on a warm-cache run), so fetch Microsoft's official redistributable.
    # Pinned by version and SHA-256. Only bin/x64-win/DirectML.dll is used.
    $version = "1.15.4"
    $nupkgHash = "4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9"
    $nupkg = Join-Path $env:RUNNER_TEMP "directml.nupkg"
    $ProgressPreference = "SilentlyContinue"
    Invoke-WebRequest "https://www.nuget.org/api/v2/package/Microsoft.AI.DirectML/$version" -OutFile $nupkg
    $actual = (Get-FileHash $nupkg -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $nupkgHash) { throw "DirectML package hash mismatch: $actual" }
    $extract = Join-Path $env:RUNNER_TEMP "directml"
    Expand-Archive $nupkg $extract -Force
    $directml = Get-Item (Join-Path $extract "bin/x64-win/DirectML.dll")
    Write-Host "DirectML $version from NuGet: $($directml.FullName)"
}

# The Foundry Local SDK loads its native libraries (Microsoft.AI.Foundry.Local.Core.dll,
# onnxruntime, onnxruntime-genai) from beside the program. The build stages all of them
# in target/debug/foundry-libs; the GPU tests fail with "Could not locate native library
# 'Microsoft.AI.Foundry.Local.Core.dll'" without the whole set.
$foundryLibs = @(Get-ChildItem "target/debug/foundry-libs" -Filter *.dll -File -ErrorAction SilentlyContinue)
if (-not ($foundryLibs | Where-Object { $_.Name -eq "Microsoft.AI.Foundry.Local.Core.dll" })) {
    throw "Microsoft.AI.Foundry.Local.Core.dll is not in target/debug/foundry-libs"
}
$toCopy = @($directml) + $foundryLibs
foreach ($dll in $toCopy) {
    Copy-Item $dll.FullName -Destination "target/debug/deps" -Force
    Copy-Item $dll.FullName -Destination $Out -Force
    Write-Host "runtime DLL copied: $($dll.FullName)"
}
