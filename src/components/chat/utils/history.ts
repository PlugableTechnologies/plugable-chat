/**
 * Builds the history sent to the model. Anything the app itself said (switch/fallback
 * notices from older builds that were persisted as assistant turns) is dropped: a model
 * given "This model isn't compatible… Switching…" reasons about it as if the user said it.
 */
export interface HistoryMessage {
    role: string;
    content: string;
    systemPromptText?: string;
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

export function buildChatHistory(messages: HistoryMessage[]) {
    return messages
        .filter((m) => !isAppNotice(m))
        .map((m) => ({
            role: m.role,
            content: m.role === 'assistant' ? stripTrailingNotice(m.content) : m.content,
            system_prompt: m.systemPromptText,
        }));
}
