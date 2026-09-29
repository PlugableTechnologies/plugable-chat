#!/usr/bin/env node
/**
 * Sign Windows files with the company EV code signing certificate held in
 * DigiCert KeyLocker (Software Trust Manager), through `smctl`.
 *
 * Two callers:
 *   - Tauri, via `bundle.windows.signCommand` in the CI-only config that
 *     .github/workflows/release.yml generates: one call per exe / installer.
 *   - The release workflow, for native libraries that get bundled as resources
 *     (--skip-signed), so Microsoft's own already-signed DLLs are not re-signed.
 *
 * Usage:  node scripts/sign-windows.mjs [--skip-signed] <file> [<file> ...]
 *
 * Environment (supplied by the `release-signing` GitHub Environment):
 *   SM_HOST, SM_API_KEY, SM_CLIENT_CERT_FILE, SM_CLIENT_CERT_PASSWORD
 *   and one of SM_KEYPAIR_ALIAS or SM_CODE_SIGNING_CERT_SHA1_HASH.
 *
 * This never runs on developer machines: without those variables it refuses,
 * so a local `tauri build` cannot silently produce an unsigned "signed" build.
 */

import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';

const argv = process.argv.slice(2);
const skipSigned = argv.includes('--skip-signed');
const files = argv.filter((a) => !a.startsWith('--'));

function fail(message) {
  console.error(`[sign-windows] ERROR: ${message}`);
  process.exit(1);
}

if (files.length === 0) fail('no files given');

for (const name of ['SM_HOST', 'SM_API_KEY', 'SM_CLIENT_CERT_FILE', 'SM_CLIENT_CERT_PASSWORD']) {
  if (!process.env[name]) fail(`${name} is not set; signing only runs in the release workflow`);
}

const keypairAlias = process.env.SM_KEYPAIR_ALIAS;
const fingerprint = process.env.SM_CODE_SIGNING_CERT_SHA1_HASH;
if (!keypairAlias && !fingerprint) {
  fail('set SM_KEYPAIR_ALIAS or SM_CODE_SIGNING_CERT_SHA1_HASH');
}
const selector = keypairAlias
  ? ['--keypair-alias', keypairAlias]
  : ['--fingerprint', fingerprint];

/** True when Windows already reports a valid Authenticode signature. */
function hasValidSignature(file) {
  const result = spawnSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      '(Get-AuthenticodeSignature -LiteralPath $args[0]).Status.ToString()',
      file,
    ],
    { encoding: 'utf8' },
  );
  return result.status === 0 && result.stdout.trim() === 'Valid';
}

for (const file of files) {
  if (!existsSync(file)) fail(`file not found: ${file}`);

  if (skipSigned && hasValidSignature(file)) {
    console.log(`[sign-windows] already validly signed, leaving alone: ${file}`);
    continue;
  }

  console.log(`[sign-windows] signing: ${file}`);
  const result = spawnSync('smctl', ['sign', ...selector, '--input', file], {
    stdio: 'inherit',
  });
  if (result.error) fail(`could not run smctl (is it installed?): ${result.error.message}`);
  if (result.status !== 0) fail(`smctl exited ${result.status} for ${file}`);
}
