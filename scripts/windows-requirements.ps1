#Requires -Version 5.1
<#
.SYNOPSIS
    Installs development dependencies for plugable-chat on Windows.

.DESCRIPTION
    This script uses winget to check for and install required dependencies
    in an idempotent manner. It will skip already-installed packages.

.NOTES
    DO NOT RUN THIS SCRIPT DIRECTLY!
    
    Use the requirements.bat wrapper in the project root instead:
        requirements.bat
    
    Running this script directly may cause permission issues (UAC dialogs
    appearing behind windows, making install appear to hang).
    
    The .bat wrapper ensures proper execution policy and permissions.
#>

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
    Write-Host "Initializing winget package sources..." -ForegroundColor Gray
    Write-Host "  (This may take a moment on first run)" -ForegroundColor Gray
    
    # Accept source agreements and update sources in the background
    # Using 'winget source update' to ensure the package database is ready
    try {
        $job = Start-Job -ScriptBlock {
            # Force winget to initialize by listing sources
            winget source update --accept-source-agreements 2>&1 | Out-Null
        }
        
        # Wait up to 60 seconds for initialization
        $completed = Wait-Job -Job $job -Timeout 60
        Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
        
        if ($completed) {
            Write-Host "  Package sources ready" -ForegroundColor Green
        }
        else {
            Write-Host "  Source update timed out (continuing anyway)" -ForegroundColor Yellow
        }
    }
    catch {
        Write-Host "  Could not update sources: $_" -ForegroundColor Yellow
        Write-Host "  (Continuing with installation)" -ForegroundColor Gray
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
        
        # Download the binary
        Write-Host "  -> Downloading from Google Storage..." -ForegroundColor Gray
        # Use a temporary file first to avoid corrupted state if download is interrupted
        $tempExe = "$toolboxExe.tmp"
        Invoke-WebRequest -Uri $downloadUrl -OutFile $tempExe -UseBasicParsing
        
        if (Test-Path $tempExe) {
            # Verify the downloaded file before moving it
            if (&$verifyExe $tempExe) {
                if (Test-Path $toolboxExe) { Remove-Item $toolboxExe -Force }
                Move-Item $tempExe $toolboxExe
                
                Write-Host "  -> Downloaded and verified successfully" -ForegroundColor Green
                
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
                return $false
            }
        }
        else {
            Write-Host "  -> Download failed" -ForegroundColor Red
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
        
        Write-Host "  -> Downloading from GitHub..." -ForegroundColor Gray
        Invoke-WebRequest -Uri $downloadUrl -OutFile $tempZip -UseBasicParsing
        
        if (Test-Path $tempZip) {
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
    # Verify winget is available
    if (-not (Test-Winget)) {
        Write-Host "ERROR: winget is not available on this system." -ForegroundColor Red
        Write-Host "Please install the App Installer from the Microsoft Store or update Windows." -ForegroundColor Yellow
        Write-Host "https://apps.microsoft.com/store/detail/app-installer/9NBLGGH4NNS1" -ForegroundColor Cyan
        exit 1
    }
    
    # Initialize winget sources first (prevents hangs on fresh systems)
    Initialize-Winget
    
    Write-Host "Using winget to install dependencies..." -ForegroundColor White
    Write-Host ""
    
    $allSucceeded = $true
    
    # Visual Studio Build Tools - Required for compiling native Rust dependencies on Windows
    # MUST be installed BEFORE Rust to avoid "installing msvc toolchain without its prerequisites" warning
    # This includes the MSVC compiler and Windows SDK
    # We use a custom install to include the C++ workload automatically
    if (-not (Install-BuildToolsWithCpp)) {
        $allSucceeded = $false
    }
    
    # Node.js LTS - Required for frontend build (React/Vite)
    if (-not (Install-WingetPackage -PackageId "OpenJS.NodeJS.LTS" -DisplayName "Node.js LTS" -TrackVariable "Node")) {
        $allSucceeded = $false
    }
    
    # Rust - Required for Tauri backend (installed after Build Tools to avoid MSVC warning)
    if (-not (Install-WingetPackage -PackageId "Rustlang.Rustup" -DisplayName "Rust (rustup)" -TrackVariable "Rust")) {
        $allSucceeded = $false
    }

    # Microsoft Foundry Local - Local model runtime
    if (-not (Install-WingetPackage -PackageId "Microsoft.FoundryLocal" -DisplayName "Microsoft Foundry Local")) {
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
        if (Test-CommandExists "rustup") {
            Write-Host "  Running 'rustup default stable'..." -ForegroundColor Gray
            rustup default stable
        }
    }
    
    # Disable rustup auto-self-update (reduces network calls, enables air-gapped builds)
    if (Test-CommandExists "rustup") {
        rustup set auto-self-update disable 2>$null
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

# Run the installation
Install-Requirements
