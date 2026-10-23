/**
 * Turns the saved transcript JSON back into chat messages. A chat the app died while saving can
 * hold a damaged entry; one bad entry must not make the whole chat unreadable, so it is skipped.
 */
export interface LoadedMessages {
    messages: Array<Record<string, unknown>>;
    /** How many saved entries could not be read and were left out. */
    skipped: number;
}

const ROLES = new Set(['user', 'assistant', 'tool', 'system']);

export function parseSavedMessages(json: string, now: number = Date.now()): LoadedMessages {
    let raw: unknown;
    try {
        raw = JSON.parse(json);
    } catch {
        return { messages: [], skipped: 1 };
    }
    if (!Array.isArray(raw)) return { messages: [], skipped: 1 };

    const messages: Array<Record<string, unknown>> = [];
    let skipped = 0;
    raw.forEach((m, idx) => {
        if (!m || typeof m !== 'object' || Array.isArray(m)) {
            skipped++;
            return;
        }
        const entry = m as Record<string, unknown>;
        if (typeof entry.role !== 'string' || !ROLES.has(entry.role)) {
            skipped++;
            return;
        }
        // A native tool-call turn is saved with empty or null content.
        const content = typeof entry.content === 'string' ? entry.content : '';
        messages.push({
            ...entry,
            content,
            id: typeof entry.id === 'string' && entry.id ? entry.id : `${now}-${idx}`,
            timestamp: typeof entry.timestamp === 'number' ? entry.timestamp : now,
            systemPromptText: entry.system_prompt ?? entry.systemPromptText,
        });
    });
    return { messages, skipped };
}
