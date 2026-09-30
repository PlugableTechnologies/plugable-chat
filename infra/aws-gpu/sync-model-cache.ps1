# Make models downloaded by the compiled GPU tests visible to the installed app.
#
# The tests run through SSM as SYSTEM, whose model cache is
# C:\Windows\System32\config\systemprofile\.foundry\cache. The installed app runs as the
# logged-on Administrator and reads C:\Users\Administrator\.foundry\cache, so a model the tests
# downloaded is "Model path does not exist" to the app (seen on 2026-09-30). This copies every
# model directory that is missing on the app side; models already there are left alone.
#
#   sync-model-cache.ps1
param(
    [string]$From = "C:\Windows\System32\config\systemprofile\.foundry\cache",
    [string]$To = "C:\Users\Administrator\.foundry\cache"
)
$ErrorActionPreference = "Continue"
if (-not (Test-Path $From)) { "nothing to sync: $From does not exist"; return }
New-Item -ItemType Directory -Force $To | Out-Null
$copied = 0
foreach ($publisher in Get-ChildItem $From -Directory) {
    foreach ($model in Get-ChildItem $publisher.FullName -Directory) {
        $dest = Join-Path (Join-Path $To $publisher.Name) $model.Name
        if (Test-Path $dest) { "already present: $($publisher.Name)\$($model.Name)"; continue }
        robocopy $model.FullName $dest /E /NFL /NDL /NJH /NJS /nc /ns /np | Out-Null
        if ($LASTEXITCODE -lt 8) { "copied: $($publisher.Name)\$($model.Name)"; $copied++ }
        else { "FAILED (robocopy $LASTEXITCODE): $($publisher.Name)\$($model.Name)" }
    }
}
"sync done, $copied model(s) copied"
