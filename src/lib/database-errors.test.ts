import assert from 'node:assert/strict';
import { test } from 'node:test';
import { dropResolvedErrors, isToolboxBlockedError, refreshSucceeded } from './database-errors.ts';

const err = (name: string) => ({ source_name: name, error: 'x' });

test('a completed refresh without error counts as success; an error or partial does not', () => {
    assert.equal(refreshSucceeded({ source_name: 'a', is_complete: true, error: null }), true);
    assert.equal(refreshSucceeded({ source_name: 'a', is_complete: true, error: 'boom' }), false);
    assert.equal(refreshSucceeded({ source_name: 'a', is_complete: false, error: null }), false);
});

test('success clears that source only (Wash: schema error stayed until restart)', () => {
    const next = dropResolvedErrors({ s1: err('Chicago'), s2: err('Other') }, { source_name: 'Chicago', is_complete: true, error: null });
    assert.deepEqual(Object.keys(next), ['s2']);
});

test('a whole-refresh success (empty source name, from the auto-retry) clears every source error', () => {
    const next = dropResolvedErrors({ s1: err('Chicago'), s2: err('Other') }, { source_name: '', is_complete: true, error: null });
    assert.deepEqual(next, {});
});

test('progress events and failures leave errors alone (same object returned)', () => {
    const errors = { s1: err('Chicago') };
    assert.equal(dropResolvedErrors(errors, { source_name: 'Chicago', is_complete: false, error: null }), errors);
    assert.equal(dropResolvedErrors(errors, { source_name: 'Chicago', is_complete: true, error: 'again' }), errors);
});

test('toolbox-blocked errors are recognised', () => {
    assert.equal(isToolboxBlockedError('Toolbox not downloaded yet'), true);
    assert.equal(isToolboxBlockedError('Toolbox failed to start: x'), true);
    assert.equal(isToolboxBlockedError('syntax error'), false);
    assert.equal(isToolboxBlockedError(null), false);
});
