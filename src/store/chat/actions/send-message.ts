import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import type { Message, ChatSummary, ReasoningEffort, OperationStatus } from '../types';
import { 
    generateClientChatIdentifier, 
    deriveChatTitleFromPrompt, 
    deriveChatPreviewFromMessage 
} from '../helpers';

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
    launchPromptApplied: boolean;
    
    // Operation status
    operationStatus: OperationStatus | null;
}

export interface SendMessageSlice {
    // Send the initial prompt from CLI
    sendLaunchPrompt: () => Promise<void>;
    
    // Start a system-initiated chat (for help messages)
    startSystemChat: (assistantMessage: string, title?: string) => void;
}

export const createSendMessageSlice: StateCreator<
    SendMessageSlice & SendMessageSliceDeps,
    [],
    [],
    SendMessageSlice
> = (set, get) => ({
    sendLaunchPrompt: async () => {
        const state = get();
        const rawPrompt = state.launchInitialPrompt;
        
        if (!rawPrompt || state.chatMessages.length > 0) {
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
            const returnedChatId = await invoke<string>('chat', {
                chatId,
                title: derivedTitle,
                message: text,
                history: [],
                reasoningEffort: state.reasoningEffort,
                model: state.currentModel, // Frontend is source of truth for model
            });

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
