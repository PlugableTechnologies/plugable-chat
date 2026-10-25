import test from 'node:test';
import assert from 'node:assert/strict';
import { KEYWORD_ONLY_LABEL, searchModeLabel, tableSearchLabel } from './search-mode.ts';

test('keyword mode gets the label, semantic mode gets none', () => {
    assert.equal(searchModeLabel({ mode: 'keyword', label: KEYWORD_ONLY_LABEL }), KEYWORD_ONLY_LABEL);
    assert.equal(searchModeLabel({ mode: 'keyword', label: null }), KEYWORD_ONLY_LABEL);
    assert.equal(searchModeLabel({ mode: 'semantic', label: null }), null);
    assert.equal(searchModeLabel(null), null);
});

test('label text is the agreed wording', () => {
    assert.equal(KEYWORD_ONLY_LABEL, 'Keyword search only (search model unavailable)');
});

test('table search is labelled when any row is keyword-only', () => {
    assert.equal(tableSearchLabel([{ keyword_only: false }, { keyword_only: true }]), KEYWORD_ONLY_LABEL);
    assert.equal(tableSearchLabel([{ keyword_only: false }, {}]), null);
    assert.equal(tableSearchLabel([]), null);
});
