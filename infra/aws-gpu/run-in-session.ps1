# Run a script in the logged-on desktop session (session 1) and wait for it.
# SSM and user-data run in session 0, which has no desktop; a scheduled task with
# /it (interactive) and the logged-on user is how to reach session 1.
#
#   run-in-session.ps1 -Script C:\gpu\capture.ps1 -Args "-Seconds 30" -TimeoutSec 120
param([Parameter(Mandatory)][string]$Script, [string]$Args = "", [int]$TimeoutSec = 300)
$name = "gpu-session-" + [guid]::NewGuid().ToString("N").Substring(0, 8)
$cmd = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$Script`" $Args"
# /st 00:00 in the past only produces a harmless warning; the task is run by hand below.
schtasks /create /tn $name /tr $cmd /sc once /st 00:00 /ru Administrator /it /rl highest /f | Out-Null
schtasks /run /tn $name | Out-Null
$deadline = (Get-Date).AddSeconds($TimeoutSec)
do { Start-Sleep 2; $state = (Get-ScheduledTask -TaskName $name).State } while ($state -eq "Running" -and (Get-Date) -lt $deadline)
schtasks /delete /tn $name /f | Out-Null
if ($state -eq "Running") { throw "session task timed out after $TimeoutSec s" }
