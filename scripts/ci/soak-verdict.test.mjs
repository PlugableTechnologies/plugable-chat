import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { nextFlapNotBefore, verdictFor } from './soak-verdict.mjs';
import { problemsFor, requiredScenarios } from './check-release-evidence.mjs';

const root = fileURLToPath(new URL('../..', import.meta.url));
const T0 = 1_800_000_000_000;
const s = (n) => T0 + n * 1000;
const ok = (i, dur = 10) => ({ prompt_index: i, outcome: 'ok', detail: '', duration_ms: dur * 1000 });
const drop = (i, dur = 30) => ({ prompt_index: i, outcome: 'error', detail: 'network_dropped: connection lost', duration_ms: dur * 1000 });
const bad = (i, dur = 10) => ({ prompt_index: i, outcome: 'error', detail: 'model produced nothing', duration_ms: dur * 1000 });
const rec = (i, endAt) => ({ t: s(endAt), event: 'record', prompt_index: i });
const flap = (a, b) => [{ t: s(a), event: 'flap_start' }, { t: s(b), event: 'flap_end' }];

test('a turn that ended network_dropped because the harness flapped is expected, not a failure', () => {
  const v = verdictFor([ok(0), drop(1), ok(2)], [...flap(100, 120), rec(0, 50), rec(1, 125), rec(2, 160)], { prompts: 3 });
  assert.deepEqual(v.failures, []);
  assert.equal(v.stats.expectedDrops, 1);
});

test('a turn that began with the network up and failed is a failure, named as such', () => {
  const v = verdictFor([ok(0), bad(1), ok(2)], [...flap(300, 320), rec(0, 50), rec(1, 70), rec(2, 90), { t: s(400), event: 'record', prompt_index: 9 }], { prompts: 3 });
  assert.ok(v.failures.some((f) => /turn 1 began with the network up/.test(f)), v.failures.join('|'));
});

test('two clean-turn failures in a row are reported as the app not recovering', () => {
  const v = verdictFor([bad(0), bad(1), ok(2)], [...flap(300, 320), rec(0, 20), rec(1, 40), rec(2, 60)], { prompts: 3 });
  assert.ok(v.failures.some((f) => /did not recover/.test(f)));
});

test('a drop failure after a flap does not make the next ok turn a recovery failure', () => {
  const v = verdictFor([drop(0), ok(1)], [...flap(10, 30), rec(0, 32), rec(1, 60)], { prompts: 2 });
  assert.deepEqual(v.failures, []);
});

test('network_dropped with no flap anywhere near it is a failure (the harness did not cause it)', () => {
  const v = verdictFor([ok(0), drop(1, 5)], [...flap(500, 520), rec(0, 20), rec(1, 40)], { prompts: 2 });
  assert.ok(v.failures.some((f) => /no flap overlapped/.test(f)));
});

test('a non-drop failure during a flap is still a failure', () => {
  const v = verdictFor([bad(0, 30), drop(1, 30)], [...flap(10, 60), rec(0, 40), rec(1, 70)], { prompts: 2 });
  assert.ok(v.failures.some((f) => /not as a clean network_dropped/.test(f)));
});

test('flaps that never landed on a turn fail the soak', () => {
  const v = verdictFor([ok(0), ok(1)], [...flap(500, 520), rec(0, 20), rec(1, 40)], { prompts: 2 });
  assert.ok(v.failures.some((f) => /never landed on a running turn/.test(f)));
});

test('the run stopping before the last due prompt fails', () => {
  const v = verdictFor([ok(0), ok(1)], [rec(0, 20), rec(1, 40)], { prompts: 5 });
  assert.ok(v.failures.some((f) => /stopped at turn 1/.test(f)));
});

test('an Application Error from before the soak is not passed in; one near the harness kill is excluded by time window', () => {
  const events = [rec(0, 20), { t: s(100), event: 'kill' }];
  const near = verdictFor([ok(0)], events, { prompts: 1, crashes: [s(110)] });
  assert.deepEqual(near.failures, []);
  assert.ok(near.notes.some((n) => /ignored/.test(n)));
  const far = verdictFor([ok(0)], events, { prompts: 1, crashes: [s(100) + 600_000] });
  assert.ok(far.failures.some((f) => /Application Error/.test(f)));
});

test('a flap is never scheduled in the first 90 s after a relaunch', () => {
  assert.equal(nextFlapNotBefore(s(0), s(30)), s(90));
  assert.equal(nextFlapNotBefore(s(0), s(200)), s(200));
});

test('the command line prints the verdict as JSON and exits 1 on failure', () => {
  const dir = mkdtempSync(join(tmpdir(), 'soak-verdict-'));
  const replies = join(dir, 'r.jsonl'), events = join(dir, 'e.jsonl');
  writeFileSync(replies, [ok(0), bad(1)].map((r) => JSON.stringify(r)).join('\n') + '\n');
  writeFileSync(events, [rec(0, 10), rec(1, 20)].map((r) => JSON.stringify(r)).join('\r\n') + '\r\n');
  const r = spawnSync('node', [join(root, 'scripts/ci/soak-verdict.mjs'), '--replies', replies, '--events', events, '--prompts', '2'], { encoding: 'utf8' });
  assert.equal(r.status, 1);
  assert.ok(JSON.parse(r.stdout).failures.length >= 1);
});

test('soak-run.ps1 uses the verdict script, a run-start look-back and the 90 s settle', () => {
  const ps = readFileSync(join(root, 'infra/aws-gpu/soak-run.ps1'), 'utf8');
  assert.match(ps, /soak-verdict\.mjs/);
  assert.match(ps, /\$soakStartedAt/);
  assert.doesNotMatch(ps, /AddMinutes\(-\$MaxMinutes\)/, 'the Application Error look-back starts at this run, not 150 minutes ago');
  assert.match(ps, /90/);
  assert.match(readFileSync(join(root, 'infra/aws-gpu/run-soak.sh'), 'utf8'), /soak-verdict\.mjs/, 'the box fetches the verdict script');
});

test('crash-capture looks where --chat-home puts the marker, and skips (never passes) on a hosted runner without dumps', () => {
  const faults = readFileSync(join(root, 'scripts/ci/faults.ps1'), 'utf8');
  const body = faults.slice(faults.indexOf('Register "crash-capture"'), faults.indexOf('Register "xet-hosts-blocked"'));
  assert.match(body, /chat-home\\config\\last-crash\.json/);
  assert.match(body, /APPDATA/);
  assert.match(body, /GITHUB_ACTIONS/);
  assert.match(body, /Skip = /);
  assert.match(body, /minidump appeared/, 'off a hosted runner a missing dump is still a failure');
});

test('a crash-capture skipped on a hosted runner counts only when it passed on the real Windows host', () => {
  const faults = readFileSync(join(root, 'scripts/ci/faults.ps1'), 'utf8');
  const wf = readFileSync(join(root, '.github/workflows/clean-host.yml'), 'utf8');
  const required = requiredScenarios(faults, wf);
  assert.ok(required.gpu_matrix.includes('crash-capture'), 'crash-capture must be required in the GPU matrix');
  const results = Object.fromEntries(required.clean_host_cpu.map((n) => [n, n === 'crash-capture' ? 'skipped' : 'passed']));
  const mk = (names, over = {}) => ({ run_at: '2026-10-30T08:00:00Z', junit_sha256: 'c'.repeat(64), results: { ...Object.fromEntries(names.map((n) => [n, 'passed'])), ...over } });
  const ctx = { tag: 't', headSha: 'b'.repeat(40), required, now: new Date('2026-11-01T00:00:00Z'), maxAgeDays: 14, git: { isAncestor: () => true, changedFiles: () => [] } };
  const withGpu = (gpuOver) => ({ tag: 't', tested_commit: 'a'.repeat(40), gpu_matrix: mk(required.gpu_matrix, gpuOver), clean_host_cpu: { ...mk([]), results } });
  assert.deepEqual(problemsFor(withGpu({}), ctx), [], 'passed on the box, skipped on the runner: accepted');
  const p = problemsFor(withGpu({ 'crash-capture': 'failed' }), ctx);
  assert.ok(p.some((x) => /GPU matrix.*crash-capture.*"failed"/.test(x)) && p.some((x) => /fresh-host.*crash-capture.*"skipped", not passed/.test(x)), p.join('|'));
});
