import assert from 'node:assert/strict';
import { test } from 'node:test';
import { formatVersionLine, formatVersionReport, type VersionInfo } from './version-info.ts';

const base: VersionInfo = {
    appVersion: '0.1.0',
    releaseLabel: 'v0.1.0-rc12',
    gitSha: 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678',
    gitShaShort: 'a1b2c3d',
    dirty: false,
    buildTime: 1760000000,
};

// R12 (Bob): the title bar did not say which version or candidate was installed.
test('the line shows both the release label and the commit', () => {
    assert.equal(formatVersionLine(base), 'v0.1.0-rc12 (a1b2c3d)');
    assert.equal(formatVersionLine({ ...base, dirty: true }), 'v0.1.0-rc12 (a1b2c3d+dirty)');
});

test('a missing answer is stated, never blank', () => {
    assert.equal(formatVersionLine(null), 'Version unavailable');
    assert.match(formatVersionReport(undefined), /unavailable/);
});

test('the report carries the full commit and the build time', () => {
    const r = formatVersionReport(base);
    assert.match(r, /v0\.1\.0-rc12/);
    assert.match(r, /a1b2c3d4e5f60718293a4b5c6d7e8f9012345678/);
    assert.match(r, /2025-10-09T08:53:20.000Z/);
});
