<#
Fails unless the Foundry Local Core that was bundled is byte-for-byte the pinned one.

The override in scripts/ci/fetch-foundry-core.ps1 is silent when it does nothing: if the crate's build
script ignored the folder, the build would still pass and ship Core 1.2.3 (the version that refuses to
register GPU providers). The file version inside the DLL says 1.2.0 for both, so compare hashes.

  pwsh scripts/ci/assert-foundry-core.ps1 -Bundled target/debug/foundry-libs/Microsoft.AI.Foundry.Local.Core.dll
#>
param([Parameter(Mandatory)][string]$Bundled)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$version = (Get-Content (Join-Path $root "src-tauri/foundry-core-version.txt") -Raw).Trim()
$staged = Join-Path $root ".foundry-core-override-$version/Microsoft.AI.Foundry.Local.Core.dll"
if (-not (Test-Path $staged)) { throw "the pinned Core $version was not staged at $staged" }
if (-not (Test-Path $Bundled)) { throw "no bundled Core at $Bundled" }
$a = (Get-FileHash $staged -Algorithm SHA256).Hash
$b = (Get-FileHash $Bundled -Algorithm SHA256).Hash
Write-Host "pinned  $version  $a`nbundled          $b  ($Bundled)"
if ($a -ne $b) { throw "the bundled Core is not the pinned $version (the override did not take effect)" }
Write-Host "bundled Core is the pinned $version"
