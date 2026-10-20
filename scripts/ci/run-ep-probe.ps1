<#
Runs `plugable-chat --ep-probe` on a CI runner and checks that the experiment harness itself works.

A hosted runner has no GPU, so most hypotheses come back "inconclusive" here; what this proves is
that the probe starts, every engine configuration produced a result, and the report is complete.
It also runs the version comparison (H3): the same experiments against Core 1.2.3 (the version with
the duplicate-provider bug) and the bundled Core, so a Core that cannot be created or changes
behaviour is caught before a build reaches a user.

  pwsh scripts/ci/run-ep-probe.ps1 -Exe target/debug/plugable-chat.exe
#>
param(
    [Parameter(Mandatory)][string]$Exe,
    [string]$OtherCoreVersion = "1.2.3",
    [string]$Out = "ep-probe-out"
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$exePath = (Resolve-Path $Exe).Path
$libs = Join-Path (Split-Path $exePath) "foundry-libs"
if (-not (Test-Path (Join-Path $libs "Microsoft.AI.Foundry.Local.Core.dll"))) { throw "no Core in $libs" }

# Another Core build needs its siblings beside it: copy the bundled folder, then swap the Core in.
$other = Join-Path $root ".ep-probe-core-$OtherCoreVersion"
New-Item -ItemType Directory -Force -Path $other | Out-Null
Copy-Item (Join-Path $libs "*") $other -Force
$stage = Join-Path $root ".ep-probe-stage-$OtherCoreVersion"
pwsh "$PSScriptRoot/fetch-foundry-core.ps1" -Version $OtherCoreVersion -Out $stage -NoExport
Copy-Item (Join-Path $stage "*") $other -Force

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$run = Start-Process -FilePath $exePath -ArgumentList @("--ep-probe", "`"$Out`"", "--ep-probe-other-core", "`"$other`"") -Wait -PassThru -WindowStyle Hidden
if ($run.ExitCode -ne 0) { throw "the probe exited with $($run.ExitCode)" }

$report = Get-ChildItem $Out -Filter "ep-probe-*.json" | Sort-Object LastWriteTime | Select-Object -Last 1
if (-not $report) { throw "the probe wrote no report" }
$doc = Get-Content $report.FullName -Raw | ConvertFrom-Json

$fail = @()
foreach ($label in "ours", "bootstrap", "stock", "other-core") {
    $r = $doc.runs | Where-Object { $_.label -eq $label }
    if (-not $r) { $fail += "no run for '$label'"; continue }
    if ($r.error) { $fail += "run '$label' failed: $($r.error)" }
    elseif (-not $r.attempts -or $r.attempts.Count -lt 1) { $fail += "run '$label' made no attempts" }
}
$ids = @($doc.verdicts | ForEach-Object { $_.id })
foreach ($id in "H1", "H2", "H3", "H4", "H5", "H6", "H7", "H8", "H9") { if ($ids -notcontains $id) { $fail += "no verdict for $id" } }
# The control asks for a name that does not exist: the engine must say which name it did not know.
$control = ($doc.runs | Where-Object { $_.label -eq "ours" }).attempts | Where-Object { $_.step -eq "control: bogus name" }
if ($control -and $control.status -notmatch "NoSuchExecutionProvider") { $fail += "control: the engine did not name the unknown provider: $($control.status)" }

Write-Host "Probe verdicts:"
foreach ($v in $doc.verdicts) { Write-Host ("  {0}  {1,-12} {2}" -f $v.id, $v.state, $v.reason) }
if ($fail.Count) { $fail | ForEach-Object { Write-Error $_ }; exit 1 }
Write-Host "probe harness ok"
