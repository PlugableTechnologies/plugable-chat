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

.EXAMPLE
    .\setup-kiosk.ps1 -UserName BoothUser -Password 'Tr@deShow2026!' -AppPath 'C:\Plugable\plugable-chat.exe'

    Creates a kiosk user "BoothUser" that auto-runs plugable-chat.exe.

.EXAMPLE
    .\setup-kiosk.ps1 -UserName DemoKiosk -Password 'SecurePass123' -AppPath 'C:\MyApp\app.exe' -AppArgs '--fullscreen --kiosk'

    Creates a kiosk user with custom app arguments.

.EXAMPLE
    .\setup-kiosk.ps1 -UserName BoothUser -Password 'Pass123' -AppPath 'C:\App\app.exe' -Harden:$false

    Creates a kiosk user without the additional hardening (useful for debugging).

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

    [Parameter(Mandatory=$true, HelpMessage="Full path to the application executable (e.g., 'C:\MyApp\app.exe')")]
    [ValidateScript({ Test-Path $_ -PathType Leaf })]
    [string]$AppPath,

    [Parameter(HelpMessage="Optional command-line arguments for the application")]
    [string]$AppArgs = "",

    [Parameter(HelpMessage="Directory for kiosk support files (default: C:\Kiosk)")]
    [string]$KioskDir = "C:\Kiosk",

    [Parameter(HelpMessage="Apply hardening settings (disable Task Manager, Control Panel, etc.)")]
    [switch]$Harden = $true
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

    $sid = (Get-LocalUser -Name $UserName).SID.Value
    $profilePath = (Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\$sid").ProfileImagePath
    $ntUserDat = Join-Path $profilePath "NTUSER.DAT"

    if (-not (Test-Path $ntUserDat)) {
        Write-Host "User profile not created yet (no NTUSER.DAT). The user must log in once for per-user hardening to apply."
        return
    }

    $hiveName = "KIOSK_$UserName"
    reg.exe load "HKU\$hiveName" "$ntUserDat" | Out-Null

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
        reg.exe unload "HKU\$hiveName" | Out-Null
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
    New-LocalKioskUser -UserName $UserName -Password $Password

    $autologonExe = Download-And-Install-Autologon -KioskDir $KioskDir
    Configure-Autologon -AutologonExe $autologonExe -UserName $UserName -Password $Password

    $watchdogPath = Write-WatchdogScript -KioskDir $KioskDir -AppPath $AppPath -AppArgs $AppArgs

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
    Write-Host "2) It should auto-logon as $UserName and keep $AppPath running."
    Write-Host "3) If per-user hardening did not apply yet, log in once as $UserName, then rerun this script with -Harden."
}
catch {
    Write-Error $_
    exit 1
}
