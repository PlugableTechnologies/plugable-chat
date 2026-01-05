#!/usr/bin/env node
/**
 * Network-aware Tauri wrapper for air-gapped environments.
 * Detects network availability and sets RUSTUP_OFFLINE/CARGO_NET_OFFLINE
 * environment variables when offline, allowing builds with local toolchains.
 * 
 * Usage: node scripts/tauri-offline.mjs dev
 *        node scripts/tauri-offline.mjs build
 */

import { spawn } from 'child_process';
import { connect } from 'net';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { appendFileSync } from 'fs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const LOG_PATH = '/Users/bernie/git/plugable-chat/.cursor/debug.log';

const TIMEOUT_MS = 2000;

// #region agent log
function debugLog(msg, data, hypothesisId) {
  try {
    appendFileSync(LOG_PATH, JSON.stringify({ location: 'tauri-offline.mjs', message: msg, data, hypothesisId, timestamp: Date.now(), sessionId: 'debug-session' }) + '\n');
  } catch {}
}
// #endregion

/** Check if we can reach the Rust distribution server */
function checkNetwork() {
  return new Promise((resolve) => {
    const socket = connect({ host: 'static.rust-lang.org', port: 443, timeout: TIMEOUT_MS });
    socket.on('connect', () => { socket.destroy(); resolve(true); });
    socket.on('timeout', () => { socket.destroy(); resolve(false); });
    socket.on('error', () => { socket.destroy(); resolve(false); });
  });
}

async function main() {
  // #region agent log
  debugLog('main() entry', { argv: process.argv, TAURI_OFFLINE_WRAPPED: process.env.TAURI_OFFLINE_WRAPPED }, 'H1');
  // #endregion

  // Prevent infinite recursion - if we're already wrapped, exit
  if (process.env.TAURI_OFFLINE_WRAPPED === '1') {
    // #region agent log
    debugLog('Recursion guard triggered - already wrapped', {}, 'H1');
    // #endregion
    console.error('[offline-mode] ERROR: Recursion detected, exiting.');
    process.exit(1);
  }
  process.env.TAURI_OFFLINE_WRAPPED = '1';

  const args = process.argv.slice(2);
  
  if (args.length === 0) {
    console.error('Usage: node scripts/tauri-offline.mjs <dev|build|...>');
    process.exit(1);
  }
  
  const hasNetwork = await checkNetwork();
  
  if (!hasNetwork) {
    console.log('[offline-mode] No network detected, enabling offline build...');
    process.env.RUSTUP_OFFLINE = '1';
    process.env.CARGO_NET_OFFLINE = 'true';
  }
  
  // Call the real tauri CLI directly (not through npx which would find our wrapper)
  const projectRoot = join(__dirname, '..');
  const tauriCliPath = join(projectRoot, 'node_modules', '@tauri-apps', 'cli', 'tauri.js');
  
  // #region agent log
  debugLog('Spawning real tauri CLI', { 
    tauriCliPath, 
    args, 
    hasNetwork,
    RUSTUP_OFFLINE: process.env.RUSTUP_OFFLINE,
    CARGO_NET_OFFLINE: process.env.CARGO_NET_OFFLINE
  }, 'H4');
  // #endregion
  
  const child = spawn(process.execPath, [tauriCliPath, ...args], {
    stdio: 'inherit',
    env: process.env,
    cwd: projectRoot,
  });
  
  // #region agent log
  debugLog('Child process spawned', { pid: child.pid }, 'H5');
  // #endregion
  
  child.on('exit', (code) => process.exit(code ?? 0));
  child.on('error', (err) => {
    // #region agent log
    debugLog('Spawn error', { error: err.message }, 'H1');
    // #endregion
    console.error('Failed to start tauri:', err.message);
    process.exit(1);
  });
}

main();
