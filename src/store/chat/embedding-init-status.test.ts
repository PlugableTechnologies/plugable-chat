// Run with: npm run test:unit
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { nextOperationStatusForEmbeddingInit } from './embedding-init-status.ts';

test('in-progress init is a loading status', () => {
    const s = nextOperationStatusForEmbeddingInit(null, { message: 'Initializing', is_complete: false }, 5)!;
    assert.equal(s.type, 'loading');
    assert.equal(s.completed, false);
    assert.equal(s.startTime, 5);
});

test('success completes and is eligible for auto-dismiss', () => {
    const s = nextOperationStatusForEmbeddingInit(null, { message: 'loaded', is_complete: true }, 5)!;
    assert.equal(s.type, 'loading');
    assert.equal(s.completed, true);
});

test('failure is never shown as a completed (green check) status', () => {
    const s = nextOperationStatusForEmbeddingInit(
        null,
        { message: 'Failed to load CPU embedding model: x', is_complete: true, error: true },
        5,
    )!;
    assert.equal(s.type, 'error');
    assert.equal(s.completed, false);
    assert.match(s.message, /Failed to load CPU embedding model/);
});

test('later events keep the original start time', () => {
    const first = nextOperationStatusForEmbeddingInit(null, { message: 'a', is_complete: false }, 1)!;
    const second = nextOperationStatusForEmbeddingInit(first, { message: 'b', is_complete: false }, 99)!;
    assert.equal(second.startTime, 1);
});
