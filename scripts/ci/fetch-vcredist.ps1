# Stage Microsoft's Visual C++ 2015-2022 x64 Redistributable for the Windows installer.
#
#   pwsh scripts/ci/fetch-vcredist.ps1                 # normal: download, verify, stage
#   pwsh scripts/ci/fetch-vcredist.ps1 -UpdatePin      # human step: review, then record the hash
#
# The installer hook (src-tauri/windows/hooks.nsh) runs this file at install time, so the build
# must be certain what it ships. Three checks, all must pass or nothing is staged:
#   1. the SHA-256 equals the one committed in scripts/ci/vcredist.pin.json
#   2. the Authenticode signature is valid
#   3. the signer is Microsoft
# The pin ships as a TODO on purpose: without -UpdatePin this script FAILS until a person has
# looked at a download and committed the hash. -UpdatePin still enforces 2 and 3, then writes the
# hash and file version into the pin file for review in a normal commit.
#
# Not a signing step: the file is already Microsoft-signed and release.yml's smctl step leaves
# validly signed files alone (--skip-signed).
param(
    [string]$PinFile = (Join-Path $PSScriptRoot "vcredist.pin.json"),
    [string]$OutDir = (Join-Path $PSScriptRoot "..\..\src-tauri\windows-redist"),
    [string]$Url = "",
    [switch]$UpdatePin
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # the progress bar makes Invoke-WebRequest ~50x slower
Import-Module (Join-Path $PSScriptRoot "CleanHost.psm1") -Force

$pinState = Test-VcRedistPin -PinFile $PinFile
if (-not $pinState.Ok -and -not $UpdatePin) {
    throw "VC++ redistributable pin is not usable: $($pinState.Reason)"
}
$pin = $pinState.Pin
if (-not $pin) { throw "cannot continue: $($pinState.Reason)" }
if (-not $Url) { $Url = $pin.url }

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$dest = Join-Path $OutDir "vc_redist.x64.exe"

function Assert-MicrosoftSigned($path) {
    $sig = Get-AuthenticodeSignature -LiteralPath $path
    if ($sig.Status -ne "Valid") { throw "Authenticode signature of $path is '$($sig.Status)', expected Valid" }
    $subject = $sig.SignerCertificate.Subject
    if (-not (Test-SignerSubject -Subject $subject -Contains $pin.signerContains)) {
        throw "signer '$subject' does not contain '$($pin.signerContains)'; refusing to stage"
    }
    Write-Host "signature valid: $subject"
}

# Already staged and matching: nothing to do (keeps repeated cargo builds offline-friendly).
if (-not $UpdatePin -and (Test-Path -LiteralPath $dest)) {
    $have = (Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash
    if ($have -ieq $pin.sha256) {
        Assert-MicrosoftSigned $dest
        Write-Host "vc_redist.x64.exe already staged at $dest (hash matches the pin)"
        return
    }
    Write-Host "staged file does not match the pin; re-downloading"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("vc_redist-" + [guid]::NewGuid().ToString("N") + ".exe")
try {
    for ($i = 1; $i -le 4; $i++) {
        try { Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $tmp; break }
        catch { if ($i -eq 4) { throw }; Write-Host "download failed ($($_.Exception.Message)); retry $i"; Start-Sleep -Seconds (5 * $i) }
    }
    Assert-MicrosoftSigned $tmp
    $hash = (Get-FileHash -LiteralPath $tmp -Algorithm SHA256).Hash.ToLowerInvariant()
    $version = (Get-Item -LiteralPath $tmp).VersionInfo.FileVersion

    if ($UpdatePin) {
        $pin.sha256 = $hash
        $pin.version = $version
        if ($Url -ne $pin.url) { $pin.url = $Url }
        $pin | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $PinFile -Encoding utf8
        Write-Host "pin updated: sha256=$hash version=$version"
        Write-Host "Review it (signer above, version, size $((Get-Item $tmp).Length) bytes), set minBld (and PC_VCREDIST_MIN_BLD in src-tauri/windows/hooks.nsh) if needed, then commit $PinFile."
    }
    elseif ($hash -ne $pin.sha256.ToLowerInvariant()) {
        throw "SHA-256 of the download ($hash) does not match the pin ($($pin.sha256)). Microsoft may have published a newer redistributable at $Url; review it and re-run with -UpdatePin."
    }
    Move-Item -LiteralPath $tmp -Destination $dest -Force
    Write-Host "staged $dest (version $version, sha256 $hash)"
}
finally {
    if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue }
}
