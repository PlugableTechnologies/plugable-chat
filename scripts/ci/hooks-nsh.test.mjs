import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const hooks = readFileSync(new URL('../../src-tauri/windows/hooks.nsh', import.meta.url), 'utf8');

test('the WER LocalDumps key names plugable-chat.exe, not an unexpanded macro', () => {
  const line = hooks.split(/\r?\n/).find((l) => l.startsWith('!define PC_WER_KEY'));
  assert.ok(line, 'PC_WER_KEY is defined');
  assert.ok(!line.includes('${'), `registry key must not contain an NSIS macro: ${line}`);
  assert.ok(line.endsWith('LocalDumps\\plugable-chat.exe"'), line);
});
