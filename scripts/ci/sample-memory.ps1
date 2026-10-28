<#
.SYNOPSIS
  Samples memory of a running plugable-chat.exe once a second into a CSV (issue 38 soak evidence).

.DESCRIPTION
  Each row: time, the process's private bytes and working set, the machine's committed bytes and commit limit,
  and the first NVIDIA card's used/free memory and utilization from nvidia-smi, plus the sum of the memory
  nvidia-smi attributes to the process. Run it beside a soak, stop it with Ctrl+C or -Seconds.
  Read-only: it changes nothing on the box. Columns that cannot be read are left empty.

.EXAMPLE
  pwsh -File scripts/ci/sample-memory.ps1 -OutFile C:\soak\memory.csv -Seconds 3600
#>
[CmdletBinding()]
param(
    [string]$ProcessName = 'plugable-chat',
    [string]$OutFile = (Join-Path (Get-Location) ("memory-{0}.csv" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))),
    [int]$IntervalSeconds = 1,
    [int]$Seconds = 0   # 0 = until stopped
)

$ErrorActionPreference = 'Continue'
$MiB = 1MB

$header = 'time_utc,pid,private_mb,working_set_mb,committed_mb,commit_limit_mb,gpu_used_mb,gpu_free_mb,gpu_util_pct,gpu_process_mb'
Set-Content -Path $OutFile -Value $header -Encoding ascii
Write-Host "Sampling '$ProcessName' every ${IntervalSeconds}s to $OutFile"

function Get-CommitMb {
    # Win32_OperatingSystem: TotalVirtualMemorySize is the commit limit, FreeVirtualMemory what is left (KB).
    $os = Get-CimInstance -ClassName Win32_OperatingSystem -ErrorAction SilentlyContinue
    if (-not $os) { return @($null, $null) }
    $limit = [math]::Round($os.TotalVirtualMemorySize / 1024)
    $committed = $limit - [math]::Round($os.FreeVirtualMemory / 1024)
    return @($committed, $limit)
}

function Get-GpuSample([int]$ProcessId) {
    $smi = Get-Command nvidia-smi -ErrorAction SilentlyContinue
    if (-not $smi) { return @($null, $null, $null, $null) }
    $card = & nvidia-smi --query-gpu=memory.used,memory.free,utilization.gpu --format=csv,noheader,nounits 2>$null |
        Select-Object -First 1
    $used = $free = $util = $null
    if ($card -and $card -match '^\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)') {
        $used = [int]$Matches[1]; $free = [int]$Matches[2]; $util = [int]$Matches[3]
    }
    $procMb = $null
    $rows = & nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits 2>$null
    if ($rows) {
        $sum = 0
        foreach ($r in $rows) {
            if ($r -match '^\s*(\d+)\s*,\s*(\d+)') { if ([int]$Matches[1] -eq $ProcessId) { $sum += [int]$Matches[2] } }
        }
        $procMb = $sum
    }
    return @($used, $free, $util, $procMb)
}

$stop = if ($Seconds -gt 0) { (Get-Date).AddSeconds($Seconds) } else { $null }
while (-not $stop -or (Get-Date) -lt $stop) {
    $t0 = Get-Date
    $p = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Sort-Object WorkingSet64 -Descending | Select-Object -First 1
    $procId = $null; $priv = $null; $ws = $null
    if ($p) {
        $procId = $p.Id
        $priv = [math]::Round($p.PrivateMemorySize64 / $MiB)
        $ws = [math]::Round($p.WorkingSet64 / $MiB)
    }
    $commit = Get-CommitMb
    $gpu = Get-GpuSample $procId
    $row = @((Get-Date).ToUniversalTime().ToString('o'), $procId, $priv, $ws, $commit[0], $commit[1], $gpu[0], $gpu[1], $gpu[2], $gpu[3]) -join ','
    Add-Content -Path $OutFile -Value $row -Encoding ascii
    $sleep = $IntervalSeconds - ((Get-Date) - $t0).TotalSeconds
    if ($sleep -gt 0) { Start-Sleep -Milliseconds ([int]($sleep * 1000)) }
}
