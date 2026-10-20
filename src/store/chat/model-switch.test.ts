// Run with: npm run test:unit  (node's built-in runner; excluded from tsc because there are no node typings)
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { chatModelNotice, setModelSucceeded, shouldEndChatOnModelSwitch } from './model-switch.ts';

test('opening a chat saved with another model offers it and does not switch', () => {
    assert.equal(chatModelNotice('qwen3.5-4b-generic-cpu:3', 'phi-4-mini-generic-cpu:5'), 'qwen3.5-4b-generic-cpu:3');
});

test('no notice when the chat model matches, or is unknown', () => {
    assert.equal(chatModelNotice('a', 'a'), null);
    assert.equal(chatModelNotice(undefined, 'a'), null);
    assert.equal(chatModelNotice(null, 'a'), null);
    assert.equal(chatModelNotice('', 'a'), null);
});

test('switching models never ends the open chat', () => {
    assert.equal(shouldEndChatOnModelSwitch('a', 'b'), false);
});

test('only an explicit false from set_model is a failed switch', () => {
    assert.equal(setModelSucceeded(true), true);
    assert.equal(setModelSucceeded(undefined), true);
    assert.equal(setModelSucceeded(false), false);
});

// The decisions above are only worth testing if the store uses them. These read the source because
// the store imports Tauri and cannot be loaded under node.
const slice = (name: string) => readFileSync(new URL(`./slices/${name}`, import.meta.url), 'utf8');

test('loadChat does not call setModel any more', () => {
    const src = slice('chat-history-slice.ts');
    const loadChat = src.slice(src.indexOf('loadChat: async'));
    assert.ok(!/setModel\(/.test(loadChat.slice(0, loadChat.indexOf('load_chat'))), 'loadChat must not force the saved model');
    assert.match(loadChat, /chatModelNotice\(/);
});

test('loadModel no longer clears the open chat, and checks the backend answer', () => {
    const src = slice('model-slice.ts');
    const loadModel = src.slice(src.indexOf('loadModel: async'), src.indexOf('downloadModel: async'));
    assert.ok(!/currentChatId: null/.test(loadModel), 'loadModel must keep the open chat');
    assert.ok(!/chatMessages: \[\]/.test(loadModel), 'loadModel must keep the messages');
    assert.match(loadModel, /setModelSucceeded\(/);
});
