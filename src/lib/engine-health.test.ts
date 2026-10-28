import test from 'node:test';
import assert from 'node:assert/strict';
import { engineNotice } from './engine-health.ts';

const healthy = { gpuOff: false, gpuOffReason: null, uncleanDeaths: 0, lastQuarantine: null };

test('a healthy engine says nothing', () => {
  assert.equal(engineNotice(null), null);
  assert.equal(engineNotice(healthy), null);
});

test('GPU-off offers Try GPU again and names the reason', () => {
  const n = engineNotice({ ...healthy, gpuOff: true, gpuOffReason: 'closed twice' })!;
  assert.equal(n.action, 'try-gpu-again');
  assert.ok(n.lines[0].includes('closed twice'));
});

test('a quarantine alone is informational, with no button', () => {
  const n = engineNotice({ ...healthy, lastQuarantine: 'quarantined x' })!;
  assert.equal(n.tone, 'info');
  assert.equal(n.action, null);
});

test('a crash inside a GPU provider is named, with where its dump is', () => {
  const n = engineNotice({
    ...healthy,
    lastQuarantine: 'quarantined x',
    lastCrash: { diedIn: 'chatting', inEp: true, provider: 'webgpu-ep', faultingModule: 'a.dll', dumpPath: 'C:\\dumps\\plugable-chat.exe.1.dmp' },
  })!;
  assert.ok(n.lines[0].includes('"webgpu-ep"'));
  assert.ok(n.lines[0].includes('plugable-chat.exe.1.dmp'));
  const quiet = engineNotice({ ...healthy, lastQuarantine: 'q', lastCrash: { diedIn: 'idle', inEp: false, provider: null, faultingModule: null, dumpPath: null } })!;
  assert.ok(!quiet.lines[0].includes('GPU component "'));
});
