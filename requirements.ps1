#Requires -Version 5.1
<#
.SYNOPSIS
    Installs development dependencies for plugable-chat on Windows.

.DESCRIPTION
    This script uses winget to check for and install required dependencies
    in an idempotent manner. It will skip already-installed packages.

.NOTES
    Run this script with: powershell.exe -ExecutionPolicy Bypass -File requirements.ps1
    Or use the requirements.bat wrapper.
#>

$ErrorActionPreference = "Stop"

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
$script:InstalledAnything = $false

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

# Check if a package is installed via winget
function Test-WingetPackage {
    param([string]$PackageId)
    
    $result = winget list --id $PackageId 2>&1
    return $LASTEXITCODE -eq 0 -and $result -match $PackageId
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
    $installedTargets = rustup target list --installed 2>&1
    if ($installedTargets -match "wasm32-wasi(p1)?$") {
        Write-Host "already installed" -ForegroundColor Green
        return
    }
    
    Write-Host "installing..." -ForegroundColor Yellow
    
    try {
        rustup target add wasm32-wasip1
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
    
    return $allAvailable
}

# Main installation logic
function Install-Requirements {
    # Verify winget is available
    if (-not (Test-Winget)) {
        Write-Host "ERROR: winget is not available on this system." -ForegroundColor Red
        Write-Host "Please install the App Installer from the Microsoft Store or update Windows." -ForegroundColor Yellow
        Write-Host "https://apps.microsoft.com/store/detail/app-installer/9NBLGGH4NNS1" -ForegroundColor Cyan
        exit 1
    }
    
    Write-Host "Using winget to install dependencies..." -ForegroundColor White
    Write-Host ""
    
    $allSucceeded = $true
    
    # Node.js LTS - Required for frontend build (React/Vite)
    if (-not (Install-WingetPackage -PackageId "OpenJS.NodeJS.LTS" -DisplayName "Node.js LTS" -TrackVariable "Node")) {
        $allSucceeded = $false
    }
    
    # Rust - Required for Tauri backend
    if (-not (Install-WingetPackage -PackageId "Rustlang.Rustup" -DisplayName "Rust (rustup)" -TrackVariable "Rust")) {
        $allSucceeded = $false
    }
    
    # Visual Studio Build Tools - Required for compiling native Rust dependencies on Windows
    # This includes the MSVC compiler and Windows SDK
    if (-not (Install-WingetPackage -PackageId "Microsoft.VisualStudio.2022.BuildTools" -DisplayName "Visual Studio 2022 Build Tools" -TrackVariable "BuildTools")) {
        $allSucceeded = $false
    }
    
    # Git - For version control (optional but recommended)
    if (-not (Install-WingetPackage -PackageId "Git.Git" -DisplayName "Git" -TrackVariable "Git")) {
        $allSucceeded = $false
    }
    
    # Protocol Buffers (protoc) - Required for compiling lance-embedding and other protobuf-dependent crates
    if (-not (Install-WingetPackage -PackageId "Google.Protobuf" -DisplayName "Protocol Buffers (protoc)" -TrackVariable "Protoc")) {
        $allSucceeded = $false
    }
    
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
    
    # Check for Visual Studio Build Tools C++ workload requirement
    if ($script:InstalledBuildTools) {
        Write-Host ""
        Write-Host "========================================" -ForegroundColor Yellow
        Write-Host "  ACTION REQUIRED: Build Tools Setup   " -ForegroundColor Yellow
        Write-Host "========================================" -ForegroundColor Yellow
        Write-Host ""
        Write-Host "Visual Studio Build Tools was just installed." -ForegroundColor White
        Write-Host "You MUST install the C++ workload for Rust to compile:" -ForegroundColor White
        Write-Host ""
        Write-Host "  1. Open 'Visual Studio Installer' from the Start Menu" -ForegroundColor Gray
        Write-Host "  2. Click 'Modify' on Build Tools 2022" -ForegroundColor Gray
        Write-Host "  3. Check 'Desktop development with C++'" -ForegroundColor Gray
        Write-Host "  4. Click 'Modify' to install" -ForegroundColor Gray
        Write-Host ""
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
        if (Test-CommandExists "rustup") {
            Write-Host "  Running 'rustup default stable'..." -ForegroundColor Gray
            rustup default stable
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

# Run the installation
Install-Requirements
