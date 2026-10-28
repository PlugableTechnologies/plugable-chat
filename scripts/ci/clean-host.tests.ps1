# Tests for the pure logic behind the clean-host scripts. No Pester, no Windows needed:
#
#   pwsh scripts/ci/clean-host.tests.ps1        # exit code = number of failed checks
#
# Covers the pin check, the hook/pin consistency, state-file assertions, directory snapshots,
# JUnit output and the -ExpectFail verdict. The Windows-only behaviour (installing, registry,
# Authenticode) is exercised by faults.ps1 and verify-installers.ps1 on a Windows host.
$ErrorActionPreference = "Stop"
Import-Module (Join-Path $PSScriptRoot "CleanHost.psm1") -Force
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("clean-host-tests-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmp | Out-Null
$script:failed = 0
function Check([string]$name, [bool]$ok, $detail = "") {
    if (-not $ok) { $script:failed++ }
    "{0}  {1} {2}" -f ($(if ($ok) { "PASS" } else { "FAIL" })), $name, $detail
}
function Rec($phase, $status, $detail = "") { [pscustomobject]@{ ts = "t"; phase = $phase; status = $status; detail = $detail } }
function WritePin($name, $sha) {
    $p = Join-Path $tmp $name
    @{ url = "https://example.invalid/x.exe"; sha256 = $sha; version = "1"; minBld = 33135; signerContains = "Microsoft Corporation" } | ConvertTo-Json | Set-Content $p
    return $p
}

try {
    # --- pin ---
    $shipped = Test-VcRedistPin -PinFile (Join-Path $PSScriptRoot "vcredist.pin.json")
    Check "shipped pin is a TODO and is rejected (until a human pins it)" ((-not $shipped.Ok) -or ($shipped.Pin.sha256 -match '^[0-9a-f]{64}$')) $shipped.Reason
    Check "placeholder hash rejected" (-not (Test-VcRedistPin -PinFile (WritePin "p1.json" "TODO")).Ok)
    Check "63-char hash rejected" (-not (Test-VcRedistPin -PinFile (WritePin "p2.json" ("a" * 63))).Ok)
    Check "64-hex hash accepted" ((Test-VcRedistPin -PinFile (WritePin "p3.json" ("A1" * 32))).Ok)
    Check "missing pin file rejected" (-not (Test-VcRedistPin -PinFile (Join-Path $tmp "nope.json")).Ok)
    Set-Content (Join-Path $tmp "bad.json") "{ not json"
    Check "malformed pin rejected" (-not (Test-VcRedistPin -PinFile (Join-Path $tmp "bad.json")).Ok)

    # fetch-vcredist must refuse before touching the network while the pin is unset
    $pwsh = (Get-Process -Id $PID).Path
    $out = & $pwsh -NoProfile -File (Join-Path $PSScriptRoot "fetch-vcredist.ps1") -PinFile (WritePin "p4.json" "TODO") -OutDir (Join-Path $tmp "out") 2>&1 | Out-String
    Check "fetch-vcredist fails on an unset pin" (($LASTEXITCODE -ne 0) -and ($out -match "pin is not usable")) ($out.Trim().Split("`n")[0])

    # --- hook vs pin consistency ---
    $pinNow = Get-Content (Join-Path $PSScriptRoot "vcredist.pin.json") -Raw | ConvertFrom-Json
    $nshBld = Get-NshMinBld (Join-Path $repo "src-tauri/windows/hooks.nsh")
    Check "hooks.nsh PC_VCREDIST_MIN_BLD equals pin minBld" ($nshBld -eq [int]$pinNow.minBld) "(hook $nshBld, pin $($pinNow.minBld))"
    $conf = Get-Content (Join-Path $repo "src-tauri/tauri.conf.json") -Raw | ConvertFrom-Json
    Check "tauri.conf.json points at the hook file" (Test-Path (Join-Path $repo "src-tauri/$($conf.bundle.windows.nsis.installerHooks)"))
    $winConf = Get-Content (Join-Path $repo "src-tauri/tauri.windows.conf.json") -Raw | ConvertFrom-Json
    Check "windows config bundles windows-redist as redist/" ($winConf.bundle.resources.'windows-redist/*' -eq "redist/")

    # --- network-drop and crash-capture helpers ---
    Check "Wait-Until returns true as soon as the condition holds" (Wait-Until -Condition { $true } -TimeoutSec 5)
    Check "Wait-Until gives up false at the limit" (-not (Wait-Until -Condition { $false } -TimeoutSec 1 -PollSec 0.2))
    $xet = Get-XetHosts
    $blockLines = New-HostsBlockLines -Hosts $xet
    Check "the Xet block names exactly the two Xet hosts" (($blockLines.Count -eq 2) -and ($blockLines -contains "127.0.0.1 cas-bridge.xethub.hf.co") -and ($blockLines -contains "127.0.0.1 transfer.xethub.hf.co"))
    Check "the Xet block leaves huggingface.co and the old cdn-lfs name alone" (-not ($blockLines -match "huggingface\.co|cdn-lfs"))
    $rf = Join-Path $tmp "replies.jsonl"
    @('{"prompt_index":0,"outcome":"ok","detail":"","reply":"three rows","tool_calls":[]}',
      '{"prompt_index":1,"outcome":"error","detail":"network_dropped: the network connection was lost while a tool was running","reply":"","tool_calls":[{"tool":"slow_lookup","is_error":true,"result_summary":"Interrupted: the network connection was lost before this tool finished."}]}',
      '{"prompt_index":2,"outcome":"error","detail":"engine_error: boom","reply":"","tool_calls":[]}',
      'garbage') | Set-Content $rf
    $reps = Read-ReplyRecords $rf
    Check "reply records parse (garbage kept as unparseable)" (($reps.Count -eq 4) -and ($reps[3].outcome -eq "unparseable"))
    Check "a network-drop failure passes the drop assertion" ((Assert-TurnStoppedByNetworkDrop $reps 1).Count -eq 0)
    Check "an ok turn fails the drop assertion" ((Assert-TurnStoppedByNetworkDrop $reps 0).Count -eq 1)
    Check "an engine error is not accepted as a network drop" ((Assert-TurnStoppedByNetworkDrop $reps 2).Count -eq 1)
    Check "a missing turn record fails the drop assertion (the app hung or died)" ((Assert-TurnStoppedByNetworkDrop $reps 9).Count -eq 1)
    Check "Assert-TurnOk passes an ok turn and fails an error turn" (((Assert-TurnOk $reps 0).Count -eq 0) -and ((Assert-TurnOk $reps 1).Count -eq 1))
    Check "an interrupted tool call is recognised" ((Assert-ToolCallInterrupted $reps 1).Count -eq 0)
    Check "a turn with no tool call cannot prove an interrupted one" ((Assert-ToolCallInterrupted $reps 0).Count -eq 1)
    $okRec = { param($i, $o) [pscustomobject]@{ prompt_index = $i; outcome = $o } }
    Check "a failure followed by an ok turn is a recovery" ((Assert-TurnsRecoverAfterFailure @((& $okRec 0 "ok"), (& $okRec 1 "error"), (& $okRec 2 "ok"))).Count -eq 0)
    Check "two failures in a row mean the app did not recover" ((Assert-TurnsRecoverAfterFailure @((& $okRec 0 "error"), (& $okRec 1 "error"))).Count -eq 1)
    Check "a failure with prompts still due and none run is reported" ((Assert-TurnsRecoverAfterFailure @((& $okRec 0 "ok"), (& $okRec 1 "error")) 5).Count -eq 1)
    Check "a failure on the last due prompt is not" ((Assert-TurnsRecoverAfterFailure @((& $okRec 0 "ok"), (& $okRec 1 "error")) 1).Count -eq 0)
    $nshText = Get-Content (Join-Path $repo "src-tauri/windows/hooks.nsh") -Raw
    Check "the installer turns on WER minidumps for plugable-chat.exe" (($nshText -match "LocalDumps") -and ($nshText -match "DumpType" ) -and ($nshText -match "NSIS_HOOK_POSTINSTALL[\s\S]*Call PcEnableCrashDumps"))
    Check "the installer removes the WER key on uninstall" ($nshText -match "NSIS_HOOK_POSTUNINSTALL[\s\S]*DeleteRegKey HKLM `"\$\{PC_WER_KEY\}`"")

    # --- signer ---
    Check "Microsoft signer accepted" (Test-SignerSubject "CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond" "Microsoft Corporation")
    Check "other signer rejected" (-not (Test-SignerSubject "CN=LEANCODE, INC." "Microsoft Corporation"))
    Check "empty signer rejected" (-not (Test-SignerSubject "" "Microsoft Corporation"))

    # --- state file ---
    $sf = Join-Path $tmp "state.jsonl"
    @('{"ts":"1","phase":"ep-registration","status":"ok","detail":""}',
      '{"ts":"2","phase":"embedding","status":"started","detail":""}',
      '',
      '{"ts":"3","phase":"embedding","status":"error","detail":"network unreachable: huggingface.co"}',
      'not json') | Set-Content $sf
    $recs = Read-StateFile $sf
    Check "state file parses 4 records (blank skipped, garbage kept as error)" ($recs.Count -eq 4) "(got $($recs.Count))"
    Check "missing state file reads as empty" ((Read-StateFile (Join-Path $tmp "none")).Count -eq 0)
    Check "last record per phase wins" ((Get-LastPhase $recs "embedding").status -eq "error")
    Check "Assert-PhaseOk passes ok phase" ((Assert-PhaseOk $recs "ep-registration").Count -eq 0)
    Check "Assert-PhaseOk fails error phase" ((Assert-PhaseOk $recs "embedding").Count -eq 1)
    Check "Assert-PhaseOk fails absent phase" ((Assert-PhaseOk $recs "toolbox").Count -eq 1)
    Check "Assert-PhaseError matches detail" ((Assert-PhaseError $recs "embedding" "(?i)network").Count -eq 0)
    Check "Assert-PhaseError rejects wrong detail" ((Assert-PhaseError $recs "embedding" "disk").Count -eq 1)
    Check "Assert-PhaseError rejects an ok phase" ((Assert-PhaseError $recs "ep-registration").Count -eq 1)
    Check "Assert-PhaseError rejects empty detail" ((Assert-PhaseError @((Rec "embedding" "error" "")) "embedding").Count -eq 1)
    Check "Assert-Terminal: ready ok" ((Assert-Terminal @((Rec "ready" "ok"))).Count -eq 0)
    Check "Assert-Terminal: explicit error" ((Assert-Terminal @((Rec "error" "error" "x"))).Count -eq 0)
    Check "Assert-Terminal: hang (only progress)" ((Assert-Terminal @((Rec "embedding" "progress"))).Count -eq 1)
    Check "Assert-Terminal: no records" ((Assert-Terminal @()).Count -eq 1)
    Check "embedding ok with no model files is a lie" ((Assert-EmbeddingTruthful @((Rec "embedding" "ok")) 0).Count -eq 1)
    Check "embedding ok with model files passes" ((Assert-EmbeddingTruthful @((Rec "embedding" "ok")) 2).Count -eq 0)
    Check "embedding error with detail passes" ((Assert-EmbeddingTruthful @((Rec "embedding" "error" "boom")) 0).Count -eq 0)
    Check "embedding stuck in progress fails" ((Assert-EmbeddingTruthful @((Rec "embedding" "progress")) 0).Count -eq 1)
    Check "embedding retrying after a reported error passes" ((Assert-EmbeddingTruthful @((Rec "embedding" "error" "boom"), (Rec "embedding" "started")) 0).Count -eq 0)

    # --- snapshots ---
    $d = Join-Path $tmp "snap"; New-Item -ItemType Directory -Path (Join-Path $d "sub") | Out-Null
    Set-Content (Join-Path $d "a.txt") "a"; Set-Content (Join-Path $d "sub/b.txt") "b"
    $before = Get-DirSnapshot $d
    Check "no change gives no diff" ((Compare-DirSnapshot $before (Get-DirSnapshot $d)).Count -eq 0)
    Set-Content (Join-Path $d "sub/new.txt") "n"; Set-Content (Join-Path $d "a.txt") "changed!"; Remove-Item (Join-Path $d "sub/b.txt")
    $diff = Compare-DirSnapshot $before (Get-DirSnapshot $d)
    Check "snapshot diff sees added, modified and removed" ((($diff -contains "added: sub/new.txt") -and ($diff -contains "modified: a.txt") -and ($diff -contains "removed: sub/b.txt"))) ($diff -join ", ")

    # --- verdict + JUnit ---
    Check "pass when no failures" ((Resolve-ScenarioStatus @() $false).Status -eq "passed")
    Check "fail when failures" ((Resolve-ScenarioStatus @("x") $false).Status -eq "failed")
    Check "ExpectFail: failures are the expected red" ((Resolve-ScenarioStatus @("x") $true).Status -eq "expected-red")
    Check "ExpectFail: green is a failed test (does not discriminate)" ((Resolve-ScenarioStatus @() $true).Status -eq "failed")
    $xml = New-JUnitXml -SuiteName "s & <t>" -Results @(
        @{ Name = "a"; Status = "passed"; Seconds = 1.5; Failures = @(); Output = "" },
        @{ Name = "b"; Status = "failed"; Seconds = 2; Failures = @("bad <thing> & more"); Output = "" },
        @{ Name = "c"; Status = "skipped"; Seconds = 0; Failures = @("no gpu"); Output = "" },
        @{ Name = "d"; Status = "expected-red"; Seconds = 1; Failures = @(); Output = "saw it" })
    $doc = $null
    try { $doc = [xml]$xml } catch { }
    Check "JUnit is well-formed XML with special characters" ($null -ne $doc)
    if ($doc) {
        $s = $doc.testsuites.testsuite
        Check "JUnit counts: 4 tests, 1 failure, 1 skipped" (($s.tests -eq "4") -and ($s.failures -eq "1") -and ($s.skipped -eq "1"))
        Check "JUnit marks the expected-red case" ($xml -match "expected red observed")
    }
}
finally { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }

"clean-host tests: $script:failed failed"
exit $script:failed
