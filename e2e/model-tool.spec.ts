import { expect, test, type Page } from '@playwright/test';
import { callsTo, emit, openApp, setConnectedServers, type ConnectedServerFixture } from './support/tauri-mock';

// An MCP tool that takes one text field is offered in the model drop-down and called directly, with no local
// model: see docs/user-guide.md "Ask an MCP tool directly". The backend is a mock, so these cases show what the
// screen does with the list and the answers; that the backend picks the right tools and saves the turn is the
// Rust tests' job (src-tauri/src/mcp_direct_tools.rs, direct_tool_turn.rs).

const ask = {
    name: 'ask',
    description: 'Ask PlugableIQ a business question. Returns its answer with the footer: sources, data-through dates, receipts, facts used and confidence.',
    inputSchema: { type: 'object', properties: { question: { type: 'string' } }, required: ['question'] },
};
const twoFieldSearch = {
    name: 'search_orders',
    description: 'Search orders.',
    inputSchema: { type: 'object', properties: { query: { type: 'string' }, limit: { type: 'string' } }, required: ['query'] },
};
const countOnly = { name: 'count_orders', description: 'Count.', inputSchema: { type: 'object', properties: { since: { type: 'number' } } } };
const noInputs = { name: 'status', description: 'Status.', inputSchema: { type: 'object', properties: {} } };

const plugableiq: ConnectedServerFixture = { server_id: 'iq-box', server_name: 'plugableiq', tools: [ask, twoFieldSearch, countOnly, noInputs] };

const select = (page: Page) => page.getByTestId('model-tool-select');
const label = (page: Page) => page.locator('.app-model-label');
const sendButton = (page: Page) => page.getByTestId('chat-send-button');
const input = (page: Page) => page.getByTestId('chat-input-textarea');

async function chooseTool(page: Page, text: string) {
    await select(page).selectOption({ label: text });
}

test.describe('the model drop-down offers MCP tools', () => {
    test('with no server connected it says Model: and offers no tool; a server that connects turns it into Model / Tool:', async ({ page }) => {
        await openApp(page);
        await expect(label(page)).toHaveText('Model:');
        await expect(page.getByRole('option', { name: 'plugableiq - ask' })).toHaveCount(0);

        await setConnectedServers(page, [plugableiq]);
        await expect(label(page)).toHaveText('Model / Tool:');
        await expect(page.getByRole('option', { name: 'plugableiq - ask', exact: true })).toHaveCount(1);
        // The model is still there, ahead of the tools.
        const options = await select(page).locator('option').allTextContents();
        expect(options.map((o) => o.replace(' ✓', ''))).toEqual(['qwen3.5-4b', 'plugableiq - ask']);
    });

    test('a tool with two fields, a non-text field or no fields is not offered', async ({ page }) => {
        await openApp(page, { connectedServers: [plugableiq] });
        await expect(page.getByRole('option', { name: 'plugableiq - ask', exact: true })).toHaveCount(1);
        for (const name of ['search_orders', 'count_orders', 'status']) {
            await expect(page.getByRole('option', { name: new RegExp(name) })).toHaveCount(0);
        }
    });

    test('a tool is offered while the local model is still downloading, and a message sent to it shows the tool reply', async ({ page }) => {
        await openApp(page, {
            startup: 'failed-while-downloading',
            connectedServers: [plugableiq],
            directToolBehavior: { ask: { reply: 'Orders last week: 6,992. Sources: orders table. Confidence: high.' } },
        });
        // No model is usable here: the box says so and the text box is shut.
        await expect(input(page)).toBeDisabled();
        await expect(label(page)).toHaveText('Model / Tool:');

        await chooseTool(page, 'plugableiq - ask');
        await expect(input(page)).toBeEnabled();
        await input(page).fill('how many orders last week?');
        await sendButton(page).click();

        await expect(page.getByText('Orders last week: 6,992. Sources: orders table. Confidence: high.')).toBeVisible();
        const direct = await callsTo(page, 'call_mcp_tool_directly');
        expect(direct).toHaveLength(1);
        expect(direct[0].args).toMatchObject({ message: 'how many orders last week?', serverId: 'iq-box', toolName: 'ask' });
        expect(await callsTo(page, 'chat'), 'no local model is asked').toHaveLength(0);
        await expect(sendButton(page)).toBeDisabled(); // the box is empty again, ready for the next independent question
    });

    test('with the model ready, choosing the tool sends to it and choosing the model sends to the model again', async ({ page }) => {
        await openApp(page, { connectedServers: [plugableiq], directToolBehavior: { ask: { reply: 'tool says hi' } } });
        await expect.poll(async () => (await callsTo(page, 'get_cached_models')).length).toBeGreaterThan(0);
        await chooseTool(page, 'plugableiq - ask');
        await input(page).fill('first');
        await sendButton(page).click();
        await expect(page.getByText('tool says hi')).toBeVisible();

        await select(page).selectOption('qwen3.5-4b');
        await expect.poll(async () => (await callsTo(page, 'set_selected_direct_tool')).map((c) => (c.args as { tool: unknown }).tool)).toEqual([
            { server_id: 'iq-box', tool_name: 'ask' },
            null,
        ]);
        await emit(page, 'model-state-changed', { state: 'ready', model_id: 'qwen3.5-4b' });
        await input(page).fill('second');
        await sendButton(page).click();
        await expect.poll(async () => (await callsTo(page, 'chat')).length).toBe(1);
        expect(await callsTo(page, 'call_mcp_tool_directly')).toHaveLength(1);
    });

    test('the choice is remembered: a tool saved from the last launch is selected once its server is connected', async ({ page }) => {
        await openApp(page, { selectedDirectTool: { server_id: 'iq-box', tool_name: 'ask' } });
        // Not connected yet: the model is shown.
        await expect(select(page)).not.toHaveValue(/^tool:/);
        await setConnectedServers(page, [plugableiq]);
        await expect(select(page).locator('option:checked')).toHaveText('plugableiq - ask');
    });

    test('a tool that fails shows a failed turn with Retry, and Retry asks the tool again', async ({ page }) => {
        await openApp(page, {
            connectedServers: [plugableiq],
            directToolBehavior: { ask: { failure: 'Request timed out after 30s waiting for id 3 (raise request_timeout_secs for this server)' } },
        });
        await expect.poll(async () => (await callsTo(page, 'get_cached_models')).length).toBeGreaterThan(0);
        await chooseTool(page, 'plugableiq - ask');
        await input(page).fill('slow question');
        await sendButton(page).click();

        const notice = page.locator('.chat-notice-bar');
        await expect(notice).toContainText('did not give an answer');
        await expect(notice).toContainText('timed out after 30s');
        await expect(notice.getByRole('button', { name: 'Repair engine' })).toHaveCount(0);
        await page.waitForTimeout(300);
        expect(await callsTo(page, 'call_mcp_tool_directly'), 'nothing is re-sent until Retry is pressed').toHaveLength(1);

        await notice.getByRole('button', { name: 'Retry' }).click();
        await expect.poll(async () => (await callsTo(page, 'call_mcp_tool_directly')).length).toBe(2);
        expect((await callsTo(page, 'call_mcp_tool_directly'))[1].args).toMatchObject({ message: 'slow question', toolName: 'ask' });
    });
});
