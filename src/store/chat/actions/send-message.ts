import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import type { Message, ChatSummary, ReasoningEffort, OperationStatus } from '../types';
import { 
    generateClientChatIdentifier, 
    deriveChatTitleFromPrompt, 
    deriveChatPreviewFromMessage 
} from '../helpers';
import { buildFollowUp, takeNextLaunchPrompt } from '../launch-queue';
import { launchTurnInvocation } from '../launch-turn';
import { buildChatHistory } from '../../../components/chat/utils/history';
import { lastUserQuestion } from '../failed-turn';

// Dependencies from other slices
interface SendMessageSliceDeps {
    // Message state
    chatMessages: Message[];
    chatInputValue: string;
    
    // Streaming state
    assistantStreamingActive: boolean;
    streamingChatId: string | null;
    lastStreamActivityTs: number | null;
    
    // Chat ID
    currentChatId: string | null;
    setCurrentChatId: (id: string | null) => void;
    
    // History
    upsertHistoryEntry: (summary: ChatSummary) => void;
    
    // Model
    currentModel: string;
    reasoningEffort: ReasoningEffort;
    
    // Launch overrides
    launchInitialPrompt: string | null;
    launchQueuedPrompts: string[];
    launchSameChat: boolean;
    launchPromptApplied: boolean;
    // --direct-tool: where the launch prompts go instead of the model
    launchDirectToolRef: { server_id: string; tool_name: string } | null;
    
    // Operation status
    operationStatus: OperationStatus | null;
}

export interface SendMessageSlice {
    // Send the initial prompt from CLI
    sendLaunchPrompt: () => Promise<void>;

    // Start the next --prompts-file prompt in a fresh chat (called when a chat finishes)
    advanceLaunchPromptQueue: () => Promise<void>;
    
    // Start a system-initiated chat (for help messages)
    startSystemChat: (assistantMessage: string, title?: string) => void;

    // "Retry" on a stopped turn: puts the last question back and asks the chat screen to send it.
    // Nothing is ever re-sent without this explicit request.
    retryRequestId: number;
    requestRetryOfLastTurn: () => void;
}

export const createSendMessageSlice: StateCreator<
    SendMessageSlice & SendMessageSliceDeps,
    [],
    [],
    SendMessageSlice
> = (set, get) => ({
    retryRequestId: 0,
    requestRetryOfLastTurn: () => {
        const question = lastUserQuestion(get().chatMessages);
        if (!question) return;
        set({ chatInputValue: question, retryRequestId: get().retryRequestId + 1 } as any);
    },

    sendLaunchPrompt: async () => {
        const state = get();
        const rawPrompt = state.launchInitialPrompt;
        
        // The welcome notice of a first run is not a conversation: a direct-tool launch (no model, so that notice is showing) still runs.
        const openConversation = state.chatMessages.length > 0 && !(state.launchDirectToolRef && state.currentChatId === null);
        if (!rawPrompt || openConversation) {
            set({ launchPromptApplied: true } as any);
            return;
        }
        const text = rawPrompt.trim();
        if (!text) {
            set({ launchPromptApplied: true } as any);
            return;
        }

        const chatId = generateClientChatIdentifier();
        const derivedTitle = deriveChatTitleFromPrompt(text);
        const preview = deriveChatPreviewFromMessage(text);
        const summaryScore = 0;
        const summaryPinned = false;
        const timestamp = Date.now();

        // Seed history entry
        state.upsertHistoryEntry({
            id: chatId,
            title: derivedTitle,
            preview,
            score: summaryScore,
            pinned: summaryPinned,
        });

        // Seed UI messages
        set({
            chatMessages: [
                { id: timestamp.toString(), role: 'user', content: text, timestamp },
                { id: (timestamp + 1).toString(), role: 'assistant', content: '', timestamp: timestamp + 1 },
            ],
            currentChatId: chatId,
            chatInputValue: '',
            assistantStreamingActive: true,
            streamingChatId: chatId,
            operationStatus: {
                type: 'streaming',
                message: 'Generating response...',
                startTime: Date.now(),
            },
            statusBarDismissed: false,
            lastStreamActivityTs: Date.now(),
        } as any);

        try {
            const returnedChatId = await invoke<string>(
                ...launchTurnInvocation(state.launchDirectToolRef, {
                    chatId,
                    title: derivedTitle,
                    message: text,
                    history: [],
                    reasoningEffort: state.reasoningEffort,
                    model: state.currentModel, // Frontend is source of truth for model
                }),
            );

            if (returnedChatId && returnedChatId !== chatId) {
                state.setCurrentChatId(returnedChatId);
                state.upsertHistoryEntry({
                    id: returnedChatId,
                    title: derivedTitle,
                    preview,
                    score: summaryScore,
                    pinned: summaryPinned,
                });
            }
        } catch (error) {
            console.error('[ChatStore] Failed to send launch prompt:', error);
            set((s) => {
                const newMessages = [...s.chatMessages];
                const lastIdx = newMessages.length - 1;
                if (lastIdx >= 0) {
                    newMessages[lastIdx] = {
                        ...newMessages[lastIdx],
                        content: `Error: ${error}`,
                    };
                }
                return {
                    chatMessages: newMessages,
                    assistantStreamingActive: false,
                } as any;
            });
        } finally {
            set({ launchPromptApplied: true } as any);
        }
    },

    advanceLaunchPromptQueue: async () => {
        const state = get();
        if (state.launchQueuedPrompts.length === 0 || !state.launchPromptApplied) {
            return;
        }
        const { nextLaunchPrompt, remainingLaunchPrompts } = takeNextLaunchPrompt(state.launchQueuedPrompts);
        if (!nextLaunchPrompt) {
            set({ launchQueuedPrompts: [] } as any);
            return;
        }
        if (state.launchSameChat && state.currentChatId && state.chatMessages.length > 0) {
            // --same-chat: a follow-up in the conversation so far, as a user would type it.
            const chatId = state.currentChatId;
            const { history, message } = buildFollowUp(state.chatMessages, nextLaunchPrompt, buildChatHistory);
            const ts = Date.now();
            set({
                launchQueuedPrompts: remainingLaunchPrompts,
                chatMessages: [
                    ...state.chatMessages,
                    { id: ts.toString(), role: 'user', content: message, timestamp: ts },
                    { id: (ts + 1).toString(), role: 'assistant', content: '', timestamp: ts + 1 },
                ],
                assistantStreamingActive: true,
                streamingChatId: chatId,
                operationStatus: { type: 'streaming', message: 'Generating response...', startTime: ts },
                statusBarDismissed: false,
                lastStreamActivityTs: ts,
            } as any);
            try {
                await invoke<string>(
                    ...launchTurnInvocation(state.launchDirectToolRef, {
                        chatId,
                        title: undefined,
                        message,
                        history,
                        reasoningEffort: state.reasoningEffort,
                        model: state.currentModel,
                    }),
                );
            } catch (error) {
                console.error('[ChatStore] Failed to send follow-up launch prompt:', error);
                set({ assistantStreamingActive: false } as any);
            }
            return;
        }
        // A fresh chat for each prompt: sendLaunchPrompt only runs on an empty conversation.
        set({
            launchQueuedPrompts: remainingLaunchPrompts,
            launchInitialPrompt: nextLaunchPrompt,
            launchPromptApplied: false,
            chatMessages: [],
            currentChatId: null,
        } as any);
        await get().sendLaunchPrompt();
    },

    startSystemChat: (assistantMessage: string, title?: string) => {
        const cryptoObj = typeof globalThis !== 'undefined' ? globalThis.crypto : undefined;
        const chatId = (cryptoObj && typeof cryptoObj.randomUUID === 'function')
            ? cryptoObj.randomUUID()
            : `system-chat-${Date.now()}-${Math.floor(Math.random() * 1000)}`;
        
        console.log(`[ChatStore] Starting system chat: ${chatId.slice(0, 8)} "${title || 'System Message'}"`);
        
        set({
            chatMessages: [{
                id: Date.now().toString(),
                role: 'assistant',
                content: assistantMessage,
                timestamp: Date.now()
            }],
            currentChatId: null, // Don't set a chat ID - this is a non-persistent help chat
            chatInputValue: '',
        } as any);
    },
});
