<#
Fetch the pinned Foundry Local Core WinML build and stage its native files for the SDK crate's
FOUNDRY_NATIVE_OVERRIDE_DIR hook.

Why: foundry-local-sdk 1.2.3 downloads Core 1.2.3, which answers `download_and_register_eps` with
"Unknown EP bootstrapper name(s): ." whenever its own provider list repeats a name (WebGpu twice),
so no GPU provider ever registers. Microsoft fixed that in Core 1.2.4 (microsoft/Foundry-Local
issue 792) but the Rust crate has no 1.2.4 release. Core 1.2.4 has the same ONNX Runtime (1.26.0),
GenAI (0.14.1) and Windows ML (2.1.1) dependencies as the 1.2.3 crate pins, so dropping its Core
and Windows ML DLLs in is the smallest change. The crate's build script copies override files
first and then skips downloading Core.

The version comes from src-tauri/foundry-core-version.txt. The override folder name carries the
version so a cached build never keeps an older Core.

Usage: pwsh scripts/ci/fetch-foundry-core.ps1            (in CI: exports FOUNDRY_NATIVE_OVERRIDE_DIR)
#>
param(
    [string]$Version = "",
    [string]$Out = "",
    # Stage the files without pointing later build steps at them (used to fetch a second Core to compare).
    [switch]$NoExport
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $Version) { $Version = (Get-Content (Join-Path $root "src-tauri/foundry-core-version.txt") -Raw).Trim() }
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "bad Core version '$Version'" }
if (-not $Out) { $Out = Join-Path $root ".foundry-core-override-$Version" }

$rid = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq "Arm64") { "win-arm64" } else { "win-x64" }
$id = "microsoft.ai.foundry.local.core.winml"
$url = "https://api.nuget.org/v3-flatcontainer/$id/$Version/$id.$Version.nupkg"

$work = Join-Path ([IO.Path]::GetTempPath()) "foundry-core-$Version-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $work | Out-Null
$pkg = Join-Path $work "core.nupkg"
Invoke-WebRequest $url -OutFile $pkg
Expand-Archive $pkg (Join-Path $work "x")

$native = Join-Path $work "x/runtimes/$rid/native"
if (-not (Test-Path $native)) { throw "package has no runtimes/$rid/native" }
New-Item -ItemType Directory -Force -Path $Out | Out-Null
Get-ChildItem $native -Filter *.dll | Copy-Item -Destination $Out -Force

$core = Join-Path $Out "Microsoft.AI.Foundry.Local.Core.dll"
if (-not (Test-Path $core)) { throw "Core DLL missing after extraction" }
$found = (Get-Item $core).VersionInfo.FileVersion
Write-Host "staged Foundry Local Core $Version ($rid): $core (file version $found)"
Get-ChildItem $Out | ForEach-Object { Write-Host ("  {0}  {1:N0} bytes" -f $_.Name, $_.Length) }

if (-not $NoExport) {
    if ($env:GITHUB_ENV) { "FOUNDRY_NATIVE_OVERRIDE_DIR=$Out" >> $env:GITHUB_ENV }
    $env:FOUNDRY_NATIVE_OVERRIDE_DIR = $Out
}
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
