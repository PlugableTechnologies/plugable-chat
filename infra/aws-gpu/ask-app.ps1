# Run the INSTALLED app on the box with the Chicago crimes demo database attached, ask it one
# question through the app's own launch switches, wait for the answer, take a screenshot.
# Runs on the box (fetched by commit hash). Uses only the app's documented environment variables
# (CLI parity), so there are no argument-quoting problems.
#
#   ask-app.ps1 -Question "How many crimes are there?" -Model "Phi-4-mini-instruct-cuda-gpu:5" -PutUrl <presigned S3 PUT>
param(
    [Parameter(Mandatory)][string]$Question,
    [Parameter(Mandatory)][string]$Model,
    [Parameter(Mandatory)][string]$PutUrl,
    [int]$WaitSeconds = 150,
    [string]$AppDir = "C:\gpu\app"
)
$ErrorActionPreference = "Continue"
$work = "C:\gpu"

Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
Start-Sleep 3

# Launch script runs in the logged-on desktop session (the app needs a desktop).
$q = $Question.Replace("'", "''")
Set-Content "$work\launch-ask.ps1" @"
`$env:PLUGABLE_MODEL = '$Model'
`$env:PLUGABLE_INITIAL_PROMPT = '$q'
`$env:PLUGABLE_ENABLE_DEMO_DB = 'true'
`$env:PLUGABLE_ALWAYS_ON_TABLES = 'embedded-demo::main.chicago_crimes'
Start-Process $AppDir\plugable-chat.exe -WorkingDirectory $AppDir
"@
Set-Content "$work\snap-ask.vbs" "CreateObject(`"WScript.Shell`").Run `"$work\ffmpeg.exe -y -hide_banner -f gdigrab -i desktop -frames:v 1 $work\ask.png`", 0, True"

function InSession($script, $name) {
    schtasks /create /tn $name /tr $script /sc once /st 00:00 /ru Administrator /it /rl highest /f | Out-Null
    schtasks /run /tn $name | Out-Null
}

Remove-Item "$work\ask.png", "$work\ask_smi.csv" -ErrorAction SilentlyContinue
Set-Content "$work\smi-ask.ps1" "while(`$true){ (nvidia-smi --query-gpu=timestamp,utilization.gpu,memory.used --format=csv,noheader) | Add-Content $work\ask_smi.csv; Start-Sleep -Seconds 2 }"
$smi = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList "-NoProfile", "-File", "$work\smi-ask.ps1"

$sw = [Diagnostics.Stopwatch]::StartNew()
InSession "powershell.exe -NoProfile -ExecutionPolicy Bypass -File $work\launch-ask.ps1" "ask-launch"
Start-Sleep -Seconds $WaitSeconds
InSession "wscript.exe $work\snap-ask.vbs" "ask-snap"
Start-Sleep 8
curl.exe -s -S -X PUT -T "$work\ask.png" $PutUrl
"screenshot upload exit=$LASTEXITCODE after $([int]$sw.Elapsed.TotalSeconds) s"

Stop-Process -Id $smi.Id -Force -ErrorAction SilentlyContinue
Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
if (Test-Path "$work\ask_smi.csv") {
    $rows = Get-Content "$work\ask_smi.csv"
    $u = $rows | ForEach-Object { [int](($_ -split ", ")[1] -replace "[^0-9]", "") }
    $m = $rows | ForEach-Object { [int](($_ -split ", ")[2] -replace "[^0-9]", "") }
    "gpu peak util=$(($u | Measure-Object -Maximum).Maximum)% peak mem=$(($m | Measure-Object -Maximum).Maximum) MiB"
}
