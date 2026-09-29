# Installer lifecycle test, run on the box: fresh install, launch, upgrade over a running copy,
# repair after damage, uninstall, reinstall. Prints one PASS/FAIL line per check and exits non-zero
# if any failed. Uses the same silent switches an IT department would.
#
#   installer-lifecycle.ps1 -Installer C:\gpu\inst2\plugable-chat_0.0.0_x64-setup.exe
param(
    [Parameter(Mandatory)][string]$Installer,
    [string]$Dir = "C:\gpu\life"
)
$ErrorActionPreference = "Continue"
$script:failed = 0
function Check($name, [bool]$ok, $detail = "") {
    if (-not $ok) { $script:failed++ }
    "{0}  {1} {2}" -f ($(if ($ok) { "PASS" } else { "FAIL" })), $name, $detail
}
function Install() {
    $p = Start-Process $Installer -ArgumentList "/S", "/D=$Dir" -Wait -PassThru
    $p.ExitCode
}
function UninstallEntry() {
    Get-ItemProperty HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*, HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\* -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -eq "plugable-chat" -and $_.InstallLocation -like "*$Dir*" }
}
function LaunchInSession() {
    schtasks /create /tn life-launch /tr "$Dir\plugable-chat.exe" /sc once /st 00:00 /ru Administrator /it /rl highest /f | Out-Null
    schtasks /run /tn life-launch | Out-Null
}
$needed = @("plugable-chat.exe", "uninstall.exe", "foundry-libs", "test-data\demo.db")

Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
if (Test-Path "$Dir\uninstall.exe") { Start-Process "$Dir\uninstall.exe" -ArgumentList "/S" -Wait }

# 1. fresh install
$sw = [Diagnostics.Stopwatch]::StartNew()
$code = Install
Check "fresh install exit code 0" ($code -eq 0) "(exit $code, $([int]$sw.Elapsed.TotalSeconds) s)"
foreach ($n in $needed) { Check "installed: $n" (Test-Path "$Dir\$n") }
Check "uninstall entry registered" ($null -ne (UninstallEntry))
Check "start-menu shortcut" ([bool](Get-ChildItem "$env:ProgramData\Microsoft\Windows\Start Menu", "C:\Users\*\AppData\Roaming\Microsoft\Windows\Start Menu", "C:\Windows\System32\config\systemprofile\AppData\Roaming\Microsoft\Windows\Start Menu" -Recurse -Filter "plugable-chat*.lnk" -ErrorAction SilentlyContinue))

# 2. the installed app starts and stays up
LaunchInSession
Start-Sleep 25
$proc = Get-Process plugable-chat -ErrorAction SilentlyContinue
Check "app running 25 s after launch" ($null -ne $proc)

# 3. upgrade over the top while the app is running
$before = (Get-Item "$Dir\plugable-chat.exe").LastWriteTime
$code = Install
Start-Sleep 5
Check "install over a running copy exit code 0" ($code -eq 0) "(exit $code)"
Check "exe present after upgrade" (Test-Path "$Dir\plugable-chat.exe")
Stop-Process -Name plugable-chat, msedgewebview2 -Force -ErrorAction SilentlyContinue
Start-Sleep 5

# 4. repair: delete the exe and a bundled native library, reinstall, expect both back
Remove-Item "$Dir\plugable-chat.exe" -Force -ErrorAction SilentlyContinue
$lib = Get-ChildItem "$Dir\foundry-libs" -Filter "*.dll" | Select-Object -First 1
if ($lib) { Remove-Item $lib.FullName -Force }
$code = Install
Check "repair install exit code 0" ($code -eq 0) "(exit $code)"
Check "repair restored exe" (Test-Path "$Dir\plugable-chat.exe")
Check "repair restored native library" ($lib -and (Test-Path $lib.FullName)) "($($lib.Name))"

# 5. uninstall
Start-Process "$Dir\uninstall.exe" -ArgumentList "/S" -Wait
Start-Sleep 5
Check "uninstall removed the exe" (-not (Test-Path "$Dir\plugable-chat.exe"))
Check "uninstall removed the registry entry" ($null -eq (UninstallEntry))

# 6. reinstall after uninstall
$code = Install
Check "reinstall exit code 0" ($code -eq 0) "(exit $code)"
Check "reinstall complete" ((Test-Path "$Dir\plugable-chat.exe") -and (Test-Path "$Dir\foundry-libs"))

schtasks /delete /tn life-launch /f 2>&1 | Out-Null
"lifecycle: $script:failed failed"
exit $script:failed
