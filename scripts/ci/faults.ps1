# Fault-injection matrix for the Windows installer and first run (plan: "Step 0").
#
# Each scenario installs the app, injects one fault, launches `plugable-chat.exe --smoke` with
# PLUGABLE_CHAT_TEST_STATE=<file>, asserts on the JSON-lines state file the app writes
# ({ts, phase, status, detail}; phases ep-registration, embedding, model-download, toolbox, ready,
# error), restores the host, and records the result. JUnit XML goes to <OutDir>\clean-host-junit.xml.
#
#   .\faults.ps1 -Installer C:\x\plugable-chat_setup.exe -List
#   .\faults.ps1 -Installer C:\x\plugable-chat_setup.exe -Scenario cpu
#   .\faults.ps1 -Installer C:\x\rc9-setup.exe -Scenario all -ExpectFail     # every row must be RED on rc9
#
# -ExpectFail inverts the verdict: a scenario passes only if its assertions FAIL (the old build
# reproduces the issue). A scenario that is green on the old build does not discriminate and is
# reported as failed. A build without the test hook writes no state file, which also reads as red.
#
# DESTRUCTIVE: edits the hosts file, creates local users, mounts a VHD, changes proxy variables,
# uninstalls VC++. Refuses to run unless GITHUB_ACTIONS=true, CLEAN_HOST_DISPOSABLE=1 or
# -AllowDestructive is given. Never run it on a machine you care about.
#
# Network-drop, crash-capture and download scenarios (tags net, gpu) run the installed app for real turns, so they
# need a model: FAULTS_CHAT_MODEL (default qwen3.5-4b) and FAULTS_DOWNLOAD_MODEL (default a CPU build) choose them.
# They drop the network the way a cable pull looks to the app: a Windows Firewall block on the app's own
# program (the management channel of the AWS box must stay up) plus the PLUGABLE_CHAT_TEST_OFFLINE_FILE seam,
# which makes the app's own connectivity probe agree. JUnit goes to <OutDir>\clean-host-junit.xml as for every scenario.
#
# Written on macOS and validated there only by a PowerShell parse and -List; see
# docs/clean-host-testing.md for what has actually been exercised on Windows.
param(
    [Parameter(Mandatory)][string]$Installer,
    [string[]]$Scenario = @("cpu"),            # names, or "cpu" (runnable on windows-latest), "gpu", "net" (network drops during use), "all"
    [string]$OldInstaller = "",                # an older rc setup.exe, for upgrade-over-old
    [string]$VcRedist = "",                    # a vc_redist.x64.exe, lets no-vcredist uninstall the runtime
    [string]$OutDir = (Join-Path (Get-Location) "clean-host-out"),
    [string]$InstallDir = (Join-Path $env:ProgramFiles "plugable-chat"),
    [int]$SmokeTimeoutSec = 1500,   # the app itself gives up 30 s earlier and records "smoke timed out"
    [string[]]$ExtraBlockHosts = @(),
    [switch]$ExpectFail,
    [switch]$List,
    [switch]$AllowDestructive
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
Import-Module (Join-Path $PSScriptRoot "CleanHost.psm1") -Force

$WorkRoot = Join-Path $env:SystemDrive "clean-host-work"
$Exe = Join-Path $InstallDir "plugable-chat.exe"
$HostsFile = Join-Path $env:SystemRoot "System32\drivers\etc\hosts"
$HfHosts = @("huggingface.co", "www.huggingface.co", "cdn-lfs.huggingface.co", "cdn-lfs-us-1.huggingface.co",
    "hf.co", "cdn-lfs.hf.co", "cas-bridge.xethub.hf.co", "transfer.xethub.hf.co") + $ExtraBlockHosts
# Since the search model also ships as a GitHub release asset, the scenarios that are about huggingface.co
# failing must leave Hugging Face as the only source (a test seam, honoured only under PLUGABLE_CHAT_TEST_STATE).
$HfOnly = @{ PLUGABLE_CHAT_TEST_EMBEDDING_SOURCES = "huggingface" }

# ------------------------------------------------------------------------------------------
# host plumbing
# ------------------------------------------------------------------------------------------
function Stop-App {
    Stop-Process -Name plugable-chat -Force -ErrorAction SilentlyContinue
    Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$InstallDir*" } | Stop-Process -Force -ErrorAction SilentlyContinue
}

function Uninstall-App {
    Stop-App
    $un = Join-Path $InstallDir "uninstall.exe"
    if (Test-Path -LiteralPath $un) { Start-Process $un -ArgumentList "/S" -Wait | Out-Null; Start-Sleep 2 }
    if (Test-Path -LiteralPath $InstallDir) { Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue }
}

function Install-App([string]$Path) {
    # No /D=: perMachine must pick Program Files itself, like IT tools rely on.
    $p = Start-Process $Path -ArgumentList "/S" -Wait -PassThru
    return $p.ExitCode
}

# User data of the account running the harness; scenarios start from "never launched".
function Reset-UserData {
    foreach ($d in (Join-Path $env:LOCALAPPDATA "plugable-chat"), (Join-Path $env:APPDATA "plugable-chat")) {
        if (Test-Path -LiteralPath $d) {
            # a leftover junction (low-disk) must be removed as a link, not recursed into
            cmd /c "rmdir `"$d`" 2>nul" | Out-Null
            if (Test-Path -LiteralPath $d) { Remove-Item -LiteralPath $d -Recurse -Force -ErrorAction SilentlyContinue }
        }
    }
}

function New-ScenarioContext([string]$Name) {
    $dir = Join-Path $WorkRoot $Name
    Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    return [pscustomobject]@{ Name = $Name; Dir = $dir; StateFile = (Join-Path $dir "state.jsonl"); Runs = 0 }
}

function Start-Fresh($ctx, [string]$Path = $Installer) {
    Uninstall-App | Out-Null
    Reset-UserData | Out-Null
    Remove-Item -LiteralPath (Join-Path $env:TEMP "plugable-chat-install.log") -Force -ErrorAction SilentlyContinue
    $code = Install-App $Path
    if ($code -ne 0) { return @("installer exited with code $code") }
    if (-not (Test-Path -LiteralPath $Exe)) { return @("install reported success but $Exe does not exist") }
    return @()
}

# Run the app in smoke mode and wait for it to finish. -Credential runs it as another (standard)
# local user; -KillWhen is polled once a second with the records so far and, when it returns
# true, the process tree is killed (kill-mid-download).
function Invoke-Smoke {
    param($Ctx, [hashtable]$EnvVars = @{}, [pscredential]$Credential, [string]$WorkingDirectory = $InstallDir,
          [scriptblock]$KillWhen, [int]$TimeoutSec = $SmokeTimeoutSec,
          [switch]$NoSmoke)   # run the app normally (hidden) instead of --smoke: for scenarios that wait for later events
    $Ctx.Runs++
    $idx = $Ctx.Runs
    Remove-Item -LiteralPath $Ctx.StateFile -Force -ErrorAction SilentlyContinue
    $out = Join-Path $Ctx.Dir "smoke-$idx.out.txt"
    $appArgs = if ($NoSmoke) { "--hidden" } else { "--smoke" }
    $vars = @{ PLUGABLE_CHAT_TEST_STATE = $Ctx.StateFile; PLUGABLE_CHAT_SMOKE_TIMEOUT_SECS = [string][math]::Max(30, $TimeoutSec - 30) }
    foreach ($k in $EnvVars.Keys) { $vars[$k] = $EnvVars[$k] }

    if ($Credential) {
        $cmd = Join-Path $Ctx.Dir "run-smoke-$idx.cmd"
        $lines = @("@echo off")
        foreach ($k in $vars.Keys) { $lines += "set `"$k=$($vars[$k])`"" }
        $lines += "cd /d `"$WorkingDirectory`""
        $lines += "`"$Exe`" $appArgs > `"$out`" 2>&1"
        Set-Content -LiteralPath $cmd -Value $lines -Encoding ASCII
        $proc = Start-Process cmd.exe -ArgumentList "/c `"$cmd`"" -Credential $Credential -LoadUserProfile -WorkingDirectory $env:SystemRoot -PassThru -WindowStyle Hidden
    }
    else {
        $saved = @{}
        foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k, "Process"); [Environment]::SetEnvironmentVariable($k, [string]$vars[$k], "Process") }
        try { $proc = Start-Process $Exe -ArgumentList $appArgs -WorkingDirectory $WorkingDirectory -PassThru -WindowStyle Hidden -RedirectStandardOutput $out -RedirectStandardError "$out.err" }
        finally { foreach ($k in $vars.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k], "Process") } }
    }
    $null = $proc.Handle   # keeps ExitCode readable after exit
    $killed = $false; $timedOut = $false
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while (-not $proc.HasExited) {
        if ($sw.Elapsed.TotalSeconds -gt $TimeoutSec) { $timedOut = $true; break }
        if ($KillWhen -and (& $KillWhen (Read-StateFile $Ctx.StateFile))) { $killed = $true; break }
        Start-Sleep -Seconds 1
    }
    if ($timedOut -or $killed) { taskkill /F /T /PID $proc.Id 2>&1 | Out-Null; Stop-App; Start-Sleep 1 }
    $exit = if ($proc.HasExited) { $proc.ExitCode } else { $null }
    return [pscustomobject]@{
        ExitCode = $exit; TimedOut = $timedOut; Killed = $killed; Seconds = [int]$sw.Elapsed.TotalSeconds
        Records = (Read-StateFile $Ctx.StateFile); OutFile = $out
    }
}

# Failures every smoke run shares: it must finish, and the build must support the test hook.
function Get-RunFailures($run) {
    $f = @()
    if ($run.TimedOut) { $f += "smoke run did not finish within the timeout ($($run.Seconds) s): the app hung" }
    $err = Get-LastPhase $run.Records "error"
    if ($err -and ([string]$err.detail -match "smoke timed out")) { $f += "the app's own smoke timeout fired: it never reached ready or a specific error" }
    if (@($run.Records).Count -eq 0) { $f += "no state file records: this build lacks PLUGABLE_CHAT_TEST_STATE / --smoke (or crashed before writing any)" }
    return $f
}

function Get-EmbeddingFileCount([string]$DataRoot = (Join-Path $env:LOCALAPPDATA "plugable-chat")) {
    $d = Join-Path $DataRoot "data\models\fastembed"
    if (-not (Test-Path -LiteralPath $d)) { return 0 }
    return @(Get-ChildItem -LiteralPath $d -Recurse -File -Filter "*.onnx" -ErrorAction SilentlyContinue).Count
}


# The newest preflight report the app wrote (config dir = %APPDATA%\plugable-chat), or $null.
function Get-PreflightReport {
    $dir = Join-Path $env:APPDATA "plugable-chat\logs"
    $f = Get-ChildItem -LiteralPath $dir -Filter "preflight-*.json" -ErrorAction SilentlyContinue | Sort-Object Name | Select-Object -Last 1
    if (-not $f) { return $null }
    return (Get-Content -LiteralPath $f.FullName -Raw | ConvertFrom-Json)
}

# Environment for the app's test seams (honoured only because PLUGABLE_CHAT_TEST_STATE is also set):
# what Windows "lists", what the engine "reports", what nvidia-smi "says".
function New-GpuSeams([switch]$Nvidia, [switch]$Thunderbolt, [string]$EpsAfterRetry = "", [string]$LateAfterSecs = "") {
    # JSON is written out by hand: Windows PowerShell 5.1's ConvertTo-Json turns an empty array into
    # null and a one-element array into a bare value, which the app (correctly) rejects.
    $adapter = if ($Nvidia) { '{"name":"NVIDIA GeForce RTX 4090","vendor":"NVIDIA","vramMb":4095,"pnpId":"PCI\\VEN_10DE&DEV_2684","status":"OK","problemCode":0}' } else { "" }
    $tb = if ($Thunderbolt) { '"Intel(R) Thunderbolt(TM) Controller"' } else { "" }
    $scan = '{"adapters":[' + $adapter + '],"thunderboltDevices":[' + $tb + ']}'
    $vars = @{
        PLUGABLE_CHAT_TEST_EPS        = '[{"name":"CPUExecutionProvider","isRegistered":true}]'
        PLUGABLE_CHAT_TEST_NVIDIA_SMI = "572.16"
    }
    if ($LateAfterSecs) {
        $vars.PLUGABLE_CHAT_TEST_ADAPTERS_LATE = $scan
        $vars.PLUGABLE_CHAT_TEST_ADAPTERS_LATE_AFTER_SECS = $LateAfterSecs
        $vars.PLUGABLE_CHAT_TEST_ADAPTERS = '{"adapters":[],"thunderboltDevices":[]}'
    }
    else { $vars.PLUGABLE_CHAT_TEST_ADAPTERS = $scan }
    if ($EpsAfterRetry) { $vars.PLUGABLE_CHAT_TEST_EPS_AFTER_RETRY = $EpsAfterRetry }
    return $vars
}

# Every gpu-assessment record, oldest first, as "State | headline".
function Get-GpuAssessments($recs) { @($recs | Where-Object { $_.phase -eq "gpu-assessment" } | ForEach-Object { [string]$_.detail }) }

function Get-HookRegistry { Get-ItemProperty -Path "HKLM:\SOFTWARE\Plugable\plugable-chat\Installer" -ErrorAction SilentlyContinue }
function Get-UninstallEntries {
    @(Get-ItemProperty "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*" -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -eq "plugable-chat" })
}
function Test-HasNvidiaDriver { return [bool](Get-Command nvidia-smi -ErrorAction SilentlyContinue) -or (Test-Path (Join-Path $env:SystemRoot "System32\nvidia-smi.exe")) }
function Test-VcRuntimeRegistered {
    $v = Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" -ErrorAction SilentlyContinue
    return ($null -ne $v -and $v.Installed -eq 1)
}

# A standard (non-admin) local user with a one-run random password, created and removed here.
function New-TestUser([string]$Name) {
    $pw = "Fx!" + [guid]::NewGuid().ToString("N").Substring(0, 18)
    Remove-LocalUser -Name $Name -ErrorAction SilentlyContinue
    New-LocalUser -Name $Name -Password (ConvertTo-SecureString $pw -AsPlainText -Force) -PasswordNeverExpires -AccountNeverExpires | Out-Null
    Add-LocalGroupMember -Group "Users" -Member $Name -ErrorAction SilentlyContinue
    return (New-Object pscredential(".\$Name", (ConvertTo-SecureString $pw -AsPlainText -Force)))
}
function Remove-TestUser([string]$Name) {
    Remove-LocalUser -Name $Name -ErrorAction SilentlyContinue
    Get-ChildItem (Join-Path $env:SystemDrive "Users") -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "$Name*" } |
        ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
}

function Copy-Diagnostics($ctx) {
    $dest = Join-Path $OutDir $ctx.Name
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Copy-Item -Path (Join-Path $ctx.Dir "*") -Destination $dest -Recurse -Force -ErrorAction SilentlyContinue
    foreach ($f in "plugable-chat-install.log", "plugable-chat-vcredist.log") {
        $p = Join-Path $env:TEMP $f
        if (Test-Path -LiteralPath $p) { Copy-Item -LiteralPath $p -Destination $dest -Force }
    }
}

# Run $Action with the HuggingFace hosts pointed at localhost; the hosts file is always restored.
function Invoke-WithHostsBlock($ctx, [scriptblock]$Action) {
    $backup = Join-Path $ctx.Dir "hosts.backup"
    Copy-Item -LiteralPath $HostsFile -Destination $backup -Force
    try {
        Add-Content -LiteralPath $HostsFile -Value ($HfHosts | ForEach-Object { "127.0.0.1 $_" })
        ipconfig /flushdns | Out-Null
        & $Action
    }
    finally {
        Copy-Item -LiteralPath $backup -Destination $HostsFile -Force
        ipconfig /flushdns | Out-Null
    }
}

# ------------------------------------------------------------------------------------------
# network drops, real turns and crash capture (scenarios tagged net / gpu)
# ------------------------------------------------------------------------------------------
$NetDropRule = "faults-netdrop"
$ChatModel = if ($env:FAULTS_CHAT_MODEL) { $env:FAULTS_CHAT_MODEL } else { "qwen3.5-4b" }
$DownloadModel = if ($env:FAULTS_DOWNLOAD_MODEL) { $env:FAULTS_DOWNLOAD_MODEL } else { "qwen3.5-4b-generic-cpu:3" }
$CheckStore = Join-Path $PSScriptRoot "check-chat-store-integrity.mjs"

# The network goes away for the app only: outbound and inbound blocked for its program, and the offline file
# makes the app's connectivity probe say so. Loopback is not filtered by Windows Firewall, so a local server stays reachable.
function Start-NetworkDrop([string]$FlagFile = "") {
    Remove-NetFirewallRule -DisplayName "$NetDropRule*" -ErrorAction SilentlyContinue
    New-NetFirewallRule -DisplayName "$NetDropRule-out" -Direction Outbound -Action Block -Program $Exe | Out-Null
    New-NetFirewallRule -DisplayName "$NetDropRule-in" -Direction Inbound -Action Block -Program $Exe | Out-Null
    if ($FlagFile) { Set-Content -LiteralPath $FlagFile -Value "offline" }
}

function Stop-NetworkDrop([string]$FlagFile = "") {
    Remove-NetFirewallRule -DisplayName "$NetDropRule*" -ErrorAction SilentlyContinue
    if ($FlagFile) { Remove-Item -LiteralPath $FlagFile -Force -ErrorAction SilentlyContinue }
}

# Starts the installed app for a run of prompts in one chat, reading and writing under the scenario's own
# chat home so a relaunch sees the same chat store. Returns what the scenario needs to watch and assert on.
function Start-TurnApp {
    param($Ctx, [string[]]$Prompts = @(), [hashtable]$EnvVars = @{}, [string[]]$ExtraArgs = @(), [string]$Name = "run")
    $Ctx.Runs++
    $chatHome = Join-Path $Ctx.Dir "chat-home"
    New-Item -ItemType Directory -Force -Path $chatHome | Out-Null
    $promptFile = Join-Path $Ctx.Dir "prompts-$Name.json"
    $replyFile = Join-Path $Ctx.Dir "replies-$Name.jsonl"
    $out = Join-Path $Ctx.Dir "app-$Name.out.txt"
    Remove-Item -LiteralPath $replyFile -Force -ErrorAction SilentlyContinue
    $appArgs = @("--hidden", "--ephemeral", "--chat-home", $chatHome)
    if ($Prompts.Count) {
        ConvertTo-Json -InputObject @($Prompts) | Out-File -LiteralPath $promptFile -Encoding ascii
        $appArgs += @("--same-chat", "--prompts-file", $promptFile, "--reply-file", $replyFile, "--exit-after-reply", "--reply-timeout-secs", "1500")
    }
    $appArgs += $ExtraArgs
    $vars = @{ PLUGABLE_CHAT_TEST_STATE = (Join-Path $Ctx.Dir "state-$Name.jsonl"); PLUGABLE_MODEL = $ChatModel }
    foreach ($k in $EnvVars.Keys) { $vars[$k] = $EnvVars[$k] }
    $saved = @{}
    foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k, "Process"); [Environment]::SetEnvironmentVariable($k, [string]$vars[$k], "Process") }
    try { $proc = Start-Process $Exe -ArgumentList $appArgs -WorkingDirectory $InstallDir -PassThru -RedirectStandardOutput $out -RedirectStandardError "$out.err" }
    finally { foreach ($k in $vars.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k], "Process") } }
    $null = $proc.Handle
    return [pscustomobject]@{ Process = $proc; Home = $chatHome; ReplyFile = $replyFile; Out = $out; StartedAt = Get-Date
        RunStateFile = (Join-Path $chatHome "config\run-state.json"); StateFile = $vars.PLUGABLE_CHAT_TEST_STATE }
}

function Get-RunPhase($App) {
    if (-not (Test-Path -LiteralPath $App.RunStateFile)) { return "" }
    try { return [string](Get-Content -LiteralPath $App.RunStateFile -Raw | ConvertFrom-Json).phase } catch { return "" }
}

function Wait-AppExit($App, [int]$TimeoutSec) {
    if ($App.Process.WaitForExit($TimeoutSec * 1000)) { return $true }
    taskkill /F /T /PID $App.Process.Id 2>&1 | Out-Null
    return $false
}

# "Application Error" events naming plugable-chat since the app started: a clean stop must leave none.
function Get-AppCrashEvents($App) {
    @(Get-WinEvent -FilterHashtable @{ LogName = "Application"; ProviderName = "Application Error"; StartTime = $App.StartedAt } -ErrorAction SilentlyContinue |
        Where-Object { $_.Message -match "plugable-chat" })
}

# The chat store, read back through the app itself (--dump-chats), then judged by the same checker the soak test uses.
function Get-ChatStoreFailures($Ctx, $App, [string[]]$AllowedFailure = @("network_dropped"), [string]$Name = "dump") {
    $dump = Join-Path $Ctx.Dir "chats-$Name.json"
    Remove-Item -LiteralPath $dump -Force -ErrorAction SilentlyContinue
    $d = Start-TurnApp $Ctx -ExtraArgs @("--dump-chats", $dump) -Name "dump-$Name"
    if (-not (Wait-AppExit $d 180)) { return @("the app did not finish reading the chat store back within 180 s") }
    if ($d.Process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $dump)) { return @("--dump-chats failed (exit $($d.Process.ExitCode)): the chat store could not be read") }
    $node = (Get-Command node -ErrorAction SilentlyContinue).Source
    if (-not $node) { $node = "C:\Program Files\nodejs\node.exe" }
    if (-not (Test-Path -LiteralPath $node)) { return @("node is not installed on this host, so the chat store could not be checked") }
    $checkArgs = @($CheckStore, $dump, $App.ReplyFile)
    foreach ($a in $AllowedFailure) { $checkArgs += @("--allow-failed-turns-with", $a) }
    $res = & $node @checkArgs 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) { return @("chat store check failed: " + ($res.Trim() -replace "\r?\n", " | ")) }
    return @()
}

# A one-tool MCP server over HTTP whose tool takes $SleepSeconds to answer, so a drop can land mid-call. It writes
# $StartedFile when a call begins. Returns the config file to pass as --mcp-server @file.
function Start-SlowMcpServer($Ctx, [string]$StartedFile, [int]$SleepSeconds = 60) {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop()
    $job = Start-Job -ArgumentList $port, $StartedFile, $SleepSeconds -ScriptBlock {
        param($port, $started, $sleep)
        $l = [Net.HttpListener]::new(); $l.Prefixes.Add("http://127.0.0.1:$port/"); $l.Start()
        while ($l.IsListening) {
            $c = $l.GetContext()
            $body = [IO.StreamReader]::new($c.Request.InputStream).ReadToEnd()
            $msg = $body | ConvertFrom-Json
            $result = $null
            switch ($msg.method) {
                "initialize" { $result = @{ protocolVersion = $msg.params.protocolVersion; capabilities = @{ tools = @{} }; serverInfo = @{ name = "slow"; version = "1" } } }
                "tools/list" { $result = @{ tools = @(@{ name = "slow_lookup"; description = "Looks up the value stored under a key. Use it whenever the user asks for the value of a key."; inputSchema = @{ type = "object"; properties = @{ key = @{ type = "string" } }; required = @("key") } }) } }
                "tools/call" { Set-Content -LiteralPath $started -Value "started"; Start-Sleep -Seconds $sleep; $result = @{ content = @(@{ type = "text"; text = "value-42" }) } }
            }
            if ($null -eq $msg.id -or $null -eq $result) { $c.Response.StatusCode = 202; $c.Response.Close(); continue }
            $bytes = [Text.Encoding]::UTF8.GetBytes((@{ jsonrpc = "2.0"; id = $msg.id; result = $result } | ConvertTo-Json -Depth 8 -Compress))
            $c.Response.ContentType = "application/json"; $c.Response.OutputStream.Write($bytes, 0, $bytes.Length); $c.Response.Close()
        }
    }
    $cfg = Join-Path $Ctx.Dir "slow-mcp.json"
    (@{ id = "slow"; name = "slow"; enabled = $true; transport = @{ type = "http"; url = "http://127.0.0.1:$port/" }; auto_approve_tools = $true; defer_tools = $false; request_timeout_secs = 300 } | ConvertTo-Json -Depth 5 -Compress) |
        Out-File -LiteralPath $cfg -Encoding ascii
    return [pscustomobject]@{ Job = $job; Config = $cfg; Port = $port }
}

# ------------------------------------------------------------------------------------------
# scenarios: each returns @{ Failures = @(...) } or @{ Skip = "reason" } and restores what it changed
# ------------------------------------------------------------------------------------------
$Scenarios = [ordered]@{}
function Register($name, $issue, $tags, $desc, [scriptblock]$body) { $Scenarios[$name] = @{ Name = $name; Issue = $issue; Tags = $tags; Desc = $desc; Body = $body } }

Register "baseline" "#1 #2" @("cpu") "fresh install, first run on a healthy host: hook ran, embedding and ready ok" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    if (-not (Get-HookRegistry)) { $f += "installer hook registry key missing" }
    $run = Invoke-Smoke $ctx
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    $f += Assert-PhaseOk $run.Records "embedding"
    $f += Assert-PhaseOk $run.Records "ready"
    return @{ Failures = $f }
}

Register "readonly-nonadmin" "#1" @("cpu") "read-only install dir, cwd = install dir (a relative .fastembed_cache must not matter)" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    # A second Windows user cannot start WebView2 from a non-interactive CI session ("Access is
    # denied"), so make the install dir unwritable for the running account with a deny ACE. That is
    # exactly the condition the bug needs: nothing under Program Files can be created by the app.
    $me = "$env:USERDOMAIN\$env:USERNAME"
    icacls $InstallDir /deny "${me}:(OI)(CI)(WD,AD,DC)" | Out-Null
    try {
        $probe = Join-Path $InstallDir "write-probe.tmp"
        $wrote = $true
        try { Set-Content -LiteralPath $probe -Value x -ErrorAction Stop } catch { $wrote = $false }
        if ($wrote) { Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue; $f += "install dir is still writable; the fault is not in place" }
        $before = Get-DirSnapshot $InstallDir
        $run = Invoke-Smoke $ctx -WorkingDirectory $InstallDir
        $f += Get-RunFailures $run
        $f += Assert-Terminal $run.Records
        $f += Assert-PhaseOk $run.Records "embedding"
        if ((Get-EmbeddingFileCount) -lt 1) { $f += "no embedding model files under the user's data directory" }
        $changes = Compare-DirSnapshot $before (Get-DirSnapshot $InstallDir)
        if ($changes.Count) { $f += "install dir changed while running: $(($changes | Select-Object -First 5) -join '; ')" }
    }
    finally { icacls $InstallDir /remove:d "$me" | Out-Null }
    return @{ Failures = $f }
}

Register "offline-hosts-block" "#1 #3" @("cpu") "huggingface.co blocked via the hosts file: embedding must report a truthful error, never a green check" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $f += @(Invoke-WithHostsBlock $ctx {
            $run = Invoke-Smoke $ctx -EnvVars $HfOnly
            Get-RunFailures $run
            Assert-Terminal $run.Records
            Assert-EmbeddingTruthful $run.Records (Get-EmbeddingFileCount)
            # Hugging Face is the only source here ($HfOnly), so this must be an error that names the cause.
            # With the release copy allowed it loads instead: see embedding-no-huggingface.
            Assert-PhaseError $run.Records "embedding" "(?i)network|offline|connect|resolve|internet|huggingface|download"
        })
    return @{ Failures = $f }
}

Register "proxy-dead" "#1 #4" @("cpu") "HTTPS_PROXY points at a closed port: error names the proxy/connection, no hang" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $run = Invoke-Smoke $ctx -EnvVars @{ HTTPS_PROXY = "http://127.0.0.1:9"; HTTP_PROXY = "http://127.0.0.1:9"; ALL_PROXY = "http://127.0.0.1:9" }
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    # Startup may end before the embedding step runs at all (see below), so only check it when it ran.
    # Embedding now starts in parallel with startup and keeps retrying in the background, so the run may end
    # with it still in progress; that is the intended behaviour (chat is usable), not a hang.
    $emb = Get-LastPhase $run.Records "embedding"
    if ($emb -and ((Test-StatusOk $emb) -or (Test-StatusError $emb))) { $f += Assert-EmbeddingTruthful $run.Records (Get-EmbeddingFileCount) }
    # With every outbound request refused the AI engine cannot fetch its catalog, so startup ends
    # on the "can't start" card before the embedding step; either record may carry the cause.
    # What matters: the text points at the network/proxy and never tells the user to reinstall.
    $net = @($run.Records | Where-Object { [string]$_.detail -match "(?i)could not reach the internet|proxy|refused|network|unreachable|huggingface" })
    if ($net.Count -eq 0) { $f += "no record names the network/proxy as the cause" }
    $bad = @($run.Records | Where-Object { [string]$_.detail -match "Reinstall the latest" })
    if ($bad.Count -gt 0) { $f += "a network failure was answered with reinstall advice" }
    return @{ Failures = $f }
}

Register "proxy-tls-intercept" "#1 #4" @("cpu") "mitmproxy with an untrusted CA: error names the certificate problem (skipped when mitmdump is not installed)" {
    param($ctx)
    if (-not (Get-Command mitmdump -ErrorAction SilentlyContinue)) { return @{ Skip = "mitmdump not on PATH (pip install mitmproxy)" } }
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $mitm = Start-Process mitmdump -ArgumentList "--listen-port", "18080" -PassThru -WindowStyle Hidden
    try {
        Start-Sleep 3
        $run = Invoke-Smoke $ctx -EnvVars @{ HTTPS_PROXY = "http://127.0.0.1:18080" }
        $f += Get-RunFailures $run
        $f += Assert-Terminal $run.Records
        $f += Assert-EmbeddingTruthful $run.Records (Get-EmbeddingFileCount)
        $f += Assert-PhaseError $run.Records "embedding" "(?i)certificate|tls|ssl|trust|intercept|proxy"
    }
    finally { Stop-Process -Id $mitm.Id -Force -ErrorAction SilentlyContinue }
    return @{ Failures = $f }
}

Register "no-vcredist" "#7 #3" @("cpu") "no VC++ runtime before install: the installer hook must install it and the app must start" {
    param($ctx)
    if (Test-VcRuntimeRegistered) {
        if (-not $AllowDestructive -or -not $VcRedist) { return @{ Skip = "VC++ runtime is installed; pass -AllowDestructive -VcRedist <vc_redist.x64.exe> on a disposable host to remove it first" } }
        Start-Process $VcRedist -ArgumentList "/uninstall", "/quiet", "/norestart" -Wait | Out-Null
        if (Test-VcRuntimeRegistered) { return @{ Skip = "could not remove the VC++ runtime (another product on this host still registers it); use a clean Windows Server image" } }
    }
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $reg = Get-HookRegistry
    if (-not $reg) { $f += "installer hook registry key missing: the hook did not run" }
    elseif ($reg.VCRedistStatus -notin "installed", "installed-reboot") { $f += "VCRedistStatus is '$($reg.VCRedistStatus)', expected the hook to install the runtime" }
    if (-not (Test-VcRuntimeRegistered)) { $f += "VC++ runtime still not registered after the install" }
    $run = Invoke-Smoke $ctx
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    $f += Assert-PhaseOk $run.Records "ep-registration"
    return @{ Failures = $f }
}

Register "driver-absent" "#3" @("cpu") "no NVIDIA driver: the GPU step explains itself (driver wording) and the app still reaches ready on CPU" {
    param($ctx)
    if (Test-HasNvidiaDriver) { return @{ Skip = "this host has an NVIDIA driver; run on a CPU-only host, or after rolling the driver back" } }
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $run = Invoke-Smoke $ctx
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    $ep = Get-LastPhase $run.Records "ep-registration"
    if ($null -eq $ep) { $f += "phase 'ep-registration' never reported" }
    elseif (-not (Test-StatusOk $ep) -and ([string]$ep.detail -notmatch "(?i)driver|gpu|nvidia|cuda|no compatible|cpu")) {
        $f += "ep-registration '$($ep.status)' without a cause the user can act on: '$($ep.detail)'"
    }
    $f += Assert-PhaseOk $run.Records "ready"
    return @{ Failures = $f }
}

Register "kill-mid-download" "#5" @("cpu") "kill the app during a download, relaunch: no corrupt partial state, the run completes" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $run1 = Invoke-Smoke $ctx -TimeoutSec 180 -KillWhen {
        param($recs)
        @($recs | Where-Object { ($_.phase -in @("embedding", "model-download")) -and ("$($_.status)" -in @("started", "start", "progress", "running")) }).Count -gt 0
    }
    if (-not $run1.Killed) { $f += "never saw a download in progress to interrupt ($(@($run1.Records).Count) state records)" }
    $run2 = Invoke-Smoke $ctx
    $f += Get-RunFailures $run2
    $f += Assert-Terminal $run2.Records
    $f += Assert-PhaseOk $run2.Records "embedding"
    foreach ($ph in "embedding", "model-download") {
        $r = Get-LastPhase $run2.Records $ph
        if ($r -and (Test-StatusError $r) -and ([string]$r.detail -match "(?i)corrupt|checksum|truncat|partial|already exists|invalid")) { $f += "relaunch tripped on state left by the kill ($ph): $($r.detail)" }
    }
    return @{ Failures = $f }
}

Register "unicode-username" "data-dir bugs" @("cpu") "local user with a space and non-ASCII letters in the name (so in the profile path)" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $name = "Zo$([char]0xEB) M$([char]0xFC)ller"
    $cred = New-TestUser $name
    try {
        $run = Invoke-Smoke $ctx -Credential $cred
        $out = if (Test-Path -LiteralPath $run.OutFile) { Get-Content -LiteralPath $run.OutFile -Raw } else { "" }
        if ($out -match "WebView2 error" -and $out -match "Access is denied") {
            return @{ Skip = "WebView2 cannot start for a second user without an interactive desktop (CI session); run this scenario on the AWS box over RDP" }
        }
        $f += Get-RunFailures $run
        $f += Assert-Terminal $run.Records
        $f += Assert-PhaseOk $run.Records "embedding"
        $profileDir = Get-ChildItem (Join-Path $env:SystemDrive "Users") -Directory | Where-Object { $_.Name -like "Zo*ller*" } | Select-Object -First 1
        if (-not $profileDir) { $f += "no profile directory was created for the test user" }
        elseif (-not (Test-Path -LiteralPath (Join-Path $profileDir.FullName "AppData\Local\plugable-chat"))) { $f += "app created no data directory under '$($profileDir.FullName)'" }
    }
    finally { Remove-TestUser $name }
    return @{ Failures = $f }
}

Register "low-disk" "download failures" @("cpu") "data dir on a 300 MB volume: the download fails with a disk-space cause, not a hang or a green check" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $vhd = Join-Path $ctx.Dir "small.vhd"
    $letter = (69..90 | ForEach-Object { [char]$_ } | Where-Object { -not (Test-Path "$($_):\") } | Select-Object -First 1)
    $link = Join-Path $env:LOCALAPPDATA "plugable-chat"
    $up = Join-Path $ctx.Dir "diskpart-up.txt"
    try {
        Set-Content $up @("create vdisk file=`"$vhd`" maximum=300 type=fixed", "select vdisk file=`"$vhd`"", "attach vdisk", "create partition primary", "format fs=ntfs quick label=smalldisk", "assign letter=$letter")
        diskpart /s $up | Out-Null
        New-Item -ItemType Directory -Force -Path "$($letter):\plugable-chat" | Out-Null
        Reset-UserData | Out-Null
        cmd /c "mklink /J `"$link`" `"$($letter):\plugable-chat`"" | Out-Null
        $run = Invoke-Smoke $ctx
        $f += Get-RunFailures $run
        $f += Assert-Terminal $run.Records
        $f += Assert-EmbeddingTruthful $run.Records (Get-EmbeddingFileCount)
        $f += Assert-PhaseError $run.Records "embedding" "(?i)space|disk|full|write|os error 112|storage"
    }
    finally {
        Stop-App
        cmd /c "rmdir `"$link`" 2>nul" | Out-Null
        $down = Join-Path $ctx.Dir "diskpart-down.txt"
        Set-Content $down @("select vdisk file=`"$vhd`"", "detach vdisk")
        diskpart /s $down | Out-Null
        Remove-Item -LiteralPath $vhd -Force -ErrorAction SilentlyContinue
    }
    return @{ Failures = $f }
}

Register "quarantined-dll" "rc9 can't-start card" @("cpu") "security software removes onnxruntime.dll: an actionable error naming the file, or self-repair" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $dll = Join-Path $InstallDir "foundry-libs\onnxruntime.dll"
    if (-not (Test-Path -LiteralPath $dll)) { return @{ Failures = @("fault not injectable: $dll is not in the install") } }
    Remove-Item -LiteralPath $dll -Force
    $run = Invoke-Smoke $ctx
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    $healed = Test-Path -LiteralPath $dll
    $ready = Get-LastPhase $run.Records "ready"
    if (-not $healed -and (Test-StatusOk $ready)) { $f += "reported ready with onnxruntime.dll missing and not repaired" }
    # The actionable text (the "can't start" card) may be on an earlier record than the final
    # generic 'error' one, so look at every record's detail.
    if (-not $healed -and -not (Test-StatusOk $ready)) {
        $hit = @($run.Records | Where-Object { [string]$_.detail -match "(?i)engine files|onnxruntime|foundry-libs|reinstall|runtime" })
        if ($hit.Count -eq 0) { $f += "no record gave actionable guidance for a missing onnxruntime.dll" }
    }
    return @{ Failures = $f }
}

Register "upgrade-over-old" "stale files, resources" @("cpu") "install an older rc, then this build over it: stale runtime files removed, user data kept" {
    param($ctx)
    if (-not $OldInstaller) { return @{ Skip = "no -OldInstaller given" } }
    $f = @(Start-Fresh $ctx $OldInstaller); if ($f.Count) { return @{ Failures = $f } }
    $stale = Join-Path $InstallDir "foundry-libs\stale-sentinel.dll"
    New-Item -ItemType Directory -Force -Path (Split-Path $stale) | Out-Null
    Set-Content $stale "stale"
    $userFile = Join-Path $env:LOCALAPPDATA "plugable-chat\data\upgrade-sentinel.txt"
    New-Item -ItemType Directory -Force -Path (Split-Path $userFile) | Out-Null
    Set-Content $userFile "keep me"
    $code = Install-App $Installer
    if ($code -ne 0) { $f += "upgrade installer exited with code $code" }
    if (Test-Path -LiteralPath $stale) { $f += "stale file survived the upgrade: foundry-libs\stale-sentinel.dll" }
    if (-not (Test-Path -LiteralPath $userFile)) { $f += "user data was deleted by the upgrade" }
    # @(): Windows PowerShell 5.1 (the GPU box) gives a single object no .Count; PowerShell 7 (CI) does
    $entries = @(Get-UninstallEntries)
    if ($entries.Count -ne 1) { $f += "expected exactly one uninstall entry, found $($entries.Count)" }
    if (-not (Get-HookRegistry)) { $f += "installer hook registry key missing after upgrade" }
    $run = Invoke-Smoke $ctx
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    $f += Assert-PhaseOk $run.Records "ready"
    return @{ Failures = $f }
}

Register "nvidia-present-no-providers" "Pat: CPU model despite NVIDIA (R1, R3)" @("cpu") "driver + NVIDIA adapter present, engine reports only CPU: a named cause (not 'No GPU'), CPU model runs, same answer after a cold relaunch" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $seams = New-GpuSeams -Nvidia
    foreach ($pass in 1, 2) {
        $run = Invoke-Smoke $ctx -EnvVars $seams
        $f += Get-RunFailures $run
        $f += Assert-PhaseOk $run.Records "ready"   # a CPU model is running
        $a = @(Get-GpuAssessments $run.Records)
        if ($a.Count -eq 0) { $f += "run ${pass}: no gpu-assessment record: this build does not explain the GPU decision" }
        elseif (-not ($a | Where-Object { $_ -match "HardwareNotUsed" -and $_ -match "NVIDIA" })) { $f += "run ${pass}: expected HardwareNotUsed naming the NVIDIA adapter, got: $($a -join ' / ')" }
        if ($a | Where-Object { $_ -match "NoHardware" }) { $f += "run ${pass}: an NVIDIA machine was reported as having no GPU" }
        $rep = Get-PreflightReport
        if (-not $rep) { $f += "run ${pass}: no preflight report was written" }
        else {
            if (-not (@($rep.adapters.adapters) | Where-Object { $_.vendor -eq "NVIDIA" })) { $f += "run ${pass}: preflight report does not list the NVIDIA adapter" }
            if (-not (@($rep.engineProviders.Ok) | Where-Object { $_.name -eq "CPUExecutionProvider" })) { $f += "run ${pass}: preflight report does not carry the engine's own provider list" }
        }
        Stop-App
    }
    return @{ Failures = $f }
}

Register "egpu-thunderbolt-hint" "Pat: TBT5-AI (R2)" @("cpu") "NVIDIA adapter plus a Thunderbolt controller, engine reports only CPU: the card tells the user to check the Thunderbolt connection" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $run = Invoke-Smoke $ctx -EnvVars (New-GpuSeams -Nvidia -Thunderbolt)
    $f += Get-RunFailures $run
    $rep = Get-PreflightReport
    if (-not $rep) { return @{ Failures = $f + "no preflight report was written" } }
    if (-not (@($rep.assessment.hints) | Where-Object { $_ -match "Thunderbolt" })) { $f += "no hint mentions the Thunderbolt connection: $(@($rep.assessment.hints) -join ' / ')" }
    if (-not (@($rep.adapters.thunderboltDevices) | Where-Object { $_ -match "Thunderbolt" })) { $f += "the Thunderbolt controller is not in the report" }
    return @{ Failures = $f }
}

Register "engine-declined" "Pat: engine refuses with no reason (rc12, E1/E2)" @("cpu") "NVIDIA adapter present, engine lists CUDA as not registered and answers the registration with an empty refusal: the card names the engine, not the hardware, and points to the engine log" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $seams = New-GpuSeams -Nvidia -Thunderbolt
    # Pat's machine listed some providers twice; the duplicate must be asked for once.
    $seams.PLUGABLE_CHAT_TEST_EPS = '[{"name":"CPUExecutionProvider","isRegistered":true},{"name":"CUDAExecutionProvider","isRegistered":false},{"name":"WebGpuExecutionProvider","isRegistered":false},{"name":"WebGpuExecutionProvider","isRegistered":false}]'
    $seams.PLUGABLE_CHAT_TEST_REGISTRATION = '{"success":false,"status":"","registered":[],"failed":[]}'
    $run = Invoke-Smoke $ctx -EnvVars $seams
    $f += Get-RunFailures $run
    $f += Assert-PhaseOk $run.Records "ready"   # a CPU model still runs
    $rep = Get-PreflightReport
    if (-not $rep) { return @{ Failures = $f + "no preflight report was written" } }
    if ($rep.assessment.state -ne "hardware_not_used") { $f += "expected hardware_not_used, got $($rep.assessment.state): $($rep.assessment.headline)" }
    $detail = @($rep.assessment.detail) -join " / "
    if ($detail -notmatch "declined to register it") { $f += "the refusal is not named in the details: $detail" }
    if (@([regex]::Matches($detail, "Setting up WebGpuExecutionProvider")).Count -ne 1) { $f += "WebGpu should be reported once, not per duplicate: $detail" }
    if (-not (@($rep.assessment.hints) | Where-Object { $_ -match "engine's own log" })) { $f += "no hint points to the engine log: $(@($rep.assessment.hints) -join ' / ')" }
    return @{ Failures = $f }
}

Register "egpu-late-arrival" "Pat: eGPU enumerated late (R2, R4)" @("cpu") "no GPU at launch, an NVIDIA adapter appears later: the app notices, retries GPU setup by itself, and the assessment changes" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $withCuda = '[{"name":"CPUExecutionProvider","isRegistered":true},{"name":"CUDAExecutionProvider","isRegistered":true}]'
    $seams = New-GpuSeams -Nvidia -LateAfterSecs "20" -EpsAfterRetry $withCuda
    # Not --smoke: the watcher polls every 30 s, so the app has to stay up past the first poll.
    $run = Invoke-Smoke $ctx -NoSmoke -EnvVars $seams -TimeoutSec 600 -KillWhen {
        param($recs)
        $a = @($recs | Where-Object { $_.phase -eq "gpu-assessment" } | ForEach-Object { [string]$_.detail })
        # done once the assessment has moved on from "no GPU" to a state that includes the new provider
        ($a | Where-Object { $_ -match "NoHardware" }).Count -gt 0 -and ($a | Where-Object { $_ -match "AcceleratedNoModels|Ready" }).Count -gt 0
    }
    $a = @(Get-GpuAssessments $run.Records)
    if ($a.Count -eq 0) { $f += "no gpu-assessment records: this build never explains the GPU decision" }
    elseif (-not ($a | Where-Object { $_ -match "NoHardware" })) { $f += "the app never reported the starting state (no GPU yet): $($a -join ' / ')" }
    if (-not $run.Killed) { $f += "the app did not notice the late adapter and retry within $($run.Seconds) s: $($a -join ' / ')" }
    return @{ Failures = $f }
}

Register "embedding-first-run-contention" "Bob: first-start embedding error (R6, R10)" @("cpu") "huggingface unreachable for the first 25 s of a first start: the embedding model must still load, not end in a red row" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $backup = Join-Path $ctx.Dir "hosts.backup"
    Copy-Item -LiteralPath $HostsFile -Destination $backup -Force
    $script:contentionUnblocked = $false
    $script:contentionClock = [Diagnostics.Stopwatch]::StartNew()
    try {
        Add-Content -LiteralPath $HostsFile -Value ($HfHosts | ForEach-Object { "127.0.0.1 $_" })
        ipconfig /flushdns | Out-Null
        # The hook runs once a second with the records so far: it lifts the block after 25 s and never kills the app.
        $run = Invoke-Smoke $ctx -EnvVars $HfOnly -KillWhen {
            param($recs)
            if (-not $script:contentionUnblocked -and $script:contentionClock.Elapsed.TotalSeconds -gt 25) {
                Copy-Item -LiteralPath $backup -Destination $HostsFile -Force
                ipconfig /flushdns | Out-Null
                $script:contentionUnblocked = $true
            }
            return $false
        }
    }
    finally { Copy-Item -LiteralPath $backup -Destination $HostsFile -Force; ipconfig /flushdns | Out-Null }
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    $f += Assert-PhaseOk $run.Records "embedding"
    return @{ Failures = $f }
}

Register "embedding-tls-reset-recovers" "rc16 Bob: TLS connection resets (os error 10054) while the search model downloads" @("cpu") "every HTTPS connection to huggingface.co is reset until the quick retries are spent: the error must say the connection was interrupted (not a certificate problem), chat must not be blocked, and the background retry must load the model once the reset stops" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $backup = Join-Path $ctx.Dir "hosts.backup"
    Copy-Item -LiteralPath $HostsFile -Destination $backup -Force
    # A listener on the HTTPS port that closes every connection with a reset (SO_LINGER 0) is what a
    # middlebox cutting the connection looks like to the app: os error 10054 during the TLS handshake.
    $resetter = Start-Job -ScriptBlock {
        $l = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 443)
        $l.Start()
        while ($true) {
            $c = $l.AcceptTcpClient()
            $c.LingerState = [Net.Sockets.LingerOption]::new($true, 0)
            $c.Close()
        }
    }
    $script:resetLifted = $false
    try {
        Add-Content -LiteralPath $HostsFile -Value ($HfHosts | ForEach-Object { "127.0.0.1 $_" })
        ipconfig /flushdns | Out-Null
        # Not --smoke: smoke ends at the first embedding failure, and this scenario is about what happens after it.
        $run = Invoke-Smoke $ctx -NoSmoke -TimeoutSec 900 -EnvVars $HfOnly -KillWhen {
            param($recs)
            $emb = @($recs | Where-Object { $_.phase -eq "embedding" })
            $failedSeen = @($emb | Where-Object { "$($_.status)" -eq "failed" }).Count -gt 0
            if ($failedSeen -and -not $script:resetLifted) {
                # The quick attempts gave up. Stop resetting: the background retry has to notice by itself.
                Stop-Job $resetter -ErrorAction SilentlyContinue
                Copy-Item -LiteralPath $backup -Destination $HostsFile -Force
                ipconfig /flushdns | Out-Null
                $script:resetLifted = $true
            }
            return ($script:resetLifted -and @($emb | Where-Object { "$($_.status)" -eq "ok" }).Count -gt 0)
        }
    }
    finally {
        Stop-Job $resetter -ErrorAction SilentlyContinue
        Remove-Job $resetter -Force -ErrorAction SilentlyContinue
        Copy-Item -LiteralPath $backup -Destination $HostsFile -Force
        ipconfig /flushdns | Out-Null
    }
    if ($run.TimedOut) { $f += "the search model never loaded after the resets stopped ($($run.Seconds) s): no background retry" }
    $emb = @($run.Records | Where-Object { $_.phase -eq "embedding" })
    $failed = @($emb | Where-Object { "$($_.status)" -eq "failed" })
    if ($failed.Count -eq 0) { $f += "no embedding failure was recorded while every connection was being reset" }
    elseif ("$($failed[0].detail)" -notmatch "(?i)interrupted|firewall|antivirus") { $f += "the reset was not described as an interrupted connection: '$($failed[0].detail)'" }
    elseif ("$($failed[0].detail)" -match "(?i)certificate problem") { $f += "a connection reset was reported as a certificate problem: '$($failed[0].detail)'" }
    $f += Assert-PhaseOk $run.Records "embedding"
    if ((Get-EmbeddingFileCount) -lt 1) { $f += "no embedding model files under the user's data directory after recovery" }
    return @{ Failures = $f }
}

Register "embedding-no-huggingface" "rc17 Bob: huggingface.co unreachable for good" @("cpu") "huggingface.co is blocked for the whole run (hosts file): the search model must load from the project's GitHub release asset, and, with the release also unavailable, from a folder the user chose; chat must be usable and embedding must end ok" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $modelRoot = Join-Path $env:LOCALAPPDATA "plugable-chat\data\models\fastembed"
    $f += @(Invoke-WithHostsBlock $ctx {
            # 1. Hugging Face blocked, the release reachable: the model comes from the GitHub release.
            $run = Invoke-Smoke $ctx
            Get-RunFailures $run
            Assert-Terminal $run.Records
            Assert-PhaseOk $run.Records "embedding"
            Assert-PhaseOk $run.Records "ready"
            $e = Get-LastPhase $run.Records "embedding"
            if ($e -and ("$($e.detail)" -notmatch "(?i)release")) { "the search model loaded, but not from the release asset: '$($e.detail)'" }
            if ((Get-EmbeddingFileCount) -lt 1) { "no embedding model files under the user's data directory after loading from the release" }

            # 2. Only a local folder allowed (a user who copied the files by hand): same files, no network at all.
            $pinned = Get-ChildItem -LiteralPath $modelRoot -Directory -Filter "bge-base-en-v1.5-*" -ErrorAction SilentlyContinue | Select-Object -First 1
            if (-not $pinned) { "no pinned model folder under $modelRoot to reuse as the user's folder"; return }
            $userFolder = Join-Path $ctx.Dir "user-model-folder"
            Copy-Item -LiteralPath $pinned.FullName -Destination $userFolder -Recurse -Force
            Stop-App
            Remove-Item -LiteralPath $modelRoot -Recurse -Force -ErrorAction SilentlyContinue
            if ((Get-EmbeddingFileCount) -ne 0) { "could not remove the downloaded model before the folder test" ; return }
            $run2 = Invoke-Smoke $ctx -EnvVars @{ PLUGABLE_CHAT_TEST_EMBEDDING_SOURCES = "local"; PLUGABLE_OFFLINE_MODEL_FOLDER = $userFolder }
            Get-RunFailures $run2
            Assert-Terminal $run2.Records
            Assert-PhaseOk $run2.Records "embedding"
            Assert-PhaseOk $run2.Records "ready"
            $e2 = Get-LastPhase $run2.Records "embedding"
            if ($e2 -and ("$($e2.detail)" -notmatch "(?i)folder you chose")) { "the search model did not come from the folder the user chose: '$($e2.detail)'" }
        })
    return @{ Failures = $f }
}

Register "version-visible" "Bob: no version in the title; Pat: same filename (R12, R13)" @("cpu") "--version reports the release label and commit, and the installer file name carries the same release" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $out = Join-Path $ctx.Dir "version.txt"
    Start-Process $Exe -ArgumentList "--version" -Wait -RedirectStandardOutput $out -RedirectStandardError "$out.err" -WindowStyle Hidden | Out-Null
    $text = if (Test-Path -LiteralPath $out) { Get-Content -LiteralPath $out -Raw } else { "" }
    $name = [IO.Path]::GetFileName($Installer)
    $tag = if ($name -match "_0\.0\.0_") { "" } elseif ($name -match "_(\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?)_") { "v$($Matches[1])" } else { "" }
    # An unsigned CI build has no release tag (its installer is numbered 0.0.0) and calls itself "dev".
    $expectDev = $name -match "_0\.0\.0_"
    if ($expectDev) { if ($text -notmatch "\bdev\b") { $f += "--version printed no 'dev' label for an untagged build: '$($text.Trim())'" } }
    elseif ($text -notmatch "v\d+\.\d+\.\d+") { $f += "--version printed no release label: '$($text.Trim())'" }
    if ($text -match "\+dirty") { $f += "a CI build reports uncommitted changes: '$($text.Trim())'" }
    if ($text -notmatch "\(([0-9a-f]{7})(\+dirty)?\)") { $f += "--version printed no short commit id: '$($text.Trim())'" }
    if ($tag -and $text -notmatch [regex]::Escape($tag)) { $f += "--version says '$($text.Trim())' but the installer file name says $tag" }
    if ($name -match "^plugable-chat_\d+\.\d+\.\d+_x64-setup\.exe$" -and $tag -match "-rc") { $f += "installer name '$name' carries no release candidate label" }
    return @{ Failures = $f }
}

Register "gpu-baseline" "#3" @("gpu") "NVIDIA host with a current driver: the GPU execution provider registers and a model downloads" {
    param($ctx)
    if (-not (Test-HasNvidiaDriver)) { return @{ Skip = "no NVIDIA driver on this host (run on the AWS GPU box)" } }
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $run = Invoke-Smoke $ctx
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    $f += Assert-PhaseOk $run.Records "ep-registration"
    $f += Assert-PhaseOk $run.Records "model-download"
    $f += Assert-PhaseOk $run.Records "ready"
    return @{ Failures = $f }
}

# ------------------------------------------------------------------------------------------
# rc18 Phase B: crash capture, Xet hosts, network drops during use, download recovery
# ------------------------------------------------------------------------------------------
Register "crash-capture" "rc15 David: native crash left no evidence" @("cpu") "the app is killed the way rc15 died (fast-fail at launch): Windows keeps a minidump, and the next launch (told the death was in a GPU provider) writes a last-crash marker that says so and points at the dump" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $werKey = "HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\plugable-chat.exe"
    if (-not (Test-Path $werKey)) { return @{ Failures = @("the installer did not enable WER LocalDumps for plugable-chat.exe ($werKey is missing)") } }
    $dumpDir = Join-Path $env:LOCALAPPDATA "CrashDumps"
    Get-ChildItem -LiteralPath $dumpDir -Filter "plugable-chat.exe*.dmp" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
    $marker = Join-Path $env:APPDATA "plugable-chat\last-crash.json"
    Remove-Item -LiteralPath $marker -Force -ErrorAction SilentlyContinue

    $first = Start-TurnApp $ctx -EnvVars @{ PLUGABLE_CHAT_TEST_FAULT = "crash-at-launch" } -Name "crash"
    if (-not (Wait-AppExit $first 120)) { return @{ Failures = @("the app did not die at launch with the crash fault set") } }
    # 0xC0000409 is the fast-fail code (-1073740791 as a signed exit code)
    if ($first.Process.ExitCode -ne -1073740791) { $f += "the forced crash exited with $($first.Process.ExitCode), expected the fast-fail code 0xC0000409" }
    $dumpSeen = Wait-Until -TimeoutSec 90 -Condition { @(Get-ChildItem -LiteralPath $dumpDir -Filter "plugable-chat.exe*.dmp" -ErrorAction SilentlyContinue).Count -gt 0 }
    if (-not $dumpSeen) { $f += "no plugable-chat.exe minidump appeared under $dumpDir within 90 s of the crash" }

    $second = Start-TurnApp $ctx -EnvVars @{ PLUGABLE_CHAT_TEST_FAULT = "ep-fault" } -Name "after-crash"
    $written = Wait-Until -TimeoutSec 120 -Condition { Test-Path -LiteralPath $marker }
    taskkill /F /T /PID $second.Process.Id 2>&1 | Out-Null
    if (-not $written) { return @{ Failures = @($f + "the launch after the crash wrote no last-crash.json at $marker") } }
    $m = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
    if (-not $m.inEp) { $f += "the marker does not say the crash was inside a GPU provider (inEp=$($m.inEp))" }
    if ($m.provider -ne "webgpu-ep") { $f += "the marker names provider '$($m.provider)', expected webgpu-ep" }
    if ($m.diedIn -ne "startup") { $f += "the marker says the run died in '$($m.diedIn)', expected startup" }
    if (-not $m.dumpPath -or -not (Test-Path -LiteralPath $m.dumpPath)) { $f += "the marker's dump path does not exist: '$($m.dumpPath)'" }
    return @{ Failures = $f }
}

Register "xet-hosts-blocked" "rc18 Pat: cdn-lfs DNS failure advice; the real large-file hosts are the Xet hosts" @("cpu") "only cas-bridge.xethub.hf.co and transfer.xethub.hf.co are blocked (huggingface.co stays reachable): the search model's failure names the Xet hosts, never cdn-lfs, chat stays usable, and once the block is lifted the background retry loads the model" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $backup = Join-Path $ctx.Dir "hosts.backup"
    Copy-Item -LiteralPath $HostsFile -Destination $backup -Force
    $script:xetLifted = $false
    try {
        # huggingface.co now redirects model.onnx to us.aws.cdn.hf.co (xet-bridge-us), not only to the two *.xethub.hf.co hosts (checked on the GPU box 2026-10-28)
        Add-Content -LiteralPath $HostsFile -Value (New-HostsBlockLines -Hosts ((Get-XetHosts) + "us.aws.cdn.hf.co"))
        ipconfig /flushdns | Out-Null
        $run = Invoke-Smoke $ctx -NoSmoke -TimeoutSec 900 -EnvVars $HfOnly -KillWhen {
            param($recs)
            $emb = @($recs | Where-Object { $_.phase -eq "embedding" })
            if (-not $script:xetLifted -and @($emb | Where-Object { "$($_.status)" -eq "failed" }).Count -gt 0) {
                Copy-Item -LiteralPath $backup -Destination $HostsFile -Force
                ipconfig /flushdns | Out-Null
                $script:xetLifted = $true
            }
            return ($script:xetLifted -and @($emb | Where-Object { "$($_.status)" -eq "ok" }).Count -gt 0)
        }
    }
    finally { Copy-Item -LiteralPath $backup -Destination $HostsFile -Force; ipconfig /flushdns | Out-Null }
    if (-not $script:xetLifted) { $f += "the search model never reported a failure while only the Xet hosts were blocked" }
    if ($run.TimedOut) { $f += "the search model did not load after the Xet block was lifted ($($run.Seconds) s)" }
    $failed = @($run.Records | Where-Object { $_.phase -eq "embedding" -and "$($_.status)" -eq "failed" })
    if ($failed.Count -gt 0) {
        if ("$($failed[0].detail)" -match "cdn-lfs") { $f += "the failure still blames the retired cdn-lfs host: '$($failed[0].detail)'" }
        if ("$($failed[0].detail)" -notmatch "(?i)xethub|hugging ?face|interrupted|reach") { $f += "the failure names neither a Xet host nor Hugging Face: '$($failed[0].detail)'" }
    }
    $f += Assert-PhaseOk $run.Records "embedding"
    return @{ Failures = $f }
}

Register "network-drop-mid-turn" "rc15 David: network drop while answering, then every prompt crashed" @("gpu", "net") "the network drops while an answer is being written: that turn ends as a clean network-drop failure (not a crash), the app is still running, the next prompt in the same chat answers, and the chat store reads back whole with the words so far" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $flag = Join-Path $ctx.Dir "offline.flag"
    $long = "Write a very long story, at least 1500 words, about a lighthouse keeper. Do not stop early."
    $app = Start-TurnApp $ctx -Prompts @($long, "Reply with exactly one word: ready") -EnvVars @{ PLUGABLE_CHAT_TEST_OFFLINE_FILE = $flag } -Name "drop"
    try {
        if (-not (Wait-Until -TimeoutSec 1200 -Condition { (Get-RunPhase $app) -eq "chatting" -or $app.Process.HasExited })) { return @{ Failures = @("the app never started answering the first prompt") } }
        if ($app.Process.HasExited) { return @{ Failures = @("the app exited before the drop, code $($app.Process.ExitCode)") } }
        Start-Sleep -Seconds 10   # let some words stream first
        Start-NetworkDrop -FlagFile $flag
        $null = Wait-Until -TimeoutSec 120 -Condition { (Read-ReplyRecords $app.ReplyFile).Count -ge 1 -or $app.Process.HasExited }
        Start-Sleep -Seconds 20
    }
    finally { Stop-NetworkDrop -FlagFile $flag }
    $exited = Wait-AppExit $app 1500
    if (-not $exited) { $f += "the app did not finish the run after the network came back (hung)" }
    $recs = Read-ReplyRecords $app.ReplyFile
    $f += Assert-TurnStoppedByNetworkDrop $recs 0
    $f += Assert-TurnOk $recs 1
    if ($exited -and $app.Process.ExitCode -notin 0, 1) { $f += "the app exited with $($app.Process.ExitCode): a crash, not a clean failed turn" }
    $crashes = Get-AppCrashEvents $app
    if ($crashes.Count -gt 0) { $f += "the app crashed during the drop ($($crashes.Count) Application Error event(s) naming plugable-chat)" }
    if (Test-Path -LiteralPath $app.RunStateFile) { $f += "run-state.json was left behind: the app did not exit cleanly" }
    $f += Get-ChatStoreFailures $ctx $app
    return @{ Failures = $f }
}

Register "network-drop-mid-tool-call" "rc15 David: tool responses lost after a network drop" @("gpu", "net") "the network drops while a tool on a network server is running: the call is recorded as interrupted (not lost, not left as raw text), the turn ends as a clean network-drop failure, the next prompt answers, and the saved chat holds the call and its result" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $flag = Join-Path $ctx.Dir "offline.flag"
    $started = Join-Path $ctx.Dir "tool-started.flag"
    $srv = Start-SlowMcpServer $ctx $started -SleepSeconds 60
    try {
        $app = Start-TurnApp $ctx -Prompts @("Use the slow_lookup tool to look up the key 'alpha' and tell me the value it returns.", "Reply with exactly one word: ready") `
            -EnvVars @{ PLUGABLE_CHAT_TEST_OFFLINE_FILE = $flag } -ExtraArgs @("--mcp-server", "@$($srv.Config)", "--tools", "slow", "--tool-search", "false") -Name "toolcall"
        try {
            if (-not (Wait-Until -TimeoutSec 1200 -Condition { (Test-Path -LiteralPath $started) -or $app.Process.HasExited })) { return @{ Failures = @("the model never called the slow tool") } }
            if ($app.Process.HasExited) { return @{ Failures = @("the app exited before calling the tool, code $($app.Process.ExitCode)") } }
            Start-NetworkDrop -FlagFile $flag
            $null = Wait-Until -TimeoutSec 120 -Condition { (Read-ReplyRecords $app.ReplyFile).Count -ge 1 -or $app.Process.HasExited }
            Start-Sleep -Seconds 15
        }
        finally { Stop-NetworkDrop -FlagFile $flag }
        $exited = Wait-AppExit $app 1500
        if (-not $exited) { $f += "the app did not finish the run after the network came back (hung)" }
        $recs = Read-ReplyRecords $app.ReplyFile
        $f += Assert-TurnStoppedByNetworkDrop $recs 0
        $f += Assert-ToolCallInterrupted $recs 0
        $f += Assert-TurnOk $recs 1
        if (@(Get-AppCrashEvents $app).Count -gt 0) { $f += "the app crashed during the drop" }
        $f += Get-ChatStoreFailures $ctx $app
    }
    finally { Stop-Job $srv.Job -ErrorAction SilentlyContinue; Remove-Job $srv.Job -Force -ErrorAction SilentlyContinue }
    return @{ Failures = $f }
}

Register "foundry-download-resume" "rc18 Foundry model download: a drop mid-download ended it" @("gpu", "net") "the network is blocked for the app for 40 s while a chat model downloads: the download never ends as a failure, and completes after the network is back (retries with backoff; whether bytes already fetched are reused is up to the engine)" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $script:dlBlocked = $false
    # A launch model override never downloads ("Requested model ... is not downloaded"); the first-run download of the default model is
    # what emits model-download, so the cached models from earlier scenarios must go (checked on the GPU box 2026-10-28)
    Remove-Item -LiteralPath (Join-Path $env:USERPROFILE ".foundry") -Recurse -Force -ErrorAction SilentlyContinue
    try {
        $run = Invoke-Smoke $ctx -NoSmoke -TimeoutSec 2400 -KillWhen {
            param($recs)
            $dl = @($recs | Where-Object { $_.phase -eq "model-download" })
            if (-not $script:dlBlocked -and @($dl | Where-Object { "$($_.status)" -in @("started", "progress") }).Count -gt 0) {
                Start-Sleep -Seconds 10   # some bytes first
                Start-NetworkDrop
                $script:dlBlocked = $true
                $script:dlBlockedAt = Get-Date
            }
            if ($script:dlBlocked -and ((Get-Date) - $script:dlBlockedAt).TotalSeconds -gt 40) { Stop-NetworkDrop; $script:dlBlockedAt = [datetime]::MaxValue }
            return (@($dl | Where-Object { "$($_.status)" -eq "ok" }).Count -gt 0)
        }
    }
    finally { Stop-NetworkDrop }
    if (-not $script:dlBlocked) { $f += "never saw the model download start, so nothing was interrupted" }
    if ($run.TimedOut) { $f += "the download did not complete after the network came back ($($run.Seconds) s)" }
    $dl = @($run.Records | Where-Object { $_.phase -eq "model-download" })
    if (@($dl | Where-Object { "$($_.status)" -eq "failed" }).Count -gt 0) { $f += "the download was reported as failed instead of being retried: '$((@($dl | Where-Object { "$($_.status)" -eq "failed" }))[0].detail)'" }
    if (@($dl | Where-Object { "$($_.status)" -eq "ok" }).Count -eq 0) { $f += "the download never finished" }
    $f += @($dl | Where-Object { "$($_.status)" -eq "retrying" } | ForEach-Object { if ("$($_.detail)" -notmatch "attempt \d+ of \d+") { "a retry record does not say which attempt it is: '$($_.detail)'" } })
    return @{ Failures = $f }
}

# ------------------------------------------------------------------------------------------
# driver
# ------------------------------------------------------------------------------------------
if ($List) {
    foreach ($s in $Scenarios.Values) { "{0,-22} {1,-5} {2,-24} {3}" -f $s.Name, ($s.Tags -join ","), $s.Issue, $s.Desc }
    return
}

if ($env:OS -ne "Windows_NT") { throw "faults.ps1 drives a Windows installer; run it on Windows (use -List anywhere)" }
if (-not ($env:GITHUB_ACTIONS -eq "true" -or $env:CLEAN_HOST_DISPOSABLE -eq "1" -or $AllowDestructive)) {
    throw "refusing to run: this edits the hosts file, creates users, mounts disks and uninstalls software. Use a disposable host and set CLEAN_HOST_DISPOSABLE=1 (or pass -AllowDestructive)."
}
$wanted = New-Object System.Collections.ArrayList
foreach ($n in $Scenario) {
    switch ($n) {
        "all" { foreach ($s in $Scenarios.Values) { [void]$wanted.Add($s.Name) } }
        "cpu" { foreach ($s in $Scenarios.Values) { if ($s.Tags -contains "cpu") { [void]$wanted.Add($s.Name) } } }
        "gpu" { foreach ($s in $Scenarios.Values) { if ($s.Tags -contains "gpu") { [void]$wanted.Add($s.Name) } } }
        "net" { foreach ($s in $Scenarios.Values) { if ($s.Tags -contains "net") { [void]$wanted.Add($s.Name) } } }
        default { if (-not $Scenarios.Contains($n)) { throw "unknown scenario '$n'; see -List" }; [void]$wanted.Add($n) }
    }
}
$wanted = @($wanted | Select-Object -Unique)

New-Item -ItemType Directory -Force -Path $OutDir, $WorkRoot | Out-Null
icacls $WorkRoot /grant "Everyone:(OI)(CI)M" | Out-Null   # standard-user scenarios write their state file here
$Installer = (Resolve-Path $Installer).Path
if ($OldInstaller) { $OldInstaller = (Resolve-Path $OldInstaller).Path }

$results = @()
$hostsOriginal = Get-Content -LiteralPath $HostsFile -Raw
try {
    foreach ($name in $wanted) {
        $s = $Scenarios[$name]
        Write-Host "=== $name ($($s.Issue)): $($s.Desc)"
        $ctx = New-ScenarioContext $name
        $sw = [Diagnostics.Stopwatch]::StartNew()
        try { $raw = & $s.Body $ctx }
        catch { $raw = @{ Failures = @("scenario threw: $($_.Exception.Message)") } }
        $sw.Stop()
        # Stray pipeline output from native commands must not be mistaken for the result.
        $outcome = @($raw | Where-Object { $_ -is [hashtable] })[-1]
        if (-not $outcome) { $outcome = @{ Failures = @("scenario produced no result") } }
        $r = @{ Name = $name; Seconds = $sw.Elapsed.TotalSeconds; Failures = @(); Output = "" }
        if ($outcome.Skip) { $r.Status = "skipped"; $r.Failures = @($outcome.Skip) }
        else {
            $observed = @($outcome.Failures | Where-Object { $_ })
            $verdict = Resolve-ScenarioStatus -Failures $observed -ExpectFail ([bool]$ExpectFail)
            $r.Status = $verdict.Status; $r.Failures = $verdict.Failures
            if ($verdict.Status -eq "expected-red") { $r.Output = "observed on this build: " + ($observed -join "; ") }
        }
        Copy-Diagnostics $ctx
        Write-Host ("{0}: {1} {2}" -f $r.Status.ToUpper(), $name, ((@($r.Failures) + $r.Output) -join " | "))
        $results += [pscustomobject]$r
    }
}
finally {
    Set-Content -LiteralPath $HostsFile -Value $hostsOriginal -NoNewline   # belt and braces for an aborted run
    Remove-NetFirewallRule -DisplayName "$NetDropRule*" -ErrorAction SilentlyContinue   # never leave the app blocked
    Stop-App
}

$xml = New-JUnitXml -SuiteName "clean-host$(if ($ExpectFail) { '-expect-fail' })" -Results $results
$junit = Join-Path $OutDir "clean-host-junit.xml"
Set-Content -LiteralPath $junit -Value $xml -Encoding utf8
$failed = @($results | Where-Object { $_.Status -eq "failed" }).Count
Write-Host "`n$($results.Count) scenarios: $(@($results | Where-Object { $_.Status -in 'passed','expected-red' }).Count) as expected, $failed failed, $(@($results | Where-Object { $_.Status -eq 'skipped' }).Count) skipped. JUnit: $junit"
exit $failed
