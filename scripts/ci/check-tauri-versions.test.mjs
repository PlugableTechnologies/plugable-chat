import test from 'node:test';
import assert from 'node:assert/strict';
import { minor, tauriCrateVersion } from './check-tauri-versions.mjs';

const LOCK = '[[package]]\nname = "tao"\nversion = "0.34.0"\n\n[[package]]\nname = "tauri"\nversion = "2.12.0"\n';

test('reads the tauri crate version from an LF Cargo.lock', () => {
  assert.equal(tauriCrateVersion(LOCK), '2.12.0');
});

test('reads it from a CRLF Cargo.lock (Windows checkout)', () => {
  assert.equal(tauriCrateVersion(LOCK.replace(/\n/g, '\r\n')), '2.12.0');
});

test('does not confuse tauri with tauri-build or tauri-utils', () => {
  const lock = '[[package]]\nname = "tauri-build"\nversion = "2.7.0"\n';
  assert.equal(tauriCrateVersion(lock), undefined);
});

test('compares major.minor only', () => {
  assert.equal(minor('2.12.3'), '2.12');
  assert.notEqual(minor('2.11.1'), minor('2.12.0'));
});
