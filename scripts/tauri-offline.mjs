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

const TIMEOUT_MS = 2000;

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
  
  // Spawn npx tauri with the provided arguments
  const isWindows = process.platform === 'win32';
  const npx = isWindows ? 'npx.cmd' : 'npx';
  
  const child = spawn(npx, ['tauri', ...args], {
    stdio: 'inherit',
    env: process.env,
    shell: false,
  });
  
  child.on('exit', (code) => process.exit(code ?? 0));
  child.on('error', (err) => {
    console.error('Failed to start tauri:', err.message);
    process.exit(1);
  });
}

main();
