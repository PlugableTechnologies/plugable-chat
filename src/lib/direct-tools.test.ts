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
