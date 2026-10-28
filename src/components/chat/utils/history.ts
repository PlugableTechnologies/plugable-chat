/**
 * Builds the history sent to the model. Anything the app itself said (switch/fallback
 * notices from older builds that were persisted as assistant turns) is dropped: a model
 * given "This model isn't compatible… Switching…" reasons about it as if the user said it.
 */
import { stripResponseWrapperEcho } from '../../../lib/response-parser.ts';

export interface HistoryMessage {
    role: string;
    content: string;
    systemPromptText?: string;
    toolCalls?: HistoryToolRecord[];
}

// Prefixes of app-authored notices that older builds saved as assistant messages.
const APP_NOTICE_PREFIXES = [
    "⚠️ This model isn't compatible",
    '⚠️ The current model ran into a problem',
];

export function isAppNotice(m: HistoryMessage): boolean {
    if (m.role !== 'assistant') return false;
    const text = m.content.trimStart();
    return APP_NOTICE_PREFIXES.some((p) => text.startsWith(p));
}

/** Strips a notice appended mid-reply (a model's partial reply followed by the old warning). */
function stripTrailingNotice(content: string): string {
    let cut = content.length;
    for (const p of APP_NOTICE_PREFIXES) {
        const i = content.indexOf(p);
        if (i >= 0) cut = Math.min(cut, i);
    }
    return content.slice(0, cut).trimEnd();
}

// Same words as INTERRUPTED_TOOL_RESULT in src-tauri/src/turn_failure.rs.
const INTERRUPTED_TOOL_RESULT =
    'Interrupted: the network connection was lost before this tool finished. Whether it completed is not known.';

export interface HistoryToolRecord {
    result: string;
    isError: boolean;
}

const TOOL_CALL_BLOCK = /<tool_call>[\s\S]*?<\/tool_call>/g;

function visibleText(text: string): string {
    return stripResponseWrapperEcho(stripTrailingNotice(text));
}

/**
 * The screen keeps every token of a turn in one assistant message and the tool results in a side list,
 * so sent as-is the model (and the saved chat) would see calls with no answers. Each call is put back
 * as the call followed by its result message, in the wrapper the backend uses; a call that has no
 * recorded result (the turn was cut off) is answered "Interrupted", and a half-written call is dropped.
 */
function assistantTurnWithToolResults(m: HistoryMessage): { role: string; content: string; system_prompt?: string }[] {
    const out: { role: string; content: string; system_prompt?: string }[] = [];
    const records = m.toolCalls ?? [];
    let last = 0;
    let callIndex = 0;
    for (const block of m.content.matchAll(TOOL_CALL_BLOCK)) {
        const end = block.index + block[0].length;
        const before = visibleText(m.content.slice(last, block.index));
        out.push({ role: 'assistant', content: before ? `${before}\n${block[0]}` : block[0], system_prompt: m.systemPromptText });
        const record = records[callIndex++];
        const result = record ? record.result : INTERRUPTED_TOOL_RESULT;
        const isError = record ? record.isError : true;
        out.push({ role: 'user', content: `<tool_response${isError ? ' error="true"' : ''}>\n${result}\n</tool_response>`, system_prompt: undefined });
        last = end;
    }
    const open = m.content.indexOf('<tool_call>', last);
    const tail = visibleText(open >= 0 ? m.content.slice(last, open) : m.content.slice(last));
    if (tail) out.push({ role: 'assistant', content: tail, system_prompt: m.systemPromptText });
    return out;
}

export function buildChatHistory(messages: HistoryMessage[]) {
    return messages
        .filter((m) => !isAppNotice(m))
        .flatMap((m) => {
            if (m.role === 'assistant' && m.content.includes('<tool_call>')) return assistantTurnWithToolResults(m);
            return [{
                role: m.role,
                // A model that copied a tool-result wrapper into its answer would copy it again next turn.
                content: m.role === 'assistant' ? visibleText(m.content) : m.content,
                system_prompt: m.systemPromptText,
            }];
        });
}
