import { expect, test, type Page } from '@playwright/test';
import { callsTo, emit, openApp, type MockOptions } from './support/tauri-mock';

// Each case below is a tester report from rc18 (Bob, Pat, David): see docs/testing-ladder.md,
// "Escapes so far". The backend is a mock, so these prove what the screens do with an answer.

async function openSettingsTab(page: Page, tab: string) {
    await page.locator('.sidebar-settings-button').click();
    await expect(page.locator('#settings-modal')).toBeVisible();
    await page.locator('.settings-tablist').getByRole('button', { name: tab, exact: true }).click();
}

const saveButton = (page: Page) => page.locator('.settings-header-actions').getByRole('button', { name: 'Save', exact: true });

const bigQuerySource = { id: 'sales-bq', name: 'Sales BigQuery', kind: 'bigquery', enabled: true, command: '/usr/bin/toolbox', project_id: 'acme-sales' };

const hostRows = [
    { host: 'huggingface.co', ok: true, status: 404, millis: 40, reason: null, purpose: 'Search model: file list and small files' },
    { host: 'cas-bridge.xethub.hf.co', ok: false, status: null, millis: null, reason: 'name did not resolve: dns error', purpose: 'Search model: large files (Xet storage)' },
];

test.describe('Settings > Network', () => {
    test('a 404 on the home page is reported as the expected answer, and each host says what it is for', async ({ page }) => {
        await openApp(page, { responses: { test_network_connection: hostRows } });
        await openSettingsTab(page, 'Network');
        await page.getByRole('button', { name: 'Test connection' }).click();

        const results = page.locator('#network-test-results');
        await expect(results).toContainText('Search model: large files (Xet storage)');
        await expect(results.locator('li', { hasText: 'huggingface.co' })).toContainText(/404.*expected/);
        // A 404 proves the name resolved and TLS worked, nothing more; the screen must not imply a download would succeed.
        await expect(results).toContainText(/name resolved and a secure connection/);
    });

    test('a loaded search model is not shown as a green success beside a failing host', async ({ page }) => {
        await openApp(page, { responses: { test_network_connection: hostRows } });
        await openSettingsTab(page, 'Network');
        await page.getByRole('button', { name: 'Retry search model' }).click();
        await expect(page.getByText('The search model loaded.')).toBeVisible();

        await page.getByRole('button', { name: 'Test connection' }).click();
        const results = page.locator('#network-test-results');
        await expect(results).toBeVisible();
        await expect(page.getByText('The search model loaded.')).toHaveCount(0);
        await expect(results).toContainText(/search model is already (loaded|on this computer)/i);
    });
});

test.describe('Settings > Tools', () => {
    const devServer: MockOptions = {
        mcpServers: [{ id: 'dev-mcp', name: 'docs_server', enabled: false, command: 'cargo', args: ['run', '--release'] }],
        mcpStartFailures: { 'dev-mcp': "Could not start 'cargo': program not found. Install the Rust toolchain (rustup.rs), or change the command in Settings > Tools." },
    };
    const toggleFor = (page: Page, name: string) => page.getByRole('switch', { name: `Enable ${name}` });

    test('a server that cannot start stays off and the error says what to install', async ({ page }) => {
        await openApp(page, devServer);
        await openSettingsTab(page, 'Tools');
        await toggleFor(page, 'docs_server').click();

        await expect(page.getByText(/program not found/).first()).toBeVisible();
        await expect(toggleFor(page, 'docs_server')).toHaveAttribute('aria-checked', 'false');
        // The failed start must not leave the card claiming a connection or an unsaved edit.
        await expect(page.getByText('Unsaved')).toHaveCount(0);
        // "enabled: true" must never reach the saved settings for a server that did not start.
        const saved = await callsTo(page, 'update_mcp_server');
        expect(saved.filter((c) => (c.args as { config: { enabled: boolean } }).config.enabled)).toHaveLength(0);
    });

    test('turning a working server on saves at once and leaves nothing to save', async ({ page }) => {
        await openApp(page, { mcpServers: [{ id: 'ok-mcp', name: 'docs_server', enabled: false, command: 'node' }] });
        await openSettingsTab(page, 'Tools');
        await toggleFor(page, 'docs_server').click();

        await expect(toggleFor(page, 'docs_server')).toHaveAttribute('aria-checked', 'true');
        await expect(page.getByText('Unsaved')).toHaveCount(0);
        await expect(saveButton(page)).toHaveCount(0);
    });
});

test.describe('Settings > Databases', () => {
    test('Refresh is not offered while the Database Toolbox is off, and the screen says why', async ({ page }) => {
        await openApp(page, { databaseToolbox: { enabled: false, sources: [bigQuerySource] } });
        await openSettingsTab(page, 'Databases');

        const card = page.locator('.database-source-card').first();
        await expect(card.getByRole('button', { name: 'Refresh' })).toBeDisabled();
        await expect(card).toContainText(/Database Toolbox is off/);
    });

    test("Bob's flow: switching the toolbox on and pressing Refresh saves first, so the refresh sees it", async ({ page }) => {
        await openApp(page, { databaseToolbox: { enabled: false, sources: [bigQuerySource] } });
        await openSettingsTab(page, 'Databases');

        await page.getByRole('switch', { name: 'Enable Database Toolbox' }).click();
        await page.locator('.database-source-card').first().getByRole('button', { name: 'Refresh' }).click();

        // The backend refreshes what is saved. Saving also refreshes every enabled source, so no separate refresh call follows.
        await expect.poll(async () => (await callsTo(page, 'update_database_toolbox_config')).length).toBe(1);
        const [save] = await callsTo(page, 'update_database_toolbox_config');
        expect((save.args as { config: { enabled: boolean } }).config.enabled).toBe(true);
        const order = (await page.evaluate(() => window.__mockCalls)).map((c) => c.cmd);
        const refresh = order.indexOf('refresh_database_schema_for_source');
        expect(refresh === -1 || refresh > order.indexOf('update_database_toolbox_config')).toBe(true);
    });

    test('Save appears only while there is something to save', async ({ page }) => {
        await openApp(page, { databaseToolbox: { enabled: false, sources: [bigQuerySource] } });
        await openSettingsTab(page, 'Databases');
        await expect(saveButton(page)).toHaveCount(0);

        await page.getByRole('switch', { name: 'Enable Database Toolbox' }).click();
        await expect(saveButton(page)).toBeEnabled();
        await saveButton(page).click();
        await expect(saveButton(page)).toHaveCount(0);
    });
});

test.describe('Settings window', () => {
    test('an unsaved change does not leave a Save button behind after the window is closed and reopened', async ({ page }) => {
        await openApp(page, { databaseToolbox: { enabled: false, sources: [bigQuerySource] } });
        await openSettingsTab(page, 'Databases');
        await page.getByRole('switch', { name: 'Enable Database Toolbox' }).click();
        await expect(saveButton(page)).toBeVisible();

        await page.locator('.settings-backdrop').click({ position: { x: 5, y: 5 } });
        await expect(page.locator('#settings-modal')).toHaveCount(0);
        await openSettingsTab(page, 'Databases');
        await expect(saveButton(page)).toHaveCount(0);
        await expect(page.getByRole('switch', { name: 'Enable Database Toolbox' })).toHaveAttribute('aria-checked', 'false');
    });
});

test.describe('Attach Database', () => {
    const table = { table: { source_id: 'sales-bq', source_name: 'Sales BigQuery', table_fq_name: 'acme-sales.orders.daily', enabled: true, column_count: 12, description: null }, relevance_score: 0, keyword_only: false };

    test('an empty list says why and offers Refresh, which fills it', async ({ page }) => {
        await openApp(page, { databaseToolbox: { enabled: true, sources: [bigQuerySource] }, tablesAfterRefresh: [table] });
        await page.locator('.chat-attach-button').click();
        await page.getByRole('button', { name: /Attach Database/ }).click();

        const empty = page.locator('.database-attachment-empty');
        await expect(empty).toContainText(/not been read yet|no tables .* cached/i);
        await empty.getByRole('button', { name: 'Refresh' }).click();
        await expect(page.getByText('acme-sales.orders.daily'.split('.').pop()!)).toBeVisible();
    });
});

test.describe('Settings screen layout', () => {
    test('every tab fits inside the window without a sideways scroll bar, wrapping to a second row when narrow', async ({ page }) => {
        // Pixel baselines differ between macOS and the Linux CI runner, so this checks geometry instead.
        await page.setViewportSize({ width: 760, height: 800 });
        await openApp(page);
        await page.locator('.sidebar-settings-button').click();
        const tablist = page.locator('.settings-tablist');
        await expect(tablist).toBeVisible();
        const { scrollWidth, clientWidth } = await tablist.evaluate((el) => ({ scrollWidth: el.scrollWidth, clientWidth: el.clientWidth }));
        expect(scrollWidth).toBeLessThanOrEqual(clientWidth);
        const modal = (await page.locator('.settings-surface').boundingBox())!;
        for (const button of await tablist.getByRole('button').all()) {
            const box = (await button.boundingBox())!;
            expect(box.x + box.width).toBeLessThanOrEqual(modal.x + modal.width + 1);
        }
        const rowTops = new Set((await Promise.all((await tablist.getByRole('button').all()).map((b) => b.boundingBox()))).map((b) => Math.round(b!.y)));
        expect(rowTops.size).toBeGreaterThan(1);
    });
});

test.describe('Start-up banner', () => {
    test('no red "Startup failed" while the chat model download is moving', async ({ page }) => {
        await openApp(page, { startup: 'failed-while-downloading' });
        // The app asks for the default model itself when none is cached; progress events only count once it has.
        await expect.poll(async () => (await callsTo(page, 'download_model')).length).toBeGreaterThan(0);
        await emit(page, 'model-download-started', { id: 'qwen3.5-4b', model: 'qwen3.5-4b', size_bytes: 2_500_000_000 });
        for (const progress of [12, 31, 48]) await emit(page, 'model-download-progress', { file: 'qwen3.5-4b', progress });

        const bar = page.locator('.startup-state-bar');
        await expect(bar).toBeVisible();
        await expect(bar).not.toContainText('Startup failed');
        await expect(bar).not.toHaveClass(/bg-red/);
    });

    test('a failed model with no download running is still the red failure', async ({ page }) => {
        await openApp(page, { startup: 'failed-while-downloading' });
        await expect(page.locator('.startup-state-bar')).toContainText('Startup failed');
    });
});
