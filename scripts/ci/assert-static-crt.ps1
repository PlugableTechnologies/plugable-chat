# Fail if the built exe still imports the dynamic Visual C++ *C* runtime. +crt-static
# (.cargo/config.toml) removes vcruntime140*.dll; msvcp140*.dll (the C++ standard library) is
# still imported because a native dependency (ONNX Runtime / DirectML glue) is built /MD, so the
# exe cannot start without the VC++ Redistributable. The installer hook (src-tauri/windows/hooks.nsh)
# is therefore the real defence; this check only stops the C runtime from creeping back in and
# reports the remaining C++ dependency so it stays visible.
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
$bad = @($deps | Where-Object { $_ -match '^(vcruntime140|concrt140|vcomp140)' })
$cpp = @($deps | Where-Object { $_ -match '^msvcp140' })
if ($bad.Count -gt 0) { throw "dynamic VC++ runtime still imported: $($bad -join ', ')" }
Write-Host "static CRT confirmed: no vcruntime imports"
if ($cpp.Count -gt 0) { Write-Host "NOTE: still needs the VC++ Redistributable for: $($cpp -join ', ') (installed by the NSIS hook)" }
