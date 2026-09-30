// Tests for the signing wrapper. No Windows, smctl or DigiCert needed: both are injected.
//   node --test scripts/sign-windows.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  SIGNATURE_SCRIPT,
  signatureEnv,
  encodedSignatureScript,
  signAndVerify,
  signatureProblems,
  smctlArgs,
  smctlOutputShowsFailure,
} from './sign-windows.mjs';

const GOOD = { status: 'Valid', subject: 'CN="LEANCODE, INC.", O="LEANCODE, INC."', timestamp: true };
const UNSIGNED = { status: 'NotSigned', subject: '', timestamp: false };
const selector = ['--keypair-alias', 'alias'];
const quiet = { log: () => {}, sleep: () => {}, route: 'signtool', selector };

test('smctl that exits 0 but prints FAILED, leaving the file unsigned, is a failure', () => {
  const runSmctl = () => ({
    status: 0,
    stdout: 'Error : \n exec: "signtool": executable file not found in %PATH%: \nsignCommand command for file a.exe FAILED',
    stderr: '',
  });
  const result = signAndVerify('a.exe', { ...quiet, runSmctl, getSignature: () => UNSIGNED, attempts: 2 });
  assert.equal(result.ok, false);
  assert.ok(result.problems.some((p) => p.includes('signing failure')));
  assert.ok(result.problems.some((p) => p.includes('NotSigned')));
});

test('exit 0, clean output, valid timestamped signature from us is success', () => {
  const runSmctl = () => ({ status: 0, stdout: 'Done', stderr: '' });
  const result = signAndVerify('a.exe', { ...quiet, runSmctl, getSignature: () => GOOD });
  assert.deepEqual(result, { ok: true, problems: [] });
});

test('a transient failure is retried and then succeeds', () => {
  let calls = 0;
  const runSmctl = () => {
    calls++;
    return calls === 1
      ? { status: 1, stdout: '', stderr: 'timestamp server unavailable' }
      : { status: 0, stdout: 'Done', stderr: '' };
  };
  const getSignature = () => (calls === 1 ? UNSIGNED : GOOD);
  const result = signAndVerify('a.exe', { ...quiet, runSmctl, getSignature });
  assert.equal(result.ok, true);
  assert.equal(calls, 2);
});

test('gives up after the configured attempts', () => {
  let calls = 0;
  const runSmctl = () => {
    calls++;
    return { status: 1, stdout: '', stderr: 'nope' };
  };
  const result = signAndVerify('a.exe', { ...quiet, runSmctl, getSignature: () => UNSIGNED, attempts: 3 });
  assert.equal(result.ok, false);
  assert.equal(calls, 3);
});

test('a valid signature from someone else is rejected', () => {
  const problems = signatureProblems({ status: 'Valid', subject: 'CN=Someone Else', timestamp: true });
  assert.ok(problems.some((p) => p.startsWith('signer is')));
});

test('a signature without a countersignature timestamp is rejected', () => {
  const problems = signatureProblems({ ...GOOD, timestamp: false });
  assert.deepEqual(problems, ['no timestamp countersignature']);
});

test('smctl cannot be started', () => {
  const runSmctl = () => ({ error: new Error('ENOENT') });
  const result = signAndVerify('a.exe', { ...quiet, runSmctl, getSignature: () => UNSIGNED });
  assert.equal(result.ok, false);
  assert.ok(result.problems[0].includes('could not run smctl'));
});

test('command line: simple route adds --simple, default route does not', () => {
  assert.deepEqual(smctlArgs({ route: 'simple', selector, file: 'a.exe' }), [
    'sign', '--keypair-alias', 'alias', '--simple', '--input', 'a.exe',
  ]);
  assert.deepEqual(smctlArgs({ route: 'signtool', selector, file: 'a.exe' }), [
    'sign', '--keypair-alias', 'alias', '--input', 'a.exe',
  ]);
});

test('ordinary smctl output is not mistaken for a failure', () => {
  assert.equal(smctlOutputShowsFailure('Signing smoke/hello.exe\nDone'), false);
  assert.equal(smctlOutputShowsFailure('signCommand command for file a.exe FAILED'), true);
  assert.equal(smctlOutputShowsFailure('Error : \n boom'), true);
});

test('the signature script reads the path from the environment, never from $args', () => {
  assert.ok(SIGNATURE_SCRIPT.includes('$env:SIGN_TARGET_FILE'));
  assert.ok(!SIGNATURE_SCRIPT.includes('$args'));
});

test('the script is passed encoded, so quotes inside it cannot be mangled', () => {
  const decoded = Buffer.from(encodedSignatureScript(), 'base64').toString('utf16le');
  assert.equal(decoded, SIGNATURE_SCRIPT);
});

test('the PowerShell child does not inherit PSModulePath from a PowerShell 7 parent', () => {
  const env = signatureEnv('a.exe', { PSModulePath: 'C:\\Program Files\\PowerShell\\7\\Modules', PATH: 'x' });
  assert.equal(env.PSModulePath, undefined);
  assert.equal(env.SIGN_TARGET_FILE, 'a.exe');
  assert.equal(env.PATH, 'x');
});
