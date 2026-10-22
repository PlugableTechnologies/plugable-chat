# One-conversation follow-up run on the GPU box (the rc14 crash scenario): the installed app answers every prompt
# of chicago-followups.json in a single chat. Run in the desktop session (run-in-session.ps1).
param([string]$Model = "qwen3.5-4b-cuda-gpu:4", [string]$AppDir = "C:\Program Files\plugable-chat", [string]$Prompts = "C:\gpu\chicago-followups.json")
$ErrorActionPreference = "Continue"
$work = "C:\gpu\followups"
New-Item -ItemType Directory -Force $work | Out-Null
Remove-Item "$work\*" -Recurse -Force -ErrorAction SilentlyContinue
$env:PLUGABLE_MODEL = $Model
$env:PLUGABLE_ENABLE_DEMO_DB = "true"
$env:PLUGABLE_ALWAYS_ON_TABLES = "embedded-demo::main.chicago_crimes"
$env:PLUGABLE_CHAT_HOME = "$work\home"
$sw = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process "$AppDir\plugable-chat.exe" -WorkingDirectory $AppDir -PassThru `
    -RedirectStandardOutput "$work\app.out" -RedirectStandardError "$work\app.err" `
    -ArgumentList "--hidden", "--ephemeral", "--same-chat", "--prompts-file", $Prompts, "--reply-file", "$work\replies.jsonl", "--exit-after-reply", "--reply-timeout-secs", "3600"
$null = $p.Handle   # without this, ExitCode is empty for a process started with redirected output
$exited = $p.WaitForExit(3700000)
$code = if ($exited) { $p.ExitCode } else { "still running after 3700 s" }
$runState = @(Get-ChildItem "$work\home" -Recurse -Filter "run-state.json" -ErrorAction SilentlyContinue)
$lines = if (Test-Path "$work\replies.jsonl") { @(Get-Content "$work\replies.jsonl") } else { @() }
$outcomes = $lines | ForEach-Object { try { ($_ | ConvertFrom-Json).outcome } catch { "unparsed" } }
$lines = @($lines)
"exit_code=$code" | Out-File "$work\result.txt" -Encoding ascii
"seconds=$([int]$sw.Elapsed.TotalSeconds)" | Out-File "$work\result.txt" -Append -Encoding ascii
"prompts_answered=$($lines.Count)" | Out-File "$work\result.txt" -Append -Encoding ascii
"outcomes=$(($outcomes | Group-Object | ForEach-Object { "$($_.Name):$($_.Count)" }) -join ',')" | Out-File "$work\result.txt" -Append -Encoding ascii
"run_state_left_behind=$($runState.Count -gt 0)" | Out-File "$work\result.txt" -Append -Encoding ascii
if (-not $exited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
