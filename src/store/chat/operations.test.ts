// Run with: npm run test:unit
import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
    classifyDatabaseError, describeOperationError, isQuietOperation, embeddingEventToAction, epEventToAction, hasActiveOperations,
    listOperations, reduceOperations, shouldAutoClear,
} from './operations.ts';

test('concurrent operations do not overwrite each other', () => {
    let ops = reduceOperations({}, { type: 'progress', key: 'ep-registration', percent: 10 }, 1);
    ops = reduceOperations(ops, { type: 'progress', key: 'model-download', percent: 5, file: 'a.onnx' }, 2);
    ops = reduceOperations(ops, embeddingEventToAction({ message: 'Loading', is_complete: false }), 3);
    assert.equal(listOperations(ops).length, 3);
    assert.equal(ops['ep-registration']!.percent, 10);
});

test('embedding failure is an error entry, never done', () => {
    const ops = reduceOperations({}, embeddingEventToAction({ message: 'boom', is_complete: true, error: true }), 1);
    assert.equal(ops.embedding!.state, 'error');
    assert.equal(ops.embedding!.retryable, true);
    assert.equal(shouldAutoClear(ops.embedding), false);
});

test('success auto-clears, errors do not', () => {
    const ops = reduceOperations({}, embeddingEventToAction({ message: 'ok', is_complete: true }), 1);
    assert.equal(shouldAutoClear(ops.embedding), true);
    assert.equal(reduceOperations(ops, { type: 'clear', key: 'embedding' }, 2).embedding, undefined);
});

test('progress keeps start time while active and restarts after error', () => {
    let ops = reduceOperations({}, { type: 'progress', key: 'model-download', percent: 1 }, 100);
    ops = reduceOperations(ops, { type: 'progress', key: 'model-download', percent: 2 }, 900);
    assert.equal(ops['model-download']!.startTime, 100);
    ops = reduceOperations(ops, { type: 'error', key: 'model-download', message: 'x' }, 950);
    ops = reduceOperations(ops, { type: 'progress', key: 'model-download', percent: 0 }, 1000);
    assert.equal(ops['model-download']!.startTime, 1000);
    assert.equal(ops['model-download']!.state, 'active');
});

test('progress after cancel request is ignored', () => {
    let ops = reduceOperations({}, { type: 'progress', key: 'ep-registration', percent: 3, cancellable: true }, 1);
    ops = reduceOperations(ops, { type: 'cancel-requested', key: 'ep-registration' }, 2);
    const next = reduceOperations(ops, { type: 'progress', key: 'ep-registration', percent: 4 }, 3);
    assert.equal(next, ops);
    assert.equal(hasActiveOperations(ops), true);
});

test('ep done without failures is cleared; unknown phases are ignored', () => {
    assert.equal(epEventToAction({ phase: 'done', message: 'failed' })!.type, 'clear');
    assert.equal(epEventToAction({ phase: 'done' })!.type, 'clear');
    assert.equal(epEventToAction({ phase: 'other' }), null);
});

test('percent clamped', () => {
    const ops = reduceOperations({}, { type: 'progress', key: 'toolbox-download', percent: 250 }, 1);
    assert.equal(ops['toolbox-download']!.percent, 100);
});

test('database error classification', () => {
    assert.equal(classifyDatabaseError('Schema refresh failed: embedding model not ready'), 'embedding-not-ready');
    assert.equal(classifyDatabaseError('Schema refresh failed: toolbox not downloaded'), 'toolbox-missing');
    assert.equal(classifyDatabaseError('toolbox failed to start: stderr'), 'toolbox-failed');
    assert.equal(classifyDatabaseError('nope'), 'other');
});

test('formatBytes', async () => {
    const { formatBytes } = await import('./operations.ts');
    assert.equal(formatBytes(2.5 * 1024 ** 3), '2.5 GB');
    assert.equal(formatBytes(216 * 1024 ** 2), '216 MB');
});

import { epEventToAction as _epEventToAction } from './operations.ts';

test('ep done with a success message clears the row instead of showing an error', () => {
    const a = _epEventToAction({ phase: 'done', message: 'GPU acceleration ready (CUDA).', failures: [] });
    assert.equal(a?.type, 'clear');
});

test('ep done after a cancel clears the row', () => {
    const a = _epEventToAction({ phase: 'done', message: 'GPU acceleration setup cancelled; using CPU models.', failures: [] });
    assert.equal(a?.type, 'clear');
});

test('ep done with failures shows a persistent error', () => {
    const a = _epEventToAction({ phase: 'done', message: 'could not set up: CUDA (network problem)', failures: [{ ep: 'CUDA' }] });
    assert.equal(a?.type, 'error');
});

test('an error banner gets a one-line cause and keeps the raw text as details', () => {
    const raw = 'Failed to retrieve onnx/model.onnx: os error 10054 '.repeat(20);
    const v = describeOperationError('embedding', `Failed to load CPU embedding model after 5 attempts. The connection to huggingface.co was interrupted - a firewall may be blocking it. Details: ${raw}`);
    assert.match(v.summary, /^Search features unavailable - retrying\. The connection to huggingface\.co was interrupted/);
    assert.ok(!v.summary.includes('10054'));
    assert.equal(v.details, raw);
    assert.equal(v.quiet, true);
});

test('a long message with no marker is cut to its first sentence and kept whole as details', () => {
    const msg = `Model failed to load. ${'x'.repeat(500)}`;
    const v = describeOperationError('model-download', msg);
    assert.equal(v.summary, 'Model failed to load.');
    assert.equal(v.details, msg);
    assert.equal(v.quiet, false);
});

test('a short message with no details is shown as is', () => {
    const v = describeOperationError('toolbox-download', 'Toolbox download failed.');
    assert.deepEqual(v, { summary: 'Toolbox download failed.', details: undefined, quiet: false });
});

test('only search-model and GPU-component failures are quiet', () => {
    assert.equal(isQuietOperation('embedding'), true);
    assert.equal(isQuietOperation('ep-registration'), true);
    assert.equal(isQuietOperation('model-download'), false);
});
