$ErrorActionPreference = "Stop"

# Ensure we are in the project root
Set-Location "$PSScriptRoot/.."

Write-Host "Building Windows Bundle..."

# Use Tauri CLI via npm to handle the bundling (msi/setup.exe)
npm run tauri build

