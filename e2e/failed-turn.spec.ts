import { expect, test } from '@playwright/test';
import { callsTo, emit, openApp } from './support/tauri-mock';

// David (rc15): a network drop ended a turn badly and every later prompt failed. The backend now stops the
// turn cleanly (src-tauri/src/turn_failure.rs); these cases show what the chat screen does with that answer.
// The backend is a mock: whether the real engine stops is the Rust tests' and the fault harness's job.

async function askAndGetStopped(page: import('@playwright/test').Page, kind: 'network_dropped' | 'engine_error') {
    await openApp(page);
    // The mock never says the engine is up; this is the event the real backend sends when it is.
    await emit(page, 'model-state-changed', { state: 'ready', model_id: 'qwen3.5-4b' });
    const box = page.getByTestId('chat-input-textarea');
    await box.fill('how many rows are there?');
    await page.getByTestId('chat-send-button').click();
    await expect.poll(async () => (await callsTo(page, 'chat')).length).toBe(1);
    await emit(page, 'chat-token', 'There are 3 rows and I was');
    await emit(page, 'chat-error', {
        error: 'The network connection was lost, so this answer was stopped. Press Retry when the connection is back.',
        kind,
        retryable: true,
        offerRepair: kind === 'engine_error',
    });
    await emit(page, 'chat-finished', null);
}

test.describe('a turn the backend stopped', () => {
    test('a network drop keeps the words so far, says what happened, and sends nothing until Retry is pressed', async ({ page }) => {
        await askAndGetStopped(page, 'network_dropped');
        const notice = page.locator('.chat-notice-bar');
        await expect(notice).toContainText('network connection was lost');
        await expect(page.getByText('There are 3 rows and I was')).toBeVisible();
        await page.waitForTimeout(500);
        expect((await callsTo(page, 'chat')).length, 'the turn must not restart by itself').toBe(1);
        await expect(notice.getByRole('button', { name: 'Repair engine' })).toHaveCount(0);

        await notice.getByRole('button', { name: 'Retry' }).click();
        await expect.poll(async () => (await callsTo(page, 'chat')).length).toBe(2);
        const second = (await callsTo(page, 'chat'))[1].args as { message: string };
        expect(second.message).toBe('how many rows are there?');
    });

    test('an engine error offers Repair engine, which runs the one-click repair', async ({ page }) => {
        await askAndGetStopped(page, 'engine_error');
        const notice = page.locator('.chat-notice-bar');
        await notice.getByRole('button', { name: 'Repair engine' }).click();
        await expect.poll(async () => (await callsTo(page, 'repair_engine')).length).toBe(1);
        await expect(notice).toContainText('Engine repaired');
    });
});
