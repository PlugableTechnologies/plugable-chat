# Engine self-recovery on the installed app (rc16): a damaged GPU component is set aside and the app still answers, two deaths in a row
# switch the GPU off, and --repair-engine undoes it. Run in the desktop session (run-in-session.ps1). Evidence -> C:\gpu\recovery\result.txt.
param([string]$Model = "qwen3.5-4b-cuda-gpu:4", [string]$AppDir = "C:\Program Files\plugable-chat", [string]$EpDir = "$env:USERPROFILE\.plugable-chat\ep")
$ErrorActionPreference = "Continue"
$w = "C:\gpu\recovery"
New-Item -ItemType Directory -Force $w | Out-Null
Remove-Item "$w\*" -Recurse -Force -ErrorAction SilentlyContinue
$env:PLUGABLE_MODEL = $Model
$env:PLUGABLE_CHAT_HOME = "$w\home"
function say($m) { "$(Get-Date -Format HH:mm:ss)  $m" | Tee-Object -FilePath "$w\result.txt" -Append }
function health { $f = Get-ChildItem "$w\home" -Recurse -Filter engine-health.json -ErrorAction SilentlyContinue | Select-Object -First 1; if ($f) { (Get-Content $f.FullName -Raw) -replace "\s+", " " } else { "(no engine-health.json)" } }
function ask([string]$tag, [string[]]$extra = @(), [int]$timeout = 900) {
    $pf = "$w\p-$tag.json"; $rf = "$w\r-$tag.jsonl"
    ConvertTo-Json -InputObject @("Reply with exactly one word: ready") | Out-File $pf -Encoding ascii
    $p = Start-Process "$AppDir\plugable-chat.exe" -WorkingDirectory $AppDir -PassThru -RedirectStandardOutput "$w\$tag.out" -RedirectStandardError "$w\$tag.err" `
        -ArgumentList (@("--hidden", "--prompts-file", $pf, "--reply-file", $rf, "--exit-after-reply", "--reply-timeout-secs", "$timeout") + $extra)
    $null = $p.Handle
    $null = $p.WaitForExit($timeout * 1000 + 60000)
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force; say "$tag : app still running after the timeout, killed" }
    $o = if (Test-Path $rf) { (Get-Content $rf | ForEach-Object { try { ($_ | ConvertFrom-Json).outcome } catch { "unparsed" } }) -join "," } else { "no reply file" }
    say "$tag : outcome=$o ; exit=$($p.ExitCode) ; health=$(health)"
}
function kill-while-chatting([string]$tag) {
    ConvertTo-Json -InputObject @("Write a very long story, at least 1500 words, about a lighthouse keeper. Do not stop early.") | Out-File "$w\story.json" -Encoding ascii
    $p = Start-Process "$AppDir\plugable-chat.exe" -WorkingDirectory $AppDir -PassThru -RedirectStandardOutput "$w\$tag.out" -RedirectStandardError "$w\$tag.err" `
        -ArgumentList @("--hidden", "--prompts-file", "$w\story.json", "--reply-file", "$w\r-$tag.jsonl", "--reply-timeout-secs", "600")
    $null = $p.Handle
    for ($i = 0; $i -lt 240; $i++) { $f = Get-ChildItem "$w\home" -Recurse -Filter run-state.json -ErrorAction SilentlyContinue | Select-Object -First 1; if ($f -and (Get-Content $f.FullName | ConvertFrom-Json).phase -eq "chatting") { break }; if ($p.HasExited) { break }; Start-Sleep 2 }
    Start-Sleep 5; Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue; Start-Sleep 3
    say "$tag : killed while chatting"
}

say "EP dir: $EpDir"
ask "t1-first-good-turn"
$dll = Get-ChildItem $EpDir -Recurse -Filter "onnxruntime_providers_cuda.dll" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $dll) { $dll = Get-ChildItem $EpDir -Recurse -Filter "onnxruntime_providers_webgpu.dll" | Select-Object -First 1 }
say "damaging $($dll.FullName) ($($dll.Length) bytes) -> half"
$fs = [IO.File]::Open($dll.FullName, "Open", "ReadWrite"); $fs.SetLength([int64]($dll.Length / 2)); $fs.Close()
ask "t2-after-damage"
say ("quarantine folders: " + ((Get-ChildItem $EpDir -Directory -Filter "*quarantine*" -ErrorAction SilentlyContinue | ForEach-Object Name) -join ", "))
$dll2 = Get-ChildItem $EpDir -Recurse -Filter $dll.Name -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch "quarantine" } | Select-Object -First 1
say "component after: $(if ($dll2) { "$($dll2.FullName) $($dll2.Length) bytes" } else { 'MISSING' })"
ask "t3-after-recovery"
kill-while-chatting "t4-kill-1"
kill-while-chatting "t4-kill-2"
ask "t5-after-two-deaths" @() 600
say "after two deaths: health=$(health)"
ask "t6-repair" @("--repair-engine") 900
say "done"
