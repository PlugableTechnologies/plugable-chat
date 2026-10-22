import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fallbackQuestion, planFallback } from './fallback.ts';

const cached = [{ model_id: 'qwen3.5-9b-generic-gpu:4', alias: 'qwen3.5-9b' }];
const base = { current_model: 'gpt-oss-20b-generic-gpu:1', fallback_model: 'phi-4-mini-instruct' };

test('a fallback that is not on disk waits for the user instead of downloading (Wash: 2.2 GB with no prompt)', () => {
    const plan = planFallback({ ...base, fallback_resolved_name: 'phi-4-mini-instruct-generic-cpu:5', fallback_size_bytes: 4_900_000_000 }, cached, 'x');
    assert.deepEqual(plan, { kind: 'confirm-download', model: 'phi-4-mini-instruct', label: 'phi-4-mini-instruct-generic-cpu:5', sizeBytes: 4_900_000_000 });
});

test('a cached fallback is switched to without a download', () => {
    const plan = planFallback({ ...base, fallback_model: 'qwen3.5-9b' }, cached, 'gpt-oss-20b-generic-gpu:1');
    assert.deepEqual(plan, { kind: 'switch', modelId: 'qwen3.5-9b-generic-gpu:4', name: 'qwen3.5-9b' });
});

test('already on the fallback is a no-op', () => {
    assert.equal(planFallback({ ...base, fallback_model: 'qwen3.5-9b' }, cached, 'qwen3.5-9b-generic-gpu:4').kind, 'none');
});

test('unknown size and name fall back to the requested name', () => {
    const plan = planFallback(base, [], 'x');
    assert.deepEqual(plan, { kind: 'confirm-download', model: 'phi-4-mini-instruct', label: 'phi-4-mini-instruct', sizeBytes: null });
});

test('the question names the model and size when known', () => {
    const fmt = (n: number) => `${(n / 1e9).toFixed(1)} GB`;
    assert.equal(fallbackQuestion('gpt-oss-20b', 'phi-4-mini', 2_200_000_000, fmt), "gpt-oss-20b can't run on this PC. Download phi-4-mini (2.2 GB) to keep chatting?");
    assert.equal(fallbackQuestion('a', 'b', null, fmt), "a can't run on this PC. Download b to keep chatting?");
});
