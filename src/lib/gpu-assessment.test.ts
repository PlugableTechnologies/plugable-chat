import assert from 'node:assert/strict';
import { test } from 'node:test';
import { cardLines, cardTone, findGpuUpgrade, type GpuAssessment } from './gpu-assessment.ts';

const a = (state: GpuAssessment['state'], over: Partial<GpuAssessment> = {}): GpuAssessment => ({
    state, headline: 'h', detail: ['Windows lists: NVIDIA RTX 4090.'], hints: ['Choose Retry GPU setup.'], retryMayHelp: true, ...over,
});

// R5 (Pat, Bob): a failed catalog read or engine must look like itself, not like "no GPU".
test('r5: catalog and engine failures are red, never the quiet "no GPU" tone', () => {
    assert.equal(cardTone(a('catalog_failed')), 'red');
    assert.equal(cardTone(a('engine_unavailable')), 'red');
    assert.equal(cardTone(a('no_hardware')), 'info');
});

// R1: hardware present but unused is a fault worth an amber card, with both views of the machine.
test('r1: hardware the engine does not use is amber and shows what was observed and what to try', () => {
    const x = a('hardware_not_used', { headline: 'Windows sees NVIDIA RTX 4090, but the model engine is not using it' });
    assert.equal(cardTone(x), 'amber');
    const lines = cardLines(x);
    assert.match(lines[0], /not using it/);
    assert.ok(lines.some((l) => l.includes('Windows lists')));
    assert.ok(lines.some((l) => l.startsWith('Try: ')));
});

test('ready, still-checking and absent assessments show nothing', () => {
    assert.equal(cardTone(a('ready')), 'none');
    assert.equal(cardTone(a('checking')), 'none');
    assert.equal(cardTone(null), 'none');
});

const catalog = [
    { name: 'qwen3.5-4b-generic-cpu:3', alias: 'qwen3.5-4b', runtime: { deviceType: 'CPU' } },
    { name: 'qwen3.5-4b-cuda-gpu:4', alias: 'qwen3.5-4b', runtime: { deviceType: 'GPU' } },
    { name: 'phi-4-mini-generic-cpu:5', alias: 'phi-4-mini', runtime: { deviceType: 'CPU' } },
];

// R2: an eGPU that arrives after launch should be offered, not silently ignored.
test('r2: once the GPU is ready, a CPU build of a model with a GPU build gets an upgrade offer', () => {
    assert.equal(findGpuUpgrade('qwen3.5-4b-generic-cpu:3', catalog, a('ready'))?.name, 'qwen3.5-4b-cuda-gpu:4');
});

test('no upgrade offer while the GPU is not ready, on a GPU model, or when no GPU build exists', () => {
    assert.equal(findGpuUpgrade('qwen3.5-4b-generic-cpu:3', catalog, a('hardware_not_used')), null);
    assert.equal(findGpuUpgrade('qwen3.5-4b-cuda-gpu:4', catalog, a('ready')), null);
    assert.equal(findGpuUpgrade('phi-4-mini-generic-cpu:5', catalog, a('ready')), null);
    assert.equal(findGpuUpgrade(null, catalog, a('ready')), null);
    assert.equal(findGpuUpgrade('unknown-model', catalog, a('ready')), null);
});

test('a GPU build marked incompatible is not offered', () => {
    const c = catalog.map((m) => (m.runtime.deviceType === 'GPU' ? { ...m, incompatible: true } : m));
    assert.equal(findGpuUpgrade('qwen3.5-4b-generic-cpu:3', c, a('ready')), null);
});

import { modelsTabNotice } from './gpu-assessment.ts';

// R5: the empty list left by a failed catalog read was shown as "No GPU models are available".
test('r5: a catalog read failure is its own notice and never mentions a missing GPU', () => {
    const n = modelsTabNotice({ loading: false, catalogError: 'connection refused', gpuFilterEmpty: true, assessment: a('hardware_not_used') })!;
    assert.equal(n.kind, 'catalog-error');
    assert.equal(n.tone, 'red');
    assert.ok(!/no gpu/i.test(n.headline));
    assert.ok(n.lines.includes('connection refused'));
});

test('r1: with the GPU filter empty the notice is the backend\'s cause, with its observations and hints', () => {
    const x = a('hardware_not_used', { headline: 'Windows sees NVIDIA RTX 4090, but the model engine is not using it' });
    const n = modelsTabNotice({ loading: false, catalogError: null, gpuFilterEmpty: true, assessment: x })!;
    assert.equal(n.kind, 'gpu');
    assert.equal(n.tone, 'amber');
    assert.match(n.headline, /not using it/);
    assert.ok(n.lines.some((l) => l.includes('Windows lists')));
});

test('no notice while loading or when GPU models exist; "checking" before the first answer', () => {
    assert.equal(modelsTabNotice({ loading: true, catalogError: 'x', gpuFilterEmpty: true, assessment: null }), null);
    assert.equal(modelsTabNotice({ loading: false, catalogError: null, gpuFilterEmpty: false, assessment: a('hardware_not_used') }), null);
    assert.match(modelsTabNotice({ loading: false, catalogError: null, gpuFilterEmpty: true, assessment: null })!.headline, /Checking/);
});

test('a ready GPU with an empty list says the list is refreshing, not that the GPU is missing', () => {
    const n = modelsTabNotice({ loading: false, catalogError: null, gpuFilterEmpty: true, assessment: a('ready') })!;
    assert.match(n.headline, /refreshing/);
});

test('r5: when only the backend knows the catalog failed (empty list, no rejection) it is still not a GPU problem', () => {
    const x = a('catalog_failed', { headline: 'The model list could not be read, so GPU models cannot be shown.', detail: ['The model list could not be read: timed out'] });
    const n = modelsTabNotice({ loading: false, catalogError: null, gpuFilterEmpty: true, assessment: x })!;
    assert.equal(n.kind, 'catalog-error');
    assert.ok(n.lines.some((l) => l.includes('timed out')));
    assert.ok(!/no gpu/i.test(n.headline));
});

import { deviceFromModelId } from './gpu-assessment.ts';

test('the active device is read from the model id, and placeholders say nothing', () => {
    assert.equal(deviceFromModelId('qwen3.5-4b-cuda-gpu:4'), 'GPU');
    assert.equal(deviceFromModelId('qwen3.5-4b-generic-cpu:3'), 'CPU');
    assert.equal(deviceFromModelId('Loading...'), null);
    assert.equal(deviceFromModelId(null), null);
});
