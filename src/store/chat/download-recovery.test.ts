import test from 'node:test';
import assert from 'node:assert/strict';
import {
    DOWNLOAD_STALL_AFTER_MS,
    isDiskSpaceError,
    isDownloadStalled,
    smallerModelButtonText,
    smallerModelOffer,
} from './download-recovery.ts';
import { downloadToOperationEntry, reduceDownloads, type DownloadsMap } from './downloads.ts';

const run = (actions: Parameters<typeof reduceDownloads>[1][], t0 = 1000): DownloadsMap =>
    actions.reduce<DownloadsMap>((m, a, i) => reduceDownloads(m, a, t0 + i), {});

test('a download with no rising percentage becomes stalled after the threshold', () => {
    const map = run([
        { type: 'start', id: 'qwen3.5-4b' },
        { type: 'started', id: 'qwen3.5-4b' },
        { type: 'progress', id: 'qwen3.5-4b', percent: 40 },
        { type: 'progress', id: 'qwen3.5-4b', percent: 40 },
    ]);
    const e = map['qwen3.5-4b'];
    assert.equal(isDownloadStalled(e, 1100 + DOWNLOAD_STALL_AFTER_MS - 200), false);
    assert.equal(isDownloadStalled(e, 1100 + DOWNLOAD_STALL_AFTER_MS + 10), true);
});

test('rising progress resets the stall clock; non-active entries are never stalled', () => {
    let map = run([{ type: 'start', id: 'm' }, { type: 'progress', id: 'm', percent: 10 }]);
    map = reduceDownloads(map, { type: 'progress', id: 'm', percent: 11 }, 500_000);
    assert.equal(isDownloadStalled(map.m, 500_000 + DOWNLOAD_STALL_AFTER_MS - 1), false);
    map = reduceDownloads(map, { type: 'error', id: 'm', message: 'boom' }, 600_000);
    assert.equal(isDownloadStalled(map.m, 600_000 + DOWNLOAD_STALL_AFTER_MS * 2), false);
    assert.equal(isDownloadStalled(undefined, 1), false);
});

test('a stalled download says so in its status row', () => {
    const map = run([{ type: 'start', id: 'm', label: 'Qwen' }, { type: 'progress', id: 'm', percent: 5 }]);
    assert.match(downloadToOperationEntry(map.m, true).message, /no progress/);
    assert.equal(downloadToOperationEntry(map.m, true).stalled, true);
    assert.doesNotMatch(downloadToOperationEntry(map.m).message, /no progress/);
});

test('disk space errors are recognised', () => {
    assert.equal(isDiskSpaceError('Not enough disk space for qwen3.5-4b: it needs about 5.8 GB free'), true);
    assert.equal(isDiskSpaceError('connection reset'), false);
    assert.equal(isDiskSpaceError(undefined), false);
});

test('the smaller model is offered for the big default, not for the fallback itself', () => {
    assert.deepEqual(smallerModelOffer('qwen3.5-4b', []), { kind: 'download', model: 'phi-4-mini-instruct', label: 'phi-4-mini-instruct' });
    assert.equal(smallerModelOffer('Phi-4-mini-instruct-cuda-gpu:5', []), null);
});

test('an already downloaded fallback is switched to, not downloaded again', () => {
    const cached = [{ model_id: 'Phi-4-mini-instruct-cuda-gpu:5', alias: 'phi-4-mini-instruct' }];
    const offer = smallerModelOffer('qwen3.5-4b', cached);
    assert.deepEqual(offer, { kind: 'switch', modelId: 'Phi-4-mini-instruct-cuda-gpu:5', label: 'phi-4-mini-instruct' });
    assert.equal(smallerModelButtonText(offer!), 'Use phi-4-mini-instruct');
    assert.equal(smallerModelButtonText({ kind: 'download', model: 'x', label: 'x' }), 'Use smaller model');
});
