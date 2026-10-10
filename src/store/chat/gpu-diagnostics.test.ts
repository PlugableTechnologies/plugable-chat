import assert from 'node:assert/strict';
import { test } from 'node:test';
import { describeGpuDiagnostics, effectiveDeviceFilter } from './gpu-diagnostics.ts';

test('falls back to CPU only when GPU empty and CPU not', () => {
    assert.deepEqual(effectiveDeviceFilter('GPU', { GPU: 0, CPU: 3 }), { filter: 'CPU', fellBack: true });
    assert.deepEqual(effectiveDeviceFilter('GPU', { GPU: 2, CPU: 3 }), { filter: 'GPU', fellBack: false });
    assert.deepEqual(effectiveDeviceFilter('GPU', {}), { filter: 'GPU', fellBack: false });
    assert.deepEqual(effectiveDeviceFilter('CPU', { CPU: 0 }), { filter: 'CPU', fellBack: false });
});

test('describes failures and preflight facts', () => {
    const lines = describeGpuDiagnostics({
        registered: [],
        failed: [{ ep: 'CUDAExecutionProvider', kind: 'driver', message: 'driver 400' }],
        driver_version: '400.1',
        vcpp_installed: false,
    });
    assert.match(lines[0], /CUDAExecutionProvider: The graphics driver/);
    assert.ok(lines.some((l) => /Visual C\+\+/.test(l)));
    assert.ok(lines.some((l) => /400\.1/.test(l)));
    assert.deepEqual(describeGpuDiagnostics(null), []);
});
