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
 * Optional:
 *   SIGN_ROUTE             "signtool" (default, smctl drives signtool + the DigiCert KSP) or
 *                          "simple" (smctl --simple: no signtool, no KSP).
 *   SIGN_EXPECTED_SUBJECT  text the signer subject must contain (default "LEANCODE, INC.").
 *
 * `smctl` prints "signCommand command ... FAILED" and still exits 0 when the signing tool is
 * missing, so success is never taken from its exit code: after every signing call the file is
 * checked with Get-AuthenticodeSignature (valid, our signer, countersigned timestamp) and the
 * step fails otherwise.
 *
 * This never runs on developer machines: without those variables it refuses,
 * so a local `tauri build` cannot silently produce an unsigned "signed" build.
 */

import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const DEFAULT_SUBJECT = 'LEANCODE, INC.';
const SMCTL_FAILURE = /(^|\n)\s*(Error\s*:|.*\bFAILED\b|.*exec: ".*": executable file not found)/i;

/** Whether smctl's own output says signing failed (its exit code cannot be trusted). */
export function smctlOutputShowsFailure(output) {
  return SMCTL_FAILURE.test(output ?? '');
}

/**
 * PowerShell script that prints the Authenticode signature of $env:SIGN_TARGET_FILE as JSON.
 * The path travels in the environment: with `powershell -Command`, extra arguments are
 * appended to the command text and never become $args, which made every check read "Unknown".
 */
export const SIGNATURE_SCRIPT =
  '$s = Get-AuthenticodeSignature -LiteralPath $env:SIGN_TARGET_FILE; ' +
  '[pscustomobject]@{ status = $s.Status.ToString(); ' +
  'subject = if ($s.SignerCertificate) { $s.SignerCertificate.Subject } else { "" }; ' +
  'timestamp = [bool]$s.TimeStamperCertificate } | ConvertTo-Json -Compress';

/** The script as a PowerShell -EncodedCommand argument (UTF-16LE base64): immune to Windows quoting rules. */
export const encodedSignatureScript = () => Buffer.from(SIGNATURE_SCRIPT, 'utf16le').toString('base64');

/**
 * Environment for the PowerShell child. When the workflow shell is PowerShell 7, its PSModulePath is
 * inherited and Windows PowerShell 5.1 then cannot load Microsoft.PowerShell.Security ("the module
 * could not be loaded"), so Get-AuthenticodeSignature fails. Dropping it lets each PowerShell use its own.
 */
export function signatureEnv(file, baseEnv = process.env) {
  const env = { ...baseEnv, SIGN_TARGET_FILE: file };
  delete env.PSModulePath;
  return env;
}

/** Read the Authenticode signature Windows reports for a file. */
export function getSignatureFromWindows(file, powershell = process.env.SIGN_POWERSHELL || 'powershell.exe') {
  const result = spawnSync(
    powershell,
    ['-NoProfile', '-NonInteractive', '-EncodedCommand', encodedSignatureScript()],
    { encoding: 'utf8', env: signatureEnv(file) },
  );
  if (result.status !== 0) {
    console.error(`[sign-windows] could not read the signature of ${file}: ${(result.stderr || '').trim()}`);
    return { status: 'Unknown', subject: '', timestamp: false };
  }
  try {
    return JSON.parse(result.stdout.trim());
  } catch {
    console.error(`[sign-windows] unreadable signature output for ${file}: ${result.stdout}`);
    return { status: 'Unknown', subject: '', timestamp: false };
  }
}

/** Problems that make a signature unacceptable; empty when it is valid, ours and timestamped. */
export function signatureProblems(sig, expectedSubject = DEFAULT_SUBJECT) {
  const problems = [];
  if (sig.status !== 'Valid') problems.push(`status is ${sig.status}`);
  if (!sig.subject || !sig.subject.includes(expectedSubject)) {
    problems.push(`signer is '${sig.subject ?? ''}'`);
  }
  if (!sig.timestamp) problems.push('no timestamp countersignature');
  return problems;
}

/** The smctl command line for one file. */
export function smctlArgs({ route, selector, file }) {
  const args = ['sign', ...selector];
  if (route === 'simple') args.push('--simple');
  args.push('--input', file);
  return args;
}

/**
 * Sign one file and prove the result. Returns { ok, problems } and never throws for a
 * signing failure. Every dependency is injectable so the tests need no Windows or DigiCert.
 */
export function signAndVerify(file, options) {
  const {
    route,
    selector,
    expectedSubject = DEFAULT_SUBJECT,
    runSmctl,
    getSignature,
    sleep = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms),
    attempts = 3,
    backoffMs = [5000, 15000],
    log = console.log,
  } = options;

  let problems = ['not attempted'];
  for (let attempt = 1; attempt <= attempts; attempt++) {
    const result = runSmctl(smctlArgs({ route, selector, file }));
    const combined = `${result.stdout ?? ''}\n${result.stderr ?? ''}`;
    if (result.stdout) log(result.stdout.trimEnd());
    if (result.stderr) log(result.stderr.trimEnd());

    if (result.error) {
      return { ok: false, problems: [`could not run smctl (is it installed?): ${result.error.message}`] };
    }
    problems = [];
    if (result.status !== 0) problems.push(`smctl exited ${result.status}`);
    if (smctlOutputShowsFailure(combined)) problems.push('smctl reported a signing failure');
    problems.push(...signatureProblems(getSignature(file), expectedSubject));

    if (problems.length === 0) return { ok: true, problems };
    if (attempt < attempts) {
      log(`[sign-windows] attempt ${attempt} for ${file} failed (${problems.join('; ')}); retrying`);
      sleep(backoffMs[Math.min(attempt - 1, backoffMs.length - 1)]);
    }
  }
  return { ok: false, problems };
}

function main() {
  const argv = process.argv.slice(2);
  const skipSigned = argv.includes('--skip-signed');
  const files = argv.filter((a) => !a.startsWith('--'));

  const fail = (message) => {
    console.error(`[sign-windows] ERROR: ${message}`);
    process.exit(1);
  };

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

  const route = process.env.SIGN_ROUTE || 'signtool';
  if (!['signtool', 'simple'].includes(route)) fail(`SIGN_ROUTE must be signtool or simple, got '${route}'`);
  const expectedSubject = process.env.SIGN_EXPECTED_SUBJECT || DEFAULT_SUBJECT;

  for (const file of files) {
    if (!existsSync(file)) fail(`file not found: ${file}`);

    if (skipSigned && getSignatureFromWindows(file).status === 'Valid') {
      console.log(`[sign-windows] already validly signed, leaving alone: ${file}`);
      continue;
    }

    console.log(`[sign-windows] signing (${route} route): ${file}`);
    const { ok, problems } = signAndVerify(file, {
      route,
      selector,
      expectedSubject,
      runSmctl: (args) => spawnSync('smctl', args, { encoding: 'utf8' }),
      getSignature: getSignatureFromWindows,
    });
    if (!ok) fail(`${file} was not signed: ${problems.join('; ')}`);
    console.log(`[sign-windows] signed and verified: ${file}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
