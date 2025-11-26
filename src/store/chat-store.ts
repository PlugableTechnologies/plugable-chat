import { create } from 'zustand'
import { invoke, listen } from '../lib/api';

export type ReasoningEffort = 'low' | 'medium' | 'high';

export interface CachedModel {
    alias: string;
    model_id: string;
}

export interface ChatSummary {
    id: string;
    title: string;
    preview: string;
    score: number;
    pinned: boolean;
}

export interface Message {
    id: string;
    role: 'user' | 'assistant';
    content: string;
    timestamp: number;
}

interface ChatState {
    messages: Message[];
    addMessage: (msg: Message) => void;
    input: string;
    setInput: (s: string) => void;
    isLoading: boolean;
    setIsLoading: (loading: boolean) => void;
    stopGeneration: () => void;
    generationId: number;

    currentChatId: string | null;
    setCurrentChatId: (id: string | null) => void;

    availableModels: string[];
    cachedModels: CachedModel[];
    currentModel: string;
    isConnecting: boolean;
    fetchModels: () => Promise<void>;
    fetchCachedModels: () => Promise<void>;
    retryConnection: () => Promise<void>;
    setModel: (model: string) => Promise<void>;
    reasoningEffort: ReasoningEffort;
    setReasoningEffort: (effort: ReasoningEffort) => void;

    // History
    history: ChatSummary[];
    pendingSummaries: Record<string, ChatSummary>;
    fetchHistory: () => Promise<void>;
    clearPendingSummary: (id: string) => void;
    upsertHistoryEntry: (summary: ChatSummary) => void;
    loadChat: (id: string) => Promise<void>;
    deleteChat: (id: string) => Promise<void>;
    renameChat: (id: string, newTitle: string) => Promise<void>;
    togglePin: (id: string) => Promise<void>;

    // Relevance search (embedding-based)
    relevanceResults: ChatSummary[] | null; // null = not searching, use history
    isSearchingRelevance: boolean;
    triggerRelevanceSearch: (query: string) => void;
    clearRelevanceSearch: () => void;

    // Listener management
    isListening: boolean;
    setupListeners: () => Promise<void>;
    cleanupListeners: () => void;

    // Error handling
    backendError: string | null;
    clearError: () => void;

    // Editor State
    isEditorOpen: boolean;
    editorContent: string;
    editorLanguage: string;
    setEditorOpen: (open: boolean) => void;
    setEditorContent: (content: string, language: string) => void;
}

// Module-level variables to hold unlisten functions
// This ensures they persist even if the store is recreated (though Zustand stores are usually singletons)
let unlistenToken: (() => void) | undefined;
let unlistenFinished: (() => void) | undefined;
let unlistenModelSelected: (() => void) | undefined;
let unlistenChatSaved: (() => void) | undefined;
let unlistenSidebarUpdate: (() => void) | undefined;
let isSettingUp = false; // Guard against async race conditions
let listenerGeneration = 0; // Generation counter to invalidate stale setup calls
let modelFetchPromise: Promise<void> | null = null;
let modelFetchRetryTimer: ReturnType<typeof setTimeout> | null = null;
const MODEL_FETCH_MAX_RETRIES = 10;
const MODEL_FETCH_INITIAL_DELAY_MS = 1000;

// Relevance search debounce/cancellation state
let relevanceSearchTimeout: ReturnType<typeof setTimeout> | null = null;
let relevanceSearchGeneration = 0; // Incremented on each new search to cancel stale results
const RELEVANCE_SEARCH_DEBOUNCE_MS = 400; // Wait 400ms after typing stops
const RELEVANCE_SEARCH_MIN_LENGTH = 3; // Minimum chars before searching

export const useChatStore = create<ChatState>((set, get) => ({
    messages: [],
    addMessage: (msg) => set((state) => ({ messages: [...state.messages, msg] })),
    input: '',
    setInput: (input) => set({ input }),
    isLoading: false,
    setIsLoading: (isLoading) => set({ isLoading }),
    generationId: 0,
    stopGeneration: () => {
        // Increment generationId to ignore any incoming tokens from the stopped generation
        set((state) => ({ 
            isLoading: false, 
            generationId: state.generationId + 1 
        }));
    },

    currentChatId: null,
    setCurrentChatId: (id) => set({ currentChatId: id }),

    availableModels: [],
    cachedModels: [],
    currentModel: 'Loading...',
    isConnecting: false,
    reasoningEffort: 'low',
    fetchModels: async () => {
        if (modelFetchPromise) {
            return modelFetchPromise;
        }

        // Clear any pending retry
        if (modelFetchRetryTimer) {
            clearTimeout(modelFetchRetryTimer);
            modelFetchRetryTimer = null;
        }

        set({ isConnecting: true });

        modelFetchPromise = (async () => {
            let retryCount = 0;
            let delay = MODEL_FETCH_INITIAL_DELAY_MS;

            const attemptFetch = async (): Promise<boolean> => {
                try {
                    console.log(`[ChatStore] Fetching models (attempt ${retryCount + 1}/${MODEL_FETCH_MAX_RETRIES})...`);
                    const models = await invoke<string[]>('get_models');
                    
                    if (models.length > 0) {
                        set({ availableModels: models, backendError: null, isConnecting: false });
                        if (get().currentModel === 'Loading...' || get().currentModel === 'Unavailable') {
                            set({ currentModel: models[0] });
                        }
                        console.log(`[ChatStore] Successfully fetched ${models.length} model(s)`);
                        return true;
                    } else {
                        console.log("[ChatStore] No models returned, will retry...");
                        return false;
                    }
                } catch (e: any) {
                    console.error(`[ChatStore] Fetch models attempt ${retryCount + 1} failed:`, e);
                    return false;
                }
            };

            // Initial attempt
            if (await attemptFetch()) {
                return;
            }

            // Retry loop with exponential backoff
            while (retryCount < MODEL_FETCH_MAX_RETRIES - 1) {
                retryCount++;
                console.log(`[ChatStore] Retrying in ${delay}ms...`);
                
                await new Promise(resolve => {
                    modelFetchRetryTimer = setTimeout(resolve, delay);
                });
                modelFetchRetryTimer = null;

                if (await attemptFetch()) {
                    return;
                }

                // Exponential backoff with max of 10 seconds
                delay = Math.min(delay * 1.5, 10000);
            }

            // All retries failed
            console.error(`[ChatStore] Failed to fetch models after ${MODEL_FETCH_MAX_RETRIES} attempts`);
            set({ 
                backendError: `Failed to connect to Foundry. Please ensure Foundry is running and try again.`,
                currentModel: 'Unavailable',
                isConnecting: false
            });
        })();

        try {
            await modelFetchPromise;
        } finally {
            modelFetchPromise = null;
        }
    },
    retryConnection: async () => {
        // Reset state and try again
        set({ currentModel: 'Loading...', backendError: null });
        await get().fetchModels();
    },
    fetchCachedModels: async () => {
        try {
            console.log('[ChatStore] Fetching cached models...');
            const cached = await invoke<CachedModel[]>('get_cached_models');
            set({ cachedModels: cached });
            console.log(`[ChatStore] Found ${cached.length} cached model(s)`);
        } catch (e: any) {
            console.error('[ChatStore] Failed to fetch cached models:', e);
        }
    },
    setModel: async (model) => {
        try {
            await invoke('set_model', { model });
            set({ currentModel: model });
        } catch (e) {
            console.error("Failed to set model", e);
        }
    },
    setReasoningEffort: (effort: ReasoningEffort) => set({ reasoningEffort: effort }),

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
            set({ history: mergedHistory, backendError: null });
        } catch (e: any) {
            console.error("[ChatStore] Failed to fetch history:", e);
            set({ backendError: `Failed to load history: ${e.message || e}` });
        }
    },
    clearPendingSummary: (id) => set((state) => {
        const { [id]: _, ...rest } = state.pendingSummaries;
        return { pendingSummaries: rest };
    }),
    loadChat: async (id) => {
        try {
            const messagesJson = await invoke<string | null>('load_chat', { id });
            if (messagesJson) {
                const messages = JSON.parse(messagesJson);
                // Ensure messages have IDs if missing (legacy)
                const processedMessages = messages.map((m: any, idx: number) => ({
                    ...m,
                    id: m.id || `${Date.now()}-${idx}`,
                    timestamp: m.timestamp || Date.now()
                }));
                set({ messages: processedMessages, currentChatId: id, backendError: null });
            } else {
                set({ messages: [], currentChatId: id });
            }
        } catch (e: any) {
            console.error("Failed to load chat", e);
            set({ backendError: `Failed to load chat: ${e.message || e}` });
        }
    },
    deleteChat: async (id) => {
        console.log('[ChatStore] deleteChat called with id:', id);
        try {
            const result = await invoke<boolean>('delete_chat', { id });
            console.log('[ChatStore] delete_chat backend returned:', result);
            if (get().currentChatId === id) {
                set({ messages: [], currentChatId: null });
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
            set({ backendError: `Failed to delete chat: ${e.message || e}` });
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
                // We'll set up a one-time listener or handle in setupListeners
                // For now, we'll handle it in the event listener
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

    isListening: false,
    setupListeners: async () => {
        // Prevent duplicate listeners if already listening or currently setting up
        if (get().isListening || isSettingUp) {
            console.log("[ChatStore] Listeners already active or setting up. Skipping.");
            return;
        }

        isSettingUp = true;
        const myGeneration = listenerGeneration;

        // Clean up any existing listeners just in case (defensive)
        if (unlistenToken) { unlistenToken(); unlistenToken = undefined; }
        if (unlistenFinished) { unlistenFinished(); unlistenFinished = undefined; }

        console.log(`[ChatStore] Setting up event listeners (Gen: ${myGeneration})...`);

        try {
            const tokenListener = await listen<string>('chat-token', (event) => {
                set((state) => {
                    // Ignore tokens if generation was stopped
                    if (!state.isLoading) {
                        return state;
                    }
                    const lastMsg = state.messages[state.messages.length - 1];
                    // Only append if the last message is from assistant
                    if (lastMsg && lastMsg.role === 'assistant') {
                        const newMessages = [...state.messages];
                        newMessages[newMessages.length - 1] = {
                            ...lastMsg,
                            content: lastMsg.content + event.payload
                        };
                        return { messages: newMessages };
                    }
                    return state;
                });
            });

            const finishedListener = await listen('chat-finished', () => {
                set({ isLoading: false });
            });

            const modelSelectedListener = await listen<string>('model-selected', (event) => {
                set({ currentModel: event.payload });
            });
            
            const chatSavedListener = await listen<string>('chat-saved', async (event) => {
                const chatId = event.payload;
                console.log(`[ChatStore] chat-saved event received for: ${chatId.slice(0, 8)}...`);
                
                // Fetch history first, then only clear pending if the entry exists in fetched data
                // This handles the race condition where the backend event fires before LanceDB finishes indexing
                try {
                    const fetchedHistory = await invoke<ChatSummary[]>('get_all_chats');
                    console.log(`[ChatStore] After chat-saved, backend has ${fetchedHistory.length} chats`);
                    
                    const existsInFetched = fetchedHistory.some((chat) => chat.id === chatId);
                    console.log(`[ChatStore] Chat ${chatId.slice(0, 8)} exists in backend: ${existsInFetched}`);
                    
                    if (existsInFetched) {
                        // Safe to clear pending - entry is now in backend
                        get().clearPendingSummary(chatId);
                        const pendingEntries = Object.values(get().pendingSummaries);
                        const mergedHistory = [
                            ...pendingEntries.filter(
                                (entry) => !fetchedHistory.some((chat) => chat.id === entry.id)
                            ),
                            ...fetchedHistory
                        ];
                        console.log(`[ChatStore] Cleared pending, merged history now: ${mergedHistory.length} chats`);
                        set({ history: mergedHistory });
                    } else {
                        // Entry not in backend yet - keep pending, just refresh with merged data
                        console.log(`[ChatStore] Chat ${chatId.slice(0, 8)} not yet in backend, keeping in pending`);
                        const pendingEntries = Object.values(get().pendingSummaries);
                        const mergedHistory = [
                            ...pendingEntries.filter(
                                (entry) => !fetchedHistory.some((chat) => chat.id === entry.id)
                            ),
                            ...fetchedHistory
                        ];
                        set({ history: mergedHistory });
                        
                        // Retry after a short delay
                        console.log(`[ChatStore] Scheduling retry fetch in 500ms...`);
                        setTimeout(() => {
                            get().fetchHistory();
                        }, 500);
                    }
                } catch (e) {
                    console.error("[ChatStore] Failed to fetch history after chat-saved:", e);
                }
            });

            const sidebarUpdateListener = await listen<ChatSummary[]>('sidebar-update', (event) => {
                // Only apply if we're still searching (not cancelled)
                if (get().isSearchingRelevance) {
                    set({ relevanceResults: event.payload, isSearchingRelevance: false });
                }
            });

            // Critical check: did cleanup happen (invalidating this setup) while we were awaiting?
            if (listenerGeneration !== myGeneration) {
                console.log(`[ChatStore] Setup aborted due to generation mismatch (${myGeneration} vs ${listenerGeneration}). Cleaning up new listeners.`);
                tokenListener();
                finishedListener();
                modelSelectedListener();
                chatSavedListener();
                sidebarUpdateListener();
                isSettingUp = false;
                return;
            }

            // Assign to module variables
            unlistenToken = tokenListener;
            unlistenFinished = finishedListener;
            unlistenModelSelected = modelSelectedListener;
            unlistenChatSaved = chatSavedListener;
            unlistenSidebarUpdate = sidebarUpdateListener;

            set({ isListening: true });
            console.log(`[ChatStore] Event listeners active (Gen: ${myGeneration}).`);
            
            // Proactively fetch models on startup
            // This runs in the background - don't await to avoid blocking listener setup
            get().fetchModels().catch((e) => {
                console.error("[ChatStore] Background model fetch failed:", e);
            });
            
            // Also fetch cached models (for the dropdown)
            get().fetchCachedModels().catch((e) => {
                console.error("[ChatStore] Background cached models fetch failed:", e);
            });
        } catch (e) {
            console.error("[ChatStore] Failed to setup listeners:", e);
        } finally {
            isSettingUp = false;
        }
    },
    cleanupListeners: () => {
        listenerGeneration++; // Invalidate pending setups
        if (unlistenToken) {
            unlistenToken();
            unlistenToken = undefined;
        }
        if (unlistenFinished) {
            unlistenFinished();
            unlistenFinished = undefined;
        }
        if (unlistenModelSelected) {
            unlistenModelSelected();
            unlistenModelSelected = undefined;
        }
        if (unlistenChatSaved) {
            unlistenChatSaved();
            unlistenChatSaved = undefined;
        }
        if (unlistenSidebarUpdate) {
            unlistenSidebarUpdate();
            unlistenSidebarUpdate = undefined;
        }
        set({ isListening: false });
        isSettingUp = false; // Reset setup guard
        console.log(`[ChatStore] Event listeners cleaned up. New Gen: ${listenerGeneration}`);
    },

    backendError: null,
    clearError: () => set({ backendError: null }),

    // Editor State
    isEditorOpen: false,
    editorContent: '',
    editorLanguage: 'typescript',
    setEditorOpen: (open) => set({ isEditorOpen: open }),
    setEditorContent: (content, language) => set({ editorContent: content, editorLanguage: language, isEditorOpen: true })
}))
