// Run with: npm run test:unit
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { startupBarModel, searchFeaturesUnavailable } from './startup-display.ts';
import type { ResourceStatusType, SubsystemStatusData } from './types.ts';

const sub = (o: Partial<Record<keyof SubsystemStatusData, ResourceStatusType>>): SubsystemStatusData => {
    const s = (k: keyof SubsystemStatusData) => ({ status: o[k] ?? ('ready' as ResourceStatusType) });
    return {
        foundry_service: s('foundry_service'),
        model: s('model'),
        cpu_embedding: s('cpu_embedding'),
        mcp_servers: s('mcp_servers'),
        settings: s('settings'),
    };
};

test('a failed chat model is a red startup failure', () => {
    const m = startupBarModel('failed', sub({ model: 'failed' }));
    assert.equal(m.tone, 'red');
    assert.equal(m.message, 'Startup failed');
});

test('a failed search model alone is never red', () => {
    // The backend does not fail start-up for this; even if state were failed, chat is working.
    const m = startupBarModel('failed', sub({ cpu_embedding: 'failed' }));
    assert.notEqual(m.tone, 'red');
    assert.equal(m.tone, 'amber');
    assert.equal(startupBarModel('connecting_to_foundry', sub({ cpu_embedding: 'failed', model: 'initializing' })).tone, 'blue');
});

test('a stalled start-up with a slow GPU setup is amber, not red', () => {
    const m = startupBarModel('failed', sub({ model: 'initializing', foundry_service: 'ready' }));
    assert.equal(m.tone, 'amber');
});

test('the subsystem count excludes the search model', () => {
    const m = startupBarModel('connecting_to_foundry', sub({ cpu_embedding: 'failed', model: 'initializing' }));
    assert.equal(m.totalCount, 2);
    assert.equal(m.readyCount, 1);
});

test('embedding-failed-but-chat-ready shows the amber search chip and a ready bar', () => {
    const s = sub({ cpu_embedding: 'failed' });
    assert.equal(searchFeaturesUnavailable(s), true);
    assert.equal(startupBarModel('ready', s).tone, 'green');
    assert.equal(searchFeaturesUnavailable(sub({})), false);
    assert.equal(searchFeaturesUnavailable(null), false);
});

// rc18 (Bob, Pat): "Startup failed (1/2)" in red while the chat model was still downloading.
test('a failed model is not a red failure while a model download or GPU setup is still moving', () => {
    const failed = sub({ model: 'failed' });
    assert.equal(startupBarModel('failed', failed, true).tone, 'amber');
    assert.match(startupBarModel('failed', failed, true).message, /download or setup is in progress/i);
    assert.equal(startupBarModel('failed', failed, false).tone, 'red');
    assert.equal(startupBarModel('failed', failed).tone, 'red', 'no activity argument keeps the old verdict');
    assert.equal(startupBarModel('failed', sub({ foundry_service: 'failed' }), true).tone, 'amber');
});

test('a ready start-up is green whatever is downloading', () => {
    assert.equal(startupBarModel('ready', sub({}), true).tone, 'green');
});
