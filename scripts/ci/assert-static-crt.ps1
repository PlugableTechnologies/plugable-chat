# Fail if the built exe still imports the dynamic Visual C++ runtime. With +crt-static
# (.cargo/config.toml) plugable-chat.exe must not need vcruntime140*.dll / msvcp140*.dll, or the
# "app starts without VC++ and shows its own card" guarantee is gone.
#
#   pwsh scripts/ci/assert-static-crt.ps1 -Exe target/debug/plugable-chat.exe
param([Parameter(Mandatory)][string]$Exe)
$ErrorActionPreference = "Stop"
$exeFull = (Resolve-Path $Exe).Path
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath
$dumpbin = (Get-ChildItem "$vs\VC\Tools\MSVC" -Recurse -Filter dumpbin.exe |
    Where-Object { $_.FullName -match 'Hostx64\\x64' } | Select-Object -First 1).FullName
if (-not $dumpbin) { throw "dumpbin.exe not found (Visual Studio C++ tools missing)" }
$deps = & $dumpbin /dependents $exeFull | ForEach-Object { if ($_ -match '^\s+(\S+\.dll)\s*$') { $Matches[1] } }
Write-Host "imports: $($deps -join ', ')"
$bad = @($deps | Where-Object { $_ -match '^(vcruntime140|msvcp140|concrt140|vcomp140)' })
if ($bad.Count -gt 0) { throw "dynamic VC++ runtime still imported: $($bad -join ', ')" }
Write-Host "static CRT confirmed: no vcruntime/msvcp imports"
