#!/usr/bin/env node
// Soak-test verdict: after a long same-chat run with network flaps and an app kill, is the chat store whole?
// Inputs are what the app already writes: `--dump-chats <file>` (every saved chat, read back through the
// app's own store) and `--reply-file` (one JSON line per finished turn). Pure logic is exported for the
// node test; run as `node scripts/ci/check-chat-store-integrity.mjs <dump.json> <replies.jsonl> [--expect-prompts N] [--allow-failed-turns-with network_dropped]`.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const WRAPPER = /<\/?(?:tool_response|tool_result|function_response)(?:\s[^>]*)?>/i;
const WRAPPER_OPEN = /^\s*<(tool_response|tool_result|function_response)(\s[^>]*)?>/i;
const TOOL_CALL_TEXT = /<tool_call>[\s\S]*<\/tool_call>/;

export const isToolResult = (m) => m.role === 'tool' || (m.role === 'user' && WRAPPER_OPEN.test(m.content ?? ''));
const text = (m) => (typeof m.content === 'string' ? m.content : '');

/**
 * @param chats   the `--dump-chats` array: [{ id, title, messages_valid, messages }]
 * @param replies the reply-file records: [{ prompt_index, question, reply, outcome, detail, chat_id }]
 * @param opts    { expectPrompts, failureKindsAllowed: ['network_dropped'] }
 * @returns       a list of problems; empty means the store is whole
 */
export function problemsFor(chats, replies, opts = {}) {
  const problems = [];
  const allowed = opts.failureKindsAllowed ?? [];

  for (const c of chats) {
    if (!c.messages_valid) problems.push(`chat ${c.id} ("${c.title}") cannot be read back: its saved messages are not a JSON array`);
  }

  const readable = chats.filter((c) => c.messages_valid);
  const everyMessage = readable.flatMap((c) => c.messages.map((m) => ({ chat: c.id, ...m })));

  // The reply the user sees must never carry the tool wrapper the model was shown.
  for (const r of replies) {
    if (WRAPPER.test(r.reply ?? '')) problems.push(`turn ${r.prompt_index} reply shows a raw tool wrapper: ${(r.reply ?? '').slice(0, 80)}`);
  }
  for (const m of everyMessage.filter((m) => m.role === 'assistant')) {
    if (WRAPPER.test(text(m))) problems.push(`chat ${m.chat} holds an assistant message with a raw tool wrapper: ${text(m).slice(0, 80)}`);
  }

  // Every prompt that was answered, or failed, is in the store, and an answered one is followed by its reply.
  const userTexts = everyMessage.filter((m) => m.role === 'user' && !isToolResult(m)).map(text);
  for (const r of replies) {
    if (!userTexts.some((t) => t.includes(r.question ?? '\u0000'))) {
      problems.push(`turn ${r.prompt_index}: the question is missing from the saved chats ("${(r.question ?? '').slice(0, 60)}")`);
    }
    if (r.outcome === 'ok' && !everyMessage.some((m) => m.role === 'assistant' && text(m).includes((r.reply ?? '').trim().slice(0, 200)))) {
      problems.push(`turn ${r.prompt_index}: the reply that was shown is missing from the saved chats`);
    }
  }

  // A turn may fail only for a reason the test caused on purpose.
  for (const r of replies.filter((r) => r.outcome !== 'ok')) {
    const detail = r.detail ?? '';
    if (!allowed.some((k) => detail.startsWith(k))) problems.push(`turn ${r.prompt_index} ended "${r.outcome}" for a reason the test did not cause: ${detail}`);
  }

  // Tool calls and their results stay paired, except for one call at the very end of a chat that the
  // app kill cut off (the checkpoint written before the tool ran is allowed to be the last message).
  for (const c of readable) {
    const ms = c.messages;
    ms.forEach((m, i) => {
      const isLast = i === ms.length - 1;
      if (m.role === 'assistant' && Array.isArray(m.tool_calls)) {
        for (const call of m.tool_calls) {
          const answered = ms.slice(i + 1).some((x) => x.role === 'tool' && x.tool_call_id === call.id);
          if (!answered && !isLast) problems.push(`chat ${c.id}: native tool call ${call.id} has no result`);
        }
      } else if (m.role === 'assistant' && TOOL_CALL_TEXT.test(text(m))) {
        const next = ms[i + 1];
        if (!isLast && !(next && isToolResult(next))) problems.push(`chat ${c.id}: a tool call at message ${i} has no tool result after it`);
      }
    });
  }

  if (opts.expectPrompts !== undefined) {
    // Prompts sent = answered or failed ones, plus the one the kill may have cut off.
    if (replies.length < opts.expectPrompts - 1) problems.push(`only ${replies.length} of ${opts.expectPrompts} prompts have a record: turns were lost`);
  }
  return problems;
}

export function readJsonLines(textBody) {
  return textBody.split(/\r?\n/).filter((l) => l.trim()).map((l) => JSON.parse(l));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [dumpPath, repliesPath, ...rest] = process.argv.slice(2);
  if (!dumpPath || !repliesPath) {
    console.error('usage: check-chat-store-integrity.mjs <dump.json> <replies.jsonl> [--expect-prompts N] [--allow-failed-turns-with network_dropped]');
    process.exit(2);
  }
  const opts = { failureKindsAllowed: [] };
  for (let i = 0; i < rest.length; i++) {
    if (rest[i] === '--expect-prompts') opts.expectPrompts = Number(rest[++i]);
    else if (rest[i] === '--allow-failed-turns-with') opts.failureKindsAllowed.push(rest[++i]);
  }
  const problems = problemsFor(JSON.parse(readFileSync(dumpPath, 'utf8')), readJsonLines(readFileSync(repliesPath, 'utf8')), opts);
  if (problems.length) {
    console.error(problems.map((p) => `- ${p}`).join('\n'));
    process.exit(1);
  }
  console.log('chat store is whole: every prompt is saved, no raw tool wrapper, tool calls paired');
}
