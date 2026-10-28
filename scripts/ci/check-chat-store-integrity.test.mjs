import assert from 'node:assert/strict';
import { test } from 'node:test';
import { problemsFor } from './check-chat-store-integrity.mjs';

const chat = (messages, extra = {}) => ({ id: 'c1', title: 'T', messages_valid: true, messages, ...extra });
const u = (content) => ({ role: 'user', content });
const a = (content) => ({ role: 'assistant', content });
const rec = (i, question, reply, extra = {}) => ({ prompt_index: i, question, reply, outcome: 'ok', detail: '', chat_id: 'c1', ...extra });

const whole = chat([
  u('q1'), a('<tool_call>{"name":"x"}</tool_call>'), u('<tool_response>\n3\n</tool_response>'), a('three rows'),
  u('q2'), a('two'),
]);
const goodReplies = [rec(0, 'q1', 'three rows'), rec(1, 'q2', 'two')];

test('a whole store passes', () => {
  assert.deepEqual(problemsFor([whole], goodReplies, { expectPrompts: 2 }), []);
});

test('a prompt that vanished from the store is reported', () => {
  const lost = chat([u('q1'), a('three rows')]);
  const p = problemsFor([lost], goodReplies);
  assert.ok(p.some((x) => x.includes('turn 1') && x.includes('question is missing')), p.join('\n'));
  assert.ok(p.some((x) => x.includes('turn 1') && x.includes('reply that was shown is missing')), p.join('\n'));
});

test('a raw tool wrapper in the reply shown or in a saved assistant message is reported', () => {
  const p1 = problemsFor([whole], [rec(0, 'q1', '<tool_response>\n3\n</tool_response>'), goodReplies[1]]);
  assert.ok(p1.some((x) => x.includes('raw tool wrapper')));
  const dirty = chat([u('q1'), a('see <tool_response>3</tool_response>')]);
  const p2 = problemsFor([dirty], [rec(0, 'q1', 'see')]);
  assert.ok(p2.some((x) => x.includes('assistant message with a raw tool wrapper')));
});

test('a chat whose saved messages cannot be read is reported', () => {
  const p = problemsFor([chat([], { messages_valid: false })], []);
  assert.ok(p[0].includes('cannot be read back'));
});

test('a tool call with no result is reported, except as the last message (the app kill)', () => {
  const orphan = chat([u('q1'), a('<tool_call>{"name":"x"}</tool_call>'), u('q2'), a('two')]);
  assert.ok(problemsFor([orphan], [rec(0, 'q1', 'x'), rec(1, 'q2', 'two')]).some((x) => x.includes('has no tool result')));
  const cutByKill = chat([u('q1'), a('<tool_call>{"name":"x"}</tool_call>')]);
  assert.deepEqual(problemsFor([cutByKill], [], {}), []);
  const native = chat([u('q'), { role: 'assistant', content: '', tool_calls: [{ id: 'k1' }] }, u('q2'), a('r')]);
  assert.ok(problemsFor([native], []).some((x) => x.includes('native tool call k1 has no result')));
  const nativeOk = chat([u('q'), { role: 'assistant', content: '', tool_calls: [{ id: 'k1' }] }, { role: 'tool', tool_call_id: 'k1', content: 'ok' }, a('r')]);
  assert.deepEqual(problemsFor([nativeOk], []), []);
});

test('a failed turn is allowed only for a failure the test caused', () => {
  const failed = rec(1, 'q2', '', { outcome: 'error', detail: 'network_dropped: the network connection was lost' });
  const store = chat([u('q1'), a('three rows'), u('q2'), a('part')]);
  const replies = [rec(0, 'q1', 'three rows'), failed];
  assert.deepEqual(problemsFor([store], replies, { failureKindsAllowed: ['network_dropped'] }), []);
  assert.ok(problemsFor([store], replies, { failureKindsAllowed: [] }).some((x) => x.includes('reason the test did not cause')));
  const other = { ...failed, detail: 'engine_error: boom' };
  assert.ok(problemsFor([store], [replies[0], other], { failureKindsAllowed: ['network_dropped'] }).length > 0);
});

test('too few turn records means turns were lost', () => {
  const p = problemsFor([whole], [goodReplies[0]], { expectPrompts: 20 });
  assert.ok(p.some((x) => x.includes('of 20 prompts')));
});
