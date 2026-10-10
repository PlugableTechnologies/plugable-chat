import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isReloadShortcut } from './webview-guards.ts';

test('reload shortcuts are detected', () => {
    assert.equal(isReloadShortcut({ key: 'F5' }), true);
    assert.equal(isReloadShortcut({ key: 'r', ctrlKey: true }), true);
    assert.equal(isReloadShortcut({ key: 'R', ctrlKey: true, shiftKey: true }), true);
    assert.equal(isReloadShortcut({ key: 'r', metaKey: true }), true);
});

test('ordinary keys pass', () => {
    assert.equal(isReloadShortcut({ key: 'r' }), false);
    assert.equal(isReloadShortcut({ key: 'c', ctrlKey: true }), false);
});
