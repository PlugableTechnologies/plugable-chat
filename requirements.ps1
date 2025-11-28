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
        [string]$DisplayName
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
    if (-not (Install-WingetPackage -PackageId "OpenJS.NodeJS.LTS" -DisplayName "Node.js LTS")) {
        $allSucceeded = $false
    }
    
    # Rust - Required for Tauri backend
    if (-not (Install-WingetPackage -PackageId "Rustlang.Rustup" -DisplayName "Rust (rustup)")) {
        $allSucceeded = $false
    }
    
    # Visual Studio Build Tools - Required for compiling native Rust dependencies on Windows
    # This includes the MSVC compiler and Windows SDK
    if (-not (Install-WingetPackage -PackageId "Microsoft.VisualStudio.2022.BuildTools" -DisplayName "Visual Studio 2022 Build Tools")) {
        $allSucceeded = $false
    }
    
    # Git - For version control (optional but recommended)
    if (-not (Install-WingetPackage -PackageId "Git.Git" -DisplayName "Git")) {
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
    
    Write-Host ""
    Write-Host "IMPORTANT: After installation, you may need to:" -ForegroundColor Cyan
    Write-Host "  1. Restart your terminal/IDE to refresh PATH" -ForegroundColor White
    Write-Host "  2. For Visual Studio Build Tools, ensure C++ workload is installed:" -ForegroundColor White
    Write-Host "     - Open Visual Studio Installer" -ForegroundColor Gray
    Write-Host "     - Modify Build Tools 2022" -ForegroundColor Gray
    Write-Host "     - Select 'Desktop development with C++'" -ForegroundColor Gray
    Write-Host "  3. Run 'rustup default stable' if Rust was just installed" -ForegroundColor White
    Write-Host ""
    Write-Host "To verify installations, run:" -ForegroundColor Cyan
    Write-Host "  node --version" -ForegroundColor Gray
    Write-Host "  npm --version" -ForegroundColor Gray
    Write-Host "  rustc --version" -ForegroundColor Gray
    Write-Host "  cargo --version" -ForegroundColor Gray
    Write-Host ""
}

# Run the installation
Install-Requirements

