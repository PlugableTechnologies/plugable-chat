# Installer lifecycle test, run on the box: fresh install, launch, upgrade over a running copy,
# repair after damage, uninstall, reinstall. Prints one PASS/FAIL line per check and exits non-zero
# if any failed. Uses the same silent switches an IT department would.
#
# The shipped install mode is perMachine (src-tauri/tauri.conf.json, bundle.windows.nsis.installMode),
# so by default this must be run AS SYSTEM (SSM runs as SYSTEM, like Intune and SCCM) and the installer
# gets NO /D=: it has to choose C:\Program Files\plugable-chat, an all-users Start Menu shortcut and
# an HKLM uninstall entry by itself, and must not leave anything in the SYSTEM profile.
#
#   installer-lifecycle.ps1 -Installer C:\gpu\inst2\plugable-chat_0.0.0_x64-setup.exe
#   installer-lifecycle.ps1 -Installer ... -Mode currentUser -Dir C:\gpu\life     # legacy per-user build
param(
    [Parameter(Mandatory)][string]$Installer,
    [ValidateSet("perMachine", "currentUser")][string]$Mode = "perMachine",
    [string]$Dir = "",
    [string]$StandardUser = "lifeuser"
)
$ErrorActionPreference = "Continue"
$script:failed = 0
function Check($name, [bool]$ok, $detail = "") {
    if (-not $ok) { $script:failed++ }
    "{0}  {1} {2}" -f ($(if ($ok) { "PASS" } else { "FAIL" })), $name, $detail
}

$perMachine = $Mode -eq "perMachine"
if (-not $Dir) { $Dir = if ($perMachine) { "$env:ProgramFiles\plugable-chat" } else { "C:\gpu\life" } }
# perMachine: the installer must pick the directory itself (this is what IT tools rely on).
$installArgs = if ($perMachine) { @("/S") } else { @("/S", "/D=$Dir") }
$systemProfileDir = "C:\Windows\System32\config\systemprofile\AppData\Local\plugable-chat"
$stdPassword = "Lc!" + [guid]::NewGuid().ToString("N").Substring(0, 16)

function Install() {
    $p = Start-Process $Installer -ArgumentList $installArgs -Wait -PassThru
    $p.ExitCode
}
function UninstallEntry($hive) {
    Get-ItemProperty "${hive}:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*" -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -eq "plugable-chat" }
}
function AnyUninstallEntry() { (UninstallEntry "HKLM"), (UninstallEntry "HKCU") | Where-Object { $_ } }
function StartMenuShortcuts($root) {
    Get-ChildItem $root -Recurse -Filter "plugable-chat*.lnk" -ErrorAction SilentlyContinue
}
function ProcessOwner($name) {
    $p = Get-CimInstance Win32_Process -Filter "Name='$name.exe'" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($p) { $o = Invoke-CimMethod -InputObject $p -MethodName GetOwner; "$($o.Domain)\$($o.User)" }
}
# Launch inside the auto-logon desktop session (session 1) as the admin user, like the earlier lifecycle test.
function LaunchInSession() {
    schtasks /create /tn life-launch /tr "$Dir\plugable-chat.exe" /sc once /st 00:00 /ru Administrator /it /rl highest /f | Out-Null
    schtasks /run /tn life-launch | Out-Null
}
# Launch as a non-admin local user. There is no desktop for that user, so this runs as a batch logon
# (session 0): it proves the exe starts and stays up under a limited token, not that a window renders.
function LaunchAsStandardUser() {
    schtasks /create /tn life-launch-std /tr "$Dir\plugable-chat.exe" /sc once /st 00:00 /ru $StandardUser /rp $stdPassword /rl limited /f | Out-Null
    schtasks /run /tn life-launch-std | Out-Null
}
$needed = @("plugable-chat.exe", "uninstall.exe", "foundry-libs", "test-data\demo.db")

# 0. who are we
$me = [Security.Principal.WindowsIdentity]::GetCurrent().Name
"running as: $me   mode: $Mode   install dir: $Dir   installer args: $($installArgs -join ' ')"
if ($perMachine) { Check "test is running as SYSTEM (the IT-tool case)" ($me -eq "NT AUTHORITY\SYSTEM") "($me)" }

Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
if (Test-Path "$Dir\uninstall.exe") { Start-Process "$Dir\uninstall.exe" -ArgumentList "/S" -Wait }
if (Test-Path "$systemProfileDir\uninstall.exe") { Start-Process "$systemProfileDir\uninstall.exe" -ArgumentList "/S" -Wait }

# The standard (non-admin) user for the launch test; created with a one-run random password.
if ($perMachine) {
    net user $StandardUser /delete 2>&1 | Out-Null
    net user $StandardUser $stdPassword /add /y | Out-Null
    Check "standard user created and is not an administrator" (-not ((net localgroup Administrators) -match "\b$StandardUser\b"))
}

# 1. fresh install
$sw = [Diagnostics.Stopwatch]::StartNew()
$code = Install
Check "fresh install exit code 0" ($code -eq 0) "(exit $code, $([int]$sw.Elapsed.TotalSeconds) s)"
foreach ($n in $needed) { Check "installed: $n" (Test-Path "$Dir\$n") }
$entry = AnyUninstallEntry
Check "uninstall entry registered" ($null -ne $entry)
if ($perMachine) {
    Check "installed under Program Files without /D=" ((Test-Path "$env:ProgramFiles\plugable-chat\plugable-chat.exe"))
    Check "uninstall entry is in HKLM (visible to every user and to IT inventory)" ($null -ne (UninstallEntry "HKLM"))
    Check "no uninstall entry in HKCU" ($null -eq (UninstallEntry "HKCU"))
    Check "nothing installed into the SYSTEM profile" (-not (Test-Path "$systemProfileDir\plugable-chat.exe"))
    Check "all-users start-menu shortcut" ([bool](StartMenuShortcuts "$env:ProgramData\Microsoft\Windows\Start Menu"))
} else {
    Check "start-menu shortcut" ([bool]((StartMenuShortcuts "$env:ProgramData\Microsoft\Windows\Start Menu"), (StartMenuShortcuts "C:\Users") , (StartMenuShortcuts "C:\Windows\System32\config\systemprofile\AppData\Roaming\Microsoft\Windows\Start Menu") | Where-Object { $_ }))
}

# 2. the installed app starts and stays up (admin, in the desktop session)
LaunchInSession
Start-Sleep 25
$proc = Get-Process plugable-chat -ErrorAction SilentlyContinue
Check "app running 25 s after launch (admin, desktop session)" ($null -ne $proc)
Stop-Process -Name plugable-chat, msedgewebview2 -Force -ErrorAction SilentlyContinue
Start-Sleep 3

# 2b. perMachine only: a standard user can run it, with an unelevated token, from a read-only location
if ($perMachine) {
    $acl = icacls $Dir
    Check "install dir is read-only for standard users (no Users:(M) or (F))" (-not ($acl -match "BUILTIN\\Users:.*\((M|F)\)"))
    Check "install dir is readable and executable by standard users" ([bool]($acl -match "BUILTIN\\Users:.*RX"))
    LaunchAsStandardUser
    Start-Sleep 25
    $owner = ProcessOwner "plugable-chat"
    Check "app running 25 s after launch as the standard user" ($null -ne (Get-Process plugable-chat -ErrorAction SilentlyContinue)) "(owner: $owner)"
    Check "app process belongs to the standard user, not SYSTEM/Administrator" ($owner -like "*\$StandardUser") "(owner: $owner)"
    Stop-Process -Name plugable-chat, msedgewebview2 -Force -ErrorAction SilentlyContinue
    Start-Sleep 3
}

# 3. upgrade over the top while the app is running
LaunchInSession
Start-Sleep 15
$code = Install
Start-Sleep 5
Check "install over a running copy exit code 0" ($code -eq 0) "(exit $code)"
Check "exe present after upgrade" (Test-Path "$Dir\plugable-chat.exe")
if ($perMachine) { Check "upgrade left exactly one uninstall entry" (@(AnyUninstallEntry).Count -eq 1) "(found $(@(AnyUninstallEntry).Count))" }
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

# 5. uninstall (silent, from SYSTEM, like IT tools do it)
Start-Process "$Dir\uninstall.exe" -ArgumentList "/S" -Wait
Start-Sleep 5
Check "uninstall removed the exe" (-not (Test-Path "$Dir\plugable-chat.exe"))
Check "uninstall removed the registry entry" ($null -eq (AnyUninstallEntry))
if ($perMachine) { Check "uninstall removed the all-users shortcut" (-not (StartMenuShortcuts "$env:ProgramData\Microsoft\Windows\Start Menu")) }

# 6. reinstall after uninstall
$code = Install
Check "reinstall exit code 0" ($code -eq 0) "(exit $code)"
Check "reinstall complete" ((Test-Path "$Dir\plugable-chat.exe") -and (Test-Path "$Dir\foundry-libs"))

schtasks /delete /tn life-launch /f 2>&1 | Out-Null
schtasks /delete /tn life-launch-std /f 2>&1 | Out-Null
if ($perMachine) { net user $StandardUser /delete 2>&1 | Out-Null }
"lifecycle: $script:failed failed"
exit $script:failed
