# Run the clean-host fault matrix (scripts/ci/faults.ps1) ON the box. The same script the
# windows-latest workflow runs, so a scenario that is green on CI and red here (or the reverse)
# is a real difference between hosts (GPU, driver, Server image), not between harnesses.
#
#   clean-host-matrix.ps1 -Sha <commit> -Installer <url-or-path> [-Scenario gpu,driver-absent]
#                         [-OldInstaller <url-or-path>] [-VcRedist <url-or-path>] [-ExpectFail]
#                         [-PutUrl <presigned S3 PUT url for the results zip>]
#   clean-host-matrix.ps1 -ArgsFile C:\gpu\matrix-args.json     # same, from JSON (run-matrix.sh: no quoting)
#
# The scripts are fetched by COMMIT HASH from GitHub (the box has no repo checkout), like ask.sh.
# Run it in the desktop session (run-in-session.ps1) or as SYSTEM through SSM; run-matrix.sh does
# the former. Results: C:\gpu\clean-host-out\ (JUnit XML + per-scenario state files and logs).
param(
    [string]$Sha = "",
    [string]$Installer = "",
    [string[]]$Scenario = @("gpu"),
    [string]$OldInstaller = "",
    [string]$VcRedist = "",
    [string]$PutUrl = "",
    [switch]$ExpectFail,
    [string]$ArgsFile = ""
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
if ($ArgsFile) {
    $j = Get-Content $ArgsFile -Raw | ConvertFrom-Json
    $Sha = $j.Sha; $Installer = $j.Installer; $Scenario = @($j.Scenario)
    foreach ($n in "OldInstaller", "VcRedist", "PutUrl") { if ($j.$n) { Set-Variable $n $j.$n } }
    if ($j.ExpectFail) { $ExpectFail = $true }
    Start-Transcript "C:\gpu\matrix-transcript.txt" -Force | Out-Null
}
if (-not $Sha -or -not $Installer) { throw "-Sha and -Installer are required" }
$ci = "C:\gpu\ci"
$out = "C:\gpu\clean-host-out"
New-Item -ItemType Directory -Force $ci | Out-Null
Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue

function Get-Local($src, $name) {
    if (-not $src) { return "" }
    if ($src -notmatch '^https?://') { return (Resolve-Path $src).Path }
    $dest = Join-Path $ci $name
    for ($i = 1; $i -le 4; $i++) {
        try { Invoke-WebRequest -UseBasicParsing $src -OutFile $dest; return $dest }
        catch { if ($i -eq 4) { throw }; Start-Sleep (5 * $i) }
    }
}

$raw = "https://raw.githubusercontent.com/PlugableTechnologies/plugable-chat/$Sha/scripts/ci"
foreach ($f in "faults.ps1", "CleanHost.psm1") { Invoke-WebRequest -UseBasicParsing "$raw/$f" -OutFile (Join-Path $ci $f) }

$args2 = @{
    Installer = (Get-Local $Installer "setup.exe")
    Scenario = $Scenario
    OutDir = $out
}
if ($OldInstaller) { $args2.OldInstaller = Get-Local $OldInstaller "old-setup.exe" }
if ($VcRedist) { $args2.VcRedist = Get-Local $VcRedist "vc_redist.x64.exe"; $args2.AllowDestructive = $true }
if ($ExpectFail) { $args2.ExpectFail = $true }

$env:CLEAN_HOST_DISPOSABLE = "1"   # the box is destroyed after the run; faults.ps1 refuses without this
& (Join-Path $ci "faults.ps1") @args2
$code = $LASTEXITCODE

if ($PutUrl) {
    $zip = "C:\gpu\clean-host-out.zip"
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    Compress-Archive -Path "$out\*" -DestinationPath $zip
    Invoke-WebRequest -UseBasicParsing -Method Put -InFile $zip -Uri $PutUrl | Out-Null
    "uploaded results zip"
}
"matrix-exit=$code"
if ($ArgsFile) { Stop-Transcript | Out-Null }
exit $code
