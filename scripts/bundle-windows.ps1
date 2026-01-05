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

# Copy ONNX Runtime DLLs to the release directory
# The ort-sys crate downloads these during build, but they need to be bundled with the app
Write-Host "Copying ONNX Runtime DLLs to release bundle..."

$releaseDir = "src-tauri/target/release"
$bundleDir = "$releaseDir/bundle/msi"

# Find onnxruntime.dll in the ort-sys build output
$ortSysDir = Get-ChildItem -Path "$releaseDir/build" -Filter "ort-sys-*" -Directory | Select-Object -First 1
if ($ortSysDir) {
    $onnxDll = Get-ChildItem -Path $ortSysDir.FullName -Recurse -Filter "onnxruntime*.dll" | Select-Object -First 1
    if ($onnxDll) {
        Write-Host "Found ONNX Runtime DLL: $($onnxDll.FullName)"
        
        # Copy to release directory (next to the exe)
        Copy-Item $onnxDll.FullName -Destination $releaseDir -Force
        Write-Host "Copied to: $releaseDir"
        
        # Also copy to bundle directory if it exists
        if (Test-Path $bundleDir) {
            Copy-Item $onnxDll.FullName -Destination $bundleDir -Force
            Write-Host "Copied to: $bundleDir"
        }
    } else {
        Write-Host "Warning: Could not find onnxruntime*.dll in ort-sys build output"
        Write-Host "The application may fail to start without ONNX Runtime"
    }
} else {
    Write-Host "Warning: Could not find ort-sys build directory"
    Write-Host "The application may fail to start without ONNX Runtime"
}
