# Pure helpers shared by the clean-host scripts (fetch-vcredist.ps1, faults.ps1,
# verify-installers.ps1). Nothing here touches the registry, services or the network, so
# scripts/ci/clean-host.tests.ps1 can run them on macOS and Linux as well as Windows.
Set-StrictMode -Version Latest

# Status values the app writes in PLUGABLE_CHAT_TEST_STATE lines
# ({ts, phase, status, detail}). One place to change if the backend settles on other words.
$script:OkStatuses = @("ok", "done", "success", "ready")
$script:ErrorStatuses = @("error", "failed", "fail")
$script:Phases = @("ep-registration", "embedding", "model-download", "toolbox", "ready", "error")

function Test-Sha256Hex {
    param([string]$Value)
    return [bool]($Value -match '^[0-9a-fA-F]{64}$')
}

# Returns @{ Ok; Reason; Pin } so callers can print one clear sentence. Ok means the pin is a
# real, reviewed hash. A placeholder (the shipped TODO) is NOT ok on purpose.
function Test-VcRedistPin {
    param([Parameter(Mandatory)][string]$PinFile)
    if (-not (Test-Path -LiteralPath $PinFile)) { return @{ Ok = $false; Reason = "pin file $PinFile not found"; Pin = $null } }
    try { $pin = Get-Content -LiteralPath $PinFile -Raw | ConvertFrom-Json } catch { return @{ Ok = $false; Reason = "pin file is not valid JSON: $($_.Exception.Message)"; Pin = $null } }
    foreach ($f in "url", "sha256", "minBld", "signerContains") {
        if (-not ($pin.PSObject.Properties.Name -contains $f)) { return @{ Ok = $false; Reason = "pin file lacks '$f'"; Pin = $pin } }
    }
    if (-not (Test-Sha256Hex $pin.sha256)) {
        return @{ Ok = $false; Reason = "sha256 in the pin is not set ('$($pin.sha256)'); review a download and run fetch-vcredist.ps1 -UpdatePin"; Pin = $pin }
    }
    if ($pin.minBld -isnot [int] -and $pin.minBld -isnot [long]) { return @{ Ok = $false; Reason = "minBld must be a number"; Pin = $pin } }
    return @{ Ok = $true; Reason = ""; Pin = $pin }
}

# The Authenticode subject of the redistributable must name Microsoft. Pure string logic;
# the actual signature validity check (Get-AuthenticodeSignature) stays in the Windows scripts.
function Test-SignerSubject {
    param([string]$Subject, [Parameter(Mandatory)][string]$Contains)
    if ([string]::IsNullOrEmpty($Subject)) { return $false }
    return $Subject.IndexOf($Contains, [StringComparison]::OrdinalIgnoreCase) -ge 0
}

# Reads "!define PC_VCREDIST_MIN_BLD 33135" from the NSIS hook file.
function Get-NshMinBld {
    param([Parameter(Mandatory)][string]$HookFile)
    $m = Select-String -LiteralPath $HookFile -Pattern '^\s*!define\s+PC_VCREDIST_MIN_BLD\s+(\d+)' | Select-Object -First 1
    if (-not $m) { return $null }
    return [int]$m.Matches[0].Groups[1].Value
}

# --- state file (JSON lines written by the app under PLUGABLE_CHAT_TEST_STATE) ---------------
function Read-StateFile {
    param([Parameter(Mandatory)][string]$Path)
    $records = New-Object System.Collections.ArrayList
    if (-not (Test-Path -LiteralPath $Path)) { return , @() }
    foreach ($line in (Get-Content -LiteralPath $Path)) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        try { [void]$records.Add(($line | ConvertFrom-Json)) } catch { [void]$records.Add([pscustomobject]@{ ts = $null; phase = "unparseable"; status = "error"; detail = $line }) }
    }
    return , @($records.ToArray())
}

function Get-LastPhase {
    param($Records, [Parameter(Mandatory)][string]$Phase)
    $hits = @($Records | Where-Object { $_.phase -eq $Phase })
    if ($hits.Count -eq 0) { return $null }
    return $hits[-1]
}

function Test-StatusOk { param($Record) return ($null -ne $Record) -and ($script:OkStatuses -contains ([string]$Record.status).ToLowerInvariant()) }
function Test-StatusError { param($Record) return ($null -ne $Record) -and ($script:ErrorStatuses -contains ([string]$Record.status).ToLowerInvariant()) }

# Each assertion returns a list of failure sentences; an empty list means pass. Scenarios
# concatenate them so one run reports every broken expectation, not just the first.
function Assert-PhaseOk {
    param($Records, [string]$Phase)
    $r = Get-LastPhase $Records $Phase
    if ($null -eq $r) { return @("phase '$Phase' never reported") }
    if (-not (Test-StatusOk $r)) { return @("phase '$Phase' ended with status '$($r.status)': $($r.detail)") }
    return @()
}

function Assert-PhaseError {
    param($Records, [string]$Phase, [string]$DetailPattern = "")
    $r = Get-LastPhase $Records $Phase
    if ($null -eq $r) { return @("phase '$Phase' never reported (expected an explicit error)") }
    if (-not (Test-StatusError $r)) { return @("phase '$Phase' ended with status '$($r.status)', expected an error") }
    if ([string]::IsNullOrWhiteSpace([string]$r.detail)) { return @("phase '$Phase' reported an error with no detail") }
    if ($DetailPattern -and ([string]$r.detail -notmatch $DetailPattern)) { return @("phase '$Phase' error detail '$($r.detail)' does not match /$DetailPattern/") }
    return @()
}

# The app must finish: either 'ready' (ok) or a terminal 'error' record. A silent hang is a failure.
function Assert-Terminal {
    param($Records)
    $ready = Get-LastPhase $Records "ready"
    $err = Get-LastPhase $Records "error"
    if ((Test-StatusOk $ready) -or ($null -ne $err) -or (Test-StatusError $ready)) { return @() }
    return @("the app reached neither 'ready' nor 'error' (hung or crashed without reporting)")
}

# Truthfulness rule for issue #1: a phase may be ok or error but never ok while the artefact it
# claims is missing (the green checkmark on a failed embedding model).
function Assert-EmbeddingTruthful {
    param($Records, [int]$ModelFileCount)
    $e = Get-LastPhase $Records "embedding"
    if ($null -eq $e) { return @("phase 'embedding' never reported") }
    if ((Test-StatusOk $e) -and $ModelFileCount -lt 1) { return @("embedding reported ok but no model files exist in the cache (green check on a failure)") }
    if ((Test-StatusError $e) -and [string]::IsNullOrWhiteSpace([string]$e.detail)) { return @("embedding error has no detail") }
    if (-not (Test-StatusOk $e) -and -not (Test-StatusError $e)) { return @("embedding ended in non-terminal status '$($e.status)'") }
    return @()
}

# --- directory snapshots (nothing may be written under the install dir by running the app) ----
function Get-DirSnapshot {
    param([Parameter(Mandatory)][string]$Path)
    $root = (Resolve-Path -LiteralPath $Path).Path.TrimEnd('\', '/')
    $snap = @{}
    Get-ChildItem -LiteralPath $root -Recurse -File -Force -ErrorAction SilentlyContinue | ForEach-Object {
        $rel = $_.FullName.Substring($root.Length).TrimStart('\', '/').Replace('\', '/')
        $snap[$rel] = "$($_.Length):$($_.LastWriteTimeUtc.Ticks)"
    }
    return $snap
}

function Compare-DirSnapshot {
    param([hashtable]$Before, [hashtable]$After)
    $out = New-Object System.Collections.ArrayList
    foreach ($k in $After.Keys) {
        if (-not $Before.ContainsKey($k)) { [void]$out.Add("added: $k") }
        elseif ($Before[$k] -ne $After[$k]) { [void]$out.Add("modified: $k") }
    }
    foreach ($k in $Before.Keys) { if (-not $After.ContainsKey($k)) { [void]$out.Add("removed: $k") } }
    return , @($out.ToArray() | Sort-Object)
}

# --- JUnit ---------------------------------------------------------------------------------
# Each result: @{ Name; Status = passed|failed|skipped|expected-red; Seconds; Failures = @(); Output }.
# 'expected-red' (the -ExpectFail run observed the failure it should) is a pass in the report;
# the name carries the marker so a reader can tell.
function ConvertTo-XmlText { param([string]$Text) return [System.Security.SecurityElement]::Escape($Text) }

function New-JUnitXml {
    param([Parameter(Mandatory)][string]$SuiteName, [Parameter(Mandatory)]$Results)
    $Results = @($Results)
    $failed = @($Results | Where-Object { $_.Status -eq "failed" }).Count
    $skipped = @($Results | Where-Object { $_.Status -eq "skipped" }).Count
    $time = [math]::Round((($Results | Measure-Object -Property Seconds -Sum).Sum), 1)
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine('<?xml version="1.0" encoding="UTF-8"?>')
    [void]$sb.AppendLine(('<testsuites><testsuite name="{0}" tests="{1}" failures="{2}" skipped="{3}" time="{4}">' -f (ConvertTo-XmlText $SuiteName), $Results.Count, $failed, $skipped, $time))
    foreach ($r in $Results) {
        $name = if ($r.Status -eq "expected-red") { "$($r.Name) [expected red observed]" } else { $r.Name }
        [void]$sb.AppendLine(('  <testcase classname="{0}" name="{1}" time="{2}">' -f (ConvertTo-XmlText $SuiteName), (ConvertTo-XmlText $name), [math]::Round([double]$r.Seconds, 1)))
        if ($r.Status -eq "failed") {
            $msg = (@($r.Failures) -join "; ")
            [void]$sb.AppendLine(('    <failure message="{0}">{1}</failure>' -f (ConvertTo-XmlText $msg), (ConvertTo-XmlText $msg)))
        } elseif ($r.Status -eq "skipped") {
            [void]$sb.AppendLine(('    <skipped message="{0}"/>' -f (ConvertTo-XmlText ((@($r.Failures) -join "; ")))))
        }
        if ($r.Output) { [void]$sb.AppendLine(('    <system-out>{0}</system-out>' -f (ConvertTo-XmlText ([string]$r.Output)))) }
        [void]$sb.AppendLine('  </testcase>')
    }
    [void]$sb.AppendLine('</testsuite></testsuites>')
    return $sb.ToString()
}

# Decide a scenario's reported status from its assertion failures and the -ExpectFail switch.
# Against rc9 every scenario must come out red; a scenario that is green there does not discriminate.
function Resolve-ScenarioStatus {
    param([string[]]$Failures, [bool]$ExpectFail)
    $hasFail = @($Failures).Count -gt 0
    if ($ExpectFail) {
        if ($hasFail) { return @{ Status = "expected-red"; Failures = @() } }
        return @{ Status = "failed"; Failures = @("expected this scenario to FAIL on the old build, but every assertion passed: the test does not reproduce the issue") }
    }
    if ($hasFail) { return @{ Status = "failed"; Failures = @($Failures) } }
    return @{ Status = "passed"; Failures = @() }
}

Export-ModuleMember -Function *-*
