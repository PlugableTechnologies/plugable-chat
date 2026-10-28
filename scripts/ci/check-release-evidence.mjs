#!/usr/bin/env node
// Release gate: a tag is published only when a recorded, passing GPU-matrix run and a passing Windows fresh-host
// CPU set exist for the commit being tagged. The record is `release-evidence/<tag>.json`, written by
// scripts/ci/record-release-evidence.mjs from the JUnit files the runs produced. release.yml runs this before any
// build or signing step, so a tag without the evidence never reaches the signing approval.
//
//   node scripts/ci/check-release-evidence.mjs --tag v0.1.0-rc19 [--root .] [--max-age-days 14]
//
// What it proves and what it does not: the evidence says which scenarios passed on which commit and carries the
// hash of the JUnit file; code changed after that commit (anything but docs and the evidence itself) invalidates it.
// It cannot prove the run happened: the file is reviewed like any other change.
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const EVIDENCE_DIR = 'release-evidence';
export const SUITES = { gpu_matrix: 'GPU matrix', clean_host_cpu: 'Windows fresh-host CPU set' };
// Changes after the tested commit that cannot alter what ships.
const NEUTRAL = [/^release-evidence\//, /^docs\//, /^README\.md$/, /^\.github\/release-notes-template\.md$/];

/** Scenario names and tags from `Register "name" "issue" @("tag", ...)` lines of faults.ps1. */
export function registrations(faultsText) {
  const out = [];
  for (const m of faultsText.matchAll(/^Register "([^"]+)" "[^"]*" @\(([^)]*)\)/gm)) {
    out.push({ name: m[1], tags: [...m[2].matchAll(/"([^"]+)"/g)].map((t) => t[1]) });
  }
  return out;
}

/** The default scenario set clean-host.yml runs on a fresh windows-latest runner. */
export function cleanHostDefault(workflowText) {
  const m = workflowText.match(/default='(\[[^']*\])'/);
  return m ? JSON.parse(m[1]) : [];
}

/** Every scenario each suite must have passed, read from the files that define them so the two cannot drift apart. */
export function requiredScenarios(faultsText, workflowText) {
  const gpu = registrations(faultsText).filter((r) => r.tags.includes('gpu')).map((r) => r.name);
  return { gpu_matrix: gpu, clean_host_cpu: cleanHostDefault(workflowText) };
}

const HEX40 = /^[0-9a-f]{40}$/;
const HEX64 = /^[0-9a-f]{64}$/;

/**
 * @param evidence the parsed `release-evidence/<tag>.json`, or null when there is none
 * @param ctx { tag, headSha, required, now: Date, maxAgeDays, git: { isAncestor(a, b), changedFiles(a, b) } }
 */
export function problemsFor(evidence, ctx) {
  if (!evidence) return [`no ${EVIDENCE_DIR}/${ctx.tag}.json: record the GPU matrix and the fresh-host CPU set for this release first (scripts/ci/record-release-evidence.mjs)`];
  const problems = [];
  if (evidence.tag !== ctx.tag) problems.push(`the evidence is for tag "${evidence.tag}", not "${ctx.tag}"`);
  if (!HEX40.test(evidence.tested_commit ?? '')) {
    problems.push('the evidence names no tested_commit (a 40-character commit id)');
  } else if (!ctx.git.isAncestor(evidence.tested_commit, ctx.headSha)) {
    problems.push(`the tested commit ${evidence.tested_commit.slice(0, 8)} is not part of the tagged commit's history`);
  } else {
    const changed = ctx.git.changedFiles(evidence.tested_commit, ctx.headSha).filter((f) => !NEUTRAL.some((re) => re.test(f)));
    if (changed.length) problems.push(`code changed after the tested commit ${evidence.tested_commit.slice(0, 8)}: ${changed.slice(0, 5).join(', ')}${changed.length > 5 ? ` and ${changed.length - 5} more` : ''}; run the matrix again`);
  }

  for (const [key, label] of Object.entries(SUITES)) {
    const suite = evidence[key];
    if (!suite) {
      problems.push(`${label}: no recorded result`);
      continue;
    }
    const runAt = Date.parse(suite.run_at ?? '');
    if (Number.isNaN(runAt)) problems.push(`${label}: run_at is missing or not a date`);
    else if (runAt > ctx.now.getTime() + 3600_000) problems.push(`${label}: run_at is in the future`);
    else if ((ctx.now.getTime() - runAt) / 86_400_000 > ctx.maxAgeDays) problems.push(`${label}: the run is older than ${ctx.maxAgeDays} days`);
    if (!HEX64.test(suite.junit_sha256 ?? '')) problems.push(`${label}: no JUnit file hash recorded`);
    for (const name of ctx.required[key] ?? []) {
      const status = suite.results?.[name];
      if (status === undefined) problems.push(`${label}: scenario "${name}" has no result`);
      else if (status !== 'passed') problems.push(`${label}: scenario "${name}" is "${status}", not passed`);
    }
  }
  return problems;
}

function gitFor(root) {
  const run = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' });
  return {
    isAncestor: (a, b) => {
      try {
        execFileSync('git', ['merge-base', '--is-ancestor', a, b], { cwd: root, stdio: 'ignore' });
        return true;
      } catch {
        return false;
      }
    },
    changedFiles: (a, b) => run('diff', '--name-only', a, b).split('\n').filter(Boolean),
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const get = (flag, dflt) => (args.includes(flag) ? args[args.indexOf(flag) + 1] : dflt);
  const tag = get('--tag');
  const root = resolve(get('--root', '.'));
  if (!tag) {
    console.error('usage: check-release-evidence.mjs --tag <tag> [--root <repo>] [--max-age-days N]');
    process.exit(2);
  }
  const file = join(root, EVIDENCE_DIR, `${tag}.json`);
  const evidence = existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : null;
  const required = requiredScenarios(
    readFileSync(join(root, 'scripts/ci/faults.ps1'), 'utf8'),
    readFileSync(join(root, '.github/workflows/clean-host.yml'), 'utf8'),
  );
  const headSha = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
  const problems = problemsFor(evidence, { tag, headSha, required, now: new Date(), maxAgeDays: Number(get('--max-age-days', '14')), git: gitFor(root) });
  if (problems.length) {
    console.error(`Release ${tag} is blocked:\n` + problems.map((p) => `- ${p}`).join('\n'));
    process.exit(1);
  }
  console.log(`release evidence for ${tag}: GPU matrix (${required.gpu_matrix.length} scenarios) and fresh-host CPU set (${required.clean_host_cpu.length}) passed on ${evidence.tested_commit.slice(0, 8)}`);
}
