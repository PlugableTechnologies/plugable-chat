# Provision a fresh Windows Server 2022 GPU box (run on the box, as SYSTEM/Administrator).
# Everything here was proven by hand on a g4dn.xlarge on 2026-09-29; the timings
# are what that run measured. Idempotent: safe to run twice.
#
#   powershell -File bootstrap-box.ps1 -DeadlineMinutes 90
#
# After it returns the caller must REBOOT once: auto-logon only takes effect at
# the next boot, and it is what creates the desktop session the screen capture
# needs. The failsafe below survives that reboot.
param([int]$DeadlineMinutes = 90)

$ErrorActionPreference = "Stop"
# Lesson: Invoke-WebRequest with the progress bar on is 50x slower (a 748 MB driver
# took 15+ minutes and never finished; with it off it takes 13 seconds).
$ProgressPreference = "SilentlyContinue"

$work = "C:\gpu"
New-Item -ItemType Directory -Force $work | Out-Null
function Step($msg) { "[{0:HH:mm:ss}] {1}" -f (Get-Date), $msg }

# 1. Failsafe FIRST, so a broken run can never leave the box (and its bill) running.
# Lesson: `shutdown /t N` is a one-shot timer and is silently lost by any reboot
# (which auto-logon requires). Instead store a deadline and check it every 5 minutes
# from a scheduled task, which survives reboots. The instance is launched with
# shutdown behaviour = terminate, so this powers it off AND deletes it.
Step "arming failsafe: shutdown after $DeadlineMinutes minutes"
$deadlineFile = Join-Path $work "deadline.txt"
if (-not (Test-Path $deadlineFile)) {
    (Get-Date).ToUniversalTime().AddMinutes($DeadlineMinutes).ToString("o") | Set-Content $deadlineFile
}
$check = @'
$d = [datetime]::Parse((Get-Content C:\gpu\deadline.txt)).ToUniversalTime()
if ((Get-Date).ToUniversalTime() -gt $d) { shutdown /s /f /t 0 /c "gpu box failsafe deadline reached" }
'@
Set-Content (Join-Path $work "failsafe.ps1") $check
$action  = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoProfile -ExecutionPolicy Bypass -File C:\gpu\failsafe.ps1"
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes 5)
Register-ScheduledTask -TaskName "gpu-failsafe" -Action $action -Trigger $trigger `
    -Principal (New-ScheduledTaskPrincipal -UserId "SYSTEM" -RunLevel Highest) -Force | Out-Null

# 2. NVIDIA driver AWS publishes for G instances. Free, from AWS's public bucket.
# Measured: 13 s download, 110 s install, no reboot needed for the T4 to appear.
if (-not (Get-Command nvidia-smi -ErrorAction SilentlyContinue) -and -not (Test-Path C:\Windows\System32\nvidia-smi.exe)) {
    Step "installing NVIDIA driver"
    $name = "596.86__grid_win10_win11_server2022_server2025_dch_64bit_international_aws_swl.exe"
    Invoke-WebRequest -UseBasicParsing "https://ec2-windows-nvidia-drivers.s3.amazonaws.com/latest/$name" -OutFile "$work\driver.exe"
    $p = Start-Process "$work\driver.exe" -ArgumentList "-s","-noreboot","-noeula","-clean" -Wait -PassThru
    if ($p.ExitCode -ne 0) { throw "driver installer exit code $($p.ExitCode)" }
}

# 3. WebView2 runtime. Windows Server does not include it and the Tauri app needs it.
# Measured: 67 s.
if (-not (Get-ItemProperty "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" -ErrorAction SilentlyContinue)) {
    Step "installing WebView2 runtime"
    Invoke-WebRequest -UseBasicParsing "https://go.microsoft.com/fwlink/p/?LinkId=2124703" -OutFile "$work\wv2.exe"
    Start-Process "$work\wv2.exe" -ArgumentList "/silent","/install" -Wait | Out-Null
}

# 4. Node (for Playwright). Measured: ~10 s.
if (-not (Get-Command node -ErrorAction SilentlyContinue) -and -not (Test-Path "C:\Program Files\nodejs\node.exe")) {
    Step "installing Node LTS"
    $v = (Invoke-RestMethod https://nodejs.org/dist/index.json | Where-Object { $_.lts } | Select-Object -First 1).version
    Invoke-WebRequest -UseBasicParsing "https://nodejs.org/dist/$v/node-$v-x64.msi" -OutFile "$work\node.msi"
    Start-Process msiexec -ArgumentList "/i","$work\node.msi","/qn","/norestart" -Wait | Out-Null
}

# 5. ffmpeg for screen recording.
# Lesson: use gdigrab. ddagrab (desktop duplication) failed to open on this driver
# ("Failed to configure output pad"); gdigrab captured video and stills fine.
if (-not (Test-Path "$work\ffmpeg.exe")) {
    Step "installing ffmpeg"
    Invoke-WebRequest -UseBasicParsing "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip" -OutFile "$work\ffmpeg.zip"
    Expand-Archive "$work\ffmpeg.zip" "$work\ffmpeg-unzipped" -Force
    Copy-Item (Get-ChildItem "$work\ffmpeg-unzipped" -Recurse -Filter ffmpeg.exe | Select-Object -First 1).FullName "$work\ffmpeg.exe"
}

# 6. Auto-logon with a random one-run password, so a real desktop session exists.
# Lesson: SSM and user-data run in session 0 with no desktop. Screen capture and
# the app's window need session 1, which only exists once someone is logged on.
# The password is generated here, written only to the registry of a box that is
# destroyed at the end of the run, and never leaves it.
Step "configuring auto-logon"
$pw = -join ((48..57) + (65..90) + (97..122) | Get-Random -Count 24 | ForEach-Object { [char]$_ })
net user Administrator $pw | Out-Null
$k = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon"
Set-ItemProperty $k AutoAdminLogon "1"
Set-ItemProperty $k DefaultUserName "Administrator"
Set-ItemProperty $k DefaultDomainName $env:COMPUTERNAME
Set-ItemProperty $k DefaultPassword $pw
powercfg /change monitor-timeout-ac 0
powercfg /change standby-timeout-ac 0

Step "bootstrap complete; REBOOT NOW for auto-logon to take effect"
