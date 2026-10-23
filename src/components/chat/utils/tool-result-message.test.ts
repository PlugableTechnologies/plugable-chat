import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isToolResultMessage, toolResultView } from './tool-result-message.ts';

test('a user message wrapped in <tool_response> is a tool result, a typed one is not', () => {
    assert.equal(isToolResultMessage({ role: 'user', content: '<tool_response>\n42\n</tool_response>' }), true);
    assert.equal(isToolResultMessage({ role: 'user', content: 'what does <tool_response> mean?' }), false);
    assert.equal(isToolResultMessage({ role: 'tool', content: '42' }), true);
    assert.equal(isToolResultMessage({ role: 'assistant', content: '<tool_response>x</tool_response>' }), false);
});

test('toolResultView drops the wrapper and reads the error flag', () => {
    const ok = toolResultView({ role: 'user', content: '<tool_response>\nrows: 3\n</tool_response>\n\nAnswer now.' });
    assert.equal(ok.isError, false);
    assert.ok(ok.body.startsWith('rows: 3'));
    assert.ok(!ok.body.includes('<tool_response'));
    const bad = toolResultView({ role: 'user', content: '<tool_response error="true">\nboom\n</tool_response>' });
    assert.equal(bad.isError, true);
    assert.equal(bad.body, 'boom');
});
