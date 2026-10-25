import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
    VERIFIED_MODELS,
    groupByTrust,
    isVerifiedModel,
    modelTrust,
    trustNote,
    EXPERIMENTAL_SECTION_TITLE,
} from './verified-models.ts';

test('trust levels follow family and tested version', () => {
    assert.equal(modelTrust('qwen3.5-4b'), 'verified');
    assert.equal(modelTrust('qwen3.5-4b-cuda-gpu:4'), 'verified');
    assert.equal(modelTrust('Phi-4-mini-instruct-cuda-gpu:5'), 'verified');
    assert.equal(modelTrust('phi-4-mini-instruct-cuda-gpu:6'), 'verified-new-build');
    assert.equal(modelTrust('phi-4-mini-reasoning-cuda-gpu:3'), 'experimental');
    assert.equal(modelTrust('gpt-oss-20b-cuda-gpu:1'), 'experimental');
    assert.equal(modelTrust('qwen3.5-4bx'), 'experimental');
    assert.equal(isVerifiedModel('qwen3.5-4b-generic-cpu:3'), true);
});

test('notes: none for verified, a warning otherwise', () => {
    assert.equal(trustNote('qwen3.5-4b-cuda-gpu:4'), null);
    assert.match(trustNote('qwen3.5-4b-cuda-gpu:9')!, /newer build/i);
    assert.match(trustNote('gpt-oss-20b')!, /not tested/i);
});

test('verified models come first, experimental are separate, order otherwise kept', () => {
    const models = ['gpt-oss-20b', 'phi-4-mini-instruct-cuda-gpu:6', 'qwen3-1.7b', 'qwen3.5-4b-cuda-gpu:4'];
    const { verified, experimental } = groupByTrust(models, (m) => m);
    assert.deepEqual(verified, ['qwen3.5-4b-cuda-gpu:4', 'phi-4-mini-instruct-cuda-gpu:6']);
    assert.deepEqual(experimental, ['gpt-oss-20b', 'qwen3-1.7b']);
});

test('section title is the agreed wording', () => {
    assert.equal(EXPERIMENTAL_SECTION_TITLE, 'Experimental (not tested by Plugable)');
});

test('mirrors the Rust list exactly (ids and tested versions)', () => {
    const rust = readFileSync(new URL('../../src-tauri/src/verified_models.rs', import.meta.url), 'utf8');
    const block = rust.slice(rust.indexOf('pub const VERIFIED_MODELS'), rust.indexOf('#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum ModelTrust'));
    const rustIds = [...block.matchAll(/id: "([^"]+)"/g)].map((m) => m[1]);
    assert.deepEqual(rustIds, VERIFIED_MODELS.map((v) => v.id));
    const rustVersions = [...block.matchAll(/tested_versions: &\[([^\]]*)\]/g)].map((m) =>
        [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]),
    );
    assert.deepEqual(rustVersions, VERIFIED_MODELS.map((v) => v.testedVersions));
});
