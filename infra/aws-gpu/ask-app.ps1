# Run the INSTALLED app on the box with the Chicago crimes demo database attached, ask it one
# question through the app's own launch switches, wait for the app to FINISH answering (not a
# fixed sleep), take a screenshot, and print timings. Runs on the box (fetched by commit hash).
# Uses only the app's documented environment variables (CLI parity), so there are no
# argument-quoting problems.
#
#   ask-app.ps1 -Question "How many crimes are in the chicago_crimes table?" -Model phi-4-mini-instruct -PutUrl <presigned S3 PUT>
#   ask-app.ps1 -Warm -Model phi-4-mini-instruct        # launch once, wait until ready, no question
#
# -WaitSeconds is a ceiling, not a delay: the script returns as soon as the app logs that the
# chat finished (or, with -Warm, that the model is pre-warmed). Lessons behind this:
#  * first-run provider registration took 82 s to 681 s in the interactive profile, so any fixed
#    wait either wastes box time or screenshots "Connecting to Foundry...";
#  * killing the caller (ask.sh) does not stop this script, so it removes any other copy of itself
#    (and the app) at start, and ask.sh sends the same cleanup when it exits.
param(
    [string]$Question = "",
    [Parameter(Mandatory)][string]$Model,
    [string]$PutUrl = "",
    [int]$WaitSeconds = 900,
    [string]$AppDir = "C:\gpu\app",
    [switch]$Warm,
    [switch]$CleanupOnly
)
$ErrorActionPreference = "Continue"
$work = "C:\gpu"
$appOut = "$work\ask-app.out"
$appErr = "$work\ask-app.err"

function Stop-AskProcesses {
    # Any other ask-app.ps1 / helper of ours, then the app itself.
    Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.ProcessId -ne $PID -and $_.CommandLine -match 'ask-app\.ps1|smi-ask\.ps1|launch-ask\.ps1' } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
    Start-Sleep 2
}

# Wait until $Pattern appears in the app's stdout at or after byte offset $From, or the deadline
# passes. Returns the matching line, or $null on timeout. Also returns early if the app exited.
function Wait-ForAppLog([string]$Pattern, [datetime]$Deadline, [int]$PollSeconds = 3) {
    while ((Get-Date) -lt $Deadline) {
        if (Test-Path $appOut) {
            $hit = Select-String -Path $appOut -Pattern $Pattern -ErrorAction SilentlyContinue | Select-Object -First 1
            if ($hit) { return $hit.Line }
        }
        if (-not (Get-Process plugable-chat -ErrorAction SilentlyContinue)) { return $null }
        Start-Sleep $PollSeconds
    }
    return $null
}

function Get-LogSeconds([string]$Pattern, [string]$Capture) {
    # First regex capture group of the first matching line, or "?".
    if (-not (Test-Path $appOut)) { return "?" }
    $hit = Select-String -Path $appOut -Pattern $Pattern -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($hit -and $hit.Line -match $Capture) { return $Matches[1] }
    return "?"
}

Stop-AskProcesses
if ($CleanupOnly) { "cleanup done"; return }

if (-not $Warm -and (-not $Question -or -not $PutUrl)) { throw "-Question and -PutUrl are required unless -Warm" }

# Launch script runs in the logged-on desktop session (the app needs a desktop). The app's
# stdout goes to a file so the wait below can watch it.
$q = $Question.Replace("'", "''")
$promptLine = if ($Warm) { "" } else { "`$env:PLUGABLE_INITIAL_PROMPT = '$q'" }
Set-Content "$work\launch-ask.ps1" @"
`$env:PLUGABLE_MODEL = '$Model'
$promptLine
`$env:PLUGABLE_ENABLE_DEMO_DB = 'true'
`$env:PLUGABLE_ALWAYS_ON_TABLES = 'embedded-demo::main.chicago_crimes'
Remove-Item '$appOut', '$appErr' -ErrorAction SilentlyContinue
Start-Process '$AppDir\plugable-chat.exe' -WorkingDirectory '$AppDir' -RedirectStandardOutput '$appOut' -RedirectStandardError '$appErr'
"@
Set-Content "$work\snap-ask.vbs" "CreateObject(`"WScript.Shell`").Run `"$work\ffmpeg.exe -y -hide_banner -f gdigrab -i desktop -frames:v 1 $work\ask.png`", 0, True"

function InSession($script, $name) {
    schtasks /create /tn $name /tr $script /sc once /st 00:00 /ru Administrator /it /rl highest /f | Out-Null
    schtasks /run /tn $name | Out-Null
}

Remove-Item "$work\ask.png", "$work\ask_smi.csv", $appOut, $appErr -ErrorAction SilentlyContinue
Set-Content "$work\smi-ask.ps1" "while(`$true){ (nvidia-smi --query-gpu=timestamp,utilization.gpu,memory.used --format=csv,noheader) | Add-Content $work\ask_smi.csv; Start-Sleep -Seconds 2 }"
$smi = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList "-NoProfile", "-File", "$work\smi-ask.ps1"

$sw = [Diagnostics.Stopwatch]::StartNew()
$deadline = (Get-Date).AddSeconds($WaitSeconds)
InSession "powershell.exe -NoProfile -ExecutionPolicy Bypass -File $work\launch-ask.ps1" "ask-launch"

if ($Warm) {
    $done = Wait-ForAppLog "pre-warmed in|Launch model override not applied" $deadline
    $outcome = if ($done) { "warm-ready" } else { "warm-timeout" }
} else {
    $done = Wait-ForAppLog "chat-finished event emitted successfully" $deadline
    $outcome = if ($done) { "chat-finished" } else { "timeout-or-app-exited" }
}
$waited = [int]$sw.Elapsed.TotalSeconds

if (-not $Warm) {
    Start-Sleep 6   # let the UI render the final answer before the screenshot
    InSession "wscript.exe $work\snap-ask.vbs" "ask-snap"
    Start-Sleep 8
    curl.exe -s -S -X PUT -T "$work\ask.png" $PutUrl
    "screenshot upload exit=$LASTEXITCODE"
}

# One summary line a person (or ask.sh) can read: what happened and where the time went.
$ep = Get-LogSeconds "EpRegistrationSummary" "seconds: ([0-9.]+)"
$warmed = Get-LogSeconds "pre-warmed in" "pre-warmed in ([0-9.]+)"
$toolCalls = 0
if (Test-Path $appOut) {
    $toolCalls = (Select-String -Path $appOut -Pattern 'Sending: .*"method":"tools/call"' -ErrorAction SilentlyContinue | Measure-Object).Count
}
$override = if ((Test-Path $appOut) -and (Select-String -Path $appOut -Pattern "Launch model override not applied" -Quiet)) { "override-NOT-applied" } else { "override-ok" }
"outcome=$outcome waited=${waited}s ep_registration=${ep}s model_prewarm=${warmed}s mcp_tool_calls=$toolCalls $override"

Stop-Process -Id $smi.Id -Force -ErrorAction SilentlyContinue
Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
if (Test-Path "$work\ask_smi.csv") {
    $rows = Get-Content "$work\ask_smi.csv"
    $u = $rows | ForEach-Object { [int](($_ -split ", ")[1] -replace "[^0-9]", "") }
    $m = $rows | ForEach-Object { [int](($_ -split ", ")[2] -replace "[^0-9]", "") }
    "gpu peak util=$(($u | Measure-Object -Maximum).Maximum)% peak mem=$(($m | Measure-Object -Maximum).Maximum) MiB"
}
if ($outcome -notin @("chat-finished", "warm-ready")) { exit 1 }
