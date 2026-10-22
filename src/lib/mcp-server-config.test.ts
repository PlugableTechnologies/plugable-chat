import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
    MAX_REQUEST_TIMEOUT_SECS,
    loadsToolsImmediately,
    normalizeMcpServer,
    normalizeTransport,
    parseTimeoutInput,
    timeoutInputValue,
    urlProblem,
} from './mcp-server-config.ts';

const pythonName = (name: string) => name.replace(/[^a-z0-9]+/gi, '_').toLowerCase();

test('a config saved with the old "sse" label is read as http, and anything unknown is stdio', () => {
    assert.deepEqual(normalizeTransport({ type: 'sse', url: 'http://box:8787/mcp' }), { type: 'http', url: 'http://box:8787/mcp' });
    assert.deepEqual(normalizeTransport({ type: 'http', url: 'http://box:8787/mcp' }), { type: 'http', url: 'http://box:8787/mcp' });
    assert.deepEqual(normalizeTransport({ type: 'stdio' }), { type: 'stdio' });
    assert.deepEqual(normalizeTransport(undefined), { type: 'stdio' });
    assert.deepEqual(normalizeTransport(null), { type: 'stdio' });
    assert.deepEqual(normalizeTransport({ type: 'carrier-pigeon' } as never), { type: 'stdio' });
});

test('the address box accepts http and https addresses and says why it refuses the rest', () => {
    assert.equal(urlProblem(''), null, 'empty is not an error yet');
    assert.equal(urlProblem('http://100.127.91.127:8787/mcp'), null);
    assert.equal(urlProblem('  https://example.test/mcp  '), null);
    assert.match(urlProblem('box:8787/mcp') ?? '', /http:\/\//);
    assert.match(urlProblem('ftp://box/mcp') ?? '', /http:\/\//);
    assert.match(urlProblem('http://') ?? '', /valid|host/);
});

test('the timeout box: empty, zero and junk mean the default; whole seconds are kept; a typo is capped at a day', () => {
    assert.equal(parseTimeoutInput(''), undefined);
    assert.equal(parseTimeoutInput('   '), undefined);
    assert.equal(parseTimeoutInput('0'), undefined);
    assert.equal(parseTimeoutInput('-5'), undefined);
    assert.equal(parseTimeoutInput('abc'), undefined);
    assert.equal(parseTimeoutInput('900'), 900);
    assert.equal(parseTimeoutInput(' 600.9 '), 600);
    assert.equal(parseTimeoutInput('99999999'), MAX_REQUEST_TIMEOUT_SECS);
    assert.equal(timeoutInputValue(undefined), '');
    assert.equal(timeoutInputValue(null), '');
    assert.equal(timeoutInputValue(0), '');
    assert.equal(timeoutInputValue(900), '900');
});

test('"load tools immediately" is on only when defer_tools is explicitly false', () => {
    assert.equal(loadsToolsImmediately({}), false);
    assert.equal(loadsToolsImmediately({ defer_tools: true }), false);
    assert.equal(loadsToolsImmediately({ defer_tools: false }), true);
});

test('a server saved with a request timeout keeps it through normalisation, and one without gains none', () => {
    const saved = { id: 'brix', name: 'brix-box', transport: { type: 'stdio' as const }, request_timeout_secs: 900, auto_approve_tools: false };
    const out = normalizeMcpServer(saved, pythonName);
    assert.equal(out.request_timeout_secs, 900);
    assert.equal(out.auto_approve_tools, false, 'other fields pass through');
    assert.equal(out.defer_tools, true, 'defer_tools defaults to true');
    assert.equal(out.python_name, 'brix_box');

    const unset = normalizeMcpServer({ id: 'x', name: 'x' }, pythonName);
    assert.equal('request_timeout_secs' in unset, false, 'no timeout key is invented');
    const zero = normalizeMcpServer({ id: 'x', name: 'x', request_timeout_secs: 0 }, pythonName);
    assert.equal('request_timeout_secs' in zero, false, 'zero means unset and is dropped, as the backend treats it');
});

test('normalising reads an old sse server as http, keeps an explicit defer_tools false, and pins the built-in test server', () => {
    const old = normalizeMcpServer({ id: 'b', name: 'b', transport: { type: 'sse', url: 'http://h/mcp' }, defer_tools: false }, pythonName);
    assert.deepEqual(old.transport, { type: 'http', url: 'http://h/mcp' });
    assert.equal(old.defer_tools, false);
    const builtIn = normalizeMcpServer({ id: 'mcp-test-server', name: 'whatever', defer_tools: true }, pythonName);
    assert.equal(builtIn.name, 'mcp_test_server');
    assert.equal(builtIn.defer_tools, false);
});
