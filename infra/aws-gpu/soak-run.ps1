# Soak: 20 prompts in ONE chat on the installed app, with the network dropped and restored on a timer and the app
# killed once part-way, then the chat store read back and judged (scripts/ci/check-chat-store-integrity.mjs).
# Run in the desktop session (run-in-session.ps1) on the GPU box; run-soak.sh does that and brings the result back.
#
#   soak-run.ps1 [-Model qwen3.5-4b] [-Prompts 20] [-KillAfter 12] [-FlapEverySec 120] [-FlapForSec 20]
#
# What a pass means: every prompt has a record or was the one the kill cut off; every failed turn is a clean
# "network_dropped" the harness caused, and no turn that began with the network up failed; no raw tool wrapper in any reply or saved assistant message;
# every tool call is paired with its result; the store still opens after the kill. The network drop is a firewall
# block on the app's own program plus PLUGABLE_CHAT_TEST_OFFLINE_FILE (the box's management channel stays up).
# One kill costs one "unclean death" in the app's crash counters (a notice, not yet a model block).
#
# Written on macOS; parsed there only. NOT RUN on a GPU box.
param(
    [string]$Model = "qwen3.5-4b",
    [string]$AppDir = "C:\Program Files\plugable-chat",
    [string]$FollowUps = "C:\gpu\chicago-followups.json",
    [string]$CiDir = "C:\gpu\ci",
    [string]$Work = "C:\gpu\soak",
    [int]$Prompts = 20,
    [int]$KillAfter = 12,
    [int]$FlapEverySec = 120,
    [int]$FlapForSec = 20,
    [int]$MaxMinutes = 150
)
$ErrorActionPreference = "Continue"
Import-Module (Join-Path $CiDir "CleanHost.psm1") -Force
$exe = Join-Path $AppDir "plugable-chat.exe"
$check = Join-Path $CiDir "check-chat-store-integrity.mjs"
$verdictScript = Join-Path $CiDir "soak-verdict.mjs"
$soakStartedAt = Get-Date   # the Application Error look-back starts here, not an arbitrary number of minutes ago
$FlapSettleSec = 90         # no flap in the first 90 s after a (re)launch: the app is still loading its model
$flag = Join-Path $Work "offline.flag"
$rule = "soak-netdrop"
New-Item -ItemType Directory -Force $Work | Out-Null
Remove-Item "$Work\*" -Recurse -Force -ErrorAction SilentlyContinue
$result = Join-Path $Work "soak-result.txt"
$eventsFile = Join-Path $Work "events.jsonl"   # the harness's own timeline, judged by scripts/ci/soak-verdict.mjs
function Add-SoakEvent([string]$Name, $PromptIndex = $null) {
    $e = @{ t = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(); event = $Name }
    if ($null -ne $PromptIndex) { $e.prompt_index = [int]$PromptIndex }
    ($e | ConvertTo-Json -Compress) | Out-File $eventsFile -Append -Encoding ascii
}
function say($m) { "$(Get-Date -Format HH:mm:ss)  $m" | Tee-Object -FilePath $result -Append | Out-Null; "$(Get-Date -Format HH:mm:ss)  $m" | Out-Host }   # not on the pipeline: Watch-App's return value must stay the bool

# 20 prompts: the ten linked Chicago questions, then the same ten again prefixed so the chat keeps growing.
$base = @(Get-Content $FollowUps -Raw | ConvertFrom-Json)
$all = @()
while ($all.Count -lt $Prompts) { foreach ($q in $base) { if ($all.Count -lt $Prompts) { $all += $(if ($all.Count -lt $base.Count) { $q } else { "Following on from the above: $q" }) } } }

$home1 = Join-Path $Work "home"
$env:PLUGABLE_MODEL = $Model
$env:PLUGABLE_ENABLE_DEMO_DB = "true"
$env:PLUGABLE_ALWAYS_ON_TABLES = "embedded-demo::main.chicago_crimes"
$env:PLUGABLE_CHAT_TEST_OFFLINE_FILE = $flag
$env:PLUGABLE_CHAT_TEST_STATE = Join-Path $Work "state.jsonl"   # also suppresses the crash dialog nobody would click

function Start-Drop { Remove-NetFirewallRule -DisplayName "$rule*" -ErrorAction SilentlyContinue
    New-NetFirewallRule -DisplayName "$rule-out" -Direction Outbound -Action Block -Program $exe | Out-Null
    New-NetFirewallRule -DisplayName "$rule-in" -Direction Inbound -Action Block -Program $exe | Out-Null
    Set-Content $flag "offline" }
function Stop-Drop { Remove-NetFirewallRule -DisplayName "$rule*" -ErrorAction SilentlyContinue; Remove-Item $flag -Force -ErrorAction SilentlyContinue }

function Start-App([string[]]$PromptList, [string]$Tag) {
    $pf = Join-Path $Work "prompts-$Tag.json"
    ConvertTo-Json -InputObject @($PromptList) | Out-File $pf -Encoding ascii
    $rf = Join-Path $Work "replies-$Tag.jsonl"
    $p = Start-Process $exe -WorkingDirectory $AppDir -PassThru -RedirectStandardOutput "$Work\app-$Tag.out" -RedirectStandardError "$Work\app-$Tag.err" `
        -ArgumentList @("--hidden", "--ephemeral", "--chat-home", $home1, "--same-chat", "--prompts-file", $pf, "--reply-file", $rf, "--exit-after-reply", "--reply-timeout-secs", ($MaxMinutes * 60))
    $null = $p.Handle
    return [pscustomobject]@{ Process = $p; ReplyFile = $rf; Tag = $Tag }
}

# Runs one app until it exits or $StopWhen says to kill it; flaps the network on a timer meanwhile, never in the
# first $FlapSettleSec after the launch. $Offset turns this launch's record numbers into run-wide ones.
function Watch-App($App, [scriptblock]$StopWhen = { $false }, [datetime]$Deadline, [int]$Offset = 0) {
    $launchedAt = Get-Date
    Add-SoakEvent "launch"
    $nextFlap = $launchedAt.AddSeconds([Math]::Max($FlapEverySec, $FlapSettleSec)); $flapEnd = $null; $killed = $false; $seen = 0
    while (-not $App.Process.HasExited -and (Get-Date) -lt $Deadline) {
        Start-Sleep -Seconds 2
        $recs = Read-ReplyRecords $App.ReplyFile
        while ($seen -lt $recs.Count) { Add-SoakEvent "record" ($Offset + $seen); $seen++ }
        if ($flapEnd -and (Get-Date) -ge $flapEnd) { Stop-Drop; Add-SoakEvent "flap_end"; $flapEnd = $null; say "network restored"; $nextFlap = (Get-Date).AddSeconds($FlapEverySec) }
        elseif (-not $flapEnd -and (Get-Date) -ge $nextFlap -and (Get-Date) -ge $launchedAt.AddSeconds($FlapSettleSec)) { Start-Drop; Add-SoakEvent "flap_start"; $script:flaps++; $flapEnd = (Get-Date).AddSeconds($FlapForSec); say "network dropped (flap $script:flaps)" }
        if (& $StopWhen $recs) { $killed = $true; break }
    }
    if ($flapEnd) { Stop-Drop; Add-SoakEvent "flap_end" } else { Stop-Drop }
    $recs = Read-ReplyRecords $App.ReplyFile
    while ($seen -lt $recs.Count) { Add-SoakEvent "record" ($Offset + $seen); $seen++ }
    if (-not $App.Process.HasExited) { if ($killed) { Add-SoakEvent "kill" }; taskkill /F /T /PID $App.Process.Id 2>&1 | Out-Null; Start-Sleep 2 }
    return $killed
}

$script:flaps = 0
$deadline = (Get-Date).AddMinutes($MaxMinutes)
$failures = @()
try {
    say "phase 1: $($all.Count) prompts in one chat, app killed after $KillAfter answers"
    $one = Start-App $all "1"
    $killed = Watch-App $one { param($recs) $recs.Count -ge $KillAfter } $deadline
    say "phase 1 ended: killed=$killed, records=$((Read-ReplyRecords $one.ReplyFile).Count)"
    if (-not $killed) { $failures += "the app finished or died before the kill point ($KillAfter answers); nothing was killed mid-run" }
    $leftBehind = Get-ChildItem $home1 -Recurse -Filter run-state.json -ErrorAction SilentlyContinue
    if ($killed -and -not $leftBehind) { $failures += "a killed app left no run-state.json, so the next launch cannot know it died" }

    # The store must open and be whole straight after the kill.
    $dump1 = Join-Path $Work "chats-after-kill.json"
    $d1 = Start-Process $exe -WorkingDirectory $AppDir -PassThru -Wait -ArgumentList @("--hidden", "--ephemeral", "--chat-home", $home1, "--dump-chats", $dump1)
    if ($d1.ExitCode -ne 0 -or -not (Test-Path $dump1)) { $failures += "--dump-chats after the kill failed (exit $($d1.ExitCode)): the chat store did not open" }
    else {
        $out1 = & node $check $dump1 $one.ReplyFile --allow-failed-turns-with network_dropped 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0) { $failures += "chat store after the kill: " + ($out1.Trim() -replace "\r?\n", " | ") }
    }

    $done = (Read-ReplyRecords $one.ReplyFile).Count
    $rest = @($all | Select-Object -Skip $done)
    if ($rest.Count -gt 0) {
        say "phase 2: relaunch for the remaining $($rest.Count) prompts"
        $two = Start-App $rest "2"
        $null = Watch-App $two { $false } $deadline $done
        if (-not $two.Process.HasExited) { $failures += "phase 2 did not finish within $MaxMinutes minutes" }
    }
}
finally { Stop-Drop }

# Verdict over both launches together.
$recs1 = Read-ReplyRecords (Join-Path $Work "replies-1.jsonl")
$recs2 = Read-ReplyRecords (Join-Path $Work "replies-2.jsonl")
$allRecs = @($recs1) + @($recs2)
$offset = $recs1.Count
$renumbered = @(); foreach ($r in $recs1) { $renumbered += $r }; foreach ($r in $recs2) { $r.prompt_index = [int]$r.prompt_index + $offset; $renumbered += $r }
say "records=$($renumbered.Count) of $Prompts, flaps=$script:flaps"
$allFile = Join-Path $Work "replies-all.jsonl"
$renumbered | ForEach-Object { $_ | ConvertTo-Json -Compress -Depth 8 } | Out-File $allFile -Encoding ascii
$dump2 = Join-Path $Work "chats-final.json"
$d2 = Start-Process $exe -WorkingDirectory $AppDir -PassThru -Wait -ArgumentList @("--hidden", "--ephemeral", "--chat-home", $home1, "--dump-chats", $dump2)
if ($d2.ExitCode -ne 0 -or -not (Test-Path $dump2)) { $failures += "final --dump-chats failed (exit $($d2.ExitCode))" }
else {
    $out2 = & node $check $dump2 $allFile --expect-prompts $Prompts --allow-failed-turns-with network_dropped 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { $failures += "final chat store: " + ($out2.Trim() -replace "\r?\n", " | ") }
}
$crashTimes = @(Get-WinEvent -FilterHashtable @{ LogName = "Application"; ProviderName = "Application Error"; StartTime = $soakStartedAt } -ErrorAction SilentlyContinue | Where-Object { $_.Message -match "plugable-chat" } | ForEach-Object { [DateTimeOffset]::new($_.TimeCreated).ToUnixTimeMilliseconds() })
$crashFile = Join-Path $Work "crashes.json"
ConvertTo-Json -InputObject @($crashTimes) | Out-File $crashFile -Encoding ascii
# Which failed turns are expected (the harness had the network down), which are not, and which crashes are the deliberate kill.
$verdictJson = (& node $verdictScript --replies $allFile --events $eventsFile --crashes $crashFile --prompts $Prompts 2>$null) -join ""
try {
    $verdict = $verdictJson | ConvertFrom-Json
    foreach ($n in @($verdict.notes)) { say "note: $n" }
    foreach ($f in @($verdict.failures)) { $failures += [string]$f }
} catch { $failures += "soak-verdict.mjs produced no readable verdict: $verdictJson" }

if ($failures.Count) { $failures | ForEach-Object { say "FAIL: $_" }; say "soak: FAILED"; "soak=FAILED" | Out-File (Join-Path $Work "verdict.txt"); exit 1 }
say "soak: passed"; "soak=PASSED" | Out-File (Join-Path $Work "verdict.txt")
exit 0
