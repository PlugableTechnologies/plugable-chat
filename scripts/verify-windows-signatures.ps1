# Fail unless every given file carries a valid Authenticode signature from our
# EV certificate, with a countersignature timestamp (so it stays valid after the
# certificate expires). Run in CI after signing; also usable by hand on a
# downloaded installer.
#
#   pwsh scripts/verify-windows-signatures.ps1 -Path a.exe,b.msi -ExpectedSubject "LEANCODE, INC."
param(
    [Parameter(Mandatory = $true)][string[]]$Path,
    [string]$ExpectedSubject = "LEANCODE, INC."
)

$ErrorActionPreference = "Stop"
$failed = $false

foreach ($file in $Path) {
    $sig = Get-AuthenticodeSignature -LiteralPath $file
    $subject = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { "" }
    $problems = @()

    if ($sig.Status -ne "Valid") { $problems += "status is $($sig.Status)" }
    if ($subject -notlike "*$ExpectedSubject*") { $problems += "signer is '$subject'" }
    if (-not $sig.TimeStamperCertificate) { $problems += "no timestamp countersignature" }

    if ($problems.Count -gt 0) {
        $failed = $true
        Write-Host "FAIL  $file : $($problems -join '; ')"
    } else {
        Write-Host "OK    $file : $subject (timestamp by $($sig.TimeStamperCertificate.Subject))"
    }
}

if ($failed) { exit 1 }
