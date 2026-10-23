import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseSavedMessages } from './loaded-messages.ts';

test('a good transcript loads with ids and timestamps filled in', () => {
    const r = parseSavedMessages(JSON.stringify([
        { role: 'user', content: 'hi' },
        { role: 'assistant', content: 'hello', system_prompt: 'be brief' },
    ]), 1000);
    assert.equal(r.skipped, 0);
    assert.equal(r.messages.length, 2);
    assert.equal(r.messages[0].id, '1000-0');
    assert.equal(r.messages[1].systemPromptText, 'be brief');
});

test('a damaged entry is skipped, the rest of the chat still loads', () => {
    const r = parseSavedMessages(JSON.stringify([
        { role: 'user', content: 'first' },
        null,
        42,
        { content: 'no role' },
        { role: 'robot', content: 'unknown role' },
        { role: 'assistant', content: 'last' },
    ]));
    assert.equal(r.skipped, 4);
    assert.deepEqual(r.messages.map((m) => m.content), ['first', 'last']);
});

test('null content from a native tool-call turn becomes an empty string', () => {
    const r = parseSavedMessages(JSON.stringify([{ role: 'assistant', content: null, tool_calls: [] }]));
    assert.equal(r.messages[0].content, '');
});

test('text that is not JSON, or not a list, loads as an empty chat and says it skipped something', () => {
    assert.deepEqual(parseSavedMessages('{"half": '), { messages: [], skipped: 1 });
    assert.deepEqual(parseSavedMessages('{"role":"user"}'), { messages: [], skipped: 1 });
});
