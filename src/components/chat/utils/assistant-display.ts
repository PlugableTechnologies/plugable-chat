/**
 * Decisions about what a saved assistant message shows when parts of it are missing — a turn the
 * app died in the middle of, or a model that only reasoned. Kept out of the component so they can be tested.
 */

/** Name of the tool in a `<tool_call>` body, for a row that has no recorded result. */
export function toolCallName(body: string): string {
    try {
        const parsed = JSON.parse(body);
        const name = parsed?.name ?? parsed?.tool_name ?? parsed?.tool;
        if (typeof name === 'string' && name.trim()) return name.split('___').pop() || name;
    } catch {
        // streamed or damaged JSON: fall through to a plain label
    }
    const m = /"(?:name|tool_name|tool)"\s*:\s*"([^"]+)"/.exec(body);
    return m ? m[1].split('___').pop() || m[1] : 'tool';
}

export interface FinishedTurnFacts {
    hasThink: boolean;
    hasMeaningfulText: boolean;
    hasToolCallParts: boolean;
    toolRecordCount: number;
    /** The turn is still being produced (streaming, or the reasoning indicator is up). */
    inProgress: boolean;
}

/** A finished turn that only reasoned: say so rather than show a bare "Reasoning" row. */
export function showNoAnswerNote(f: FinishedTurnFacts): boolean {
    return !f.inProgress && f.hasThink && !f.hasMeaningfulText && !f.hasToolCallParts && f.toolRecordCount === 0;
}

/** A tool call with no recorded result, in a turn that is over (the app stopped before the result came back). */
export function showOrphanToolCall(inProgress: boolean, hasRecord: boolean): boolean {
    return !inProgress && !hasRecord;
}
