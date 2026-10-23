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
