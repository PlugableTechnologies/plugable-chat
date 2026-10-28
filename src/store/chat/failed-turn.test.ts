import assert from 'node:assert/strict';
import { test } from 'node:test';
import { failedTurnNotice, lastUserQuestion } from './failed-turn.ts';
import { parseSavedMessages } from './loaded-messages.ts';
import { isToolResultMessage, toolResultView } from '../../components/chat/utils/tool-result-message.ts';

// The transcript the backend saves for a turn that a network drop cut off: one finished tool call, one
// interrupted one, and the words written so far. Identical to the literal in
// src-tauri/src/chat_persistence.rs (`a_turn_cut_off_by_a_network_drop_saves_...`); change both together.
const SAVED_AFTER_DROP =
    '[{"role":"user","content":"how many rows, and what about the lookup?"},{"role":"assistant","content":"<tool_call>{\\"name\\":\\"count_rows\\"}</tool_call>"},{"role":"user","content":"<tool_response>\\n{\\"rows\\":3}\\n</tool_response>"},{"role":"assistant","content":"<tool_call>{\\"name\\":\\"slow_lookup\\"}</tool_call>"},{"role":"user","content":"<tool_response error=\\"true\\">\\nInterrupted: the network connection was lost before this tool finished. Whether it completed is not known.\\n</tool_response>"},{"role":"assistant","content":"There are 3 rows. For the lookup I was about to"}]';

test('a chat saved after a network drop reloads with every message and no raw tool wrapper in a visible bubble', () => {
    const { messages, skipped } = parseSavedMessages(SAVED_AFTER_DROP);
    assert.equal(skipped, 0);
    assert.equal(messages.length, 6, 'no message was lost');
    const asView = messages.map((m) => ({ role: String(m.role), content: String(m.content) }));
    const visibleBubbles = asView.filter((m) => !isToolResultMessage(m));
    for (const m of visibleBubbles.filter((m) => m.role === 'user')) {
        assert.ok(!/<tool_(response|result)/.test(m.content), `raw wrapper shown as a user bubble: ${m.content}`);
    }
    const results = asView.filter(isToolResultMessage).map(toolResultView);
    assert.equal(results.length, 2);
    assert.equal(results[0].isError, false);
    assert.equal(results[1].isError, true);
    assert.ok(results[1].body.startsWith('Interrupted:'));
    assert.ok(!results.some((r) => r.body.includes('<tool_response')));
    assert.equal(asView[5].content, 'There are 3 rows. For the lookup I was about to');
});

test('Retry asks the last real question, not a tool result', () => {
    const { messages } = parseSavedMessages(SAVED_AFTER_DROP);
    const asView = messages.map((m) => ({ role: String(m.role), content: String(m.content) }));
    assert.equal(lastUserQuestion(asView), 'how many rows, and what about the lookup?');
    assert.equal(lastUserQuestion([]), null);
});

test('a network drop offers Retry only; an engine error offers Retry and Repair; a plain error stays a status line', () => {
    const drop = failedTurnNotice({ error: 'Connection lost.', kind: 'network_dropped', retryable: true, offerRepair: false })!;
    assert.deepEqual(drop.actions, ['retry']);
    const engine = failedTurnNotice({ error: 'Engine error.', kind: 'engine_error', retryable: true, offerRepair: true })!;
    assert.deepEqual(engine.actions, ['retry', 'repair']);
    assert.equal(failedTurnNotice({ error: 'Failed to send to model gateway' }), null);
});
