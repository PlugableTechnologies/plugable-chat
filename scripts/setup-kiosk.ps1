<#
.SYNOPSIS
    Sets up a Windows kiosk account for tradeshow/booth demos.

.DESCRIPTION
    This script creates a locked-down local user account that auto-logs in and
    continuously runs your application. It's designed for unattended kiosk or
    tradeshow deployments on Windows 10/11 (any edition).

    Features:
    - Creates a standard (non-admin) local user account
    - Configures Sysinternals Autologon for automatic login
    - Creates a watchdog script that relaunches the app if it closes
    - Schedules the watchdog to run at logon
    - Disables Sticky Keys prompt (Shift 5x escape route)
    - Disables sleep/hibernate/monitor-off on AC power
    - Optional hardening: disables Task Manager, Control Panel, Win keys, etc.
    - Detects per-user app installations and offers to copy them to a shared location
    - Auto-detect mode: finds Tauri binary from project build and copies with dependencies

.PARAMETER UserName
    The name of the local kiosk user account to create. Use a dedicated account
    name like "BoothUser" or "KioskDemo". Do not use an existing admin account.

.PARAMETER Password
    The password for the kiosk user account. This is stored by Autologon to
    enable automatic login. Use a strong password even though the account is
    locked down, as it will be stored in the registry.

.PARAMETER AppPath
    Full path to the application executable to run in kiosk mode.
    Example: "C:\Program Files\MyApp\MyApp.exe"
    
    If omitted, use -ProjectRoot and -AutoDetect to auto-detect from Tauri build.

.PARAMETER ProjectRoot
    Path to the Tauri project root directory (the folder containing src-tauri/).
    Used with -AutoDetect to automatically find and copy the built binary.
    Example: "C:\src\plugable-chat"

.PARAMETER AutoDetect
    When set, auto-detects the binary from the Tauri project's release build output.
    Requires -ProjectRoot. The binary and all dependencies are automatically copied
    to the kiosk directory.

.PARAMETER AppArgs
    Optional command-line arguments to pass to the application.
    Example: "--fullscreen --no-updates"

.PARAMETER KioskDir
    Directory where kiosk support files are stored (watchdog script, Autologon).
    Defaults to "C:\Kiosk".

.PARAMETER Harden
    Apply additional lockdown settings (disable Task Manager, Control Panel,
    Win keys, notifications, lock screen). Enabled by default. Use -Harden:$false
    to skip. Note: requires the user to have logged in once to create their profile.

.PARAMETER NonInteractive
    When set, automatically copies per-user app installations to a shared location
    without prompting. Useful for automated/scripted deployments.

.EXAMPLE
    .\setup-kiosk.ps1 -UserName BoothUser -Password 'Tr@deShow2026!' -AppPath 'C:\Plugable\plugable-chat.exe'

    Creates a kiosk user "BoothUser" that auto-runs plugable-chat.exe.

.EXAMPLE
    .\setup-kiosk.ps1 -UserName DemoKiosk -Password 'SecurePass123' -AppPath 'C:\MyApp\app.exe' -AppArgs '--fullscreen --kiosk'

    Creates a kiosk user with custom app arguments.

.EXAMPLE
    .\setup-kiosk.ps1 -UserName BoothUser -Password 'Pass123' -AppPath 'C:\App\app.exe' -Harden:$false

    Creates a kiosk user without the additional hardening (useful for debugging).

.EXAMPLE
    .\setup-kiosk.ps1 -UserName BoothUser -Password 'Pass123' -AppPath 'C:\Users\Admin\AppData\Local\Programs\MyApp\app.exe' -NonInteractive

    Detects the per-user installation and automatically copies the app to C:\Kiosk\App\
    without prompting (useful for automated deployments).

.EXAMPLE
    .\setup-kiosk.ps1 -UserName BoothUser -Password 'Tr@deShow2026!' -ProjectRoot 'C:\src\plugable-chat' -AutoDetect

    Auto-detects the Tauri binary from the project's release build, copies it with all
    dependencies to C:\Kiosk\App\plugable-chat\, and configures the kiosk.

.EXAMPLE
    .\setup-kiosk.ps1 -UserName BoothUser -Password 'Pass123' -ProjectRoot 'C:\src\plugable-chat' -AutoDetect -AppArgs '--fullscreen'

    Same as above, but passes --fullscreen argument to the application.

.NOTES
    - Must be run as Administrator
    - Works on Windows 10/11 Home, Pro, Enterprise, Education
    - Per-user hardening requires the user to log in once first (creates NTUSER.DAT)
    - Ctrl+Alt+Del cannot be fully blocked on standard Windows editions
    - For "impossible to escape" kiosk, use Enterprise/Education with Shell Launcher

.LINK
    https://learn.microsoft.com/en-us/sysinternals/downloads/autologon
#>
param(
    [Parameter(Mandatory=$true, HelpMessage="Name of the local kiosk user account to create (e.g., 'BoothUser')")]
    [string]$UserName,

    [Parameter(Mandatory=$true, HelpMessage="Password for the kiosk account (stored by Autologon for auto-login)")]
    [string]$Password,

    [Parameter(HelpMessage="Full path to the application executable. If omitted, use -ProjectRoot to auto-detect from Tauri build.")]
    [string]$AppPath = "",

    [Parameter(HelpMessage="Path to the Tauri project root (contains src-tauri/). Used with -AutoDetect to find the built binary.")]
    [string]$ProjectRoot = "",

    [Parameter(HelpMessage="Auto-detect the binary from the Tauri project build output. Requires -ProjectRoot.")]
    [switch]$AutoDetect = $false,

    [Parameter(HelpMessage="Optional command-line arguments for the application")]
    [string]$AppArgs = "",

    [Parameter(HelpMessage="Directory for kiosk support files (default: C:\Kiosk)")]
    [string]$KioskDir = "C:\Kiosk",

    [Parameter(HelpMessage="Apply hardening settings (disable Task Manager, Control Panel, etc.)")]
    [switch]$Harden = $true,

    [Parameter(HelpMessage="Non-interactive mode: auto-copy per-user apps to shared location without prompting")]
    [switch]$NonInteractive = $false
)

function Assert-Admin {
    $currentIdentity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($currentIdentity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "This script must be run as Administrator."
    }
}

function New-LocalKioskUser {
    param([string]$UserName, [string]$Password)

    $existing = Get-LocalUser -Name $UserName -ErrorAction SilentlyContinue
    if (-not $existing) {
        Write-Host "Creating local user: $UserName"
        $securePass = ConvertTo-SecureString $Password -AsPlainText -Force
        New-LocalUser -Name $UserName `
            -Password $securePass `
            -FullName $UserName `
            -Description "Kiosk / tradeshow account" `
            -PasswordNeverExpires:$true `
            -UserMayNotChangePassword:$true | Out-Null
    }
    else {
        Write-Host "User already exists: $UserName"
    }

    # Ensure it's not an admin
    $adminGroup = "Administrators"
    $isAdmin = Get-LocalGroupMember -Group $adminGroup -ErrorAction SilentlyContinue | Where-Object { $_.Name -match "\\$UserName$" }
    if ($isAdmin) {
        Write-Host "Removing $UserName from Administrators group"
        Remove-LocalGroupMember -Group $adminGroup -Member $UserName -ErrorAction SilentlyContinue
    }

    # Ensure it's in Users group
    $usersGroup = "Users"
    $isUser = Get-LocalGroupMember -Group $usersGroup -ErrorAction SilentlyContinue | Where-Object { $_.Name -match "\\$UserName$" }
    if (-not $isUser) {
        Write-Host "Adding $UserName to Users group"
        Add-LocalGroupMember -Group $usersGroup -Member $UserName
    }
}

function Ensure-KioskDir {
    param([string]$KioskDir)
    if (-not (Test-Path $KioskDir)) {
        New-Item -Path $KioskDir -ItemType Directory | Out-Null
    }
}

function Find-TauriBinary {
    <#
    .SYNOPSIS
        Auto-detects the Tauri release binary from the project build output.
    .DESCRIPTION
        Parses tauri.conf.json to get the productName, then looks for the
        release binary in src-tauri/target/release/.
    #>
    param(
        [Parameter(Mandatory=$true)]
        [string]$ProjectRoot
    )
    
    # Validate project structure
    $srcTauriDir = Join-Path $ProjectRoot "src-tauri"
    $tauriConfPath = Join-Path $srcTauriDir "tauri.conf.json"
    
    if (-not (Test-Path $srcTauriDir)) {
        throw "Invalid project root: src-tauri/ directory not found at $srcTauriDir"
    }
    
    if (-not (Test-Path $tauriConfPath)) {
        throw "tauri.conf.json not found at $tauriConfPath"
    }
    
    # Parse tauri.conf.json to get product name
    Write-Host "Reading Tauri configuration from: $tauriConfPath"
    $tauriConf = Get-Content $tauriConfPath -Raw | ConvertFrom-Json
    $productName = $tauriConf.productName
    
    if (-not $productName) {
        throw "productName not found in tauri.conf.json"
    }
    
    Write-Host "  Product name: $productName"
    
    # Look for the release binary
    $releaseDir = Join-Path $srcTauriDir "target\release"
    $binaryName = "$productName.exe"
    $binaryPath = Join-Path $releaseDir $binaryName
    
    if (-not (Test-Path $binaryPath)) {
        # Also check the workspace-level target directory (if using workspace)
        $workspaceReleaseDir = Join-Path $ProjectRoot "target\release"
        $workspaceBinaryPath = Join-Path $workspaceReleaseDir $binaryName
        
        if (Test-Path $workspaceBinaryPath) {
            $binaryPath = $workspaceBinaryPath
            $releaseDir = $workspaceReleaseDir
        }
        else {
            Write-Host ""
            Write-Host "Binary not found. Have you built the project?" -ForegroundColor Yellow
            Write-Host "  Expected: $binaryPath"
            Write-Host "  Or: $workspaceBinaryPath"
            Write-Host ""
            Write-Host "Run this command to build:" -ForegroundColor Cyan
            Write-Host "  cd $ProjectRoot"
            Write-Host "  cargo tauri build --release"
            throw "Release binary not found. Please build the project first."
        }
    }
    
    Write-Host "  Found binary: $binaryPath"
    
    return @{
        BinaryPath = $binaryPath
        ReleaseDir = $releaseDir
        ProductName = $productName
    }
}

function Copy-TauriBinaryToKiosk {
    <#
    .SYNOPSIS
        Copies the Tauri binary and its dependencies to the kiosk directory.
    .DESCRIPTION
        Copies the .exe and any .dll files from the release directory,
        plus WebView2 loader if present.
    #>
    param(
        [Parameter(Mandatory=$true)]
        [string]$BinaryPath,
        
        [Parameter(Mandatory=$true)]
        [string]$ReleaseDir,
        
        [Parameter(Mandatory=$true)]
        [string]$ProductName,
        
        [Parameter(Mandatory=$true)]
        [string]$KioskDir
    )
    
    $appDestDir = Join-Path $KioskDir "App\$ProductName"
    
    Write-Host "Copying Tauri application to kiosk directory..."
    Write-Host "  From: $ReleaseDir"
    Write-Host "  To:   $appDestDir"
    
    # Create destination directory
    if (Test-Path $appDestDir) {
        Write-Host "  Removing existing copy..."
        Remove-Item -Path $appDestDir -Recurse -Force
    }
    New-Item -Path $appDestDir -ItemType Directory -Force | Out-Null
    
    # Copy the main executable
    $exeName = "$ProductName.exe"
    $srcExe = Join-Path $ReleaseDir $exeName
    $destExe = Join-Path $appDestDir $exeName
    Write-Host "  Copying: $exeName"
    Copy-Item -Path $srcExe -Destination $destExe -Force
    
    # Copy all DLLs from the release directory (Tauri dependencies)
    $dlls = Get-ChildItem -Path $ReleaseDir -Filter "*.dll" -ErrorAction SilentlyContinue
    foreach ($dll in $dlls) {
        Write-Host "  Copying: $($dll.Name)"
        Copy-Item -Path $dll.FullName -Destination $appDestDir -Force
    }
    
    # Copy WebView2Loader.dll if present (critical for Tauri)
    $webview2Paths = @(
        (Join-Path $ReleaseDir "WebView2Loader.dll"),
        (Join-Path $ReleaseDir "Microsoft.Web.WebView2.Core.dll")
    )
    foreach ($wv2Path in $webview2Paths) {
        if (Test-Path $wv2Path) {
            $wv2Name = Split-Path -Leaf $wv2Path
            Write-Host "  Copying: $wv2Name (WebView2)"
            Copy-Item -Path $wv2Path -Destination $appDestDir -Force
        }
    }
    
    # Copy any resource directories that Tauri bundles
    # These are typically specified in tauri.conf.json under bundle.resources
    $resourceDirs = @("test-data", "resources", "assets")
    foreach ($resDir in $resourceDirs) {
        $srcResDir = Join-Path $ReleaseDir $resDir
        if (Test-Path $srcResDir) {
            Write-Host "  Copying resource directory: $resDir/"
            Copy-Item -Path $srcResDir -Destination $appDestDir -Recurse -Force
        }
    }
    
    # Copy sidecar binaries if present (common in Tauri apps)
    # These are typically in the release dir with platform-specific suffixes
    $sidecars = Get-ChildItem -Path $ReleaseDir -Filter "*-x86_64-pc-windows-msvc.exe" -ErrorAction SilentlyContinue
    foreach ($sidecar in $sidecars) {
        Write-Host "  Copying sidecar: $($sidecar.Name)"
        Copy-Item -Path $sidecar.FullName -Destination $appDestDir -Force
    }
    
    # Also check for binaries directory (if Tauri bundled external binaries)
    $binariesDir = Join-Path $ReleaseDir "binaries"
    if (Test-Path $binariesDir) {
        Write-Host "  Copying binaries/ directory"
        Copy-Item -Path $binariesDir -Destination $appDestDir -Recurse -Force
    }
    
    # Set permissions for all users
    Write-Host "  Setting permissions for all users..."
    $acl = Get-Acl $appDestDir
    $usersRule = New-Object System.Security.AccessControl.FileSystemAccessRule(
        "Users",
        "ReadAndExecute",
        "ContainerInherit,ObjectInherit",
        "None",
        "Allow"
    )
    $acl.AddAccessRule($usersRule)
    Set-Acl -Path $appDestDir -AclObject $acl
    
    Write-Host "  Application copied successfully."
    Write-Host ""
    
    return $destExe
}

function Test-IsUserProfilePath {
    <#
    .SYNOPSIS
        Checks if a path is inside a user profile directory (per-user install).
    #>
    param([string]$Path)
    
    $usersRoot = "C:\Users"
    $normalizedPath = [System.IO.Path]::GetFullPath($Path).TrimEnd('\')
    
    if ($normalizedPath -like "$usersRoot\*") {
        # Extract the username from the path
        $relativePath = $normalizedPath.Substring($usersRoot.Length + 1)
        $pathParts = $relativePath -split '\\'
        if ($pathParts.Count -gt 0) {
            $profileName = $pathParts[0]
            # Exclude special system profiles
            if ($profileName -notin @('Public', 'Default', 'Default User', 'All Users')) {
                return @{
                    IsUserProfile = $true
                    ProfileName = $profileName
                    ProfilePath = Join-Path $usersRoot $profileName
                }
            }
        }
    }
    return @{ IsUserProfile = $false }
}

function Copy-AppToSharedLocation {
    <#
    .SYNOPSIS
        Copies an application (and its containing folder) to a shared location accessible by all users.
    #>
    param(
        [string]$SourceAppPath,
        [string]$KioskDir
    )
    
    $appDir = Split-Path -Parent $SourceAppPath
    $appFileName = Split-Path -Leaf $SourceAppPath
    $appFolderName = Split-Path -Leaf $appDir
    
    # Create a shared app directory
    $sharedAppDir = Join-Path $KioskDir "App"
    if (-not (Test-Path $sharedAppDir)) {
        New-Item -Path $sharedAppDir -ItemType Directory | Out-Null
    }
    
    $destAppDir = Join-Path $sharedAppDir $appFolderName
    
    Write-Host "Copying application to shared location..."
    Write-Host "  From: $appDir"
    Write-Host "  To:   $destAppDir"
    
    # Copy the entire application folder (preserves DLLs, resources, etc.)
    if (Test-Path $destAppDir) {
        Write-Host "  Removing existing copy..."
        Remove-Item -Path $destAppDir -Recurse -Force
    }
    
    Copy-Item -Path $appDir -Destination $destAppDir -Recurse -Force
    
    $newAppPath = Join-Path $destAppDir $appFileName
    
    if (-not (Test-Path $newAppPath)) {
        throw "Failed to copy application. Expected file not found: $newAppPath"
    }
    
    # Set permissions so all users can read/execute
    Write-Host "  Setting permissions for all users..."
    $acl = Get-Acl $destAppDir
    $usersRule = New-Object System.Security.AccessControl.FileSystemAccessRule(
        "Users",
        "ReadAndExecute",
        "ContainerInherit,ObjectInherit",
        "None",
        "Allow"
    )
    $acl.AddAccessRule($usersRule)
    Set-Acl -Path $destAppDir -AclObject $acl
    
    Write-Host "  Application copied successfully."
    return $newAppPath
}

function Resolve-AppPathForKiosk {
    <#
    .SYNOPSIS
        Ensures the application path is accessible to all users.
        If the app is in a per-user location, offers to copy it to a shared location.
    #>
    param(
        [string]$AppPath,
        [string]$KioskDir,
        [bool]$NonInteractive = $false
    )
    
    $profileCheck = Test-IsUserProfilePath -Path $AppPath
    
    if (-not $profileCheck.IsUserProfile) {
        # Path is already in a system-wide location (Program Files, custom folder, etc.)
        Write-Host "Application path is accessible to all users: $AppPath"
        return $AppPath
    }
    
    # App is in a user profile - this is a problem for the kiosk user
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Yellow
    Write-Host "WARNING: Per-User Installation Detected" -ForegroundColor Yellow
    Write-Host "========================================" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "The application is installed in a user profile directory:"
    Write-Host "  $AppPath"
    Write-Host ""
    Write-Host "This path belongs to user: $($profileCheck.ProfileName)"
    Write-Host "The kiosk user will NOT have access to this location."
    Write-Host ""
    
    if ($NonInteractive) {
        Write-Host "Non-interactive mode: automatically copying to shared location..."
        $newPath = Copy-AppToSharedLocation -SourceAppPath $AppPath -KioskDir $KioskDir
        Write-Host ""
        Write-Host "Application will run from: $newPath" -ForegroundColor Green
        return $newPath
    }
    
    Write-Host "Options:"
    Write-Host "  [C] Copy the application to $KioskDir\App\ (recommended)"
    Write-Host "  [S] Skip - I will reinstall the app system-wide myself"
    Write-Host "  [F] Force - Use the path anyway (will likely fail)"
    Write-Host ""
    
    $choice = Read-Host "Enter choice [C/S/F]"
    
    switch ($choice.ToUpper()) {
        'C' {
            $newPath = Copy-AppToSharedLocation -SourceAppPath $AppPath -KioskDir $KioskDir
            Write-Host ""
            Write-Host "Application will run from: $newPath" -ForegroundColor Green
            return $newPath
        }
        'S' {
            Write-Host ""
            Write-Host "Aborting. Please reinstall the application to a system-wide location like:" -ForegroundColor Yellow
            Write-Host "  C:\Program Files\YourApp\"
            Write-Host "  C:\Plugable\"
            Write-Host ""
            Write-Host "Then re-run this script with the new path."
            throw "User chose to reinstall application manually."
        }
        'F' {
            Write-Host ""
            Write-Host "Proceeding with per-user path. The kiosk may fail to launch the app." -ForegroundColor Red
            return $AppPath
        }
        default {
            Write-Host "Invalid choice. Defaulting to Copy." -ForegroundColor Yellow
            $newPath = Copy-AppToSharedLocation -SourceAppPath $AppPath -KioskDir $KioskDir
            Write-Host ""
            Write-Host "Application will run from: $newPath" -ForegroundColor Green
            return $newPath
        }
    }
}

function Download-And-Install-Autologon {
    param([string]$KioskDir)

    $zipPath = Join-Path $KioskDir "Autologon.zip"
    $autologonDir = Join-Path $KioskDir "Autologon"
    $autologonExe = Join-Path $autologonDir "Autologon.exe"
    $autologonExe64 = Join-Path $autologonDir "Autologon64.exe"

    if (Test-Path $autologonExe) {
        Write-Host "Autologon already present: $autologonExe"
        return $autologonExe
    }

    if (-not (Test-Path $autologonDir)) {
        New-Item -Path $autologonDir -ItemType Directory | Out-Null
    }

    # Try multiple download methods - Sysinternals CDN can be flaky
    $downloadSuccess = $false

    # Method 1: Try the ZIP download from download.sysinternals.com
    Write-Host "Downloading Sysinternals Autologon (method 1: ZIP)..."
    $zipUrl = "https://download.sysinternals.com/files/Autologon.zip"
    try {
        # Use TLS 1.2 for compatibility
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $zipUrl -OutFile $zipPath -UseBasicParsing -ErrorAction Stop
        
        if (Test-Path $zipPath) {
            Write-Host "Extracting Autologon..."
            Expand-Archive -Path $zipPath -DestinationPath $autologonDir -Force
            $downloadSuccess = $true
        }
    }
    catch {
        Write-Host "ZIP download failed: $($_.Exception.Message)"
    }

    # Method 2: Try direct EXE download from live.sysinternals.com
    if (-not $downloadSuccess -or -not (Test-Path $autologonExe)) {
        Write-Host "Trying alternate download (method 2: live.sysinternals.com)..."
        try {
            $liveUrl = "https://live.sysinternals.com/Autologon.exe"
            Invoke-WebRequest -Uri $liveUrl -OutFile $autologonExe -UseBasicParsing -ErrorAction Stop
            $downloadSuccess = Test-Path $autologonExe
            
            # Also try 64-bit version
            if ($downloadSuccess) {
                $liveUrl64 = "https://live.sysinternals.com/Autologon64.exe"
                Invoke-WebRequest -Uri $liveUrl64 -OutFile $autologonExe64 -UseBasicParsing -ErrorAction SilentlyContinue
            }
        }
        catch {
            Write-Host "Live download failed: $($_.Exception.Message)"
        }
    }

    # Method 3: If all else fails, give manual instructions
    if (-not (Test-Path $autologonExe)) {
        Write-Host ""
        Write-Host "========================================" -ForegroundColor Yellow
        Write-Host "MANUAL DOWNLOAD REQUIRED" -ForegroundColor Yellow
        Write-Host "========================================" -ForegroundColor Yellow
        Write-Host "Automatic download failed. Please manually download Autologon:"
        Write-Host ""
        Write-Host "1. Open a browser and go to:"
        Write-Host "   https://learn.microsoft.com/en-us/sysinternals/downloads/autologon"
        Write-Host ""
        Write-Host "2. Click 'Download Autologon' link"
        Write-Host ""
        Write-Host "3. Extract Autologon.exe to:"
        Write-Host "   $autologonDir"
        Write-Host ""
        Write-Host "4. Re-run this script"
        Write-Host "========================================" -ForegroundColor Yellow
        throw "Autologon.exe not found. Please download manually (see instructions above)."
    }

    return $autologonExe
}

function Configure-Autologon {
    param(
        [string]$AutologonExe,
        [string]$UserName,
        [string]$Password
    )

    $domain = $env:COMPUTERNAME

    Write-Host "Configuring Autologon for $domain\$UserName"

    # Accept EULA and configure autologon
    # Syntax: Autologon.exe user domain password /accepteula
    # Escape password quotes to handle special characters safely
    $escapedPassword = $Password -replace '"', '\"'
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $AutologonExe
    $psi.Arguments = "`"$UserName`" `"$domain`" `"$escapedPassword`" /accepteula"
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $stdout = $p.StandardOutput.ReadToEnd()
    $stderr = $p.StandardError.ReadToEnd()
    $p.WaitForExit()

    if ($p.ExitCode -ne 0) {
        throw "Autologon failed: $stderr"
    }

    Write-Host "Autologon configured."
}

function Write-WatchdogScript {
    param(
        [string]$KioskDir,
        [string]$AppPath,
        [string]$AppArgs
    )

    $watchdogPath = Join-Path $KioskDir "watchdog.ps1"
    $procName = [System.IO.Path]::GetFileNameWithoutExtension($AppPath)

    $content = @"
`$ErrorActionPreference = 'SilentlyContinue'
`$kioskAppExe = `"$AppPath`"
`$kioskAppArgs = `"$AppArgs`"
`$kioskProcName = `"$procName`"

while (`$true) {
    `$running = Get-Process -Name `$kioskProcName -ErrorAction SilentlyContinue
    if (-not `$running) {
        if ([string]::IsNullOrWhiteSpace(`$kioskAppArgs)) {
            Start-Process -FilePath `$kioskAppExe
        } else {
            Start-Process -FilePath `$kioskAppExe -ArgumentList `$kioskAppArgs
        }
        Start-Sleep -Seconds 2
    }
    Start-Sleep -Seconds 1
}
"@

    Set-Content -Path $watchdogPath -Value $content -Encoding UTF8
    Write-Host "Watchdog script written: $watchdogPath"
    return $watchdogPath
}

function New-LogonTask {
    param(
        [string]$TaskName,
        [string]$UserName,
        [string]$ScriptPath
    )

    # Delete existing task if present
    schtasks.exe /Delete /TN $TaskName /F 2>$null | Out-Null

    $domainUser = "$env:COMPUTERNAME\$UserName"

    # Run watchdog at logon for the kiosk user (LIMITED privilege since kiosk user is not admin)
    $action = "powershell.exe -ExecutionPolicy Bypass -NoProfile -WindowStyle Hidden -File `"$ScriptPath`""
    $cmd = @(
        "/Create",
        "/TN", $TaskName,
        "/TR", $action,
        "/SC", "ONLOGON",
        "/RL", "LIMITED",
        "/F",
        "/RU", $domainUser
    )

    Write-Host "Creating scheduled task: $TaskName"
    $result = schtasks.exe @cmd
    Write-Host $result
}

function Optional-Disable-StickyKeysPrompt {
    # Disable Sticky Keys prompt (Shift 5 times) and related toggles for all users (HKU\.DEFAULT)
    # This affects logon screen; each user may still have their own settings.
    Write-Host "Disabling Sticky Keys prompt and toggles..."
    reg.exe add "HKU\.DEFAULT\Control Panel\Accessibility\StickyKeys" /v Flags /t REG_SZ /d 506 /f | Out-Null
    reg.exe add "HKU\.DEFAULT\Control Panel\Accessibility\Keyboard Response" /v Flags /t REG_SZ /d 122 /f | Out-Null
    reg.exe add "HKU\.DEFAULT\Control Panel\Accessibility\ToggleKeys" /v Flags /t REG_SZ /d 58 /f | Out-Null
}

function Optional-Harden-For-KioskUser {
    param([string]$UserName)

    Write-Host "Applying basic kiosk hardening for $UserName (registry + policy-friendly settings)."

    # These apply to the kiosk user's profile once it exists and has logged in at least once.
    # We'll set them in HKLM\Software\Microsoft\Windows NT\CurrentVersion\ProfileList based on SID.
    # If the user has never logged in, HKCU hive does not exist yet; we use HKU by loading the user's NTUSER.DAT.

    # Get user object with error handling
    $localUser = Get-LocalUser -Name $UserName -ErrorAction SilentlyContinue
    if (-not $localUser) {
        Write-Host "User '$UserName' not found. Skipping per-user hardening." -ForegroundColor Yellow
        return
    }

    $sid = $localUser.SID.Value
    if (-not $sid) {
        Write-Host "Could not retrieve SID for '$UserName'. Skipping per-user hardening." -ForegroundColor Yellow
        return
    }

    # Check if profile exists in registry
    $profileRegPath = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$sid"
    $profileReg = Get-ItemProperty -Path $profileRegPath -ErrorAction SilentlyContinue
    if (-not $profileReg -or -not $profileReg.ProfileImagePath) {
        Write-Host "User profile not created yet (user must log in once). Skipping per-user hardening." -ForegroundColor Yellow
        Write-Host "After first login as '$UserName', re-run this script with -Harden to apply hardening." -ForegroundColor Yellow
        return
    }

    $profilePath = $profileReg.ProfileImagePath
    $ntUserDat = Join-Path $profilePath "NTUSER.DAT"

    if (-not (Test-Path $ntUserDat)) {
        Write-Host "User profile not created yet (no NTUSER.DAT). The user must log in once for per-user hardening to apply." -ForegroundColor Yellow
        return
    }

    $hiveName = "KIOSK_$UserName"
    
    # Try to load the hive
    $loadResult = reg.exe load "HKU\$hiveName" "$ntUserDat" 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Could not load user registry hive (may be in use or locked): $loadResult" -ForegroundColor Yellow
        Write-Host "Skipping per-user hardening. You can apply it later by re-running with -Harden." -ForegroundColor Yellow
        return
    }

    try {
        # Disable Task Manager
        reg.exe add "HKU\$hiveName\Software\Microsoft\Windows\CurrentVersion\Policies\System" /v DisableTaskMgr /t REG_DWORD /d 1 /f | Out-Null

        # Hide settings app access (limited effectiveness across versions)
        reg.exe add "HKU\$hiveName\Software\Microsoft\Windows\CurrentVersion\Policies\Explorer" /v NoControlPanel /t REG_DWORD /d 1 /f | Out-Null

        # Disable Win+X menu
        reg.exe add "HKU\$hiveName\Software\Microsoft\Windows\CurrentVersion\Policies\Explorer" /v NoWinKeys /t REG_DWORD /d 1 /f | Out-Null

        # Turn off toast notifications
        reg.exe add "HKU\$hiveName\Software\Microsoft\Windows\CurrentVersion\PushNotifications" /v ToastEnabled /t REG_DWORD /d 0 /f | Out-Null

        # Disable lock screen (varies by edition)
        reg.exe add "HKU\$hiveName\Software\Policies\Microsoft\Windows\Personalization" /v NoLockScreen /t REG_DWORD /d 1 /f | Out-Null

        Write-Host "Per-user hardening registry settings applied."
    }
    finally {
        reg.exe unload "HKU\$hiveName" 2>&1 | Out-Null
    }
}

function Optional-PowerSettings {
    Write-Host "Setting power options: disable sleep and monitor timeout while plugged in..."
    powercfg.exe /change standby-timeout-ac 0 | Out-Null
    powercfg.exe /change hibernate-timeout-ac 0 | Out-Null
    powercfg.exe /change monitor-timeout-ac 0 | Out-Null
}

# MAIN
try {
    Assert-Admin
    Ensure-KioskDir -KioskDir $KioskDir
    
    # Determine the application path
    if ($AutoDetect -or (-not $AppPath -and $ProjectRoot)) {
        # Auto-detect from Tauri build
        if (-not $ProjectRoot) {
            throw "Auto-detect requires -ProjectRoot parameter. Example: -ProjectRoot 'C:\src\plugable-chat'"
        }
        
        Write-Host ""
        Write-Host "Auto-detecting Tauri binary from project..." -ForegroundColor Cyan
        $tauriBuild = Find-TauriBinary -ProjectRoot $ProjectRoot
        
        # Copy to kiosk directory
        $ResolvedAppPath = Copy-TauriBinaryToKiosk `
            -BinaryPath $tauriBuild.BinaryPath `
            -ReleaseDir $tauriBuild.ReleaseDir `
            -ProductName $tauriBuild.ProductName `
            -KioskDir $KioskDir
    }
    elseif ($AppPath) {
        # Validate the provided path
        if (-not (Test-Path $AppPath -PathType Leaf)) {
            throw "Application not found: $AppPath"
        }
        
        # Resolve the application path - copy to shared location if needed
        $ResolvedAppPath = Resolve-AppPathForKiosk -AppPath $AppPath -KioskDir $KioskDir -NonInteractive $NonInteractive
    }
    else {
        throw "You must specify either -AppPath or -ProjectRoot with -AutoDetect"
    }
    
    New-LocalKioskUser -UserName $UserName -Password $Password

    $autologonExe = Download-And-Install-Autologon -KioskDir $KioskDir
    Configure-Autologon -AutologonExe $autologonExe -UserName $UserName -Password $Password

    $watchdogPath = Write-WatchdogScript -KioskDir $KioskDir -AppPath $ResolvedAppPath -AppArgs $AppArgs

    # Schedule watchdog to run at logon for that user
    $taskName = "Kiosk Watchdog ($UserName)"
    New-LogonTask -TaskName $taskName -UserName $UserName -ScriptPath $watchdogPath

    Optional-Disable-StickyKeysPrompt
    Optional-PowerSettings

    if ($Harden) {
        Optional-Harden-For-KioskUser -UserName $UserName
    }

    Write-Host ""
    Write-Host "DONE."
    Write-Host "Next steps:"
    Write-Host "1) Reboot the machine."
    Write-Host "2) It should auto-logon as $UserName and keep $ResolvedAppPath running."
    Write-Host "3) If per-user hardening did not apply yet, log in once as $UserName, then rerun this script with -Harden."
}
catch {
    Write-Error $_
    exit 1
}
