import type { Message } from './types';

interface LeaveState {
    streamingChatId: string | null;
    currentChatId: string | null;
    chatMessages: Message[];
}

/**
 * When the user leaves the chat that is currently streaming (switching chats or New Chat), its
 * live messages move to `streamingMessages` so tokens keep landing somewhere and the chat can be
 * restored when clicked again. Returns null when nothing needs stashing.
 */
export function stashStreamingOnLeave(state: LeaveState, nextChatId: string | null): { streamingMessages: Message[] } | null {
    const { streamingChatId, currentChatId } = state;
    if (!streamingChatId || streamingChatId !== currentChatId || nextChatId === currentChatId) return null;
    return { streamingMessages: [...state.chatMessages] };
}

/** Append a streamed token to a background chat's buffer, starting an assistant message if needed. */
export function appendTokenToBuffer(buffer: Message[], token: string, now: number): Message[] {
    const last = buffer[buffer.length - 1];
    if (last && last.role === 'assistant') {
        const next = [...buffer];
        next[next.length - 1] = { ...last, content: last.content + token };
        return next;
    }
    return [...buffer, { id: `bg-${now}`, role: 'assistant', content: token, timestamp: now }];
}

/** True when `chatId` is the chat whose response is still being generated. */
export function isChatRunning(chatId: string, streamingChatId: string | null, assistantStreamingActive: boolean): boolean {
    return assistantStreamingActive && streamingChatId === chatId;
}
