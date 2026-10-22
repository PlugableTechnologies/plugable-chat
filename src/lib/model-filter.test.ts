import assert from 'node:assert/strict';
import { test } from 'node:test';
import { matchesModelQuery, type FilterableModel } from './model-filter.ts';

const phi: FilterableModel = { name: 'phi-4-openvino-gpu:2', alias: 'phi-4', displayName: 'Phi-4', task: 'chat-completion', runtime: { deviceType: 'GPU', executionProvider: 'OpenVINOExecutionProvider' } };
const qwen: FilterableModel = { name: 'qwen3.5-9b-generic-gpu:4', alias: 'qwen3.5-9b', runtime: { deviceType: 'GPU' } };

test('empty or blank query matches everything', () => {
    assert.equal(matchesModelQuery(phi, ''), true);
    assert.equal(matchesModelQuery(phi, '   '), true);
});

test('matches name, alias, device and provider, case-insensitively', () => {
    assert.equal(matchesModelQuery(phi, 'PHI'), true);
    assert.equal(matchesModelQuery(qwen, '9b'), true);
    assert.equal(matchesModelQuery(phi, 'openvino'), true);
    assert.equal(matchesModelQuery(qwen, 'gpu'), true);
});

test('every word must match', () => {
    assert.equal(matchesModelQuery(phi, 'phi gpu'), true);
    assert.equal(matchesModelQuery(phi, 'phi npu'), false);
});

test('no match and missing optional fields', () => {
    assert.equal(matchesModelQuery(qwen, 'gemma'), false);
    assert.equal(matchesModelQuery({ name: 'x' }, 'x'), true);
});
