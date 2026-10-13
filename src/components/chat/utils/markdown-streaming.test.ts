import assert from 'node:assert/strict';
import { test } from 'node:test';
import { closeOpenMarkdownConstructs } from './markdown-streaming.ts';

test('closes an unterminated code fence', () => {
    assert.equal(closeOpenMarkdownConstructs('a\n```sql\nSELECT 1'), 'a\n```sql\nSELECT 1\n```');
});

test('leaves a balanced code fence alone', () => {
    const balanced = '```sql\nSELECT 1\n```\ndone';
    assert.equal(closeOpenMarkdownConstructs(balanced), balanced);
});

test('hides a table row that is still streaming', () => {
    const streaming = '| Month | Sales |\n|---|---|\n| 2025-01 | $20';
    assert.equal(closeOpenMarkdownConstructs(streaming), '| Month | Sales |\n|---|---|');
});

test('keeps a completed table row', () => {
    const complete = '| Month | Sales |\n|---|---|\n| 2025-01 | $20 |';
    assert.equal(closeOpenMarkdownConstructs(complete), complete);
});

test('plain prose passes through', () => {
    assert.equal(closeOpenMarkdownConstructs('Hello **wor'), 'Hello **wor');
});
