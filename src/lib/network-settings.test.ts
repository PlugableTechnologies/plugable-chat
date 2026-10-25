import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
    describeHostRow,
    formToNetworkSettings,
    loadNetworkForm,
    networkFormIsDirty,
    networkSettingsToForm,
    saveNetworkForm,
    summarizeHostRows,
    testNetworkForm,
    validateNetworkForm,
    type InvokeFn,
} from './network-settings.ts';

test('saved settings fill the form, and missing or null fields become empty strings', () => {
    assert.deepEqual(networkSettingsToForm({ https_proxy: 'http://p:8080', hf_endpoint: null }), {
        httpsProxy: 'http://p:8080',
        hfEndpoint: '',
        offlineModelFolder: '',
    });
    assert.deepEqual(networkSettingsToForm(null), { httpsProxy: '', hfEndpoint: '', offlineModelFolder: '' });
});

test('the form maps to backend keys: blanks are null, whitespace is trimmed, a mirror loses its trailing slash', () => {
    assert.deepEqual(
        formToNetworkSettings({ httpsProxy: '  http://proxy.corp:3128 ', hfEndpoint: 'https://hf-mirror.example.com//', offlineModelFolder: '   ' }),
        { https_proxy: 'http://proxy.corp:3128', hf_endpoint: 'https://hf-mirror.example.com', offline_model_folder: null },
    );
    assert.deepEqual(formToNetworkSettings({ httpsProxy: '', hfEndpoint: '', offlineModelFolder: '' }), {
        https_proxy: null,
        hf_endpoint: null,
        offline_model_folder: null,
    });
});

test('dirty means the saved meaning changed, not that whitespace did', () => {
    const saved = networkSettingsToForm({ https_proxy: 'http://p:1', hf_endpoint: 'https://m', offline_model_folder: null });
    assert.equal(networkFormIsDirty(saved, { ...saved, hfEndpoint: 'https://m/ ' }), false);
    assert.equal(networkFormIsDirty(saved, { ...saved, httpsProxy: '' }), true);
    assert.equal(networkFormIsDirty(saved, { ...saved, offlineModelFolder: 'C:\\models' }), true);
});

test('addresses that cannot work are explained before saving; empty fields are fine', () => {
    assert.deepEqual(validateNetworkForm({ httpsProxy: '', hfEndpoint: '', offlineModelFolder: '' }), {});
    assert.deepEqual(validateNetworkForm({ httpsProxy: 'socks5://127.0.0.1:1080', hfEndpoint: 'http://mirror.local', offlineModelFolder: '' }), {});
    const bad = validateNetworkForm({ httpsProxy: 'proxy.corp:8080', hfEndpoint: 'ftp://mirror', offlineModelFolder: '' });
    assert.match(bad.httpsProxy ?? '', /proxy address/);
    assert.match(bad.hfEndpoint ?? '', /http:\/\/ or https:\/\//);
});

test('host rows read as one line each and the summary counts failures', () => {
    const rows = [
        { host: 'huggingface.co', ok: false, status: null, millis: null, reason: 'connection reset (a firewall, antivirus or proxy may be blocking it): x' },
        { host: 'github.com', ok: true, status: 200, millis: 41, reason: null },
    ];
    assert.equal(describeHostRow(rows[1]), 'github.com: reachable (HTTP 200, 41 ms)');
    assert.match(describeHostRow(rows[0]), /^huggingface\.co: not reachable - connection reset/);
    const s = summarizeHostRows(rows);
    assert.deepEqual({ allOk: s.allOk, failed: s.failed, lines: s.lines.length }, { allOk: false, failed: 1, lines: 2 });
    assert.equal(summarizeHostRows([]).allOk, false, 'no results is not a pass');
    assert.equal(summarizeHostRows([rows[1]]).allOk, true);
});

test('load, save and test talk to the backend commands with the settings keys', async () => {
    const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
    const invoke: InvokeFn = async <T,>(cmd: string, args?: Record<string, unknown>) => {
        calls.push({ cmd, args });
        if (cmd === 'get_network_settings') return { https_proxy: 'http://p:1', hf_endpoint: null, offline_model_folder: null } as T;
        if (cmd === 'save_network_settings') return (args as { network: unknown }).network as T;
        return [{ host: 'github.com', ok: true, status: 200, millis: 5, reason: null }] as T;
    };
    const form = await loadNetworkForm(invoke);
    assert.equal(form.httpsProxy, 'http://p:1');
    const saved = await saveNetworkForm({ ...form, hfEndpoint: 'https://m/' }, invoke);
    assert.equal(saved.hfEndpoint, 'https://m');
    const rows = await testNetworkForm(saved, invoke);
    assert.equal(rows.length, 1);
    assert.deepEqual(calls.map((c) => c.cmd), ['get_network_settings', 'save_network_settings', 'test_network_connection']);
    assert.deepEqual(calls[2].args, { network: { https_proxy: 'http://p:1', hf_endpoint: 'https://m', offline_model_folder: null } });
});

test('a backend refusal reaches the screen as a readable message', async () => {
    const invoke: InvokeFn = async () => {
        throw "The model folder 'C:\\nope' does not exist or is not a folder.";
    };
    await assert.rejects(saveNetworkForm({ httpsProxy: '', hfEndpoint: '', offlineModelFolder: 'C:\\nope' }, invoke), /does not exist/);
});
