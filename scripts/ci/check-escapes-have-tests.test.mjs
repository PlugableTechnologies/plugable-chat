import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { problemsFor } from './check-escapes-have-tests.mjs';

const HEADING = '## Escapes with a named test (enforced by scripts/ci/check-escapes-have-tests.mjs)';
const doc = (...rows) => `# Ladder\n\n| Escape | Found at | Now caught at | Test |\n|---|---|---|---|\n| old row with no test | GitHub | somewhere | |\n\n${HEADING}\n\n| Escape | Found at | Now caught at | Test |\n|---|---|---|---|\n${rows.join('\n')}\n\nTrailing prose.\n`;

function repo() {
  const root = mkdtempSync(join(tmpdir(), 'escapes-'));
  mkdirSync(join(root, 'e2e'));
  writeFileSync(join(root, 'e2e', 'x.spec.ts'), "test('a 404 is expected', () => {});\n");
  return root;
}

test('a row whose test file and test name exist passes', () => {
  const row = '| 404 read as plain reachable | Bob (rc18) | rung 1 | `e2e/x.spec.ts#a 404 is expected` |';
  assert.deepEqual(problemsFor(doc(row), repo()), []);
});

test('a row with an empty Test cell fails', () => {
  const problems = problemsFor(doc('| something escaped | Bob | rung 1 | |'), repo());
  assert.equal(problems.length, 1);
  assert.match(problems[0], /something escaped/);
  assert.match(problems[0], /names no test/);
});

test('a row naming a file that does not exist, or a test name that is not in the file, fails', () => {
  const missingFile = problemsFor(doc('| a | b | c | `e2e/nope.spec.ts#x` |'), repo());
  assert.match(missingFile[0], /does not exist/);
  const missingName = problemsFor(doc('| a | b | c | `e2e/x.spec.ts#no such test` |'), repo());
  assert.match(missingName[0], /does not contain the test/);
});

test('a path that climbs out of the repository is refused', () => {
  assert.match(problemsFor(doc('| a | b | c | `../../etc/passwd#x` |'), repo())[0], /outside the repository/);
});

test('the older table above the heading is not checked, and a missing section is itself a problem', () => {
  assert.deepEqual(problemsFor(doc('| ok | b | c | `e2e/x.spec.ts#a 404 is expected` |'), repo()), []);
  assert.match(problemsFor('# no section here\n', repo())[0], /no "Escapes with a named test" section/);
});

test('a section with a heading but no rows fails, so the table cannot be emptied to pass', () => {
  assert.match(problemsFor(doc(), repo())[0], /no rows/);
});

// Windows checks docs out with CRLF line endings (see the version-check escape in docs/testing-ladder.md).
test('a CRLF checkout of the document gives the same answers', () => {
  const good = '| ok | b | c | `e2e/x.spec.ts#a 404 is expected` |';
  assert.deepEqual(problemsFor(doc(good).replace(/\n/g, '\r\n'), repo()), []);
  assert.equal(problemsFor(doc('| bad | b | c | |').replace(/\n/g, '\r\n'), repo()).length, 1);
});
