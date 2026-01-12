#!/usr/bin/env node
/**
 * Postinstall script that creates the tauri CLI wrapper.
 * 
 * On Unix: Creates a symlink from node_modules/.bin/tauri to scripts/tauri-offline.mjs
 * On Windows: Creates a .cmd file since symlinks require admin/Developer Mode
 * 
 * This allows `npx tauri dev` to use our offline-aware wrapper.
 */

import { existsSync, unlinkSync, symlinkSync, writeFileSync, mkdirSync, statSync } from 'fs';
import { join, dirname, resolve } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const projectRoot = dirname(__dirname);

const isWindows = process.platform === 'win32';
const binDir = join(projectRoot, 'node_modules', '.bin');
const tauriPath = join(binDir, 'tauri');
const tauriCmdPath = join(binDir, 'tauri.cmd');
const tauriPs1Path = join(binDir, 'tauri.ps1');

// Use absolute path for Windows, relative for Unix
const targetScriptAbsolute = resolve(projectRoot, 'scripts', 'tauri-offline.mjs');
const relativeTargetUnix = '../../scripts/tauri-offline.mjs';

console.log(`[postinstall] Platform: ${process.platform}`);
console.log(`[postinstall] Project root: ${projectRoot}`);
console.log(`[postinstall] Target script: ${targetScriptAbsolute}`);

// Verify target script exists
if (!existsSync(targetScriptAbsolute)) {
  console.error(`[postinstall] ERROR: Target script not found: ${targetScriptAbsolute}`);
  console.error('[postinstall] You can still use: npm run tauri dev');
  process.exit(0); // Don't fail the install
}

// Ensure .bin directory exists
if (!existsSync(binDir)) {
  console.log(`[postinstall] Creating bin directory: ${binDir}`);
  mkdirSync(binDir, { recursive: true });
}

// Helper to safely remove a file
function safeUnlink(filePath) {
  try {
    if (existsSync(filePath)) {
      const stat = statSync(filePath);
      if (stat.isSymbolicLink() || stat.isFile()) {
        unlinkSync(filePath);
        console.log(`[postinstall] Removed existing: ${filePath}`);
      }
    }
  } catch (err) {
    console.log(`[postinstall] Could not remove ${filePath}: ${err.message}`);
  }
}

// Clean up existing files
safeUnlink(tauriPath);
safeUnlink(tauriCmdPath);
safeUnlink(tauriPs1Path);

if (isWindows) {
  // On Windows, create .cmd and .ps1 wrappers instead of symlinks
  // This avoids the need for admin privileges or Developer Mode
  // Use absolute path to avoid issues with backslash/forward slash mixing
  
  // Escape backslashes for the cmd file
  const escapedPath = targetScriptAbsolute.replace(/\\/g, '\\\\');
  // Use forward slashes for PowerShell (works on Windows)
  const forwardSlashPath = targetScriptAbsolute.replace(/\\/g, '/');
  
  const cmdContent = `@ECHO off\r
node "${escapedPath}" %*\r
`;

  const ps1Content = `#!/usr/bin/env pwsh
& node "${forwardSlashPath}" $args
exit $LASTEXITCODE
`;

  // Also create a shell script for Git Bash/MSYS2 users on Windows
  const shContent = `#!/bin/sh
node "${forwardSlashPath}" "$@"
`;

  let success = true;
  
  try {
    writeFileSync(tauriCmdPath, cmdContent);
    console.log(`[postinstall] Created: ${tauriCmdPath}`);
  } catch (err) {
    console.error(`[postinstall] Failed to create ${tauriCmdPath}: ${err.message}`);
    success = false;
  }
  
  try {
    writeFileSync(tauriPs1Path, ps1Content);
    console.log(`[postinstall] Created: ${tauriPs1Path}`);
  } catch (err) {
    console.error(`[postinstall] Failed to create ${tauriPs1Path}: ${err.message}`);
    success = false;
  }
  
  try {
    writeFileSync(tauriPath, shContent);
    console.log(`[postinstall] Created: ${tauriPath}`);
  } catch (err) {
    console.error(`[postinstall] Failed to create ${tauriPath}: ${err.message}`);
    // This one is optional for Git Bash users
  }
  
  if (success) {
    console.log('[postinstall] Tauri CLI wrappers created successfully');
  } else {
    console.warn('[postinstall] Some wrappers could not be created. Use: npm run tauri dev');
  }
} else {
  // On Unix, use symlink
  try {
    symlinkSync(relativeTargetUnix, tauriPath);
    console.log(`[postinstall] Created symlink: ${tauriPath} -> ${relativeTargetUnix}`);
  } catch (err) {
    console.warn(`[postinstall] Could not create symlink: ${err.message}`);
    console.warn('[postinstall] You can still use: npm run tauri dev');
  }
}

console.log('[postinstall] Done');

