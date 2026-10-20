# Runs the GPU registration experiments (plugable-chat --ep-probe) on this machine and prints one
# verdict per hypothesis about why GPU acceleration was declined. Installs and changes nothing
# except that the experiments themselves may download GPU components, as the app would.
#
#   powershell -ExecutionPolicy Bypass -File ep-hypotheses.ps1
#   powershell -ExecutionPolicy Bypass -File ep-hypotheses.ps1 -OtherCore C:\path\to\folder-with-another-Core
#
# Send back the .zip it prints at the end (report, app log, engine log, host facts). Close Plugable
# Chat first: a running copy holds the engine files and its own GPU setup would overlap.
#
# Written for the rc13 failures on Intel Arc B60 (David) and RTX 4080 (Bob): the engine answered
# "Unknown EP bootstrapper name(s): ." in about 12 ms. See docs/gpu-validation.md for the hypotheses.

param(
    [string]$InstallDir = (Join-Path $env:ProgramFiles "plugable-chat"),
    [string]$OtherCore = "",
    [string]$OutDir = (Join-Path ([Environment]::GetFolderPath("Desktop")) ("plugable-chat-ep-hypotheses-" + (Get-Date -Format "yyyy-MM-dd-HHmmss")))
)

$ErrorActionPreference = "Continue"
$ProgressPreference = "SilentlyContinue"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Try-Get([scriptblock]$block) { try { & $block } catch { $null } }

# ---- host facts (every probe may fail; a failure leaves the field empty) ------------------------

$os = Try-Get { Get-CimInstance Win32_OperatingSystem }
$buildText = if ($os) { "$($os.Caption) build $($os.BuildNumber).$((Try-Get { (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').UBR }))" } else { $null }

$libs = Join-Path $InstallDir "foundry-libs"
$libFiles = @(Try-Get { Get-ChildItem -LiteralPath $libs -File | ForEach-Object { $_.Name } })

# Every onnxruntime.dll the app could load: its own, Windows' inbox copy, and a user-set one.
$ortPaths = @()
$ortPaths += Join-Path $libs "onnxruntime.dll"
$ortPaths += Join-Path $InstallDir "onnxruntime.dll"
$ortPaths += Join-Path $env:SystemRoot "System32\onnxruntime.dll"
if ($env:ORT_DYLIB_PATH) { $ortPaths += $env:ORT_DYLIB_PATH }
$ortFound = @($ortPaths | Select-Object -Unique | Where-Object { Test-Path -LiteralPath $_ } | ForEach-Object {
    "{0} {1}" -f $_, (Get-Item -LiteralPath $_).VersionInfo.FileVersion
})

$epPackages = @(Try-Get { Get-AppxPackage -AllUsers "*.EP.*" | ForEach-Object { "{0} {1}" -f $_.Name, $_.Version } })

$wuPaused = Try-Get {
    $ux = Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\WindowsUpdate\UX\Settings" -ErrorAction Stop
    $expiry = $ux.PauseUpdatesExpiryTime
    [bool]($expiry -and ([datetime]$expiry) -gt (Get-Date))
}
$rebootPending = Try-Get {
    (Test-Path "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired") -or
    (Test-Path "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending")
}
$cliVersion = Try-Get { (& foundry --version 2>$null | Select-Object -First 1) }

$facts = [ordered]@{
    os_build               = $buildText
    foundry_libs_files     = $libFiles
    onnxruntime_on_disk    = $ortFound
    ep_packages            = $epPackages
    windows_update_blocked = $wuPaused
    pending_reboot         = $rebootPending
    foundry_cli_version    = $cliVersion
}
$factsFile = Join-Path $OutDir "host-facts.json"
$facts | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $factsFile -Encoding UTF8

# ---- the experiments -----------------------------------------------------------------------------

$exe = Join-Path $InstallDir "plugable-chat.exe"
if (-not (Test-Path -LiteralPath $exe)) { throw "plugable-chat.exe not found in $InstallDir (use -InstallDir)" }
if (Get-Process -Name "plugable-chat" -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }) {
    throw "Plugable Chat is running; close it first."
}

$probeArgs = @("--ep-probe", "`"$OutDir`"", "--ep-probe-facts", "`"$factsFile`"")
if ($OtherCore) { $probeArgs += @("--ep-probe-other-core", "`"$OtherCore`"") }
Write-Host "Running the experiments. Downloading GPU components can take several minutes..."
# The app is a Windows-subsystem program: only Start-Process -Wait really waits for it.
$run = Start-Process -FilePath $exe -ArgumentList $probeArgs -Wait -PassThru -WindowStyle Hidden
$exit = $run.ExitCode

$report = Get-ChildItem -LiteralPath $OutDir -Filter "ep-probe-*.json" | Sort-Object LastWriteTime | Select-Object -Last 1
if (-not $report) { throw "The probe wrote no report (exit code $exit). Send the folder $OutDir anyway." }

$doc = Get-Content -LiteralPath $report.FullName -Raw | ConvertFrom-Json
Write-Host ""
Write-Host "Results ($($doc.version)):"
foreach ($v in $doc.verdicts) {
    Write-Host ("  {0}  {1,-12}  {2}" -f $v.id, $v.state.ToUpper(), $v.name)
    Write-Host ("        {0}" -f $v.reason)
}

# ---- the logs support needs -----------------------------------------------------------------------

$appLogs = Join-Path $env:APPDATA "plugable-chat\logs"
if (Test-Path -LiteralPath $appLogs) {
    Get-ChildItem -LiteralPath $appLogs -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 3 | Copy-Item -Destination $OutDir
    $foundryLogs = Join-Path $appLogs "foundry"
    if (Test-Path -LiteralPath $foundryLogs) {
        Get-ChildItem -LiteralPath $foundryLogs -File | Sort-Object LastWriteTime | Select-Object -Last 2 | Copy-Item -Destination $OutDir
    }
}
$zip = "$OutDir.zip"
Compress-Archive -Path (Join-Path $OutDir "*") -DestinationPath $zip -Force
Write-Host ""
Write-Host "Send this file: $zip"
