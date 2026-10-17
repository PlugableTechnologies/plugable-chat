# Collects what Windows, the NVIDIA driver, Windows ML and Foundry Local each say about the GPU, in one
# text file for support. Read-only: it installs, changes and uploads nothing.
#
#   powershell -ExecutionPolicy Bypass -File gpu-ground-truth.ps1
#
# Written for Pat's RTX 5080 in a TBT5-AI (rc12), where the `foundry` CLI used the GPU and Plugable Chat's
# engine registered no provider; the answer differs by layer, so every layer is asked separately.

$ErrorActionPreference = "Continue"
$stamp = Get-Date -Format "yyyy-MM-dd-HHmmss"
$out = Join-Path ([Environment]::GetFolderPath("Desktop")) "$stamp-plugable-chat-gpu-ground-truth.txt"
$lines = New-Object System.Collections.Generic.List[string]

function Section([string]$title) { $lines.Add(""); $lines.Add("=== $title") }
function Add-Output([scriptblock]$block) {
    try { $text = (& $block | Out-String -Width 220).TrimEnd() } catch { $text = "ERROR: $($_.Exception.Message)" }
    $lines.Add($(if ($text) { $text } else { "(nothing)" }))
}
function Add-Tail([string]$dir, [int]$n) {
    $f = Get-ChildItem -LiteralPath $dir -File -Recurse -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $f) { $lines.Add("(no files in $dir)"); return }
    $lines.Add("--- last $n lines of $($f.FullName)")
    Add-Output { Get-Content -LiteralPath $f.FullName -Tail $n }
}

Section "Windows"
Add-Output { Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber, OSArchitecture | Format-List }

Section "Display adapters (Windows' order)"
Add-Output { Get-CimInstance Win32_VideoController | Select-Object Name, AdapterCompatibility, DriverVersion, PNPDeviceID, Status, ConfigManagerErrorCode | Format-List }

Section "Real video memory (AdapterRAM above is capped at 4 GB)"
Add-Output {
    Get-ChildItem "HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}" -ErrorAction SilentlyContinue |
        ForEach-Object { Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue } |
        Where-Object { $_.DriverDesc } |
        ForEach-Object { "{0}: {1:N0} MB" -f $_.DriverDesc, ([uint64]$_."HardwareInformation.qwMemorySize" / 1MB) }
}

Section "NVIDIA driver (nvidia-smi)"
Add-Output { nvidia-smi --query-gpu=name,driver_version,compute_cap,pci.bus_id,memory.total --format=csv }

Section "Thunderbolt / USB4 devices"
Add-Output { Get-PnpDevice -PresentOnly -ErrorAction SilentlyContinue | Where-Object { $_.FriendlyName -match 'Thunderbolt|USB4' } | Select-Object Status, Class, FriendlyName | Format-Table -AutoSize }

Section "Windows graphics preference per app"
Add-Output { Get-ItemProperty "HKCU:\Software\Microsoft\DirectX\UserGpuPreferences" -ErrorAction SilentlyContinue | Format-List }

Section "Windows App SDK, Windows ML and execution-provider packages"
Add-Output { Get-AppxPackage -Name "*WindowsAppRuntime*" | Select-Object Name, Version | Sort-Object Name | Format-Table -AutoSize }
Add-Output { Get-AppxPackage | Where-Object { $_.Name -match 'WindowsML|MachineLearning|ExecutionProvider|Foundry' } | Select-Object Name, Version | Format-Table -AutoSize }

Section "Foundry Local CLI"
Add-Output { foundry --version }
Add-Output { foundry service status }

Section "Plugable Chat settings that change GPU behaviour"
Add-Output { Get-ChildItem Env: | Where-Object { $_.Name -like "PLUGABLE_*" } | Format-Table -AutoSize }

Section "Plugable Chat logs"
$chatLogs = Join-Path $env:APPDATA "plugable-chat\logs"
Add-Output { Get-ChildItem -LiteralPath $chatLogs -Recurse -File -ErrorAction SilentlyContinue | Select-Object LastWriteTime, Length, FullName | Format-Table -AutoSize }
Add-Tail (Join-Path $chatLogs "foundry") 80
$appLog = Join-Path $chatLogs "plugable-chat.log"
if (Test-Path -LiteralPath $appLog) {
    $lines.Add("--- [gpu] lines of $appLog")
    Add-Output { Select-String -LiteralPath $appLog -Pattern '\[gpu\]|\[preflight\]' | Select-Object -Last 60 | ForEach-Object { $_.Line } }
}

Section "Foundry Local's own logs"
Add-Tail (Join-Path $env:USERPROFILE ".foundry\logs") 60

$lines | Set-Content -LiteralPath $out -Encoding UTF8
Write-Host "Saved to $out. Please attach that file to your reply."
