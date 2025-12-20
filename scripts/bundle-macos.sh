#!/bin/bash
set -e

# Ensure we are in the project root
cd "$(dirname "$0")/.."

echo "Building macOS Bundle..."

# Get version from git revision count
GIT_COUNT=$(git rev-list --count HEAD)
VERSION="0.$GIT_COUNT.0"
echo "Target Version: $VERSION"

# Use Tauri CLI via npm to handle the bundling (dmg/app)
# Pass the version dynamically via TAURI_CONFIG to avoid dirtying Cargo.toml
TAURI_CONFIG="{\"version\":\"$VERSION\"}" npm run tauri build

