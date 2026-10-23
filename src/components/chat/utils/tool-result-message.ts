/**
 * Tool results come back into a saved chat either as `role: "tool"` messages (native calls) or as
 * a user message wrapped in `<tool_response>` (text-format models). Reloaded, they are shown as a
 * collapsed "Tool result" row instead of a raw bubble the user never typed.
 */
export interface ToolResultCandidate {
    role: string;
    content: string;
}

const WRAPPER_OPEN = /^\s*<(tool_response|tool_result|function_response)(\s[^>]*)?>/i;

export function isToolResultMessage(m: ToolResultCandidate): boolean {
    if (m.role === 'tool') return true;
    return m.role === 'user' && WRAPPER_OPEN.test(m.content ?? '');
}

export interface ToolResultView {
    body: string;
    isError: boolean;
}

/** The text inside the wrapper(s), and whether the app marked the result as an error. */
export function toolResultView(m: ToolResultCandidate): ToolResultView {
    const content = m.content ?? '';
    const open = WRAPPER_OPEN.exec(content);
    const isError = open ? /\berror\s*=\s*"true"/i.test(open[2] ?? '') : false;
    const body = content
        .replace(/<\/?(?:tool_response|tool_result|function_response)(?:\s[^>]*)?>/gi, '')
        .trim();
    return { body, isError };
}
