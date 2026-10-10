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
# Written on macOS and validated there only by a PowerShell parse and -List; see
# docs/clean-host-testing.md for what has actually been exercised on Windows.
param(
    [Parameter(Mandatory)][string]$Installer,
    [string[]]$Scenario = @("cpu"),            # names, or "cpu" (runnable on windows-latest), "gpu", "all"
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
          [scriptblock]$KillWhen, [int]$TimeoutSec = $SmokeTimeoutSec)
    $Ctx.Runs++
    $idx = $Ctx.Runs
    Remove-Item -LiteralPath $Ctx.StateFile -Force -ErrorAction SilentlyContinue
    $out = Join-Path $Ctx.Dir "smoke-$idx.out.txt"
    $vars = @{ PLUGABLE_CHAT_TEST_STATE = $Ctx.StateFile; PLUGABLE_CHAT_SMOKE_TIMEOUT_SECS = [string][math]::Max(30, $TimeoutSec - 30) }
    foreach ($k in $EnvVars.Keys) { $vars[$k] = $EnvVars[$k] }

    if ($Credential) {
        $cmd = Join-Path $Ctx.Dir "run-smoke-$idx.cmd"
        $lines = @("@echo off")
        foreach ($k in $vars.Keys) { $lines += "set `"$k=$($vars[$k])`"" }
        $lines += "cd /d `"$WorkingDirectory`""
        $lines += "`"$Exe`" --smoke > `"$out`" 2>&1"
        Set-Content -LiteralPath $cmd -Value $lines -Encoding ASCII
        $proc = Start-Process cmd.exe -ArgumentList "/c `"$cmd`"" -Credential $Credential -LoadUserProfile -WorkingDirectory $env:SystemRoot -PassThru -WindowStyle Hidden
    }
    else {
        $saved = @{}
        foreach ($k in $vars.Keys) { $saved[$k] = [Environment]::GetEnvironmentVariable($k, "Process"); [Environment]::SetEnvironmentVariable($k, [string]$vars[$k], "Process") }
        try { $proc = Start-Process $Exe -ArgumentList "--smoke" -WorkingDirectory $WorkingDirectory -PassThru -WindowStyle Hidden -RedirectStandardOutput $out -RedirectStandardError "$out.err" }
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

Register "readonly-nonadmin" "#1" @("cpu") "standard user, read-only install dir, cwd = install dir (a relative .fastembed_cache must not matter)" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $name = "faultstd"
    $cred = New-TestUser $name
    try {
        $acl = (icacls $InstallDir) -join "`n"
        if ($acl -match 'BUILTIN\\Users:.*\((M|F)\)') { $f += "install dir is writable by standard users; the fault is not in place" }
        $before = Get-DirSnapshot $InstallDir
        $run = Invoke-Smoke $ctx -Credential $cred -WorkingDirectory $InstallDir
        $f += Get-RunFailures $run
        $f += Assert-Terminal $run.Records
        $f += Assert-PhaseOk $run.Records "embedding"
        $profileDir = Get-ChildItem (Join-Path $env:SystemDrive "Users") -Directory | Where-Object { $_.Name -like "$name*" } | Select-Object -First 1
        $modelRoot = if ($profileDir) { Join-Path $profileDir.FullName "AppData\Local\plugable-chat" } else { "" }
        if (-not $modelRoot -or (Get-EmbeddingFileCount $modelRoot) -lt 1) { $f += "no embedding model files under the standard user's profile" }
        $changes = Compare-DirSnapshot $before (Get-DirSnapshot $InstallDir)
        if ($changes.Count) { $f += "install dir changed while running: $(($changes | Select-Object -First 5) -join '; ')" }
    }
    finally { Remove-TestUser $name }
    return @{ Failures = $f }
}

Register "offline-hosts-block" "#1 #3" @("cpu") "huggingface.co blocked via the hosts file: embedding must report a truthful error, never a green check" {
    param($ctx)
    $f = @(Start-Fresh $ctx); if ($f.Count) { return @{ Failures = $f } }
    $f += @(Invoke-WithHostsBlock $ctx {
            $run = Invoke-Smoke $ctx
            Get-RunFailures $run
            Assert-Terminal $run.Records
            Assert-EmbeddingTruthful $run.Records (Get-EmbeddingFileCount)
            # With no model bundled this must be an error that names the cause. (If the embedding
            # model is later bundled in the installer, flip this to Assert-PhaseOk.)
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
    $f += Assert-EmbeddingTruthful $run.Records (Get-EmbeddingFileCount)
    $f += Assert-PhaseError $run.Records "embedding" "(?i)proxy|connect|refused|network|unreachable"
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
    if (-not $healed -and -not (Test-StatusOk $ready)) { $f += Assert-PhaseError $run.Records "error" "(?i)onnxruntime|dll|foundry-libs|reinstall|runtime" }
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
    if ((Get-UninstallEntries).Count -ne 1) { $f += "expected exactly one uninstall entry, found $((Get-UninstallEntries).Count)" }
    if (-not (Get-HookRegistry)) { $f += "installer hook registry key missing after upgrade" }
    $run = Invoke-Smoke $ctx
    $f += Get-RunFailures $run
    $f += Assert-Terminal $run.Records
    $f += Assert-PhaseOk $run.Records "ready"
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
    Stop-App
}

$xml = New-JUnitXml -SuiteName "clean-host$(if ($ExpectFail) { '-expect-fail' })" -Results $results
$junit = Join-Path $OutDir "clean-host-junit.xml"
Set-Content -LiteralPath $junit -Value $xml -Encoding utf8
$failed = @($results | Where-Object { $_.Status -eq "failed" }).Count
Write-Host "`n$($results.Count) scenarios: $(@($results | Where-Object { $_.Status -in 'passed','expected-red' }).Count) as expected, $failed failed, $(@($results | Where-Object { $_.Status -eq 'skipped' }).Count) skipped. JUnit: $junit"
exit $failed
