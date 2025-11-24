#!/bin/bash
set -e

# Ensure we are in the project root
cd "$(dirname "$0")/.."

echo "Building macOS Bundle..."

# Use Tauri CLI via npm to handle the bundling (dmg/app)
# This handles code signing and notarization if configured
npm run tauri build

