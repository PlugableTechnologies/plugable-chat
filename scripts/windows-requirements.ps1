#Requires -Version 5.1
<#
.SYNOPSIS
    Installs development dependencies for plugable-chat on Windows.

.DESCRIPTION
    This script uses winget to check for and install required dependencies
    in an idempotent manner. It will skip already-installed packages.

.PARAMETER Check
    Run diagnostic checks only without installing anything.

.PARAMETER Diagnose
    Alias for -Check. Run diagnostic checks only.

.NOTES
    DO NOT RUN THIS SCRIPT DIRECTLY!
    
    Use the requirements.bat wrapper in the project root instead:
        requirements.bat
    
    Running this script directly may cause permission issues (UAC dialogs
    appearing behind windows, making install appear to hang).
    
    The .bat wrapper ensures proper execution policy and permissions.
#>

param(
    [switch]$Check,
    [switch]$Diagnose
)

$ErrorActionPreference = "Stop"

# Warn if running directly (not via requirements.bat from project root)
$scriptDir = Split-Path -Parent $PSCommandPath
$projectRoot = Split-Path -Parent $scriptDir
$expectedBat = Join-Path $projectRoot "requirements.bat"

if (-not (Test-Path $expectedBat)) {
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Red
    Write-Host "  WARNING: Running script directly!    " -ForegroundColor Red
    Write-Host "========================================" -ForegroundColor Red
    Write-Host ""
    Write-Host "This script should be run via requirements.bat in the project root." -ForegroundColor Yellow
    Write-Host "Running directly may cause permission issues (UAC dialogs behind windows)." -ForegroundColor Yellow
    Write-Host ""
    Write-Host "Please use: requirements.bat" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Press Enter to continue anyway, or Ctrl+C to cancel..."
    Read-Host
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Plugable Chat - Windows Requirements  " -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# Track what was newly installed (for PATH guidance)
$script:InstalledNode = $false
$script:InstalledRust = $false
$script:InstalledGit = $false
$script:InstalledProtoc = $false
$script:InstalledBuildTools = $false
$script:InstalledToolbox = $false
$script:InstalledAnything = $false

# =============================================================================
# ERROR HANDLING AND USER GUIDANCE
# =============================================================================

# Display a blocking error with clear remediation steps and exit
function Show-BlockingError {
    param(
        [string]$ErrorCode,
        [string]$Title,
        [string]$Description,
        [string[]]$Steps,
        [string]$HelpUrl = ""
    )
    
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Red
    Write-Host "  INSTALLATION CANNOT CONTINUE         " -ForegroundColor Red
    Write-Host "  Error Code: $ErrorCode               " -ForegroundColor Red
    Write-Host "========================================" -ForegroundColor Red
    Write-Host ""
    Write-Host "$Title" -ForegroundColor Yellow
    Write-Host ""
    Write-Host $Description -ForegroundColor White
    Write-Host ""
    Write-Host "To fix this issue:" -ForegroundColor Cyan
    $stepNum = 1
    foreach ($step in $Steps) {
        Write-Host "  $stepNum. $step" -ForegroundColor White
        $stepNum++
    }
    if ($HelpUrl) {
        Write-Host ""
        Write-Host "For more information: $HelpUrl" -ForegroundColor Gray
    }
    Write-Host ""
    Write-Host "After completing these steps, re-run: .\requirements.bat" -ForegroundColor Green
    Write-Host ""
    exit 1
}

# =============================================================================
# PREREQUISITE VALIDATION
# =============================================================================

# Comprehensive prerequisite checks before any installations
function Test-Prerequisites {
    Write-Host ""
    Write-Host "Running prerequisite checks..." -ForegroundColor Cyan
    Write-Host ""
    
    # Check 1: Windows version (require Windows 10 1809+ or Windows 11)
    Write-Host "  Checking Windows version... " -NoNewline
    $osVersion = [System.Environment]::OSVersion.Version
    if ($osVersion.Major -lt 10 -or ($osVersion.Major -eq 10 -and $osVersion.Build -lt 17763)) {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_WINDOWS_VERSION" `
            -Title "Windows version too old" `
            -Description "Plugable Chat requires Windows 10 version 1809 (build 17763) or later. Current build: $($osVersion.Build)" `
            -Steps @(
                "Update Windows to version 1809 or later via Settings > Windows Update",
                "Restart your computer after updating"
            )
    }
    Write-Host "OK (Build $($osVersion.Build))" -ForegroundColor Green
    
    # Check 2: winget availability
    Write-Host "  Checking winget availability... " -NoNewline
    if (-not (Test-Winget)) {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_WINGET_MISSING" `
            -Title "Windows Package Manager (winget) not found" `
            -Description "winget is required but not installed on this system." `
            -Steps @(
                "Open Microsoft Store",
                "Search for 'App Installer'",
                "Install or update 'App Installer' by Microsoft Corporation",
                "Close and reopen your terminal",
                "Re-run this script"
            ) `
            -HelpUrl "https://aka.ms/getwinget"
    }
    Write-Host "OK" -ForegroundColor Green
    
    # Check 3: Disk space (require at least 10GB free on system drive)
    Write-Host "  Checking disk space... " -NoNewline
    try {
        $systemDriveLetter = $env:SystemDrive[0]
        $drive = Get-PSDrive -Name $systemDriveLetter -ErrorAction Stop
        $freeSpaceGB = [math]::Round($drive.Free / 1GB, 1)
        if ($drive.Free -lt 10GB) {
            Write-Host "FAILED" -ForegroundColor Red
            Show-BlockingError -ErrorCode "ERR_DISK_SPACE" `
                -Title "Insufficient disk space" `
                -Description "Only $freeSpaceGB GB free on $env:SystemDrive. At least 10 GB required for Visual Studio Build Tools, Rust, and other dependencies." `
                -Steps @(
                    "Free up disk space using Windows Disk Cleanup",
                    "Or run: cleanmgr /d $systemDriveLetter",
                    "Consider moving large files to another drive"
                )
        }
        Write-Host "OK ($freeSpaceGB GB free)" -ForegroundColor Green
    }
    catch {
        Write-Host "SKIPPED (could not check)" -ForegroundColor Yellow
    }
    
    # Check 4: Network connectivity
    Write-Host "  Checking network connectivity... " -NoNewline
    $testUrls = @(
        @{ Url = "https://github.com"; Name = "GitHub" },
        @{ Url = "https://aka.ms"; Name = "Microsoft" }
    )
    $networkOk = $false
    $testedUrls = @()
    foreach ($test in $testUrls) {
        try {
            $response = Invoke-WebRequest -Uri $test.Url -UseBasicParsing -TimeoutSec 10 -Method Head -ErrorAction Stop
            if ($response.StatusCode -eq 200) { 
                $networkOk = $true
                break 
            }
        }
        catch {
            $testedUrls += $test.Name
        }
    }
    if (-not $networkOk) {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_NETWORK" `
            -Title "No internet connection" `
            -Description "Cannot reach GitHub or Microsoft servers. Tested: $($testedUrls -join ', ')" `
            -Steps @(
                "Check your internet connection",
                "If behind a corporate proxy, configure system proxy settings",
                "If on a corporate network, contact IT for firewall exceptions to github.com and aka.ms",
                "Try disabling VPN temporarily"
            )
    }
    Write-Host "OK" -ForegroundColor Green
    
    # Check 5: Visual Studio Installer not running (would conflict)
    Write-Host "  Checking for conflicting processes... " -NoNewline
    $vsInstallerRunning = Get-Process -Name "vs_installer", "vs_installershell" -ErrorAction SilentlyContinue
    if ($vsInstallerRunning) {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_VS_INSTALLER_RUNNING" `
            -Title "Visual Studio Installer is running" `
            -Description "Close Visual Studio Installer before continuing. It may be performing updates in the background." `
            -Steps @(
                "Close all Visual Studio Installer windows",
                "Check the system tray for Visual Studio Installer",
                "Wait for any pending updates to complete",
                "Re-run this script"
            )
    }
    Write-Host "OK" -ForegroundColor Green
    
    # Check 6: Not running as SYSTEM or with broken user profile
    Write-Host "  Checking user environment... " -NoNewline
    if ([string]::IsNullOrEmpty($env:USERPROFILE) -or -not (Test-Path $env:USERPROFILE)) {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_USER_PROFILE" `
            -Title "User profile not accessible" `
            -Description "Cannot access user profile directory. This may happen when running as SYSTEM or with a corrupted profile." `
            -Steps @(
                "Run this script as a regular user, not as SYSTEM",
                "Ensure your user profile exists at $env:USERPROFILE",
                "Try logging out and back in"
            )
    }
    Write-Host "OK" -ForegroundColor Green
    
    Write-Host ""
    Write-Host "  All prerequisite checks passed!" -ForegroundColor Green
    Write-Host ""
}

# =============================================================================
# DIAGNOSTIC MODE
# =============================================================================

function Show-DiagnosticReport {
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host "  Plugable Chat - Diagnostic Report    " -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host ""
    
    # System info
    Write-Host "System Information:" -ForegroundColor White
    Write-Host "  Windows:      $([System.Environment]::OSVersion.VersionString)" -ForegroundColor Gray
    Write-Host "  Build:        $([System.Environment]::OSVersion.Version.Build)" -ForegroundColor Gray
    Write-Host "  Architecture: $env:PROCESSOR_ARCHITECTURE" -ForegroundColor Gray
    Write-Host "  User Profile: $env:USERPROFILE" -ForegroundColor Gray
    Write-Host ""
    
    # Disk space
    Write-Host "Disk Space:" -ForegroundColor White
    try {
        $systemDriveLetter = $env:SystemDrive[0]
        $drive = Get-PSDrive -Name $systemDriveLetter -ErrorAction Stop
        $freeSpaceGB = [math]::Round($drive.Free / 1GB, 1)
        $usedSpaceGB = [math]::Round($drive.Used / 1GB, 1)
        Write-Host "  $env:SystemDrive Free: $freeSpaceGB GB" -ForegroundColor Gray
    }
    catch {
        Write-Host "  Could not check disk space" -ForegroundColor Yellow
    }
    Write-Host ""
    
    # Component status
    Write-Host "Component Status:" -ForegroundColor White
    
    # winget
    Write-Host "  winget:       " -NoNewline
    if (Test-Winget) {
        try {
            $wingetVersion = (winget --version 2>&1) -replace 'v', ''
            Write-Host "$wingetVersion" -ForegroundColor Green
        }
        catch {
            Write-Host "installed (version unknown)" -ForegroundColor Green
        }
    }
    else {
        Write-Host "NOT INSTALLED" -ForegroundColor Red
    }
    
    # Node.js
    Write-Host "  node:         " -NoNewline
    if (Test-CommandExists "node") {
        $version = node --version 2>&1
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
    }
    
    # npm
    Write-Host "  npm:          " -NoNewline
    if (Test-CommandExists "npm") {
        $version = npm --version 2>&1
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
    }
    
    # Rust
    Write-Host "  rustc:        " -NoNewline
    if (Test-CommandExists "rustc") {
        $version = (rustc --version 2>&1) -split " " | Select-Object -Index 1
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
    }
    
    # Cargo
    Write-Host "  cargo:        " -NoNewline
    if (Test-CommandExists "cargo") {
        $version = (cargo --version 2>&1) -split " " | Select-Object -Index 1
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
    }
    
    # rustup
    Write-Host "  rustup:       " -NoNewline
    if (Test-CommandExists "rustup") {
        $previousErrorPreference = $ErrorActionPreference
        try {
            $ErrorActionPreference = "Continue"
            $versionOutput = rustup --version 2>$null
            $version = ($versionOutput -split ' ')[1]
            Write-Host "$version" -ForegroundColor Green
        }
        catch {
            Write-Host "installed" -ForegroundColor Green
        }
        finally {
            $ErrorActionPreference = $previousErrorPreference
        }
    }
    else {
        Write-Host "not found" -ForegroundColor Red
    }
    
    # Git
    Write-Host "  git:          " -NoNewline
    if (Test-CommandExists "git") {
        $version = (git --version 2>&1) -replace "git version ", ""
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
    }
    
    # protoc
    Write-Host "  protoc:       " -NoNewline
    if (Test-CommandExists "protoc") {
        $version = (protoc --version 2>&1) -split " " | Select-Object -Last 1
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
    }
    
    # Foundry Local
    Write-Host "  foundry:      " -NoNewline
    if (Test-CommandExists "foundry") {
        Write-Host "installed" -ForegroundColor Green
    }
    else {
        # Check known paths
        $foundryPaths = @(
            "$env:LOCALAPPDATA\Programs\Microsoft\FoundryLocal\foundry.exe",
            "$env:ProgramFiles\Microsoft\FoundryLocal\foundry.exe"
        )
        $found = $false
        foreach ($p in $foundryPaths) {
            if (Test-Path $p) {
                Write-Host "installed (not in PATH)" -ForegroundColor Yellow
                $found = $true
                break
            }
        }
        if (-not $found) {
            Write-Host "not found" -ForegroundColor Red
        }
    }
    
    # toolbox
    Write-Host "  toolbox:      " -NoNewline
    if (Test-CommandExists "toolbox") {
        try {
            $null = toolbox --version 2>&1
            if ($LASTEXITCODE -eq 0) {
                Write-Host "installed" -ForegroundColor Green
            }
            else {
                Write-Host "installed but not working" -ForegroundColor Yellow
            }
        }
        catch {
            Write-Host "installed but not working" -ForegroundColor Yellow
        }
    }
    else {
        Write-Host "not found (optional)" -ForegroundColor Gray
    }
    
    Write-Host ""
    
    # Visual Studio Build Tools
    Write-Host "Build Tools:" -ForegroundColor White
    $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vsWhere) {
        $installPath = & $vsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($installPath) {
            Write-Host "  VS Build Tools: installed with C++ workload" -ForegroundColor Green
            Write-Host "    Path: $installPath" -ForegroundColor Gray
            
            # Check for link.exe
            $msvcDir = Join-Path $installPath "VC\Tools\MSVC"
            if (Test-Path $msvcDir) {
                $linkExe = Get-ChildItem -Path $msvcDir -Recurse -Filter "link.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
                if ($linkExe) {
                    Write-Host "    link.exe: found" -ForegroundColor Green
                }
                else {
                    Write-Host "    link.exe: NOT FOUND" -ForegroundColor Red
                }
            }
        }
        else {
            $anyInstall = & $vsWhere -products * -property installationPath 2>$null | Select-Object -First 1
            if ($anyInstall) {
                Write-Host "  VS Build Tools: installed but C++ workload MISSING" -ForegroundColor Yellow
            }
            else {
                Write-Host "  VS Build Tools: not installed" -ForegroundColor Red
            }
        }
    }
    else {
        Write-Host "  VS Build Tools: not installed (vswhere not found)" -ForegroundColor Red
    }
    Write-Host ""
    
    # Network connectivity
    Write-Host "Network Connectivity:" -ForegroundColor White
    $endpoints = @(
        @{ Url = "https://github.com"; Name = "GitHub" },
        @{ Url = "https://aka.ms"; Name = "Microsoft (aka.ms)" },
        @{ Url = "https://storage.googleapis.com"; Name = "Google Storage" }
    )
    foreach ($endpoint in $endpoints) {
        Write-Host "  $($endpoint.Name): " -NoNewline
        try {
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            $response = Invoke-WebRequest -Uri $endpoint.Url -UseBasicParsing -TimeoutSec 10 -Method Head -ErrorAction Stop
            $sw.Stop()
            Write-Host "OK ($($sw.ElapsedMilliseconds)ms)" -ForegroundColor Green
        }
        catch {
            Write-Host "FAILED" -ForegroundColor Red
        }
    }
    Write-Host ""
    
    Write-Host "To install missing components, run: .\requirements.bat" -ForegroundColor Cyan
    Write-Host ""
}

# Check if winget is available
function Test-Winget {
    try {
        $null = Get-Command winget -ErrorAction Stop
        return $true
    }
    catch {
        return $false
    }
}

# Initialize winget on first run (updates sources, accepts agreements)
# This prevents hangs during the first package check on fresh systems
function Initialize-Winget {
    Write-Host ""
    Write-Host "Initializing Windows Package Manager..." -ForegroundColor Cyan
    Write-Host "  (This may take 1-2 minutes on first run)" -ForegroundColor Gray
    Write-Host ""
    
    # Step 1: Force winget to accept agreements and update sources (blocking with spinner)
    $spinChars = @('|', '/', '-', '\')
    $spinIdx = 0
    $startTime = Get-Date
    $timeout = 180  # 3 minutes max
    
    $job = Start-Job -ScriptBlock {
        # Multiple attempts to initialize winget
        winget source update --accept-source-agreements 2>&1 | Out-Null
        # Trigger package database initialization with a simple list
        winget list --count 1 --accept-source-agreements 2>&1 | Out-Null
    }
    
    while ($job.State -eq 'Running') {
        $elapsed = ((Get-Date) - $startTime).TotalSeconds
        if ($elapsed -gt $timeout) {
            Stop-Job -Job $job -ErrorAction SilentlyContinue
            Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
            Show-BlockingError -ErrorCode "ERR_WINGET_TIMEOUT" `
                -Title "winget initialization timed out" `
                -Description "Windows Package Manager took too long to initialize (>3 minutes). This usually indicates a network or configuration issue." `
                -Steps @(
                    "Check your internet connection",
                    "Open Microsoft Store and check for 'App Installer' updates",
                    "Try running in PowerShell (admin): winget source reset --force",
                    "Restart your computer and try again"
                )
        }
        
        Write-Host "`r  Initializing... $($spinChars[$spinIdx % 4]) ($([int]$elapsed)s)  " -NoNewline
        $spinIdx++
        Start-Sleep -Milliseconds 250
    }
    
    Write-Host "`r  Initialization complete                 " -ForegroundColor Green
    Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
    
    # Step 2: Verify winget actually works now by running a simple command
    Write-Host "  Verifying winget functionality... " -NoNewline
    try {
        $testJob = Start-Job -ScriptBlock {
            winget list --id Microsoft.WindowsTerminal --accept-source-agreements 2>&1
        }
        $testCompleted = Wait-Job -Job $testJob -Timeout 30
        
        if ($testCompleted) {
            $testResult = Receive-Job -Job $testJob
            Remove-Job -Job $testJob -Force -ErrorAction SilentlyContinue
            
            # Check for known error patterns
            $resultText = $testResult -join "`n"
            if ($resultText -match "error|failed|0x" -and $resultText -notmatch "No installed package") {
                throw "winget returned an error"
            }
            Write-Host "OK" -ForegroundColor Green
        }
        else {
            Stop-Job -Job $testJob -ErrorAction SilentlyContinue
            Remove-Job -Job $testJob -Force -ErrorAction SilentlyContinue
            throw "verification timed out"
        }
    }
    catch {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_WINGET_BROKEN" `
            -Title "winget is not working correctly" `
            -Description "Windows Package Manager initialization completed but commands are failing. Error: $_" `
            -Steps @(
                "Open Microsoft Store and update 'App Installer'",
                "Run in PowerShell (as Administrator): winget source reset --force",
                "If that fails, try: Add-AppxPackage -RegisterByFamilyName -MainPackage Microsoft.DesktopAppInstaller_8wekyb3d8bbwe",
                "Restart your computer"
            )
    }
    
    Write-Host ""
}

# Check if a package is installed via winget (with timeout protection)
function Test-WingetPackage {
    param([string]$PackageId)
    
    try {
        # Use a job with timeout to prevent hangs on slow/uninitialized systems
        $job = Start-Job -ScriptBlock {
            param($id)
            $result = winget list --id $id 2>&1
            @{ ExitCode = $LASTEXITCODE; Result = ($result -join "`n") }
        } -ArgumentList $PackageId
        
        $completed = Wait-Job -Job $job -Timeout 30
        
        if ($completed) {
            $output = Receive-Job -Job $job
            Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
            return $output.ExitCode -eq 0 -and $output.Result -match $PackageId
        }
        else {
            # Job timed out - kill it and assume not installed
            Stop-Job -Job $job -ErrorAction SilentlyContinue
            Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
            Write-Host "(check timed out) " -NoNewline -ForegroundColor Yellow
            return $false
        }
    }
    catch {
        return $false
    }
}

# Install a package via winget if not already installed
function Install-WingetPackage {
    param(
        [string]$PackageId,
        [string]$DisplayName,
        [string]$TrackVariable = ""
    )
    
    Write-Host "Checking $DisplayName... " -NoNewline
    
    if (Test-WingetPackage -PackageId $PackageId) {
        Write-Host "already installed" -ForegroundColor Green
        return $true
    }
    
    Write-Host "installing..." -ForegroundColor Yellow
    
    try {
        winget install --id $PackageId --exact --silent --accept-source-agreements --accept-package-agreements
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  -> Installed successfully" -ForegroundColor Green
            $script:InstalledAnything = $true
            
            # Track specific installations
            switch ($TrackVariable) {
                "Node" { $script:InstalledNode = $true }
                "Rust" { $script:InstalledRust = $true }
                "Git" { $script:InstalledGit = $true }
                "Protoc" { $script:InstalledProtoc = $true }
                "BuildTools" { $script:InstalledBuildTools = $true }
            }
            
            return $true
        }
        else {
            Write-Host "  -> Installation failed (exit code: $LASTEXITCODE)" -ForegroundColor Red
            return $false
        }
    }
    catch {
        Write-Host "  -> Installation failed: $_" -ForegroundColor Red
        return $false
    }
}

# Install Visual Studio Build Tools with C++ workload
# This is a custom function because we need to pass --override to include the C++ components
function Install-BuildToolsWithCpp {
    $packageId = "Microsoft.VisualStudio.2022.BuildTools"
    $displayName = "Visual Studio 2022 Build Tools (with C++)"
    
    Write-Host "Checking $displayName... " -NoNewline
    
    # Check if already installed
    if (Test-WingetPackage -PackageId $packageId) {
        # Check if link.exe is available (indicates C++ workload is installed)
        $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path $vsWhere) {
            $installPath = & $vsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
            if ($installPath) {
                Write-Host "already installed with C++ tools" -ForegroundColor Green
                return $true
            }
        }
        
        # Build Tools installed but C++ workload missing - need to modify installation
        Write-Host "installed, but C++ workload missing" -ForegroundColor Yellow
        Write-Host "  -> Adding C++ workload..." -ForegroundColor Yellow
        
        try {
            # Use vs_installer to modify the existing installation
            $vsInstaller = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vs_installer.exe"
            if (Test-Path $vsInstaller) {
                # Find the BuildTools installation path
                $buildToolsPath = & $vsWhere -products * -property installationPath 2>$null | Select-Object -First 1
                if ($buildToolsPath) {
                    & $vsInstaller modify --installPath $buildToolsPath --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --norestart
                    if ($LASTEXITCODE -eq 0 -or $LASTEXITCODE -eq 3010) {
                        Write-Host "  -> C++ workload added successfully" -ForegroundColor Green
                        $script:InstalledAnything = $true
                        $script:InstalledBuildTools = $true
                        return $true
                    }
                }
            }
            
            # Fallback: reinstall with override
            Write-Host "  -> Reinstalling with C++ workload..." -ForegroundColor Yellow
            winget install --id $packageId --exact --silent --accept-source-agreements --accept-package-agreements --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --wait"
            if ($LASTEXITCODE -eq 0 -or $LASTEXITCODE -eq 3010) {
                Write-Host "  -> Installed successfully" -ForegroundColor Green
                $script:InstalledAnything = $true
                $script:InstalledBuildTools = $true
                return $true
            }
            else {
                Write-Host "  -> Installation failed (exit code: $LASTEXITCODE)" -ForegroundColor Red
                return $false
            }
        }
        catch {
            Write-Host "  -> Installation failed: $_" -ForegroundColor Red
            return $false
        }
    }
    
    # Not installed at all - install fresh with C++ workload
    Write-Host "installing..." -ForegroundColor Yellow
    Write-Host "  (This may take several minutes - installing C++ build tools)" -ForegroundColor Gray
    
    try {
        # Install with override to include C++ workload
        winget install --id $packageId --exact --silent --accept-source-agreements --accept-package-agreements --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --wait"
        if ($LASTEXITCODE -eq 0 -or $LASTEXITCODE -eq 3010) {
            Write-Host "  -> Installed successfully" -ForegroundColor Green
            $script:InstalledAnything = $true
            $script:InstalledBuildTools = $true
            return $true
        }
        else {
            Write-Host "  -> Installation failed (exit code: $LASTEXITCODE)" -ForegroundColor Red
            return $false
        }
    }
    catch {
        Write-Host "  -> Installation failed: $_" -ForegroundColor Red
        return $false
    }
}

# Refresh the PATH environment variable from the registry
function Update-PathFromRegistry {
    Write-Host ""
    Write-Host "Refreshing PATH from system registry..." -ForegroundColor Gray
    
    $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $env:Path = "$machinePath;$userPath"
}

# Test if a command exists in the current session
function Test-CommandExists {
    param([string]$Command)
    
    try {
        $null = Get-Command $Command -ErrorAction Stop
        return $true
    }
    catch {
        return $false
    }
}

# Probe known installation paths and add them to the session PATH
# This handles cases where winget installs don't immediately update the registry
function Probe-KnownPaths {
    Write-Host "Probing known installation paths..." -ForegroundColor Gray
    
    $pathsToAdd = @()
    
    # Node.js - check both system and user install locations
    $nodePaths = @(
        "$env:ProgramFiles\nodejs",
        "${env:ProgramFiles(x86)}\nodejs",
        "$env:LOCALAPPDATA\Programs\nodejs"
    )
    foreach ($p in $nodePaths) {
        if ((Test-Path "$p\node.exe") -and $env:Path -notlike "*$p*") {
            Write-Host "  Found Node.js at: $p" -ForegroundColor Gray
            $pathsToAdd += $p
            break  # Only add the first found location
        }
    }
    
    # Git - check standard locations
    $gitPaths = @(
        "$env:ProgramFiles\Git\cmd",
        "${env:ProgramFiles(x86)}\Git\cmd",
        "$env:ProgramFiles\Git\bin",
        "${env:ProgramFiles(x86)}\Git\bin"
    )
    foreach ($p in $gitPaths) {
        if ((Test-Path "$p\git.exe") -and $env:Path -notlike "*$p*") {
            Write-Host "  Found Git at: $p" -ForegroundColor Gray
            $pathsToAdd += $p
            break
        }
    }
    
    # Rust/Cargo - user profile location
    $cargoPath = "$env:USERPROFILE\.cargo\bin"
    if ((Test-Path "$cargoPath\cargo.exe") -and $env:Path -notlike "*$cargoPath*") {
        Write-Host "  Found Cargo at: $cargoPath" -ForegroundColor Gray
        $pathsToAdd += $cargoPath
    }
    
    # Protoc - winget installs to a shim directory
    $wingetLinks = "$env:LOCALAPPDATA\Microsoft\WinGet\Links"
    if ((Test-Path $wingetLinks) -and $env:Path -notlike "*$wingetLinks*") {
        # Check if protoc shim exists there
        if (Test-Path "$wingetLinks\protoc.exe") {
            Write-Host "  Found protoc shim at: $wingetLinks" -ForegroundColor Gray
            $pathsToAdd += $wingetLinks
        }
    }
    
    # Protoc - also check the actual install location (varies by version)
    $protocPaths = @(
        "$env:ProgramFiles\protobuf\bin",
        "${env:ProgramFiles(x86)}\protobuf\bin",
        "$env:LOCALAPPDATA\Microsoft\WinGet\Packages"
    )
    foreach ($p in $protocPaths) {
        if (Test-Path $p) {
            # For WinGet packages dir, we need to search subdirectories
            if ($p -like "*WinGet\Packages*") {
                $protocExe = Get-ChildItem -Path $p -Recurse -Filter "protoc.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
                if ($protocExe -and $env:Path -notlike "*$($protocExe.DirectoryName)*") {
                    Write-Host "  Found protoc at: $($protocExe.DirectoryName)" -ForegroundColor Gray
                    $pathsToAdd += $protocExe.DirectoryName
                    break
                }
            }
            elseif ((Test-Path "$p\protoc.exe") -and $env:Path -notlike "*$p*") {
                Write-Host "  Found protoc at: $p" -ForegroundColor Gray
                $pathsToAdd += $p
                break
            }
        }
    }
    
    # Add all discovered paths to the session PATH
    if ($pathsToAdd.Count -gt 0) {
        foreach ($p in $pathsToAdd) {
            $env:Path = "$p;$env:Path"
        }
        Write-Host "  Added $($pathsToAdd.Count) path(s) to session" -ForegroundColor Green
    }
    else {
        Write-Host "  No additional paths needed" -ForegroundColor Gray
    }
}

# Install MCP Database Toolbox from Google's storage (not available via winget)
function Install-McpToolbox {
    $displayName = "MCP Database Toolbox"
    $toolboxDir = "$env:LOCALAPPDATA\Programs\mcp-toolbox"
    $toolboxExe = "$toolboxDir\toolbox.exe"
    $toolboxVersion = "0.24.0"
    $downloadUrl = "https://storage.googleapis.com/genai-toolbox/v$toolboxVersion/windows/amd64/toolbox.exe"
    
    Write-Host "Checking $displayName... " -NoNewline
    
    # Helper to verify if the binary is valid and working
    $verifyExe = {
        param($path)
        if (-not (Test-Path $path)) { return $false }
        $size = (Get-Item $path).Length
        # If the file is too small (e.g. < 1MB), it's likely a 404 HTML page or corrupted
        if ($size -lt 1MB) { return $false }
        
        try {
            # Try to run it. If it's corrupted or wrong arch, this will fail.
            # Using 2>&1 to capture errors as data
            $null = & $path --version 2>&1
            return $LASTEXITCODE -eq 0
        } catch {
            return $false
        }
    }

    # Check if already installed locally and if it actually works
    if (Test-Path $toolboxExe) {
        if (&$verifyExe $toolboxExe) {
            Write-Host "already installed and verified" -ForegroundColor Green
            # Add to PATH for current session
            if ($env:Path -notlike "*$toolboxDir*") {
                $env:Path = "$toolboxDir;$env:Path"
            }
            return $true
        } else {
            Write-Host "found but broken, removing..." -ForegroundColor Yellow
            Remove-Item $toolboxExe -Force -ErrorAction SilentlyContinue
        }
    }
    
    # Check if in PATH elsewhere and working
    if (Test-CommandExists "toolbox") {
        try {
            # Need to capture output to avoid printing it here
            $null = toolbox --version 2>&1
            if ($LASTEXITCODE -eq 0) {
                Write-Host "already installed (in PATH)" -ForegroundColor Green
                return $true
            }
        } catch {}
        Write-Host "found in PATH but broken, installing local version..." -ForegroundColor Yellow
    }

    Write-Host "downloading..." -ForegroundColor Yellow
    
    # Detect if we are on ARM (Google only provides amd64, but Windows ARM can emulate it)
    if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
        Write-Host "  (Note: Running on ARM64, using amd64 version via emulation)" -ForegroundColor Gray
    }

    try {
        # Create directory
        if (-not (Test-Path $toolboxDir)) {
            New-Item -ItemType Directory -Path $toolboxDir -Force | Out-Null
        }
        
        # Download with retry logic
        $tempExe = "$toolboxExe.tmp"
        $downloaded = Invoke-DownloadWithRetry -Url $downloadUrl -OutFile $tempExe -DisplayName $displayName -MaxRetries 3 -TimeoutSec 120
        
        if ($downloaded -and (Test-Path $tempExe)) {
            # Verify the downloaded file before moving it
            if (&$verifyExe $tempExe) {
                if (Test-Path $toolboxExe) { Remove-Item $toolboxExe -Force }
                Move-Item $tempExe $toolboxExe
                
                Write-Host "  -> Verified and installed successfully" -ForegroundColor Green
                
                # Add to user PATH permanently
                $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
                if ($userPath -notlike "*$toolboxDir*") {
                    [Environment]::SetEnvironmentVariable("Path", "$toolboxDir;$userPath", "User")
                    Write-Host "  -> Added to user PATH" -ForegroundColor Green
                }
                
                # Add to current session
                if ($env:Path -notlike "*$toolboxDir*") {
                    $env:Path = "$toolboxDir;$env:Path"
                }
                
                $script:InstalledToolbox = $true
                $script:InstalledAnything = $true
                return $true
            }
            else {
                Remove-Item $tempExe -Force -ErrorAction SilentlyContinue
                Write-Host "  -> Downloaded file is invalid (likely corrupted or wrong architecture)" -ForegroundColor Red
                Write-Host "  -> (Optional: needed for database demo/integrations)" -ForegroundColor Yellow
                return $false
            }
        }
        else {
            Write-Host "  -> (Optional: needed for database demo/integrations)" -ForegroundColor Yellow
            return $false
        }
    }
    catch {
        Write-Host "  -> Download failed: $_" -ForegroundColor Red
        Write-Host "  -> (Optional: needed for database demo/integrations)" -ForegroundColor Yellow
        return $false
    }
}

# Download ONNX Runtime DLLs for embedding/search features
# This is required because ort-sys downloads during cargo build, but our build.rs
# runs before ort-sys, causing a timing issue. Pre-downloading ensures the DLLs are available.
function Install-OnnxRuntime {
    $displayName = "ONNX Runtime"
    $onnxDir = "$env:LOCALAPPDATA\Programs\onnxruntime"
    $onnxDll = "$onnxDir\onnxruntime.dll"
    # Version should match what ort-sys 2.0.0-rc.9 expects (ONNX Runtime 1.19.x)
    $onnxVersion = "1.19.2"
    $downloadUrl = "https://github.com/microsoft/onnxruntime/releases/download/v$onnxVersion/onnxruntime-win-x64-$onnxVersion.zip"
    
    Write-Host "Checking $displayName... " -NoNewline
    
    # Check if already installed
    if (Test-Path $onnxDll) {
        Write-Host "already installed" -ForegroundColor Green
        return $true
    }
    
    Write-Host "downloading v$onnxVersion..." -ForegroundColor Yellow
    
    try {
        # Create directory
        if (-not (Test-Path $onnxDir)) {
            New-Item -ItemType Directory -Path $onnxDir -Force | Out-Null
        }
        
        # Download and extract
        $tempZip = "$env:TEMP\onnxruntime-$onnxVersion.zip"
        $tempExtract = "$env:TEMP\onnxruntime-extract"
        
        # Download with retry logic
        $downloaded = Invoke-DownloadWithRetry -Url $downloadUrl -OutFile $tempZip -DisplayName $displayName -MaxRetries 3 -TimeoutSec 180
        
        if ($downloaded -and (Test-Path $tempZip)) {
            Write-Host "  -> Extracting..." -ForegroundColor Gray
            
            # Clean up any previous extraction
            if (Test-Path $tempExtract) {
                Remove-Item $tempExtract -Recurse -Force
            }
            
            Expand-Archive -Path $tempZip -DestinationPath $tempExtract -Force
            
            # Find the lib directory (inside onnxruntime-win-x64-<version>/lib/)
            $extractedDir = Get-ChildItem -Path $tempExtract -Directory | Select-Object -First 1
            $libDir = Join-Path $extractedDir.FullName "lib"
            
            if (Test-Path $libDir) {
                # Copy DLLs to our install location
                Copy-Item "$libDir\*.dll" -Destination $onnxDir -Force
                Write-Host "  -> Installed to $onnxDir" -ForegroundColor Green
                
                # Also copy to src-tauri/binaries if we're in the project
                $binariesDir = Join-Path $projectRoot "src-tauri\binaries"
                if (Test-Path (Split-Path $binariesDir)) {
                    if (-not (Test-Path $binariesDir)) {
                        New-Item -ItemType Directory -Path $binariesDir -Force | Out-Null
                    }
                    Copy-Item "$libDir\*.dll" -Destination $binariesDir -Force
                    Write-Host "  -> Also copied to src-tauri/binaries/ for bundling" -ForegroundColor Green
                }
                
                # Set ORT_DYLIB_PATH for the current session and permanently
                $onnxDllPath = "$onnxDir\onnxruntime.dll"
                [Environment]::SetEnvironmentVariable("ORT_DYLIB_PATH", $onnxDllPath, "User")
                $env:ORT_DYLIB_PATH = $onnxDllPath
                Write-Host "  -> Set ORT_DYLIB_PATH environment variable" -ForegroundColor Green
                
                $script:InstalledAnything = $true
            }
            else {
                Write-Host "  -> Could not find lib directory in extracted archive" -ForegroundColor Red
                return $false
            }
            
            # Cleanup
            Remove-Item $tempZip -Force -ErrorAction SilentlyContinue
            Remove-Item $tempExtract -Recurse -Force -ErrorAction SilentlyContinue
            
            return $true
        }
        else {
            Write-Host "  -> Download failed" -ForegroundColor Red
            return $false
        }
    }
    catch {
        Write-Host "  -> Download failed: $_" -ForegroundColor Red
        Write-Host "  -> (Embedding/search features may not work)" -ForegroundColor Yellow
        return $false
    }
}

# Install the wasm32-wasip1 target for WASM sandboxing
# Note: wasm32-wasi was renamed to wasm32-wasip1 in Rust 1.78+
function Install-WasmTarget {
    Write-Host ""
    Write-Host "Checking wasm32-wasip1 target... " -NoNewline
    
    if (-not (Test-CommandExists "rustup")) {
        Write-Host "rustup not found, skipping" -ForegroundColor Red
        return
    }
    
    # Check if target is already installed (check both old and new names)
    # rustup outputs info messages to stderr which PowerShell treats as errors
    $previousErrorPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $installedTargets = rustup target list --installed 2>&1
    }
    finally {
        $ErrorActionPreference = $previousErrorPreference
    }
    
    if ($installedTargets -match "wasm32-wasi(p1)?$") {
        Write-Host "already installed" -ForegroundColor Green
        return
    }
    
    Write-Host "installing..." -ForegroundColor Yellow
    
    # rustup outputs info messages to stderr which PowerShell treats as errors
    $previousErrorPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $null = rustup target add wasm32-wasip1 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  -> Installed successfully" -ForegroundColor Green
        }
        else {
            Write-Host "  -> Installation failed" -ForegroundColor Red
            Write-Host "  -> (WASM sandboxing will be disabled, but Python sandboxing still works)" -ForegroundColor Gray
        }
    }
    catch {
        Write-Host "  -> Installation failed: $_" -ForegroundColor Red
        Write-Host "  -> (WASM sandboxing will be disabled, but Python sandboxing still works)" -ForegroundColor Gray
    }
    finally {
        $ErrorActionPreference = $previousErrorPreference
    }
}

# =============================================================================
# POST-INSTALLATION VERIFICATION FUNCTIONS
# =============================================================================

# Verify Build Tools installation with double-checks
function Verify-BuildToolsInstallation {
    Write-Host "  Verifying Build Tools installation... " -NoNewline
    
    # Double-check 1: vswhere can find the installation
    $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vsWhere)) {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_BUILD_TOOLS_INCOMPLETE" `
            -Title "Visual Studio Installer not found" `
            -Description "Build Tools installation did not complete correctly. The Visual Studio Installer component is missing." `
            -Steps @(
                "Open 'Add or remove programs' in Windows Settings",
                "Search for 'Visual Studio' and uninstall any partial installations",
                "Restart your computer",
                "Re-run this script"
            )
    }
    
    # Double-check 2: C++ workload is installed (link.exe should be available)
    $vcToolsPath = & $vsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
    if (-not $vcToolsPath) {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_CPP_WORKLOAD_MISSING" `
            -Title "C++ Build Tools not installed" `
            -Description "Visual Studio Build Tools is installed but the C++ workload is missing. This is required to compile Rust native dependencies." `
            -Steps @(
                "Open 'Visual Studio Installer' from the Start Menu",
                "Click 'Modify' next to 'Build Tools 2022'",
                "Check the box for 'Desktop development with C++'",
                "Click 'Modify' and wait for installation to complete",
                "Re-run this script"
            )
    }
    
    # Double-check 3: link.exe is actually findable
    $msvcDir = Join-Path $vcToolsPath "VC\Tools\MSVC"
    if (Test-Path $msvcDir) {
        $linkExe = Get-ChildItem -Path $msvcDir -Recurse -Filter "link.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $linkExe) {
            Write-Host "WARNING" -ForegroundColor Yellow
            Write-Host ""
            Write-Host "    NOTE: link.exe not found in expected location." -ForegroundColor Yellow
            Write-Host "    If you see linker errors during build, manually add the C++ workload:" -ForegroundColor Yellow
            Write-Host "      1. Open 'Visual Studio Installer'" -ForegroundColor Gray
            Write-Host "      2. Click 'Modify' next to 'Build Tools 2022'" -ForegroundColor Gray
            Write-Host "      3. Ensure 'MSVC v143 - VS 2022 C++ x64/x86 build tools' is checked" -ForegroundColor Gray
            Write-Host ""
            return
        }
    }
    
    Write-Host "OK" -ForegroundColor Green
}

# Verify Rust installation with double-checks
function Verify-RustInstallation {
    Write-Host "  Verifying Rust installation... " -NoNewline
    
    # Refresh PATH first
    Update-PathFromRegistry
    Probe-KnownPaths
    
    # Double-check 1: rustup is in PATH
    if (-not (Test-CommandExists "rustup")) {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_RUSTUP_PATH" `
            -Title "Rust installed but not in PATH" `
            -Description "rustup was installed but is not accessible in this terminal session. This usually requires a terminal restart." `
            -Steps @(
                "Close this terminal completely",
                "Open a NEW terminal window (PowerShell or Command Prompt)",
                "Re-run: .\requirements.bat"
            )
    }
    
    # Double-check 2: stable toolchain is default
    $previousErrorPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $defaultToolchain = rustup show active-toolchain 2>&1
        if ($defaultToolchain -notmatch "stable") {
            Write-Host "setting default... " -NoNewline -ForegroundColor Yellow
            rustup default stable 2>&1 | Out-Null
        }
    }
    finally {
        $ErrorActionPreference = $previousErrorPreference
    }
    
    # Double-check 3: rustc and cargo work
    try {
        $null = rustc --version 2>&1
        $null = cargo --version 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "Command failed"
        }
    }
    catch {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_RUST_BROKEN" `
            -Title "Rust toolchain is broken" `
            -Description "rustup is installed but rustc/cargo are not working correctly." `
            -Steps @(
                "Run: rustup self uninstall",
                "Delete folder: $env:USERPROFILE\.cargo",
                "Delete folder: $env:USERPROFILE\.rustup",
                "Re-run this script to reinstall Rust"
            )
    }
    
    Write-Host "OK" -ForegroundColor Green
}

# Verify Foundry Local installation
function Verify-FoundryLocalInstallation {
    Write-Host "  Verifying Foundry Local installation... " -NoNewline
    
    # Refresh PATH
    Update-PathFromRegistry
    Probe-KnownPaths
    
    if (Test-CommandExists "foundry") {
        Write-Host "OK" -ForegroundColor Green
        return
    }
    
    # Check if it's installed but not in PATH
    $foundryPaths = @(
        "$env:LOCALAPPDATA\Programs\Microsoft\FoundryLocal\foundry.exe",
        "$env:ProgramFiles\Microsoft\FoundryLocal\foundry.exe",
        "$env:LOCALAPPDATA\Microsoft\FoundryLocal\foundry.exe"
    )
    $foundryFound = $false
    foreach ($p in $foundryPaths) {
        if (Test-Path $p) { 
            $foundryFound = $true
            break 
        }
    }
    
    if ($foundryFound) {
        Write-Host "OK (requires terminal restart for PATH)" -ForegroundColor Yellow
    }
    else {
        Write-Host "FAILED" -ForegroundColor Red
        Show-BlockingError -ErrorCode "ERR_FOUNDRY_MISSING" `
            -Title "Microsoft Foundry Local installation failed" `
            -Description "Foundry Local is required for running local AI models. The installation via winget may have failed." `
            -Steps @(
                "Try installing manually: winget install Microsoft.FoundryLocal",
                "Or open Microsoft Store and search for 'Foundry Local'",
                "Install 'Microsoft Foundry Local' by Microsoft Corporation",
                "Re-run this script after installation"
            ) `
            -HelpUrl "https://github.com/microsoft/Foundry-Local"
    }
}

# =============================================================================
# DOWNLOAD RESILIENCE
# =============================================================================

# Download a file with retry logic and manual fallback instructions
function Invoke-DownloadWithRetry {
    param(
        [string]$Url,
        [string]$OutFile,
        [string]$DisplayName,
        [int]$MaxRetries = 3,
        [int]$TimeoutSec = 120
    )
    
    for ($attempt = 1; $attempt -le $MaxRetries; $attempt++) {
        Write-Host "  -> Attempt $attempt of $MaxRetries..." -ForegroundColor Gray
        
        try {
            # Disable progress bar for faster downloads
            $ProgressPreference = 'SilentlyContinue'
            
            # Remove existing temp file if present
            if (Test-Path $OutFile) {
                Remove-Item $OutFile -Force -ErrorAction SilentlyContinue
            }
            
            Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing -TimeoutSec $TimeoutSec
            
            if (Test-Path $OutFile) {
                $size = (Get-Item $OutFile).Length
                if ($size -gt 100KB) {  # Sanity check - not an error page
                    $sizeMB = [math]::Round($size / 1MB, 1)
                    Write-Host "  -> Downloaded successfully ($sizeMB MB)" -ForegroundColor Green
                    return $true
                }
                else {
                    Write-Host "  -> Downloaded file too small (likely error page)" -ForegroundColor Yellow
                    Remove-Item $OutFile -Force -ErrorAction SilentlyContinue
                }
            }
        }
        catch {
            Write-Host "  -> Failed: $($_.Exception.Message)" -ForegroundColor Yellow
        }
        
        if ($attempt -lt $MaxRetries) {
            $waitSec = $attempt * 5
            Write-Host "  -> Retrying in $waitSec seconds..." -ForegroundColor Gray
            Start-Sleep -Seconds $waitSec
        }
    }
    
    # All retries failed - provide manual instructions
    Write-Host ""
    Write-Host "  ========================================" -ForegroundColor Yellow
    Write-Host "  Automatic download of $DisplayName failed." -ForegroundColor Yellow
    Write-Host "  To install manually:" -ForegroundColor White
    Write-Host "    1. Download from: $Url" -ForegroundColor Gray
    Write-Host "    2. Save to: $OutFile" -ForegroundColor Gray
    Write-Host "    3. Re-run this script" -ForegroundColor Gray
    Write-Host "  ========================================" -ForegroundColor Yellow
    Write-Host ""
    
    return $false
}

# Verify that critical commands are available
function Test-AllCommands {
    $allAvailable = $true
    
    Write-Host ""
    Write-Host "Verifying installations..." -ForegroundColor White
    Write-Host ""
    
    Write-Host "  node:  " -NoNewline
    if (Test-CommandExists "node") {
        $version = node --version 2>&1
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
        $allAvailable = $false
    }
    
    Write-Host "  npm:   " -NoNewline
    if (Test-CommandExists "npm") {
        $version = npm --version 2>&1
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
        $allAvailable = $false
    }
    
    Write-Host "  rustc: " -NoNewline
    if (Test-CommandExists "rustc") {
        $version = rustc --version 2>&1
        $version = ($version -split " ")[1]
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
        $allAvailable = $false
    }
    
    Write-Host "  cargo: " -NoNewline
    if (Test-CommandExists "cargo") {
        $version = cargo --version 2>&1
        $version = ($version -split " ")[1]
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
        $allAvailable = $false
    }
    
    Write-Host "  rustup: " -NoNewline
    if (Test-CommandExists "rustup") {
        # rustup writes to stderr even on success, which triggers terminating
        # errors when $ErrorActionPreference = "Stop". Temporarily set to Continue.
        $previousErrorPreference = $ErrorActionPreference
        try {
            $ErrorActionPreference = "Continue"
            $versionOutput = rustup --version 2>$null
        }
        finally {
            $ErrorActionPreference = $previousErrorPreference
        }
        $version = ($versionOutput -split ' ')[1]
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
        $allAvailable = $false
    }
    
    Write-Host "  protoc: " -NoNewline
    if (Test-CommandExists "protoc") {
        $version = protoc --version 2>&1
        $version = ($version -split " ")[-1]
        Write-Host "$version" -ForegroundColor Green
    }
    else {
        Write-Host "not found" -ForegroundColor Red
        $allAvailable = $false
    }
    
    Write-Host "  toolbox: " -NoNewline
    if (Test-CommandExists "toolbox") {
        try {
            # Use 2>&1 and check exit code to prevent red error text from command failures
            $versionOutput = toolbox --version 2>&1
            if ($LASTEXITCODE -eq 0) {
                Write-Host "$versionOutput" -ForegroundColor Green
            } else {
                Write-Host "failed to run (code $LASTEXITCODE)" -ForegroundColor Red
                if ($versionOutput) { 
                    Write-Host "    $versionOutput" -ForegroundColor Gray 
                }
            }
        } catch {
            Write-Host "error: $_" -ForegroundColor Red
        }
    }
    else {
        Write-Host "not found (optional - for database demo)" -ForegroundColor Yellow
    }
    
    return $allAvailable
}

# Main installation logic
function Install-Requirements {
    # ==========================================================================
    # PHASE 1: PREREQUISITE VALIDATION (blocking failures)
    # ==========================================================================
    
    # Run comprehensive prerequisite checks
    Test-Prerequisites
    
    # Initialize winget sources (with spinner and timeout)
    Initialize-Winget
    
    # ==========================================================================
    # PHASE 2: CORE DEPENDENCIES (with UAC warning)
    # ==========================================================================
    
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Yellow
    Write-Host "  IMPORTANT: User Action May Be Needed " -ForegroundColor Yellow
    Write-Host "========================================" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "Some installations require administrator approval." -ForegroundColor White
    Write-Host "If you see a 'User Account Control' dialog, click 'Yes'." -ForegroundColor White
    Write-Host ""
    Write-Host "  >>> The dialog may appear BEHIND this window! <<<" -ForegroundColor Cyan
    Write-Host "  >>> Check your taskbar if installation seems stuck. <<<" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Press Enter to continue..." -ForegroundColor Gray
    Read-Host
    
    Write-Host ""
    Write-Host "Installing dependencies..." -ForegroundColor White
    Write-Host ""
    
    $allSucceeded = $true
    
    # 1. Visual Studio Build Tools - MUST be installed FIRST
    # Required for compiling native Rust dependencies on Windows
    # This includes the MSVC compiler and Windows SDK
    if (-not (Install-BuildToolsWithCpp)) {
        $allSucceeded = $false
    }
    else {
        # Verify the installation immediately
        Verify-BuildToolsInstallation
    }
    
    # 2. Microsoft Foundry Local - Install early to prevent hangs later
    # Users report that having Foundry Local installed prevents hanging issues
    if (-not (Install-WingetPackage -PackageId "Microsoft.FoundryLocal" -DisplayName "Microsoft Foundry Local")) {
        $allSucceeded = $false
    }
    else {
        Verify-FoundryLocalInstallation
    }
    
    # 3. Node.js LTS - Required for frontend build (React/Vite)
    if (-not (Install-WingetPackage -PackageId "OpenJS.NodeJS.LTS" -DisplayName "Node.js LTS" -TrackVariable "Node")) {
        $allSucceeded = $false
    }
    
    # 4. Rust - Required for Tauri backend (installed after Build Tools to avoid MSVC warning)
    if (-not (Install-WingetPackage -PackageId "Rustlang.Rustup" -DisplayName "Rust (rustup)" -TrackVariable "Rust")) {
        $allSucceeded = $false
    }
    
    # 5. Git - For version control (optional but recommended)
    if (-not (Install-WingetPackage -PackageId "Git.Git" -DisplayName "Git" -TrackVariable "Git")) {
        $allSucceeded = $false
    }
    
    # 6. Protocol Buffers (protoc) - Required for compiling lance-embedding and other protobuf-dependent crates
    if (-not (Install-WingetPackage -PackageId "Google.Protobuf" -DisplayName "Protocol Buffers (protoc)" -TrackVariable "Protoc")) {
        $allSucceeded = $false
    }
    
    # ==========================================================================
    # PHASE 3: OPTIONAL COMPONENTS (non-blocking)
    # ==========================================================================
    
    Write-Host ""
    Write-Host "Installing optional components..." -ForegroundColor White
    Write-Host ""
    
    # MCP Database Toolbox - For database demo and MCP database integrations (downloaded from Google Storage)
    Install-McpToolbox  # Optional, don't fail if this doesn't work
    
    # ONNX Runtime - Required for embedding/search features
    Install-OnnxRuntime  # Optional, but needed for semantic search
    
    Write-Host ""
    
    if ($allSucceeded) {
        Write-Host "========================================" -ForegroundColor Green
        Write-Host "  All requirements installed!          " -ForegroundColor Green
        Write-Host "========================================" -ForegroundColor Green
    }
    else {
        Write-Host "========================================" -ForegroundColor Yellow
        Write-Host "  Some installations may have failed   " -ForegroundColor Yellow
        Write-Host "========================================" -ForegroundColor Yellow
    }
    
    # If anything was installed, refresh PATH from registry and probe known paths
    if ($script:InstalledAnything) {
        Update-PathFromRegistry
        Probe-KnownPaths
    }
    
    # Check if C++ tools are properly installed (link.exe should be findable)
    if ($script:InstalledBuildTools) {
        # Verify the C++ workload was actually installed by checking for vswhere results
        $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        $hasCppTools = $false
        if (Test-Path $vsWhere) {
            $installPath = & $vsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
            if ($installPath) {
                $hasCppTools = $true
            }
        }
        
        if (-not $hasCppTools) {
            Write-Host ""
            Write-Host "========================================" -ForegroundColor Yellow
            Write-Host "  ACTION REQUIRED: Build Tools Setup   " -ForegroundColor Yellow
            Write-Host "========================================" -ForegroundColor Yellow
            Write-Host ""
            Write-Host "Visual Studio Build Tools was installed, but the C++ workload" -ForegroundColor White
            Write-Host "may not have been added correctly. If you see 'link.exe not found'" -ForegroundColor White
            Write-Host "errors when building, manually install the C++ workload:" -ForegroundColor White
            Write-Host ""
            Write-Host "  1. Open 'Visual Studio Installer' from the Start Menu" -ForegroundColor Gray
            Write-Host "  2. Click 'Modify' on Build Tools 2022" -ForegroundColor Gray
            Write-Host "  3. Check 'Desktop development with C++'" -ForegroundColor Gray
            Write-Host "  4. Click 'Modify' to install" -ForegroundColor Gray
            Write-Host ""
        }
    }
    
    # Initialize Rust toolchain if rustup was just installed
    if ($script:InstalledRust) {
        Write-Host ""
        Write-Host "Initializing Rust toolchain..." -ForegroundColor Cyan
        
        # Add rustup to current session PATH
        $cargoPath = "$env:USERPROFILE\.cargo\bin"
        if (Test-Path $cargoPath) {
            $env:Path = "$cargoPath;$env:Path"
        }
        
        # Run rustup to install the stable toolchain
        # Note: rustup outputs info messages to stderr which PowerShell treats as errors
        if (Test-CommandExists "rustup") {
            Write-Host "  Running 'rustup default stable'..." -ForegroundColor Gray
            $previousErrorPreference = $ErrorActionPreference
            try {
                $ErrorActionPreference = "Continue"
                $null = rustup default stable 2>&1
            }
            finally {
                $ErrorActionPreference = $previousErrorPreference
            }
        }
        
        # Verify Rust installation
        Verify-RustInstallation
    }
    
    # Disable rustup auto-self-update (reduces network calls, enables air-gapped builds)
    # Note: rustup outputs info messages to stderr which PowerShell treats as errors with $ErrorActionPreference = "Stop"
    if (Test-CommandExists "rustup") {
        $previousErrorPreference = $ErrorActionPreference
        try {
            $ErrorActionPreference = "Continue"
            $null = rustup set auto-self-update disable 2>&1
        }
        catch {
            # Ignore - this is optional
        }
        finally {
            $ErrorActionPreference = $previousErrorPreference
        }
    }
    
    # Install wasm32-wasi target for WASM sandboxing (optional but recommended)
    Install-WasmTarget
    
    # Always probe known paths before verification (helps on re-runs)
    if (-not $script:InstalledAnything) {
        Write-Host ""
        Probe-KnownPaths
    }
    
    # Verify all commands are available
    if (Test-AllCommands) {
        Write-Host ""
        Write-Host "All tools are available in this session!" -ForegroundColor Green
        
        # Check if we're in the project directory (has package.json)
        if (Test-Path "package.json") {
            Write-Host ""
            Write-Host "========================================" -ForegroundColor Cyan
            Write-Host "  Running npm install...               " -ForegroundColor Cyan
            Write-Host "========================================" -ForegroundColor Cyan
            Write-Host ""
            
            try {
                npm install
                if ($LASTEXITCODE -eq 0) {
                    Write-Host ""
                    Write-Host "========================================" -ForegroundColor Green
                    Write-Host "  Setup complete! Ready to run.        " -ForegroundColor Green
                    Write-Host "========================================" -ForegroundColor Green
                    Write-Host ""
                    
                    if ($script:InstalledBuildTools) {
                        Write-Host "  IMPORTANT: Build Tools was just installed." -ForegroundColor Yellow
                        Write-Host "  Open a NEW terminal before running:" -ForegroundColor Yellow
                        Write-Host ""
                    }
                    
                    Write-Host "  Start the app with:" -ForegroundColor White
                    Write-Host "     npx tauri dev" -ForegroundColor Yellow
                    Write-Host ""
                    Write-Host "  Or build for production:" -ForegroundColor White
                    Write-Host "     npx tauri build" -ForegroundColor Yellow
                    Write-Host ""
                }
                else {
                    Write-Host "npm install failed. Please check the errors above." -ForegroundColor Red
                }
            }
            catch {
                Write-Host "npm install failed: $_" -ForegroundColor Red
            }
        }
        else {
            Write-Host ""
            Write-Host "========================================" -ForegroundColor Cyan
            Write-Host "  Next Steps                           " -ForegroundColor Cyan
            Write-Host "========================================" -ForegroundColor Cyan
            Write-Host ""
            
            if ($script:InstalledBuildTools -or $script:InstalledAnything) {
                Write-Host "  0. Open a NEW terminal (to pick up PATH changes)" -ForegroundColor Yellow
            }
            
            Write-Host "  1. Navigate to the project directory" -ForegroundColor White
            Write-Host "  2. Run: npm install" -ForegroundColor Yellow
            Write-Host "  3. Run: npx tauri dev" -ForegroundColor Yellow
            Write-Host ""
        }
    }
    else {
        # Some commands are missing - collect which ones
        $missingTools = @()
        if (-not (Test-CommandExists "node")) { $missingTools += "node" }
        if (-not (Test-CommandExists "npm")) { $missingTools += "npm" }
        if (-not (Test-CommandExists "rustc")) { $missingTools += "rustc" }
        if (-not (Test-CommandExists "cargo")) { $missingTools += "cargo" }
        if (-not (Test-CommandExists "rustup")) { $missingTools += "rustup" }
        if (-not (Test-CommandExists "protoc")) { $missingTools += "protoc" }
        
        Write-Host ""
        Write-Host "========================================" -ForegroundColor Yellow
        Write-Host "  Almost There! Re-run Required        " -ForegroundColor Yellow
        Write-Host "========================================" -ForegroundColor Yellow
        Write-Host ""
        Write-Host "The following tools were installed but aren't in PATH yet:" -ForegroundColor White
        Write-Host ""
        foreach ($tool in $missingTools) {
            Write-Host "  - $tool" -ForegroundColor Red
        }
        Write-Host ""
        Write-Host "This is normal! Windows needs a new terminal to pick up PATH changes." -ForegroundColor Gray
        Write-Host ""
        Write-Host "Please do the following:" -ForegroundColor White
        Write-Host ""
        Write-Host "  1. Close this terminal window completely" -ForegroundColor White
        Write-Host "  2. Open a NEW terminal (PowerShell or Command Prompt)" -ForegroundColor White
        Write-Host "  3. Re-run this script:" -ForegroundColor White
        Write-Host ""
        
        $currentDir = Get-Location
        Write-Host "     cd `"$currentDir`"" -ForegroundColor Cyan
        Write-Host "     .\requirements.bat" -ForegroundColor Cyan
        Write-Host ""
        Write-Host "  The script is safe to run multiple times (idempotent)." -ForegroundColor Gray
        Write-Host "  It will skip already-installed packages and continue setup." -ForegroundColor Gray
        Write-Host ""
        
        # Additional guidance for specific missing tools
        if (-not (Test-CommandExists "rustc") -and $script:InstalledRust) {
            Write-Host "Note: If 'rustc' is still not found after re-run:" -ForegroundColor Gray
            Write-Host "  Run: rustup default stable" -ForegroundColor Gray
            Write-Host ""
        }
    }
}

# =============================================================================
# MAIN ENTRY POINT
# =============================================================================

# Handle diagnostic mode (--check or --diagnose flags)
if ($Check -or $Diagnose) {
    Show-DiagnosticReport
    exit 0
}

# Run the installation
Install-Requirements
