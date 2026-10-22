import assert from 'node:assert/strict';
import { test } from 'node:test';
import { isCancelError, listDownloads, reduceDownloads, summarizeDownloads, type DownloadsMap } from './downloads.ts';

const T = 1000;
const run = (m: DownloadsMap, ...actions: Parameters<typeof reduceDownloads>[1][]) =>
    actions.reduce((acc, a, i) => reduceDownloads(acc, a, T + i), m);

test('a second download does not hide the first (Pat: first progress bar disappears)', () => {
    const m = run({},
        { type: 'start', id: 'a' }, { type: 'started', id: 'a' }, { type: 'progress', id: 'a', percent: 40 },
        { type: 'start', id: 'b' });
    assert.equal(listDownloads(m).length, 2);
    assert.equal(m.a.percent, 40);
    assert.equal(m.a.state, 'active');
    assert.equal(m.b.state, 'queued');
});

test('progress events are routed to their own download', () => {
    const m = run({},
        { type: 'start', id: 'a' }, { type: 'start', id: 'b' },
        { type: 'progress', id: 'b', percent: 10 }, { type: 'progress', id: 'a', percent: 70 });
    assert.equal(m.a.percent, 70);
    assert.equal(m.b.percent, 10);
});

test('completion leaves 100% done and never stays active (Wash: bar never disappears)', () => {
    const m = run({}, { type: 'start', id: 'a' }, { type: 'progress', id: 'a', percent: 100 }, { type: 'done', id: 'a' });
    assert.equal(m.a.state, 'done');
    const s = summarizeDownloads(m);
    assert.equal(s.type, 'done');
});

test('completing one leaves the other active', () => {
    const m = run({},
        { type: 'start', id: 'a' }, { type: 'start', id: 'b' }, { type: 'progress', id: 'b', percent: 5 },
        { type: 'done', id: 'a' });
    assert.equal(m.a.state, 'done');
    assert.equal(m.b.state, 'active');
    assert.equal(summarizeDownloads(m).type, 'progress');
});

test('a late progress event cannot revive a finished or cancelled download', () => {
    let m = run({}, { type: 'start', id: 'a' }, { type: 'done', id: 'a' }, { type: 'progress', id: 'a', percent: 50 });
    assert.equal(m.a.state, 'done');
    m = run({}, { type: 'start', id: 'c' }, { type: 'cancelled', id: 'c' }, { type: 'progress', id: 'c', percent: 50 });
    assert.equal(m.c, undefined);
});

test('an error keeps the row until dismissed, and is summarized as an error', () => {
    const m = run({}, { type: 'start', id: 'a', label: 'gpt-oss' }, { type: 'error', id: 'a', message: 'disk full' });
    assert.equal(m.a.state, 'error');
    const s = summarizeDownloads(m);
    assert.equal(s.type, 'error');
    assert.match((s as { message: string }).message, /gpt-oss.*disk full/);
});

test('an error raised by cancelling is not a failure and removes the row', () => {
    const m = run({}, { type: 'start', id: 'a' }, { type: 'cancel-requested', id: 'a' }, { type: 'error', id: 'a', message: 'whatever' });
    assert.equal(m.a, undefined);
    const m2 = run({}, { type: 'start', id: 'a' }, { type: 'error', id: 'a', message: 'Download cancelled' });
    assert.equal(m2.a, undefined);
    assert.equal(summarizeDownloads(m2).type, 'clear');
});

test('cancel can be requested while queued, and only while pending', () => {
    let m = run({}, { type: 'start', id: 'a' }, { type: 'cancel-requested', id: 'a' });
    assert.equal(m.a.cancelRequested, true);
    m = run({}, { type: 'start', id: 'a' }, { type: 'done', id: 'a' }, { type: 'cancel-requested', id: 'a' });
    assert.equal(m.a.cancelRequested, undefined);
});

test('the backend-resolved name and size replace the requested label', () => {
    const m = run({}, { type: 'start', id: 'phi-4-mini' }, { type: 'started', id: 'phi-4-mini', label: 'phi-4-mini-instruct-openvino-npu:4', sizeBytes: 2_200_000_000 });
    assert.equal(m['phi-4-mini'].label, 'phi-4-mini-instruct-openvino-npu:4');
    assert.equal(m['phi-4-mini'].sizeBytes, 2_200_000_000);
});

test('progress is clamped to 0..100 and NaN is 0', () => {
    const m = run({}, { type: 'start', id: 'a' }, { type: 'progress', id: 'a', percent: 140 });
    assert.equal(m.a.percent, 100);
    assert.equal(run(m, { type: 'progress', id: 'a', percent: NaN }).a.percent, 0);
});

test('summary names the lead download and counts the rest', () => {
    const m = run({}, { type: 'start', id: 'a', label: 'A' }, { type: 'progress', id: 'a', percent: 30 }, { type: 'start', id: 'b', label: 'B' });
    const s = summarizeDownloads(m) as { message: string; percent: number };
    assert.equal(s.message, 'Downloading A (+1 more)');
    assert.equal(s.percent, 30);
    assert.equal(summarizeDownloads({}).type, 'clear');
});

test('cancel-error detection', () => {
    for (const s of ['Download cancelled', 'operation was canceled', 'CANCELLED']) assert.equal(isCancelError(s), true, s);
    assert.equal(isCancelError('network unreachable'), false);
});

import { downloadToOperationEntry } from './downloads.ts';

test('row adapter: queued is indeterminate and cancellable; errors are retryable', () => {
    const m = run({}, { type: 'start', id: 'a', label: 'A' }, { type: 'start', id: 'b', label: 'B' }, { type: 'error', id: 'b', message: 'boom' });
    const q = downloadToOperationEntry(m.a);
    assert.equal(q.percent, undefined);
    assert.equal(q.cancellable, true);
    assert.match(q.message, /^Queued: A/);
    const e = downloadToOperationEntry(m.b);
    assert.equal(e.state, 'error');
    assert.equal(e.retryable, true);
    assert.equal(e.cancellable, false);
});

test('row adapter: cancel requested says cancelling', () => {
    const m = run({}, { type: 'start', id: 'a', label: 'A' }, { type: 'cancel-requested', id: 'a' });
    assert.match(downloadToOperationEntry(m.a).message, /cancelling/);
});
