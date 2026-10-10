import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildWelcomeMessage } from './welcome-message.ts';
import { reduceOperations } from './operations.ts';

test('names the model and size and reports stage status', () => {
    let ops = reduceOperations({}, { type: 'error', key: 'embedding', message: 'offline' }, 1);
    ops = reduceOperations(ops, { type: 'progress', key: 'model-download', percent: 40 }, 2);
    const msg = buildWelcomeMessage({ modelName: 'qwen3.5-4b', modelSizeBytes: 2.5 * 1024 ** 3, operations: ops });
    assert.match(msg, /qwen3\.5-4b, 2\.5 GB\): in progress, 40%/);
    assert.match(msg, /Embedding model.*failed: offline/);
});

test('falls back when the model is not yet known', () => {
    const msg = buildWelcomeMessage({ modelName: null, modelSizeBytes: null, operations: {} });
    assert.match(msg, /the default chat model\): waiting/);
});
