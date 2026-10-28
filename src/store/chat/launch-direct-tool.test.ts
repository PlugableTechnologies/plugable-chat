import assert from 'node:assert/strict';
import { test } from 'node:test';
import { runDirectToolLaunch, waitForToolByLabel } from './launch-direct-tool.ts';
import { launchTurnInvocation } from './launch-turn.ts';
import type { DirectTool } from '../../lib/direct-tools.ts';

const ask: DirectTool = { server_id: 'iq-box', server_name: 'plugableiq', tool_name: 'ask', field_name: 'question', label: 'plugableiq - ask' };

function fakeStore(offeredAfterPolls: number, label: string | null = 'plugableiq - ask') {
    let polls = 0;
    const sent: unknown[] = [];
    const state: any = {
        launchDirectToolLabel: label,
        directTools: [] as DirectTool[],
        fetchDirectTools: async () => { polls++; if (polls > offeredAfterPolls) state.directTools = [ask]; },
        sendLaunchPrompt: async () => { sent.push({ ref: state.launchDirectToolRef, chosen: state.chosenDirectTool }); },
    };
    const set = (partial: Record<string, unknown>) => Object.assign(state, partial);
    return { state, set, sent, polls: () => polls };
}

test('the launch waits until a server offers the tool, then sends to it and preselects it', async () => {
    const s = fakeStore(2);
    await runDirectToolLaunch(() => s.state, s.set, 5000, 1);
    assert.equal(s.polls(), 3);
    assert.deepEqual(s.sent, [{ ref: { server_id: 'iq-box', tool_name: 'ask' }, chosen: { server_id: 'iq-box', tool_name: 'ask' } }]);
});

test('a tool nobody offers is still sent to (the backend reports it) and is not preselected', async () => {
    const s = fakeStore(1_000_000, 'ghost - ask');
    await runDirectToolLaunch(() => s.state, s.set, 30, 5);
    assert.deepEqual(s.sent, [{ ref: { server_id: 'ghost', tool_name: 'ask' }, chosen: undefined }]);
});

test('without --direct-tool nothing happens', async () => {
    const s = fakeStore(0, null);
    await runDirectToolLaunch(() => s.state, s.set, 30, 5);
    assert.equal(s.polls(), 0);
    assert.deepEqual(s.sent, []);
    assert.equal(await waitForToolByLabel(() => s.state, 'x - y', 10, 2), null);
});

test('a launch prompt goes to the model chat command, or with a direct tool to call_mcp_tool_directly with the tool ids', () => {
    const turn = { chatId: 'c1', title: 'T', message: 'hi', history: [], reasoningEffort: 'low' as const, model: 'm' };
    const [cmd, args] = launchTurnInvocation(null, turn);
    assert.equal(cmd, 'chat');
    assert.deepEqual(Object.keys(args).sort(), ['attachedFiles', 'attachedTables', 'attachedTabularFiles', 'attachedTools', 'chatId', 'history', 'message', 'model', 'reasoningEffort', 'title']);
    const [dcmd, dargs] = launchTurnInvocation({ server_id: 'iq-box', tool_name: 'ask' }, turn);
    assert.equal(dcmd, 'call_mcp_tool_directly');
    assert.deepEqual(dargs, { chatId: 'c1', title: 'T', message: 'hi', history: [], serverId: 'iq-box', toolName: 'ask' });
});
