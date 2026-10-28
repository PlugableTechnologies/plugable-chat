#!/usr/bin/env node
// Turns the JUnit file of a clean-host / GPU matrix run (faults.ps1 writes clean-host-junit.xml) into the
// per-scenario record that check-release-evidence.mjs reads, merged into release-evidence/<tag>.json.
//
//   node scripts/ci/record-release-evidence.mjs --tag v0.1.0-rc19 --commit <sha> --suite gpu_matrix \
//        --junit matrix-out/clean-host-out/clean-host-junit.xml --host "g5.xlarge, driver 5xx"
//   node scripts/ci/record-release-evidence.mjs --tag v0.1.0-rc19 --commit <sha> --suite clean_host_cpu \
//        --junit <downloaded junit> --run-url https://github.com/<org>/plugable-chat/actions/runs/<id>
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { EVIDENCE_DIR, SUITES } from './check-release-evidence.mjs';

const unescapeXml = (s) => s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, '&');

/** scenario name -> passed | failed | skipped | expected-red, from faults.ps1's JUnit (see New-JUnitXml). */
export function resultsFromJunit(xml) {
  const results = {};
  for (const m of xml.matchAll(/<testcase\b([^>]*?)(?:\/>|>([\s\S]*?)<\/testcase>)/g)) {
    const rawName = /\bname="([^"]*)"/.exec(m[1])?.[1];
    if (!rawName) continue;
    const name = unescapeXml(rawName);
    const body = m[2] ?? '';
    const expectedRed = / \[expected red observed\]$/.test(name);
    const key = name.replace(/ \[expected red observed\]$/, '');
    results[key] = /<failure\b/.test(body) ? 'failed' : /<skipped\b/.test(body) ? 'skipped' : expectedRed ? 'expected-red' : 'passed';
  }
  return results;
}

export function mergeEvidence(existing, { tag, commit, suite, junitText, host, runUrl, runAt }) {
  if (!(suite in SUITES)) throw new Error(`unknown suite "${suite}" (use ${Object.keys(SUITES).join(' or ')})`);
  const base = existing && existing.tag === tag && existing.tested_commit === commit ? existing : { tag, tested_commit: commit };
  return {
    ...base,
    [suite]: {
      run_at: runAt,
      ...(host ? { host } : {}),
      ...(runUrl ? { run_url: runUrl } : {}),
      junit_sha256: createHash('sha256').update(junitText).digest('hex'),
      results: resultsFromJunit(junitText),
    },
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const get = (flag) => (args.includes(flag) ? args[args.indexOf(flag) + 1] : undefined);
  const [tag, commit, suite, junit] = [get('--tag'), get('--commit'), get('--suite'), get('--junit')];
  if (!tag || !commit || !suite || !junit) {
    console.error('usage: record-release-evidence.mjs --tag <tag> --commit <sha> --suite gpu_matrix|clean_host_cpu --junit <file> [--host <text>] [--run-url <url>] [--run-at <ISO date>]');
    process.exit(2);
  }
  const file = join(resolve(get('--root') ?? '.'), EVIDENCE_DIR, `${tag}.json`);
  const existing = existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : null;
  const merged = mergeEvidence(existing, {
    tag, commit, suite,
    junitText: readFileSync(junit, 'utf8'),
    host: get('--host'), runUrl: get('--run-url'), runAt: get('--run-at') ?? new Date().toISOString(),
  });
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, JSON.stringify(merged, null, 2) + '\n');
  const r = Object.values(merged[suite].results);
  console.log(`recorded ${suite} for ${tag}: ${r.filter((s) => s === 'passed').length} passed, ${r.filter((s) => s !== 'passed').length} not passed -> ${file}`);
}
