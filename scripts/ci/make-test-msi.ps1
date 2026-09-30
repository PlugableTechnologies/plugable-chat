# Create a minimal, valid Windows Installer package (no files, just summary information) to use as a
# test subject for code signing. Uses the Windows Installer COM automation, so run it with Windows
# PowerShell 5.1 (`powershell.exe`), where those COM calls are well supported.
#
#   powershell -NoProfile -File scripts/ci/make-test-msi.ps1 -Path smoke\hello.msi
param([Parameter(Mandatory = $true)][string]$Path)
$ErrorActionPreference = "Stop"

$full = [IO.Path]::GetFullPath($Path)
New-Item -ItemType Directory -Force -Path (Split-Path $full) | Out-Null
if (Test-Path $full) { Remove-Item -Force $full }

$installer = New-Object -ComObject WindowsInstaller.Installer
try {
    $msiOpenDatabaseModeCreate = 3
    $db = $installer.OpenDatabase($full, $msiOpenDatabaseModeCreate)
    $summary = $db.SummaryInformation(20)
    $summary.Property(2) = "Plugable signing smoke test"   # Title
    $summary.Property(3) = "Smoke"                         # Subject
    $summary.Property(4) = "Plugable"                      # Author
    $summary.Property(7) = "x64;1033"                      # Template: platform;language
    $summary.Property(9) = "{6F1F2D0A-3B7C-4C1E-9A4D-5B0E8C2D7A11}"   # Package code
    $summary.Property(14) = 200                            # Installer schema version
    $summary.Property(15) = 2                              # Word count: compressed, short names
    $summary.Property(19) = 2                              # Security: read-only recommended
    $summary.Persist()
    $db.Commit()
} finally {
    foreach ($o in @($summary, $db, $installer)) {
        if ($o) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($o) }
    }
}

if (-not (Test-Path $full)) { throw "could not create the test MSI at $full" }
Write-Host "created $full ($((Get-Item $full).Length) bytes)"
