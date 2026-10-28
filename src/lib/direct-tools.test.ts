import assert from 'node:assert/strict';
import { test } from 'node:test';
import { activeDirectTool, directToolOptionValue, modelDropdownLabel, parseDirectToolOptionValue, type DirectTool } from './direct-tools.ts';

const ask: DirectTool = { server_id: 'iq', server_name: 'plugableiq', tool_name: 'ask', field_name: 'question', label: 'plugableiq - ask' };

test('the drop-down says Model / Tool: once a server offers a tool, and Model: otherwise', () => {
    assert.equal(modelDropdownLabel([]), 'Model:');
    assert.equal(modelDropdownLabel([ask]), 'Model / Tool:');
});

test('a tool option value round-trips, including ids with colons and spaces, and is never mistaken for a model id', () => {
    const ref = { server_id: 'my:server one', tool_name: 'ask/it' };
    const value = directToolOptionValue(ref);
    assert.deepEqual(parseDirectToolOptionValue(value), ref);
    assert.equal(parseDirectToolOptionValue('qwen3.5-4b'), null);
    assert.equal(parseDirectToolOptionValue('tool:only-one-part'), null);
    assert.equal(parseDirectToolOptionValue('tool:%E0%A4%A:x'), null, 'a broken escape is ignored, not thrown');
});

test('a remembered choice counts only while its tool is offered', () => {
    assert.equal(activeDirectTool([ask], { server_id: 'iq', tool_name: 'ask' }), ask);
    assert.equal(activeDirectTool([], { server_id: 'iq', tool_name: 'ask' }), null, 'server not connected yet');
    assert.equal(activeDirectTool([ask], { server_id: 'iq', tool_name: 'other' }), null);
    assert.equal(activeDirectTool([ask], null), null);
});

test('--direct-tool / --select-tool find the offered tool by its drop-down text, forgiving case and spaces', async () => {
    const { toolFromLabel, refFromLabel } = await import('./direct-tools.ts');
    assert.equal(toolFromLabel([ask], 'plugableiq - ask'), ask);
    assert.equal(toolFromLabel([ask], '  PlugableIQ - Ask '), ask);
    assert.equal(toolFromLabel([ask], 'plugableiq - other'), null);
    assert.equal(toolFromLabel([ask], ''), null);
    // a tool nobody offers is still addressable, so the backend can report it instead of the run hanging
    assert.deepEqual(refFromLabel('my - server - ask'), { server_id: 'my - server', tool_name: 'ask' });
    assert.equal(refFromLabel('ask'), null);
    assert.equal(refFromLabel(' - ask'), null);
});

test('--open-settings tab names are the ids the Settings store uses, and anything else is not a tab', async () => {
    const { asSettingsTab, SETTINGS_TAB_IDS } = await import('./direct-tools.ts');
    for (const id of SETTINGS_TAB_IDS) assert.equal(asSettingsTab(id), id);
    assert.equal(asSettingsTab('nope'), null);
    assert.equal(asSettingsTab(null), null);
    const { readFileSync } = await import('node:fs');
    const store = readFileSync(new URL('../store/settings-store.ts', import.meta.url), 'utf8');
    for (const id of SETTINGS_TAB_IDS) assert.ok(store.includes(`'${id}'`), `settings-store.ts has no tab '${id}'`);
});
