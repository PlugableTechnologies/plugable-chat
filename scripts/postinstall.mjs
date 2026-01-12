#!/usr/bin/env node
/**
 * Postinstall script that creates the tauri CLI wrapper.
 * 
 * On Unix: Creates a symlink from node_modules/.bin/tauri to scripts/tauri-offline.mjs
 * On Windows: Creates a .cmd file since symlinks require admin/Developer Mode
 * 
 * This allows `npx tauri dev` to use our offline-aware wrapper.
 */

import { existsSync, unlinkSync, symlinkSync, writeFileSync, mkdirSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const projectRoot = dirname(__dirname);

const isWindows = process.platform === 'win32';
const binDir = join(projectRoot, 'node_modules', '.bin');
const tauriPath = join(binDir, 'tauri');
const tauriCmdPath = join(binDir, 'tauri.cmd');
const tauriPs1Path = join(binDir, 'tauri.ps1');
const targetScript = join(projectRoot, 'scripts', 'tauri-offline.mjs');
const relativeTarget = '../../scripts/tauri-offline.mjs';

// Ensure .bin directory exists
if (!existsSync(binDir)) {
  mkdirSync(binDir, { recursive: true });
}

// Clean up existing files
for (const p of [tauriPath, tauriCmdPath, tauriPs1Path]) {
  try {
    unlinkSync(p);
  } catch {
    // File doesn't exist, that's fine
  }
}

if (isWindows) {
  // On Windows, create .cmd and .ps1 wrappers instead of symlinks
  // This avoids the need for admin privileges or Developer Mode
  
  const cmdContent = `@ECHO off\r
GOTO start\r
:find_dp0\r
SET dp0=%~dp0\r
EXIT /b\r
:start\r
SETLOCAL\r
CALL :find_dp0\r
\r
IF EXIST "%dp0%\\node.exe" (\r
  SET "_prog=%dp0%\\node.exe"\r
) ELSE (\r
  SET "_prog=node"\r
  SET PATHEXT=%PATHEXT:;.JS;=;%\r
)\r
\r
endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\\${relativeTarget}" %*\r
`;

  const ps1Content = `#!/usr/bin/env pwsh
$basedir=Split-Path $MyInvocation.MyCommand.Definition -Parent

$exe=""
if ($PSVersionTable.PSVersion -lt "6.0" -or $IsWindows) {
  $exe=".exe"
}
$ret=0
if (Test-Path "$basedir/node$exe") {
  # Use local node if available
  & "$basedir/node$exe"  "$basedir/${relativeTarget}" $args
  $ret=$LASTEXITCODE
} else {
  # Use node from PATH
  & "node$exe"  "$basedir/${relativeTarget}" $args
  $ret=$LASTEXITCODE
}
exit $ret
`;

  try {
    writeFileSync(tauriCmdPath, cmdContent);
    writeFileSync(tauriPs1Path, ps1Content);
    console.log('Created tauri CLI wrappers for Windows (.cmd and .ps1)');
  } catch (err) {
    console.warn(`Warning: Could not create tauri CLI wrappers: ${err.message}`);
    console.warn('You can still use: npm run tauri dev');
  }
} else {
  // On Unix, use symlink
  try {
    symlinkSync(relativeTarget, tauriPath);
    console.log('Created tauri CLI symlink');
  } catch (err) {
    console.warn(`Warning: Could not create tauri CLI symlink: ${err.message}`);
    console.warn('You can still use: npm run tauri dev');
  }
}
