#!/usr/bin/env node
// Every bug that escaped to a higher rung must leave a test behind. docs/testing-ladder.md keeps those
// in the "Escapes with a named test" table, and each row's last cell names `path#test name`. This fails
// when a row names no test, names a file that is not there, or names a test the file does not contain.
// The older "Escapes so far" table predates the rule and is not checked. Run from the repo root.
import { readFileSync, existsSync } from 'node:fs';
import { resolve, relative, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';

const HEADING = /^##\s+Escapes with a named test/m;

export function problemsFor(markdown, repoRoot) {
  const start = markdown.search(HEADING);
  if (start === -1) return ['docs/testing-ladder.md has no "Escapes with a named test" section'];
  const section = markdown.slice(start).split('\n').slice(1);
  const rows = [];
  for (const line of section) {
    if (/^##\s/.test(line)) break;
    if (!line.trim().startsWith('|')) continue;
    const cells = line.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim());
    if (cells.length < 4 || /^-+$/.test(cells[0]) || cells[0] === 'Escape') continue;
    rows.push(cells);
  }
  if (rows.length === 0) return ['the "Escapes with a named test" section has no rows'];

  const problems = [];
  for (const cells of rows) {
    const escape = cells[0].slice(0, 70);
    const refs = [...cells[cells.length - 1].matchAll(/`([^`#]+)#([^`]+)`/g)];
    if (refs.length === 0) {
      problems.push(`"${escape}" names no test (the last cell needs \`path#test name\`)`);
      continue;
    }
    for (const [, path, name] of refs) {
      const full = resolve(repoRoot, path);
      const rel = relative(repoRoot, full);
      if (rel.startsWith('..') || isAbsolute(rel)) {
        problems.push(`"${escape}": ${path} is outside the repository`);
      } else if (!existsSync(full)) {
        problems.push(`"${escape}": ${path} does not exist`);
      } else if (!readFileSync(full, 'utf8').includes(name.trim())) {
        problems.push(`"${escape}": ${path} does not contain the test "${name.trim()}"`);
      }
    }
  }
  return problems;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const problems = problemsFor(readFileSync('docs/testing-ladder.md', 'utf8'), process.cwd());
  if (problems.length) {
    console.error(problems.map((p) => `- ${p}`).join('\n'));
    process.exit(1);
  }
  console.log('every escape in the enforced table names a test that exists');
}
