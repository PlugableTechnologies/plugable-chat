import { expect, test } from '@playwright/test';
import { callsTo, openApp, setConnectedServers, type ConnectedServerFixture } from './support/tauri-mock';

// The unattended-launch flags that let a GPU box run and be photographed with no mouse: --direct-tool, --open-settings and
// --select-tool (docs/testing-ladder.md, flag table). The backend is a mock that reports the flags the way
// `get_launch_overrides` does; src-tauri/src/cli.rs's tests prove the flags parse and reach that payload.

const ask = { name: 'ask', description: 'Ask.', inputSchema: { type: 'object', properties: { question: { type: 'string' } }, required: ['question'] } };
const plugableiq: ConnectedServerFixture = { server_id: 'iq-box', server_name: 'plugableiq', tools: [ask] };
const select = (page: import('@playwright/test').Page) => page.getByTestId('model-tool-select');

test.describe('--direct-tool', () => {
    test('the launch prompt goes to the tool, not the model, even when no model loads', async ({ page }) => {
        await openApp(page, {
            startup: 'failed-while-downloading',
            connectedServers: [plugableiq],
            directToolBehavior: { ask: { reply: 'Orders last week: 6,992.' } },
            launchOverrides: { initial_prompt: 'how many orders last week?', direct_tool: 'plugableiq - ask' },
        });
        await expect(page.getByText('Orders last week: 6,992.')).toBeVisible();
        const direct = await callsTo(page, 'call_mcp_tool_directly');
        expect(direct).toHaveLength(1);
        expect(direct[0].args).toMatchObject({ message: 'how many orders last week?', serverId: 'iq-box', toolName: 'ask' });
        expect(await callsTo(page, 'chat'), 'no local model is asked').toHaveLength(0);
    });

    test('every prompt of a --prompts-file run follows through the tool, each as a follow-up in the same chat with --same-chat', async ({ page }) => {
        await openApp(page, {
            connectedServers: [plugableiq],
            directToolBehavior: { ask: { reply: 'tool answer' } },
            launchOverrides: { initial_prompt: 'first question', queued_prompts: ['second question'], same_chat: true, direct_tool: 'plugableiq - ask' },
        });
        await expect.poll(async () => (await callsTo(page, 'call_mcp_tool_directly')).length).toBe(2);
        const direct = await callsTo(page, 'call_mcp_tool_directly');
        expect(direct.map((c) => (c.args as { message: string }).message)).toEqual(['first question', 'second question']);
        expect((direct[1].args as { history: unknown[] }).history.length, 'the second prompt carries the first exchange').toBeGreaterThan(0);
        expect((direct[1].args as { chatId: string }).chatId).toBe((direct[0].args as { chatId: string }).chatId);
        expect(await callsTo(page, 'chat')).toHaveLength(0);
    });

    test('the tool is picked up when its server connects after the app started', async ({ page }) => {
        await openApp(page, {
            directToolBehavior: { ask: { reply: 'late tool answer' } },
            launchOverrides: { initial_prompt: 'hello', direct_tool: 'PlugableIQ - Ask' },
        });
        await page.waitForTimeout(1500);
        expect(await callsTo(page, 'call_mcp_tool_directly'), 'nothing is sent while the tool is not connected').toHaveLength(0);
        await setConnectedServers(page, [plugableiq]);
        await expect(page.getByText('late tool answer')).toBeVisible({ timeout: 10_000 });
        expect((await callsTo(page, 'call_mcp_tool_directly'))[0].args).toMatchObject({ serverId: 'iq-box', toolName: 'ask' });
    });

    test('the launch tool is not remembered for the next launch', async ({ page }) => {
        await openApp(page, { connectedServers: [plugableiq], launchOverrides: { initial_prompt: 'hi', direct_tool: 'plugableiq - ask' } });
        await expect.poll(async () => (await callsTo(page, 'call_mcp_tool_directly')).length).toBe(1);
        expect(await callsTo(page, 'set_selected_direct_tool')).toHaveLength(0);
    });
});

test.describe('--open-settings', () => {
    for (const [tab, name] of [['network', 'Network'], ['tools', 'Tools'], ['databases', 'Databases']] as const) {
        test(`opens Settings on the ${tab} tab with no input`, async ({ page }) => {
            await openApp(page, { launchOverrides: { open_settings: tab } });
            await expect(page.locator('#settings-modal')).toBeVisible();
            // The active tab is the one with the blue underline; Models is the default, so none of these is open by accident.
            await expect(page.locator('.settings-tablist button').filter({ hasText: new RegExp(`^${name}$`) })).toHaveClass(/border-blue-500/);
            await expect(page.locator('.settings-tablist button.border-blue-500')).toHaveCount(1);
            if (tab === 'network') await expect(page.getByRole('button', { name: 'Test connection' })).toBeVisible();
        });
    }

    test('without the flag Settings stays closed, and a name that is not a tab changes nothing', async ({ page }) => {
        await openApp(page, { launchOverrides: {} });
        await expect(page.getByTestId('model-tool-select')).toBeVisible();
        await expect(page.locator('#settings-modal')).toHaveCount(0);
        await openApp(page, { launchOverrides: { open_settings: 'not-a-tab' } });
        await expect(page.locator('#settings-modal')).toHaveCount(0);
    });
});

test.describe('--select-tool', () => {
    test('the drop-down shows "server - tool" selected without a click, and nothing is sent or saved', async ({ page }) => {
        await openApp(page, { connectedServers: [plugableiq], launchOverrides: { select_tool: 'plugableiq - ask' } });
        await expect(select(page).locator('option:checked')).toHaveText(/plugableiq - ask/);
        expect(await callsTo(page, 'call_mcp_tool_directly')).toHaveLength(0);
        expect(await callsTo(page, 'set_selected_direct_tool')).toHaveLength(0);
    });

    test('a server that connects later is selected once it appears', async ({ page }) => {
        await openApp(page, { launchOverrides: { select_tool: 'plugableiq - ask' } });
        await expect(select(page).locator('option:checked')).not.toHaveText(/plugableiq - ask/);
        await setConnectedServers(page, [plugableiq]);
        await expect(select(page).locator('option:checked')).toHaveText(/plugableiq - ask/);
    });
});
