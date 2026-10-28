import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { cleanHostDefault, problemsFor, registrations, requiredScenarios } from './check-release-evidence.mjs';
import { mergeEvidence, resultsFromJunit } from './record-release-evidence.mjs';

const root = fileURLToPath(new URL('../..', import.meta.url));
const faults = readFileSync(join(root, 'scripts/ci/faults.ps1'), 'utf8');
const workflow = readFileSync(join(root, '.github/workflows/clean-host.yml'), 'utf8');
const required = requiredScenarios(faults, workflow);

const TESTED = 'a'.repeat(40);
const HEAD = 'b'.repeat(40);
const NOW = new Date('2026-11-01T12:00:00Z');
const git = (changed = [], ancestor = true) => ({ isAncestor: () => ancestor, changedFiles: () => changed });
const suite = (names, over = {}) => ({ run_at: '2026-10-30T08:00:00Z', junit_sha256: 'c'.repeat(64), results: Object.fromEntries(names.map((n) => [n, 'passed'])), ...over });
const good = () => ({ tag: 'v1.0.0-rc1', tested_commit: TESTED, gpu_matrix: suite(required.gpu_matrix), clean_host_cpu: suite(required.clean_host_cpu) });
const ctx = (over = {}) => ({ tag: 'v1.0.0-rc1', headSha: HEAD, required, now: NOW, maxAgeDays: 14, git: git(), ...over });

test('the required sets are read from the files that define the scenarios', () => {
  assert.ok(required.gpu_matrix.includes('gpu-baseline') && required.gpu_matrix.includes('network-drop-mid-turn') && required.gpu_matrix.includes('network-drop-mid-tool-call') && required.gpu_matrix.includes('foundry-download-resume'));
  assert.ok(required.clean_host_cpu.includes('baseline') && required.clean_host_cpu.includes('crash-capture') && required.clean_host_cpu.includes('xet-hosts-blocked'));
  assert.ok(!required.gpu_matrix.includes('baseline'));
  // every default clean-host scenario is a real registered one (a typo there would make the gate unsatisfiable)
  const names = registrations(faults).map((r) => r.name);
  for (const n of required.clean_host_cpu) assert.ok(names.includes(n), `clean-host.yml names "${n}", which faults.ps1 does not register`);
  assert.equal(cleanHostDefault("default='[\"a\",\"b\"]'"). length, 2);
});

test('complete, fresh, passing evidence for a tested commit lets the tag through', () => {
  assert.deepEqual(problemsFor(good(), ctx()), []);
  assert.deepEqual(problemsFor(good(), ctx({ git: git(['docs/testing-ladder.md', 'release-evidence/v1.0.0-rc1.json', 'README.md']) })), [], 'docs and the evidence file may change after the run');
});

test('a tag with no evidence is blocked', () => {
  const p = problemsFor(null, ctx());
  assert.equal(p.length, 1);
  assert.match(p[0], /no release-evidence\/v1\.0\.0-rc1\.json/);
});

test('evidence missing one of the two suites is blocked', () => {
  const e = good();
  delete e.gpu_matrix;
  assert.ok(problemsFor(e, ctx()).some((x) => x.includes('GPU matrix: no recorded result')));
  const e2 = good();
  delete e2.clean_host_cpu;
  assert.ok(problemsFor(e2, ctx()).some((x) => x.includes('fresh-host CPU set: no recorded result')));
});

test('a failed, skipped or absent scenario blocks the tag', () => {
  for (const status of ['failed', 'skipped', 'expected-red']) {
    const e = good();
    e.gpu_matrix.results['network-drop-mid-turn'] = status;
    assert.ok(problemsFor(e, ctx()).some((x) => x.includes('"network-drop-mid-turn"') && x.includes(status)), status);
  }
  const e = good();
  delete e.clean_host_cpu.results['crash-capture'];
  assert.ok(problemsFor(e, ctx()).some((x) => x.includes('"crash-capture" has no result')));
});

test('evidence for another tag, with no commit, or too old is blocked', () => {
  assert.ok(problemsFor({ ...good(), tag: 'v9.9.9' }, ctx()).some((x) => x.includes('not "v1.0.0-rc1"')));
  assert.ok(problemsFor({ ...good(), tested_commit: 'abc' }, ctx()).some((x) => x.includes('tested_commit')));
  const old = good();
  old.gpu_matrix.run_at = '2026-09-01T00:00:00Z';
  assert.ok(problemsFor(old, ctx()).some((x) => x.includes('older than 14 days')));
  const noHash = good();
  noHash.clean_host_cpu.junit_sha256 = '';
  assert.ok(problemsFor(noHash, ctx()).some((x) => x.includes('no JUnit file hash')));
});

test('code changed after the tested commit, or a commit outside the history, blocks the tag', () => {
  const p = problemsFor(good(), ctx({ git: git(['src-tauri/src/agentic_loop.rs', 'docs/x.md']) }));
  assert.ok(p.some((x) => x.includes('code changed after the tested commit') && x.includes('agentic_loop.rs') && !x.includes('docs/x.md')));
  assert.ok(problemsFor(good(), ctx({ git: git([], false) })).some((x) => x.includes('not part of the tagged commit')));
});

test('JUnit from faults.ps1 becomes per-scenario results, with the expected-red marker kept apart', () => {
  const xml = `<?xml version="1.0"?><testsuites><testsuite name="s" tests="4">
  <testcase classname="s" name="a" time="1"></testcase>
  <testcase classname="s" name="b &amp; c" time="1"><failure message="x">x</failure></testcase>
  <testcase classname="s" name="d" time="0"><skipped message="no gpu"/></testcase>
  <testcase classname="s" name="e [expected red observed]" time="1"></testcase></testsuite></testsuites>`;
  assert.deepEqual(resultsFromJunit(xml), { a: 'passed', 'b & c': 'failed', d: 'skipped', e: 'expected-red' });
});

test('recording merges the two suites for one commit and starts over for another', () => {
  const junit = '<testsuites><testsuite><testcase name="gpu-baseline"></testcase></testsuite></testsuites>';
  const first = mergeEvidence(null, { tag: 't', commit: TESTED, suite: 'gpu_matrix', junitText: junit, host: 'g5', runAt: '2026-10-30T00:00:00Z' });
  const both = mergeEvidence(first, { tag: 't', commit: TESTED, suite: 'clean_host_cpu', junitText: junit, runUrl: 'https://example.invalid/run/1', runAt: '2026-10-31T00:00:00Z' });
  assert.ok(both.gpu_matrix && both.clean_host_cpu && both.gpu_matrix.host === 'g5');
  assert.equal(both.gpu_matrix.junit_sha256.length, 64);
  const other = mergeEvidence(both, { tag: 't', commit: HEAD, suite: 'clean_host_cpu', junitText: junit, runAt: '2026-11-01T00:00:00Z' });
  assert.equal(other.gpu_matrix, undefined, 'evidence for a different commit is not carried over');
  assert.throws(() => mergeEvidence(null, { tag: 't', commit: TESTED, suite: 'nope', junitText: junit, runAt: 'x' }), /unknown suite/);
});

test('end to end in a real git repository: a tag commit without evidence is blocked, with evidence and only docs changed it passes', () => {
  const repo = mkdtempSync(join(tmpdir(), 'evidence-'));
  const sh = (...a) => execFileSync('git', a, { cwd: repo, encoding: 'utf8' }).trim();
  sh('init', '-q');
  sh('config', 'user.email', 't@example.invalid');
  sh('config', 'user.name', 't');
  mkdirSync(join(repo, 'scripts/ci'), { recursive: true });
  mkdirSync(join(repo, '.github/workflows'), { recursive: true });
  writeFileSync(join(repo, 'scripts/ci/faults.ps1'), faults);
  writeFileSync(join(repo, '.github/workflows/clean-host.yml'), workflow);
  writeFileSync(join(repo, 'app.rs'), 'fn main() {}\n');
  sh('add', '-A');
  sh('commit', '-q', '-m', 'code under test');
  const tested = sh('rev-parse', 'HEAD');
  const script = join(root, 'scripts/ci/check-release-evidence.mjs');
  const run = () => {
    try {
      return { code: 0, out: execFileSync('node', [script, '--tag', 'v1.0.0-rc1', '--root', repo], { encoding: 'utf8', stdio: 'pipe' }) };
    } catch (e) {
      return { code: e.status, out: String(e.stderr) };
    }
  };

  const without = run();
  assert.equal(without.code, 1);
  assert.match(without.out, /Release v1\.0\.0-rc1 is blocked/);

  const evidence = { tag: 'v1.0.0-rc1', tested_commit: tested, gpu_matrix: suite(required.gpu_matrix, { run_at: new Date().toISOString() }), clean_host_cpu: suite(required.clean_host_cpu, { run_at: new Date().toISOString() }) };
  mkdirSync(join(repo, 'release-evidence'));
  writeFileSync(join(repo, 'release-evidence/v1.0.0-rc1.json'), JSON.stringify(evidence));
  sh('add', '-A');
  sh('commit', '-q', '-m', 'record evidence');
  const withEvidence = run();
  assert.equal(withEvidence.code, 0, withEvidence.out);

  writeFileSync(join(repo, 'app.rs'), 'fn main() { /* changed after the run */ }\n');
  sh('add', '-A');
  sh('commit', '-q', '-m', 'late code change');
  const stale = run();
  assert.equal(stale.code, 1);
  assert.match(stale.out, /code changed after the tested commit/);
});

test('release.yml makes every build and the publish step wait for the evidence check', () => {
  const yml = readFileSync(join(root, '.github/workflows/release.yml'), 'utf8');
  const block = (job) => {
    const start = yml.indexOf(`\n  ${job}:\n`);
    assert.ok(start >= 0, `release.yml has no job ${job}`);
    const rest = yml.slice(start + 1);
    const next = rest.slice(1).search(/\n  [a-z][a-z-]*:\n/);
    return next === -1 ? rest : rest.slice(0, next + 1);
  };
  for (const job of ['build-windows', 'build-macos', 'build-linux', 'publish']) {
    assert.match(block(job), /needs:\s*\[[^\]]*release-evidence[^\]]*\]/, `${job} does not need release-evidence`);
  }
  assert.match(block('release-evidence'), /node scripts\/ci\/check-release-evidence\.mjs --tag "\$TAG"/);
  assert.match(block('release-evidence'), /fetch-depth: 0/, 'the check needs history to prove the tested commit is part of the tag');
});
