// A turn the backend stopped (network dropped, engine error) is told to the screen on `chat-error` with a
// `kind`. What to say and which buttons fit is decided here so it can be tested without the screen.
import { isToolResultMessage } from '../../components/chat/utils/tool-result-message.ts';

export interface ChatErrorPayload {
    error: string;
    kind?: 'network_dropped' | 'engine_error' | 'tool_error';
    retryable?: boolean;
    offerRepair?: boolean;
}

export type FailedTurnAction = 'retry' | 'repair';

export interface FailedTurnNotice {
    level: 'error';
    text: string;
    actions: FailedTurnAction[];
}

/** A notice with buttons for a stopped turn; null for the plain errors that have always been a status line. */
export function failedTurnNotice(p: ChatErrorPayload): FailedTurnNotice | null {
    if (!p.kind) return null;
    const actions: FailedTurnAction[] = [];
    if (p.retryable) actions.push('retry');
    if (p.offerRepair) actions.push('repair');
    return { level: 'error', text: p.error, actions };
}

interface MessageLike {
    role: string;
    content: string;
}

/** What the user last asked: the newest user message that is not a tool result wrapped as one. */
export function lastUserQuestion(messages: MessageLike[]): string | null {
    for (let i = messages.length - 1; i >= 0; i--) {
        const m = messages[i];
        if (m.role === 'user' && !isToolResultMessage(m) && m.content.trim()) return m.content;
    }
    return null;
}
