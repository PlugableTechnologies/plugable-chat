import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import type { ChatSummary, Message } from '../types';
import { RELEVANCE_SEARCH_DEBOUNCE_MS, RELEVANCE_SEARCH_MIN_LENGTH } from '../constants';

// Module-level state for relevance search
let relevanceSearchTimeout: ReturnType<typeof setTimeout> | null = null;
let relevanceSearchGeneration = 0; // Incremented on each new search to cancel stale results

// Dependencies from other slices
interface ChatHistorySliceDeps {
    chatMessages: Message[];
    currentModel: string;
    streamingChatId: string | null;
    streamingMessages: Message[];
    setModel: (model: string) => Promise<void>;
    // For clearing attachments
    attachedDatabaseTables: any[];
    attachedTools: any[];
    attachedPaths: string[];
    ragIndexedFiles: string[];
    ragChunkCount: number;
}

export interface ChatHistorySlice {
    // Current chat ID
    currentChatId: string | null;
    setCurrentChatId: (id: string | null) => void;
    
    // Chat history
    history: ChatSummary[];
    pendingSummaries: Record<string, ChatSummary>;
    fetchHistory: () => Promise<void>;
    clearPendingSummary: (id: string) => void;
    loadChat: (id: string) => Promise<void>;
    deleteChat: (id: string) => Promise<void>;
    upsertHistoryEntry: (summary: ChatSummary) => void;
    renameChat: (id: string, newTitle: string) => Promise<void>;
    togglePin: (id: string) => Promise<void>;
    
    // Relevance search (embedding-based autocomplete)
    relevanceResults: ChatSummary[] | null;
    isSearchingRelevance: boolean;
    triggerRelevanceSearch: (query: string) => void;
    clearRelevanceSearch: () => void;
}

export const createChatHistorySlice: StateCreator<
    ChatHistorySlice & ChatHistorySliceDeps,
    [],
    [],
    ChatHistorySlice
> = (set, get) => ({
    // Current chat ID
    currentChatId: null,
    setCurrentChatId: (id) => {
        if (id === null) {
            // New chat - clear all per-chat attachments (databases, tools, documents)
            set({ 
                currentChatId: null, 
                attachedDatabaseTables: [], 
                attachedTools: [],
                attachedPaths: [],
                ragIndexedFiles: [],
                ragChunkCount: 0,
            } as any);
            // Fire-and-forget clear of backend RAG context for new chats
            invoke<boolean>('clear_rag_context').catch(e => 
                console.error('[ChatStore] Failed to clear RAG context for new chat:', e)
            );
        } else {
            set({ currentChatId: id });
        }
    },
    
    // Chat history
    history: [],
    pendingSummaries: {},
    fetchHistory: async () => {
        console.log('[ChatStore] fetchHistory called');
        try {
            const fetchedHistory = await invoke<ChatSummary[]>('get_all_chats');
            console.log(`[ChatStore] Fetched ${fetchedHistory.length} chats from backend:`, 
                fetchedHistory.map(c => ({ id: c.id.slice(0, 8), title: c.title })));
            
            const pendingEntries = Object.values(get().pendingSummaries);
            console.log(`[ChatStore] Pending summaries: ${pendingEntries.length}`, 
                pendingEntries.map(c => ({ id: c.id.slice(0, 8), title: c.title })));
            
            const mergedHistory = [
                ...pendingEntries.filter(
                    (entry) => !fetchedHistory.some((chat) => chat.id === entry.id)
                ),
                ...fetchedHistory
            ];
            console.log(`[ChatStore] Final merged history: ${mergedHistory.length} chats`);
            set({ history: mergedHistory, backendError: null } as any);
        } catch (e: any) {
            console.error("[ChatStore] Failed to fetch history:", e);
            set({ backendError: `Failed to load history: ${e.message || e}` } as any);
        }
    },
    clearPendingSummary: (id) => set((state) => {
        const { [id]: _, ...rest } = state.pendingSummaries;
        return { pendingSummaries: rest };
    }),
    loadChat: async (id) => {
        try {
            const state = get();
            const currentChatId = state.currentChatId;
            const streamingChatId = state.streamingChatId;
            
            // If we're switching away from a streaming chat, save current messages to streamingMessages
            if (streamingChatId && streamingChatId === currentChatId && id !== currentChatId) {
                console.log(`[ChatStore] Switching away from streaming chat ${currentChatId?.slice(0, 8)}, saving messages`);
                set({ streamingMessages: [...state.chatMessages] } as any);
            }
            
            // If we're switching to the streaming chat, restore messages from streamingMessages
            if (streamingChatId && streamingChatId === id && state.streamingMessages.length > 0) {
                console.log(`[ChatStore] Switching to streaming chat ${id.slice(0, 8)}, restoring messages`);
                set({ 
                    chatMessages: state.streamingMessages, 
                    currentChatId: id, 
                    streamingMessages: [], 
                    backendError: null 
                } as any);
                return;
            }

            // Find the chat summary to see if it has an associated model
            const chatSummary = state.history.find(c => c.id === id);
            if (chatSummary?.model && chatSummary.model !== state.currentModel) {
                console.log(`[ChatStore] Chat ${id.slice(0, 8)} uses model ${chatSummary.model}, switching current model`);
                await get().setModel(chatSummary.model);
            }
            
            const messagesJson = await invoke<string | null>('load_chat', { id });
            if (messagesJson) {
                const messages = JSON.parse(messagesJson);
                // Ensure messages have IDs if missing (legacy)
                const processedMessages = messages.map((m: any, idx: number) => ({
                    ...m,
                    id: m.id || `${Date.now()}-${idx}`,
                    timestamp: m.timestamp || Date.now(),
                    systemPromptText: m.system_prompt || m.systemPromptText,
                }));
                set({ chatMessages: processedMessages, currentChatId: id, backendError: null } as any);
            } else {
                set({ chatMessages: [], currentChatId: id } as any);
            }
        } catch (e: any) {
            console.error("Failed to load chat", e);
            set({ backendError: `Failed to load chat: ${e.message || e}` } as any);
        }
    },
    deleteChat: async (id) => {
        console.log('[ChatStore] deleteChat called with id:', id);
        try {
            const result = await invoke<boolean>('delete_chat', { id });
            console.log('[ChatStore] delete_chat backend returned:', result);
            if (get().currentChatId === id) {
                set({ 
                    chatMessages: [], 
                    currentChatId: null,
                    // Clear all per-chat attachments
                    attachedDatabaseTables: [],
                    attachedTools: [],
                    attachedPaths: [],
                    ragIndexedFiles: [],
                    ragChunkCount: 0,
                } as any);
                // Fire-and-forget clear of backend RAG context
                invoke<boolean>('clear_rag_context').catch(e => 
                    console.error('[ChatStore] Failed to clear RAG context on deleteChat:', e)
                );
            }
            // Clear from pending summaries (important for newly created chats)
            get().clearPendingSummary(id);
            // Also remove directly from history in case fetchHistory has race conditions
            set((state) => ({
                history: state.history.filter(chat => chat.id !== id)
            }));
            await get().fetchHistory();
            console.log('[ChatStore] History refreshed after delete');
        } catch (e: any) {
            console.error("[ChatStore] Failed to delete chat", e);
            set({ backendError: `Failed to delete chat: ${e.message || e}` } as any);
        }
    },
    upsertHistoryEntry: (summary) => set((state) => {
        console.log(`[ChatStore] upsertHistoryEntry: ${summary.id.slice(0, 8)} "${summary.title}"`);
        const existing = state.history.find((chat) => chat.id === summary.id);
        const pinned = existing?.pinned ?? summary.pinned;
        const filtered = state.history.filter((chat) => chat.id !== summary.id);
        const updatedSummary = { ...summary, pinned };
        console.log(`[ChatStore] History will have ${filtered.length + 1} entries (was ${state.history.length})`);
        return {
            history: [updatedSummary, ...filtered],
            pendingSummaries: {
                ...state.pendingSummaries,
                [summary.id]: updatedSummary
            }
        };
    }),
    renameChat: async (id, newTitle) => {
        try {
            await invoke('update_chat', { id, title: newTitle });
            await get().fetchHistory();
        } catch (e: any) {
            console.error("Failed to rename chat", e);
        }
    },
    togglePin: async (id) => {
        try {
            const chat = get().history.find(c => c.id === id);
            if (chat) {
                await invoke('update_chat', { id, pinned: !chat.pinned });
                await get().fetchHistory();
            }
        } catch (e: any) {
            console.error("Failed to toggle pin", e);
        }
    },

    // Relevance search (embedding-based autocomplete)
    relevanceResults: null,
    isSearchingRelevance: false,
    triggerRelevanceSearch: (query: string) => {
        // Cancel any pending search
        if (relevanceSearchTimeout) {
            clearTimeout(relevanceSearchTimeout);
            relevanceSearchTimeout = null;
        }

        // If query is too short, clear results and return to normal history
        if (query.trim().length < RELEVANCE_SEARCH_MIN_LENGTH) {
            set({ relevanceResults: null, isSearchingRelevance: false });
            return;
        }

        // Increment generation to invalidate any in-flight requests
        const myGeneration = ++relevanceSearchGeneration;

        // Debounce: wait before actually searching
        relevanceSearchTimeout = setTimeout(async () => {
            set({ isSearchingRelevance: true });

            try {
                // Call the backend search_history command
                await invoke('search_history', { query: query.trim() });
                
                // Results will come via the 'sidebar-update' event
            } catch (e: any) {
                console.error("Failed to search history:", e);
                // On error, fall back to regular history
                if (relevanceSearchGeneration === myGeneration) {
                    set({ relevanceResults: null, isSearchingRelevance: false });
                }
            }
        }, RELEVANCE_SEARCH_DEBOUNCE_MS);
    },
    clearRelevanceSearch: () => {
        if (relevanceSearchTimeout) {
            clearTimeout(relevanceSearchTimeout);
            relevanceSearchTimeout = null;
        }
        relevanceSearchGeneration++; // Cancel any in-flight requests
        set({ relevanceResults: null, isSearchingRelevance: false });
    },
});
