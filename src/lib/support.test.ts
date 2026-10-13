import assert from 'node:assert/strict';
import { test } from 'node:test';
import { copySupportDetails } from './support.ts';

// R9 (Bob): the embedding error had no way to copy details for support.
test('r9: the bundle from the backend is copied, and includes what the screen showed', async () => {
    let copied = '';
    let asked: unknown;
    const r = await copySupportDetails('Failed to load CPU embedding model.', {
        invoke: async (cmd, args) => { asked = { cmd, args }; return 'BUNDLE: v0.1.0-rc12 (a1b2c3d)' as never; },
        writeClipboard: async (t) => { copied = t; },
    });
    assert.equal(r.ok, true);
    assert.equal(copied, 'BUNDLE: v0.1.0-rc12 (a1b2c3d)');
    assert.deepEqual(asked, { cmd: 'get_support_bundle', args: { extra: 'Failed to load CPU embedding model.' } });
});

test('r9: if the backend cannot build the bundle, the screen text, the version and the reason are still copied', async () => {
    let copied = '';
    const r = await copySupportDetails('Failed to load CPU embedding model.', {
        invoke: async (cmd) => {
            if (cmd === 'get_support_bundle') throw new Error('backend busy');
            return { appVersion: '0.1.0', releaseLabel: 'v0.1.0-rc12', gitSha: 'abc', gitShaShort: 'abc', dirty: false, buildTime: 0 } as never;
        },
        writeClipboard: async (t) => { copied = t; },
    });
    assert.equal(r.ok, true);
    assert.match(copied, /v0\.1\.0-rc12/);
    assert.match(copied, /Failed to load CPU embedding model\./);
    assert.match(copied, /backend busy/);
});

test('even with no backend at all something useful is copied', async () => {
    let copied = '';
    await copySupportDetails('red row text', {
        invoke: async () => { throw new Error('no backend'); },
        writeClipboard: async (t) => { copied = t; },
    });
    assert.match(copied, /red row text/);
    assert.match(copied, /no backend/);
});

test('a clipboard failure is reported, not swallowed', async () => {
    const r = await copySupportDetails('x', {
        invoke: async () => 'bundle' as never,
        writeClipboard: async () => { throw new Error('denied'); },
    });
    assert.equal(r.ok, false);
    assert.equal(r.text, 'bundle');
});
