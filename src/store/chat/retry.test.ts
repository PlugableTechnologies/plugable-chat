import assert from 'node:assert/strict';
import { test, beforeEach } from 'node:test';
import { runRetry, resetRetryAttempts } from './retry.ts';
import type { OperationAction } from './operations.ts';
import type { GpuAssessment } from '../../lib/gpu-assessment.ts';

beforeEach(() => resetRetryAttempts());

function harness(result: unknown | Error) {
    const actions: OperationAction[] = [];
    let refreshed = 0;
    return {
        actions,
        get refreshed() { return refreshed; },
        deps: {
            invoke: async () => { if (result instanceof Error) throw result; return result as never; },
            dispatch: (a: OperationAction) => actions.push(a),
            refreshModels: async () => { refreshed += 1; },
        },
    };
}
const gpu = (state: GpuAssessment['state'], headline = 'h'): GpuAssessment => ({ state, headline, detail: [], hints: [], retryMayHelp: true });

// R7 (Bob): Retry returned quietly (model already loaded) and the row stayed on "Retrying..." for ever.
test('r7: a successful embedding retry ends in done, not in Retrying', async () => {
    const h = harness(undefined);
    await runRetry('embedding', { command: 'retry_embedding_init' }, h.deps);
    assert.equal(h.actions[0].type, 'progress');
    assert.deepEqual(h.actions.at(-1), { type: 'done', key: 'embedding', message: 'Search model ready' });
});

// R6/R7: a retry that fails again must say it is a new attempt with the new reason.
test('r7: a second failed retry is numbered and carries the new reason', async () => {
    const h1 = harness(new Error('first reason'));
    await runRetry('embedding', { command: 'retry_embedding_init' }, h1.deps);
    const h2 = harness(new Error('second reason'));
    await runRetry('embedding', { command: 'retry_embedding_init' }, h2.deps);
    const e1 = h1.actions.at(-1) as { message: string };
    const e2 = h2.actions.at(-1) as { message: string };
    assert.match(e1.message, /Retry 1 failed: first reason/);
    assert.match(e2.message, /Retry 2 failed: second reason/);
    assert.notEqual(e1.message, e2.message);
    assert.match((h2.actions[0] as { message: string }).message, /attempt 2/);
});

// R4 (Pat, Bob): Retry GPU setup did nothing visible.
test('r4: a GPU retry that finds a usable GPU clears the row and refreshes the model list after the backend finished', async () => {
    const h = harness(gpu('ready'));
    await runRetry('ep-registration', { command: 'retry_gpu_registration' }, h.deps);
    assert.deepEqual(h.actions.at(-1), { type: 'clear', key: 'ep-registration' });
    assert.equal(h.refreshed, 1);
});

test('r4: a GPU retry that still cannot use the hardware says so, with the attempt number', async () => {
    const h = harness(gpu('hardware_not_used', 'Windows sees NVIDIA RTX 4090, but the model engine is not using it'));
    await runRetry('ep-registration', { command: 'retry_gpu_registration' }, h.deps);
    const last = h.actions.at(-1) as { type: string; message: string };
    assert.equal(last.type, 'error');
    assert.match(last.message, /not using it.*attempt 1/);
    assert.equal(h.refreshed, 1);
});

test('r4: a machine with no GPU is information, not a retry failure', async () => {
    const h = harness(gpu('no_hardware'));
    await runRetry('ep-registration', { command: 'retry_gpu_registration' }, h.deps);
    assert.equal(h.actions.at(-1)?.type, 'clear');
});

test('r4: "already running" from the backend is shown as the reason', async () => {
    const h = harness(new Error('A GPU setup attempt is already running. Wait for it to finish.'));
    await runRetry('ep-registration', { command: 'retry_gpu_registration' }, h.deps);
    assert.match((h.actions.at(-1) as { message: string }).message, /already running/);
});

test('a model download retry refreshes the model list; a toolbox retry does not', async () => {
    const m = harness(undefined);
    await runRetry('model-download', { command: 'download_model' }, m.deps);
    assert.equal(m.refreshed, 1);
    const t = harness(undefined);
    await runRetry('toolbox-download', { command: 'install_toolbox' }, t.deps);
    assert.equal(t.refreshed, 0);
});
