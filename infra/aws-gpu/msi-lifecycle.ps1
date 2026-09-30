# MSI lifecycle test, run on the box as SYSTEM (like SSM, Intune and SCCM install things):
# silent install, registration, launch, reinstall over a running copy, repair after damage,
# silent uninstall, reinstall. One PASS/FAIL line per check; exits non-zero if any failed.
# With -ExpectSigned the MSI and the installed exe must carry the company signature and a timestamp.
#
#   msi-lifecycle.ps1 -Msi C:\gpu\msi\plugable-chat_0.0.0_x64_en-US.msi
#   msi-lifecycle.ps1 -Msi ... -ExpectSigned
param(
    [Parameter(Mandatory)][string]$Msi,
    [string]$ProductName = "plugable-chat",
    [switch]$ExpectSigned,
    [string]$ExpectedSubject = "LEANCODE, INC."
)
$ErrorActionPreference = "Continue"
$script:failed = 0
function Check($name, [bool]$ok, $detail = "") {
    if (-not $ok) { $script:failed++ }
    "{0}  {1} {2}" -f ($(if ($ok) { "PASS" } else { "FAIL" })), $name, $detail
}
function Msiexec($arguments) {
    $p = Start-Process msiexec.exe -ArgumentList $arguments -Wait -PassThru
    $p.ExitCode
}
function Entries() {
    Get-ItemProperty "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*",
                     "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*" -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -eq $ProductName }
}
function Shortcuts() {
    @(Get-ChildItem "$env:ProgramData\Microsoft\Windows\Start Menu" -Recurse -Filter "$ProductName*.lnk" -ErrorAction SilentlyContinue)
}
function InstallDir() {
    $e = @(Entries) | Select-Object -First 1
    if ($e -and $e.InstallLocation) { return $e.InstallLocation.Trim('"').TrimEnd('\') }
    "C:\Program Files\$ProductName"
}
function Signature($file) {
    $s = Get-AuthenticodeSignature -LiteralPath $file
    [pscustomobject]@{ Status = $s.Status.ToString(); Subject = if ($s.SignerCertificate) { $s.SignerCertificate.Subject } else { "" }; Timestamp = [bool]$s.TimeStamperCertificate }
}
function CheckSigned($file) {
    $s = Signature $file
    Check "signed by us: $([IO.Path]::GetFileName($file))" ($s.Status -eq "Valid" -and $s.Subject -like "*$ExpectedSubject*" -and $s.Timestamp) "(status $($s.Status), timestamp $($s.Timestamp))"
}

"running as: $([Security.Principal.WindowsIdentity]::GetCurrent().Name)   msi: $Msi"
$log = "C:\gpu\msi-install.log"; New-Item -ItemType Directory -Force (Split-Path $log) | Out-Null

if ($ExpectSigned) { CheckSigned $Msi }

# Start from nothing.
Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
if (@(Entries).Count -gt 0) { [void](Msiexec "/x `"$Msi`" /qn /norestart") }

# 1. fresh silent install
$sw = [Diagnostics.Stopwatch]::StartNew()
$code = Msiexec "/i `"$Msi`" /qn /norestart /l*v `"$log`""
Check "fresh install exit code 0 or 3010" ($code -in 0, 3010) "(exit $code, $([int]$sw.Elapsed.TotalSeconds) s; log $log)"
$dir = InstallDir
Check "installs under Program Files" ($dir -like "$env:ProgramFiles*") "($dir)"
foreach ($n in @("plugable-chat.exe", "foundry-libs", "test-data\demo.db")) { Check "installed: $n" (Test-Path "$dir\$n") }
Check "registered in HKLM (visible to IT inventory)" (@(Entries).Count -eq 1) "(found $(@(Entries).Count))"
Check "all-users Start Menu shortcut" ($(Shortcuts).Count -ge 1)
Check "nothing landed in the SYSTEM profile" (-not (Test-Path "C:\Windows\System32\config\systemprofile\AppData\Local\$ProductName\plugable-chat.exe"))
if ($ExpectSigned) { CheckSigned "$dir\plugable-chat.exe" }

# 2. the installed app starts and stays up (session 1 desktop, as the auto-logon Administrator)
Set-Content "C:\gpu\msi-launch.cmd" "@echo off`r`nstart `"`" `"$dir\plugable-chat.exe`""
schtasks /create /tn msi-launch /tr "C:\gpu\msi-launch.cmd" /sc once /st 00:00 /ru Administrator /it /rl highest /f | Out-Null
schtasks /run /tn msi-launch | Out-Null
Start-Sleep 25
Check "app running 25 s after launch" ($null -ne (Get-Process plugable-chat -ErrorAction SilentlyContinue))

# 3. install the same package again over the running copy (must not fail or duplicate)
$code = Msiexec "/i `"$Msi`" /qn /norestart"
Check "reinstall over a running copy exit code 0 or 3010" ($code -in 0, 3010) "(exit $code)"
Check "still exactly one registration" (@(Entries).Count -eq 1) "(found $(@(Entries).Count))"
Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
Start-Sleep 5

# 4. repair: delete the exe and a bundled native library, then 'msiexec /fa' (reinstall all files)
$dir = InstallDir
Remove-Item "$dir\plugable-chat.exe" -Force -ErrorAction SilentlyContinue
$lib = Get-ChildItem "$dir\foundry-libs" -Filter "*.dll" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($lib) { Remove-Item $lib.FullName -Force -ErrorAction SilentlyContinue }
$code = Msiexec "/fa `"$Msi`" /qn /norestart"
Check "repair exit code 0 or 3010" ($code -in 0, 3010) "(exit $code)"
Check "repair restored the exe" (Test-Path "$dir\plugable-chat.exe")
Check "repair restored a native library" ($lib -and (Test-Path $lib.FullName)) "($($lib.Name))"

# 5. silent uninstall
$code = Msiexec "/x `"$Msi`" /qn /norestart"
Start-Sleep 3
Check "uninstall exit code 0 or 3010" ($code -in 0, 3010) "(exit $code)"
Check "uninstall removed the exe" (-not (Test-Path "$dir\plugable-chat.exe"))
Check "uninstall removed the registration" (@(Entries).Count -eq 0)
Check "uninstall removed the shortcut" ($(Shortcuts).Count -eq 0)

# 6. reinstall after uninstall
$code = Msiexec "/i `"$Msi`" /qn /norestart"
Check "reinstall after uninstall exit code 0 or 3010" ($code -in 0, 3010) "(exit $code)"
Check "reinstall complete" ((Test-Path "$(InstallDir)\plugable-chat.exe") -and (Test-Path "$(InstallDir)\foundry-libs"))

schtasks /delete /tn msi-launch /f 2>&1 | Out-Null
"msi lifecycle: $script:failed failed"
exit $script:failed
