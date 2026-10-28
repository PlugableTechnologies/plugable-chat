// The soak failure: a turn cut off by a network drop while a tool ran left the screen holding the raw
// `<tool_call>` text in the assistant bubble and the result (even "Interrupted") only in that bubble's
// toolCalls list. The next turn sends the screen's history, so the saved chat got a call with no result
// after it. This feeds what the screen sends through the same rules the soak verdict uses.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { problemsFor } from './check-chat-store-integrity.mjs';
import { buildChatHistory } from '../../src/components/chat/utils/history.ts';

const INTERRUPTED = 'Interrupted: the network connection was lost before this tool finished. Whether it completed is not known.';

// What the screen holds after the drop: every streamed token in one assistant message, results in toolCalls.
const screenAfterDrop = [
  { role: 'user', content: 'look up the slow thing' },
  {
    role: 'assistant',
    content: 'Let me check.\n<tool_call>{"name":"slow_lookup","arguments":{}}</tool_call>',
    toolCalls: [{ server: 'box', tool: 'slow_lookup', result: INTERRUPTED, isError: true }],
  },
];

const savedChatAfterNextTurn = (history, nextQuestion, answer) => ({
  id: 'c1',
  title: 'T',
  messages_valid: true,
  messages: [...history.map(({ role, content }) => ({ role, content })), { role: 'user', content: nextQuestion }, { role: 'assistant', content: answer }],
});

test('the history sent after a drop mid-tool-call is a whole chat: the call is followed by an Interrupted result', () => {
  const history = buildChatHistory(screenAfterDrop);
  const chat = savedChatAfterNextTurn(history, 'try again please', 'It is back.');
  const replies = [{ prompt_index: 1, question: 'try again please', reply: 'It is back.', outcome: 'ok', detail: '', chat_id: 'c1' }];
  assert.deepEqual(problemsFor([chat], replies, { failureKindsAllowed: ['network_dropped'] }), []);
  assert.ok(history.some((m) => m.role === 'user' && m.content.includes('<tool_response error="true">') && m.content.includes('Interrupted')), JSON.stringify(history));
});

test('a call the stream was cut inside of (no record at all) still gets an Interrupted result', () => {
  const history = buildChatHistory([{ role: 'user', content: 'q' }, { role: 'assistant', content: '<tool_call>{"name":"x"}</tool_call>' }]);
  const chat = savedChatAfterNextTurn(history, 'again', 'ok');
  assert.deepEqual(problemsFor([chat], [], {}), []);
});
