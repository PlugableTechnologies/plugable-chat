import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
    getVisibleContent,
    hasOnlyThinkContent,
    parseMessageContent,
    stripResponseWrapperEcho,
} from './response-parser.ts';

test('an echoed <tool_response> wrapper is not shown, its text is', () => {
    const raw = '<tool_response>\nI can query the data but not write.\n</tool_response>';
    assert.equal(getVisibleContent(raw), 'I can query the data but not write.');
    const parts = parseMessageContent(raw);
    assert.ok(parts.every((p) => !p.content.includes('tool_response')));
});

test('wrapper variants with attributes are stripped too', () => {
    assert.equal(stripResponseWrapperEcho('<tool_response error="true">oops</tool_response>'), 'oops');
    assert.equal(stripResponseWrapperEcho('<tool_result server="a" tool="b">ok</tool_result>'), 'ok');
    assert.equal(stripResponseWrapperEcho('<function_response>ok</function_response>'), 'ok');
    assert.equal(stripResponseWrapperEcho('plain text, no tags'), 'plain text, no tags');
});

test('Qwen output with only a closing </think> closes an implicit reasoning block', () => {
    const raw = 'reasoning here</think>\n\nThe answer is 4.';
    assert.deepEqual(parseMessageContent(raw).map((p) => p.type), ['think', 'text']);
    assert.equal(getVisibleContent(raw), 'The answer is 4.');
});

test('an unclosed <think> leaves no visible text and counts as reasoning only', () => {
    const raw = '<think>still working it out and never finished';
    assert.equal(getVisibleContent(raw), '');
    assert.equal(hasOnlyThinkContent(raw), true);
});

test('a tool call is still extracted', () => {
    const raw = '<tool_call>{"name":"ask","arguments":{"q":"x"}}</tool_call>';
    assert.deepEqual(parseMessageContent(raw).map((p) => p.type), ['tool_call']);
});
