// Run with: npm run test:unit  (node's built-in runner; excluded from tsc because there are no node typings)
import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
    cancellingEpRegistrationStatus,
    isEpRegistrationStatus,
    nextOperationStatusForEpRegistration,
} from './ep-registration-status.ts';

const downloadingEvent = (ep: string, percent: number) => ({ phase: 'downloading', ep, percent });

test('downloading event shows provider, percent and a cancel action', () => {
    const status = nextOperationStatusForEpRegistration(null, downloadingEvent('CUDAExecutionProvider', 41.6), 1000)!;
    assert.match(status.message, /^Preparing GPU acceleration: CUDAExecutionProvider 42%/);
    assert.equal(status.progress, 42);
    assert.equal(status.startTime, 1000);
    assert.ok(isEpRegistrationStatus(status));
});

test('later progress keeps the original start time so the elapsed timer keeps running', () => {
    const first = nextOperationStatusForEpRegistration(null, downloadingEvent('A', 1), 1000);
    const second = nextOperationStatusForEpRegistration(first, downloadingEvent('B', 2), 9000)!;
    assert.equal(second.startTime, 1000);
    assert.match(second.message, /: B 2%/);
});

test('percent is clamped to 0-100', () => {
    assert.equal(nextOperationStatusForEpRegistration(null, downloadingEvent('A', 250), 0)!.progress, 100);
    assert.equal(nextOperationStatusForEpRegistration(null, downloadingEvent('A', -3), 0)!.progress, 0);
});

test('done clears our bar, or shows the outcome message briefly', () => {
    const active = nextOperationStatusForEpRegistration(null, downloadingEvent('A', 50), 1000);
    assert.equal(nextOperationStatusForEpRegistration(active, { phase: 'done', message: '' }, 2000), null);
    const cancelled = nextOperationStatusForEpRegistration(
        active,
        { phase: 'done', message: 'GPU acceleration setup cancelled; using CPU models.' },
        2000,
    )!;
    assert.equal(cancelled.type, 'none');
    assert.equal(cancelled.cancelAction, undefined);
    assert.match(cancelled.message, /cancelled/);
});

test('done after clicking Cancel (cancelling status) still clears the bar', () => {
    const active = nextOperationStatusForEpRegistration(null, downloadingEvent('A', 50), 1000)!;
    const cancelling = cancellingEpRegistrationStatus(active);
    assert.ok(cancelling.cancelRequested, 'button is disabled while cancelling');
    const tick = nextOperationStatusForEpRegistration(cancelling, downloadingEvent('A', 60), 1500);
    assert.equal(tick, cancelling, 'late progress ticks keep the cancelling message');
    const after = nextOperationStatusForEpRegistration(cancelling, { phase: 'done', message: 'cancelled' }, 2000);
    assert.equal(after?.message, 'cancelled');
});

test('failed provider outcome is informational, not an error', () => {
    const active = nextOperationStatusForEpRegistration(null, downloadingEvent('A', 10), 1000);
    const after = nextOperationStatusForEpRegistration(
        active,
        { phase: 'done', message: 'GPU acceleration ready: none; not available on this machine: CUDAExecutionProvider.' },
        2000,
    )!;
    assert.equal(after.type, 'none');
});

test('events never clobber an unrelated operation', () => {
    const loading = { type: 'loading' as const, message: 'Loading model', startTime: 5 };
    assert.equal(nextOperationStatusForEpRegistration(loading, downloadingEvent('A', 5), 10), loading);
    assert.equal(nextOperationStatusForEpRegistration(loading, { phase: 'done', message: 'x' }, 10), loading);
});
