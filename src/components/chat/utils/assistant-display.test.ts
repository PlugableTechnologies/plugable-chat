import assert from 'node:assert/strict';
import { test } from 'node:test';
import { showNoAnswerNote, showOrphanToolCall, toolCallName } from './assistant-display.ts';

test('toolCallName reads the tool out of a call body, even a damaged one', () => {
    assert.equal(toolCallName('{"name":"brix___ask","arguments":{}}'), 'ask');
    assert.equal(toolCallName('{"name":"status"'), 'status');
    assert.equal(toolCallName('not json'), 'tool');
});

const base = { hasThink: true, hasMeaningfulText: false, hasToolCallParts: false, toolRecordCount: 0, inProgress: false };

test('a finished turn that only reasoned gets the no-answer note', () => {
    assert.equal(showNoAnswerNote(base), true);
});

test('no note while the turn is still running, or when there is an answer or a tool call', () => {
    assert.equal(showNoAnswerNote({ ...base, inProgress: true }), false);
    assert.equal(showNoAnswerNote({ ...base, hasMeaningfulText: true }), false);
    assert.equal(showNoAnswerNote({ ...base, hasToolCallParts: true }), false);
    assert.equal(showNoAnswerNote({ ...base, toolRecordCount: 1 }), false);
});

test('a tool call without a result is shown only once the turn is over', () => {
    assert.equal(showOrphanToolCall(false, false), true);
    assert.equal(showOrphanToolCall(true, false), false);
    assert.equal(showOrphanToolCall(false, true), false);
});
