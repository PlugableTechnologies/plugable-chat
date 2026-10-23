# Network-loss repro (a tester's rc15 crash: Ethernet unplugged while the app ran). Run in the desktop session (run-in-session.ps1).
# The box's management channel must stay up, so the network loss is a firewall block on the app's own process, not a NIC disable.
# -Mode generating : block while an answer is being written.  -Mode idle : block after the answer, while the app sits idle.
# Evidence -> C:\gpu\netloss\result.txt: did the app survive, Application Error events naming the faulting module, run-state.
param([ValidateSet("generating","idle")][string]$Mode = "generating", [string]$Model = "qwen3.5-4b-cuda-gpu:4", [string]$AppDir = "C:\Program Files\plugable-chat", [int]$BlockSeconds = 90)
$ErrorActionPreference = "Continue"
$work = "C:\gpu\netloss"
New-Item -ItemType Directory -Force $work | Out-Null
Remove-Item "$work\*" -Recurse -Force -ErrorAction SilentlyContinue
$prompt = if ($Mode -eq "generating") { "Write a very long story, at least 1500 words, about a lighthouse keeper. Do not stop early." } else { "Reply with exactly one word: ready" }
ConvertTo-Json -InputObject @($prompt) | Out-File "$work\prompts.json" -Encoding ascii
$env:PLUGABLE_MODEL = $Model
$env:PLUGABLE_CHAT_HOME = "$work\home"
$start = Get-Date
$p = Start-Process "$AppDir\plugable-chat.exe" -WorkingDirectory $AppDir -PassThru -RedirectStandardOutput "$work\app.out" -RedirectStandardError "$work\app.err" `
    -ArgumentList @("--hidden", "--ephemeral", "--prompts-file", "$work\prompts.json", "--reply-file", "$work\replies.jsonl", "--reply-timeout-secs", "600")
$null = $p.Handle
function say($m) { "$(Get-Date -Format HH:mm:ss)  $m" | Tee-Object -FilePath "$work\result.txt" -Append }
function phase { $f = Get-ChildItem "$work\home" -Recurse -Filter run-state.json -ErrorAction SilentlyContinue | Select-Object -First 1; if ($f) { (Get-Content $f.FullName | ConvertFrom-Json).phase } else { "no-file" } }
$want = if ($Mode -eq "generating") { "chatting" } else { "idle" }
for ($i = 0; $i -lt 300; $i++) {
    if ($p.HasExited) { break }
    if ($Mode -eq "idle" -and (Test-Path "$work\replies.jsonl") -and (Get-Content "$work\replies.jsonl" -Raw).Length -gt 5) { break }
    if ($Mode -eq "generating" -and (phase) -eq "chatting") { Start-Sleep 10; break }
    Start-Sleep 2
}
if ($p.HasExited) { say "app exited BEFORE the block, code $($p.ExitCode)"; return }
say "phase before block = $(phase)"
Remove-NetFirewallRule -DisplayName "netloss-repro*" -ErrorAction SilentlyContinue
New-NetFirewallRule -DisplayName "netloss-repro-out" -Direction Outbound -Action Block -Program "$AppDir\plugable-chat.exe" | Out-Null
New-NetFirewallRule -DisplayName "netloss-repro-in" -Direction Inbound -Action Block -Program "$AppDir\plugable-chat.exe" | Out-Null
say "network blocked for the app for $BlockSeconds s"
$t0 = Get-Date
while (((Get-Date) - $t0).TotalSeconds -lt $BlockSeconds -and -not $p.HasExited) { Start-Sleep 2 }
say "after block window: app alive = $(-not $p.HasExited)$(if ($p.HasExited) { ', exit code ' + $p.ExitCode })"
Remove-NetFirewallRule -DisplayName "netloss-repro*" -ErrorAction SilentlyContinue
say "network restored"
Start-Sleep 20
say "20 s after restore: app alive = $(-not $p.HasExited)$(if ($p.HasExited) { ', exit code ' + $p.ExitCode })"
$ev = Get-WinEvent -FilterHashtable @{ LogName = "Application"; ProviderName = "Application Error"; StartTime = $start } -ErrorAction SilentlyContinue | Where-Object { $_.Message -match "plugable-chat" }
foreach ($e in @($ev)) { say ("EVENT: " + (($e.Message -split "`n" | Select-String "Faulting module name|Exception code" | ForEach-Object { $_.Line.Trim() }) -join " | ")) }
if (-not $ev) { say "no Application Error events for plugable-chat" }
if (Test-Path "$work\replies.jsonl") { say ("replies: " + ((Get-Content "$work\replies.jsonl" | ForEach-Object { try { ($_ | ConvertFrom-Json).outcome } catch { "unparsed" } }) -join ",")) }
say "run-state after = $(phase)"
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
