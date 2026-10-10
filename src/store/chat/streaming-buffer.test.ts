import assert from 'node:assert/strict';
import { test } from 'node:test';
import { appendTokenToBuffer, isChatRunning, stashStreamingOnLeave } from './streaming-buffer.ts';

const msgs = [
    { id: '1', role: 'user' as const, content: 'hi', timestamp: 1 },
    { id: '2', role: 'assistant' as const, content: 'par', timestamp: 2 },
];

test('New Chat while streaming stashes the live messages', () => {
    const r = stashStreamingOnLeave({ streamingChatId: 'a', currentChatId: 'a', chatMessages: msgs }, null);
    assert.deepEqual(r?.streamingMessages, msgs);
    assert.notEqual(r?.streamingMessages, msgs);
});

test('no stash when not streaming, viewing another chat, or staying', () => {
    assert.equal(stashStreamingOnLeave({ streamingChatId: null, currentChatId: 'a', chatMessages: msgs }, null), null);
    assert.equal(stashStreamingOnLeave({ streamingChatId: 'a', currentChatId: 'b', chatMessages: msgs }, null), null);
    assert.equal(stashStreamingOnLeave({ streamingChatId: 'a', currentChatId: 'a', chatMessages: msgs }, 'a'), null);
});

test('tokens append to the buffered assistant message', () => {
    const out = appendTokenToBuffer(msgs, 'tial', 5);
    assert.equal(out[1].content, 'partial');
    assert.equal(msgs[1].content, 'par');
});

test('token with empty or user-last buffer starts an assistant message', () => {
    assert.equal(appendTokenToBuffer([], 'x', 5)[0].role, 'assistant');
    assert.equal(appendTokenToBuffer([msgs[0]], 'x', 5).length, 2);
});

test('running indicator', () => {
    assert.equal(isChatRunning('a', 'a', true), true);
    assert.equal(isChatRunning('a', 'a', false), false);
    assert.equal(isChatRunning('a', 'b', true), false);
});
