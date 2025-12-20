$ErrorActionPreference = "Stop"

# Ensure we are in the project root
Set-Location "$PSScriptRoot/.."

Write-Host "Building Windows Bundle..."

# Get version from git revision count
$gitCount = (git rev-list --count HEAD).Trim()
$version = "0.$gitCount.0"
Write-Host "Target Version: $version"

# Use Tauri CLI via npm to handle the bundling (msi/setup.exe)
# Pass the version dynamically via TAURI_CONFIG to avoid dirtying Cargo.toml
$env:TAURI_CONFIG = "{`"version`":`"$version`"}"
npm run tauri build

