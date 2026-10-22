import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildChatHistory } from './history.ts';

const NOTICE = "⚠️ This model isn't compatible with your Foundry Local runtime. Switching to a supported model…";

test('drops a persisted switch notice so the next model never sees it', () => {
    const h = buildChatHistory([
        { role: 'user', content: 'Why is the sky blue?' },
        { role: 'assistant', content: 'Rayleigh scattering.' },
        { role: 'user', content: 'Why is grass green?' },
        { role: 'assistant', content: NOTICE },
        { role: 'user', content: 'Why is the sky bluw?' },
    ]);
    assert.equal(h.length, 4);
    assert.ok(h.every((m) => !m.content.includes('Switching to a supported model')));
});

test('strips a notice appended after a partial reply (Bob: reply cut by the warning)', () => {
    const h = buildChatHistory([
        { role: 'assistant', content: `The first error was that I used\n${NOTICE}` },
    ]);
    assert.equal(h[0].content, 'The first error was that I used');
});

test('drops the legacy 5xx wording too', () => {
    const h = buildChatHistory([
        { role: 'assistant', content: '⚠️ The current model ran into a problem with your Foundry Local runtime. Switching to a supported model…' },
    ]);
    assert.equal(h.length, 0);
});

test('keeps normal turns, and a user quoting the notice', () => {
    const h = buildChatHistory([
        { role: 'user', content: `What does "${NOTICE}" mean?` },
        { role: 'assistant', content: 'It means the model failed to load.' },
    ]);
    assert.equal(h.length, 2);
    assert.equal(h[0].content.includes('isn\'t compatible'), true);
});

test('carries the system prompt text through', () => {
    const h = buildChatHistory([{ role: 'assistant', content: 'ok', systemPromptText: 'sp' }]);
    assert.equal(h[0].system_prompt, 'sp');
});
