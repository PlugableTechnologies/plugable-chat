# Verify what actually ships: install each installer silently and check the INSTALLED app exe
# (and the installers themselves) carry our signature. target/release/plugable-chat.exe is not a
# shipped file (tauri signs patched copies that go into the installers), so it is not checked.
#
#   ./scripts/ci/verify-installers.ps1 -Msi <file.msi> -Nsis <setup.exe>
# Exit code is non-zero if any file is unsigned or an installer fails to install.
param(
    [Parameter(Mandatory)][string]$Msi,
    [Parameter(Mandatory)][string]$Nsis,
    [string]$Work = (Join-Path ([IO.Path]::GetTempPath()) "verify-installers")
)
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$verifier = Join-Path $here "..\verify-windows-signatures.ps1"
New-Item -ItemType Directory -Force -Path $Work | Out-Null
$files = @((Resolve-Path $Msi).Path, (Resolve-Path $Nsis).Path)

# MSI: per-machine install, check the installed exe, uninstall.
$log = Join-Path $Work "msi-install.log"
$p = Start-Process msiexec.exe -ArgumentList "/i `"$($files[0])`" /qn /norestart /l*v `"$log`"" -Wait -PassThru
if ($p.ExitCode -notin 0, 3010) { throw "MSI install failed with exit code $($p.ExitCode); see $log" }
$msiExe = Get-ChildItem "$env:ProgramFiles\plugable-chat" -Filter plugable-chat.exe -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $msiExe) { throw "the MSI installed no plugable-chat.exe under $env:ProgramFiles" }
# Copy it out before uninstalling (a copy keeps the signature), then check the copy.
$copyMsi = Join-Path $Work "from-msi\plugable-chat.exe"
New-Item -ItemType Directory -Force -Path (Split-Path $copyMsi) | Out-Null
Copy-Item $msiExe.FullName $copyMsi -Force
$files += $copyMsi
$u = Start-Process msiexec.exe -ArgumentList "/x `"$($files[0])`" /qn /norestart" -Wait -PassThru
Write-Host "MSI uninstall exit code $($u.ExitCode)"

# NSIS: silent install into a scratch folder, check the installed exe, uninstall.
$nsisDir = Join-Path $Work "nsis-install"
$p = Start-Process $files[1] -ArgumentList "/S", "/D=$nsisDir" -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "NSIS install failed with exit code $($p.ExitCode)" }
$nsisExe = Join-Path $nsisDir "plugable-chat.exe"
if (-not (Test-Path $nsisExe)) { throw "the NSIS installer installed no plugable-chat.exe in $nsisDir" }
$files += $nsisExe

& $verifier -Path $files
$code = $LASTEXITCODE
if (Test-Path "$nsisDir\uninstall.exe") { Start-Process "$nsisDir\uninstall.exe" -ArgumentList "/S" -Wait }
exit $code
