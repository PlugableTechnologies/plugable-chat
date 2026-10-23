// Run with: npm run test:unit
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { inputDisabledReason, type InputDisabledInputs } from './input-disabled-reason.ts';
import { reduceDownloads } from './downloads.ts';
import { reduceOperations } from './operations.ts';

const ready: InputDisabledInputs = {
    streamingInOtherChat: false,
    isModelReady: true,
    modelState: { state: 'ready' },
    downloads: {},
    operations: {},
};

test('an enabled input has no reason', () => {
    assert.equal(inputDisabledReason(ready), null);
});

test('streaming in another chat is only named when that is the cause', () => {
    const r = inputDisabledReason({ ...ready, streamingInOtherChat: true })!;
    assert.equal(r.kind, 'streaming-other-chat');
    assert.match(r.placeholder, /Streaming in another chat/);
});

test('a loading model says so, not "streaming"', () => {
    const r = inputDisabledReason({ ...ready, isModelReady: false, modelState: { state: 'loading_model' } })!;
    assert.equal(r.kind, 'model-loading');
    assert.equal(r.placeholder, 'Model loading...');
    assert.doesNotMatch(r.placeholder, /treaming/);
});

test('a model download shows its name and percentage', () => {
    let downloads = reduceDownloads({}, { type: 'start', id: 'qwen3.5-4b' }, 1);
    downloads = reduceDownloads(downloads, { type: 'started', id: 'qwen3.5-4b' }, 1);
    downloads = reduceDownloads(downloads, { type: 'progress', id: 'qwen3.5-4b', percent: 38 }, 2);
    const r = inputDisabledReason({ ...ready, isModelReady: false, modelState: { state: 'initializing' }, downloads })!;
    assert.equal(r.kind, 'model-downloading');
    assert.equal(r.placeholder, 'Downloading qwen3.5-4b 38%...');
});

test('GPU setup is named while the model is not ready', () => {
    const operations = reduceOperations({}, { type: 'progress', key: 'ep-registration', percent: 4 }, 1);
    const r = inputDisabledReason({ ...ready, isModelReady: false, modelState: { state: 'initializing' }, operations })!;
    assert.equal(r.kind, 'gpu-setup');
    assert.equal(r.placeholder, 'Setting up GPU acceleration...');
    const later = inputDisabledReason({ ...ready, isModelReady: false, modelState: { state: 'initializing' }, operations, nowMs: 1 + 7 * 60000 + 5 })!;
    assert.equal(later.placeholder, 'Setting up GPU acceleration (7 min)...');
});

test('switching and unavailable models have their own text', () => {
    assert.equal(inputDisabledReason({ ...ready, isModelReady: false, modelState: { state: 'switching_model' } })!.kind, 'model-switching');
    assert.equal(inputDisabledReason({ ...ready, isModelReady: false, modelState: { state: 'service_unavailable' } })!.kind, 'model-unavailable');
});

test('a model that is not ready outranks streaming elsewhere', () => {
    const r = inputDisabledReason({ ...ready, streamingInOtherChat: true, isModelReady: false, modelState: { state: 'loading_model' } })!;
    assert.equal(r.kind, 'model-loading');
});

test('the input is enabled when only the search model failed and chat is ready', () => {
    // The embedding subsystem is not an input at all: search trouble never disables typing.
    assert.equal(inputDisabledReason({ ...ready, operations: reduceOperations({}, { type: 'error', key: 'embedding', message: 'x' }, 1) }), null);
});
