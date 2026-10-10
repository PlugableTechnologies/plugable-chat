# Verify what actually ships: install each installer silently and check the INSTALLED app exe
# (and the installers themselves) carry our signature, that the installer hook ran, that the
# bundled VC++ redistributable is the pinned one, and that running the app writes nothing under
# the install directory. target/release/plugable-chat.exe is not a shipped file (tauri signs
# patched copies that go into the installers), so it is not checked.
#
#   ./scripts/ci/verify-installers.ps1 -Nsis <setup.exe> [-Msi <file.msi>] [-AllowUnpinned] [-SkipFirstRun]
# Exit code is non-zero if any file is unsigned, an installer fails to install, or a hook/payload
# assertion fails. -Msi is optional: the MSI has no installer hook (see docs/clean-host-testing.md),
# so it only gets the signature and payload checks.
param(
    [string]$Msi = "",
    [Parameter(Mandatory)][string]$Nsis,
    [string]$Work = (Join-Path ([IO.Path]::GetTempPath()) "verify-installers"),
    [string]$PinFile = (Join-Path $PSScriptRoot "vcredist.pin.json"),
    [switch]$AllowUnpinned,      # lets a build without the reviewed hash through; never for a release
    [switch]$SkipFirstRun,       # skip the "running the app writes nothing to the install dir" check
    [int]$SmokeTimeoutSec = 1500
)
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$verifier = Join-Path $here "..\verify-windows-signatures.ps1"
Import-Module (Join-Path $here "CleanHost.psm1") -Force
New-Item -ItemType Directory -Force -Path $Work | Out-Null
$files = @((Resolve-Path $Nsis).Path)
$problems = New-Object System.Collections.ArrayList
function Fail($msg) { Write-Host "FAIL: $msg"; [void]$problems.Add($msg) }

# MSI (optional): per-machine install, check the installed exe, uninstall.
if ($Msi) {
    $msiPath = (Resolve-Path $Msi).Path
    $files += $msiPath
    $log = Join-Path $Work "msi-install.log"
    $p = Start-Process msiexec.exe -ArgumentList "/i `"$msiPath`" /qn /norestart /l*v `"$log`"" -Wait -PassThru
    if ($p.ExitCode -notin 0, 3010) { throw "MSI install failed with exit code $($p.ExitCode); see $log" }
    $msiExe = Get-ChildItem "$env:ProgramFiles\plugable-chat" -Filter plugable-chat.exe -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $msiExe) { throw "the MSI installed no plugable-chat.exe under $env:ProgramFiles" }
    # Copy it out before uninstalling (a copy keeps the signature), then check the copy.
    $copyMsi = Join-Path $Work "from-msi\plugable-chat.exe"
    New-Item -ItemType Directory -Force -Path (Split-Path $copyMsi) | Out-Null
    Copy-Item $msiExe.FullName $copyMsi -Force
    $files += $copyMsi
    # The AI runtime must ship with the app; without it the app cannot start and users are told to reinstall.
    $msiCore = Get-ChildItem $msiExe.Directory.FullName -Filter "Microsoft.AI.Foundry.Local.Core.dll" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $msiCore) { throw "the MSI install has no Microsoft.AI.Foundry.Local.Core.dll next to plugable-chat.exe" }
    $u = Start-Process msiexec.exe -ArgumentList "/x `"$msiPath`" /qn /norestart" -Wait -PassThru
    Write-Host "MSI uninstall exit code $($u.ExitCode)"
}

# NSIS: silent install into a scratch folder, check the installed exe, uninstall.
$nsisDir = Join-Path $Work "nsis-install"
$installLog = Join-Path $env:TEMP "plugable-chat-install.log"
Remove-Item -LiteralPath $installLog -Force -ErrorAction SilentlyContinue
$p = Start-Process $files[0] -ArgumentList "/S", "/D=$nsisDir" -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "NSIS install failed with exit code $($p.ExitCode)" }
$nsisExe = Join-Path $nsisDir "plugable-chat.exe"
if (-not (Test-Path $nsisExe)) { throw "the NSIS installer installed no plugable-chat.exe in $nsisDir" }
$files += $nsisExe
$nsisCore = Get-ChildItem $nsisDir -Filter "Microsoft.AI.Foundry.Local.Core.dll" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $nsisCore) { throw "the NSIS install has no Microsoft.AI.Foundry.Local.Core.dll in $nsisDir" }

# --- the installer hook (src-tauri/windows/hooks.nsh) ran ---------------------------------------
$regKey = "HKLM:\SOFTWARE\Plugable\plugable-chat\Installer"
$reg = Get-ItemProperty -Path $regKey -ErrorAction SilentlyContinue
if (-not $reg) { Fail "hook registry key $regKey is missing: the installer hook did not run" }
else {
    if ($reg.HookVersion -lt 1) { Fail "HookVersion is '$($reg.HookVersion)'" }
    $okStatuses = "present", "installed", "installed-reboot", "newer-present"
    if ($reg.VCRedistStatus -notin $okStatuses) { Fail "VCRedistStatus is '$($reg.VCRedistStatus)' (exit code $($reg.VCRedistExitCode)); expected one of $($okStatuses -join ', ')" }
    if ($reg.WebView2Status -ne "present") { Fail "WebView2Status is '$($reg.WebView2Status)'" }
    Write-Host "hook registry: VCRedist=$($reg.VCRedistStatus) exit=$($reg.VCRedistExitCode) Bld=$($reg.VCRedistBld) WebView2=$($reg.WebView2Status) $($reg.WebView2Version)"
}
$vc = Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" -ErrorAction SilentlyContinue
if (-not $vc -or $vc.Installed -ne 1) { Fail "VC++ x64 runtime is not registered (Installed=1) after the install" }
elseif ($vc.Bld -lt (Get-NshMinBld (Join-Path $here "..\..\src-tauri\windows\hooks.nsh"))) { Fail "VC++ build $($vc.Bld) is below the minimum in hooks.nsh" }

if (-not (Test-Path -LiteralPath $installLog)) { Fail "install log $installLog was not written" }
else {
    $logText = Get-Content -LiteralPath $installLog -Raw
    foreach ($marker in "install start", "vcredist:", "webview2:", "install end") {
        if ($logText -notmatch [regex]::Escape($marker)) { Fail "install log lacks '$marker'" }
    }
}

# --- the bundled redistributable is the pinned, Microsoft-signed one ---------------------------
$redist = Join-Path $nsisDir "redist\vc_redist.x64.exe"
$pinState = Test-VcRedistPin -PinFile $PinFile
if (-not (Test-Path -LiteralPath $redist)) { Fail "payload $redist is not in the install (the build did not stage it; see fetch-vcredist.ps1)" }
elseif (-not $pinState.Ok) {
    if ($AllowUnpinned) { Write-Host "WARNING: pin not set ($($pinState.Reason)); payload hash NOT verified" } else { Fail "cannot verify the payload hash: $($pinState.Reason)" }
}
else {
    $have = (Get-FileHash -LiteralPath $redist -Algorithm SHA256).Hash
    if ($have -ine $pinState.Pin.sha256) { Fail "vc_redist.x64.exe payload hash $have does not match the pin $($pinState.Pin.sha256)" }
    $sig = Get-AuthenticodeSignature -LiteralPath $redist
    if ($sig.Status -ne "Valid" -or -not (Test-SignerSubject -Subject $sig.SignerCertificate.Subject -Contains $pinState.Pin.signerContains)) {
        Fail "vc_redist.x64.exe signature is '$($sig.Status)' / '$($sig.SignerCertificate.Subject)'"
    }
    else { Write-Host "payload ok: pinned hash, signed by $($sig.SignerCertificate.Subject)" }
}

# --- running the app writes nothing under the install directory ---------------------------------
if (-not $SkipFirstRun) {
    $state = Join-Path $Work "firstrun-state.jsonl"
    Remove-Item -LiteralPath $state -Force -ErrorAction SilentlyContinue
    $before = Get-DirSnapshot $nsisDir
    $env:PLUGABLE_CHAT_TEST_STATE = $state
    $env:PLUGABLE_CHAT_SMOKE_TIMEOUT_SECS = [string]([math]::Max(30, $SmokeTimeoutSec - 30))
    $proc = Start-Process $nsisExe -ArgumentList "--smoke" -WorkingDirectory $nsisDir -PassThru -WindowStyle Hidden
    if (-not $proc.WaitForExit($SmokeTimeoutSec * 1000)) { Write-Host "smoke run still going after $SmokeTimeoutSec s; stopping it"; Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item Env:\PLUGABLE_CHAT_TEST_STATE, Env:\PLUGABLE_CHAT_SMOKE_TIMEOUT_SECS -ErrorAction SilentlyContinue
    Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$nsisDir*" } | Stop-Process -Force -ErrorAction SilentlyContinue
    $after = Get-DirSnapshot $nsisDir
    $changes = Compare-DirSnapshot $before $after
    if ($changes.Count -gt 0) { Fail "running the app changed the install directory: $(($changes | Select-Object -First 10) -join '; ')" }
    else { Write-Host "first run left the install directory untouched ($($after.Count) files)" }
    if (-not (Test-Path -LiteralPath $state)) { Write-Host "NOTE: no state file written by --smoke (build predates the test hook)" }
}

& $verifier -Path $files
$code = $LASTEXITCODE
if (Test-Path "$nsisDir\uninstall.exe") { Start-Process "$nsisDir\uninstall.exe" -ArgumentList "/S" -Wait }
# After uninstall the hook key must be gone (and user data untouched; the data dir is not ours to remove).
if (Get-ItemProperty -Path $regKey -ErrorAction SilentlyContinue) { Fail "uninstall left $regKey behind" }
if ($problems.Count -gt 0) { Write-Host "`n$($problems.Count) installer assertion(s) failed"; exit 1 }
exit $code
