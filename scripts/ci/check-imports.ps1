# Find which DLL is missing a function the test program imports.
# STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) at start-up means one of the program's
# DLLs does not export a function it imports, and Windows does not say which. This
# lists the imports of the program, resolves each DLL the way the loader does (the
# program's own folder first, then System32), and prints every import that DLL lacks.
#
#   pwsh scripts/ci/check-imports.ps1 -Exe target/debug/deps/plugable_chat_lib-<hash>.exe
param([Parameter(Mandatory)][string]$Exe)

$ErrorActionPreference = "Stop"
$exeFull = (Resolve-Path $Exe).Path
$dir = Split-Path $exeFull
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath
$dumpbin = (Get-ChildItem "$vs\VC\Tools\MSVC" -Recurse -Filter dumpbin.exe |
    Where-Object { $_.FullName -match 'Hostx64\\x64' } | Select-Object -First 1).FullName

# imports: { "KERNEL32.dll" = @("GetProcAddress", ...) }
$imports = [ordered]@{}
$current = $null
foreach ($line in (& $dumpbin /imports $exeFull)) {
    if ($line -match '^\s{4}(\S+\.dll)\s*$') { $current = $Matches[1]; $imports[$current] = New-Object System.Collections.ArrayList; continue }
    if ($current -and $line -match '^\s{8,}[0-9A-Fa-f]+\s+([A-Za-z_?@$][\w?@$.]*)\s*$') { [void]$imports[$current].Add($Matches[1]) }
}

$problems = 0
foreach ($name in $imports.Keys) {
    if ($name -like "api-ms-win-*") { continue }          # API-set stubs resolved by the OS
    $path = @((Join-Path $dir $name), (Join-Path $env:WINDIR "System32\$name")) | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $path) { Write-Host "MISSING DLL   $name (not beside the program or in System32)"; $problems++; continue }
    $exports = @{}
    foreach ($line in (& $dumpbin /exports $path)) {
        # Normal exports: "ordinal hint RVA name". Forwarded exports (e.g. kernel32 ->
        # ntdll) have no RVA column: "ordinal hint name (forwarded to ...)".
        if ($line -match '^\s+\d+\s+[0-9A-Fa-f]+\s+(?:[0-9A-Fa-f]{8}\s+)?(\S+)') { $exports[$Matches[1]] = 1 }
    }
    $missing = $imports[$name] | Where-Object { -not $exports.ContainsKey($_) }
    $version = (Get-Item $path).VersionInfo.FileVersion
    if ($missing) {
        Write-Host "MISSING EXPORT $name ($path, $version) lacks $($missing.Count): $(($missing | Select-Object -First 8) -join ', ')"
        $problems++
    } else {
        Write-Host "ok             $name ($($imports[$name].Count) imports) $version"
    }
}
if ($problems -eq 0) { Write-Host "every imported function resolves" }
