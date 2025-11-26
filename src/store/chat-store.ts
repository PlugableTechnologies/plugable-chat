import { create } from 'zustand'
import { invoke, listen } from '../lib/api';

export type ReasoningEffort = 'low' | 'medium' | 'high';

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

    currentChatId: string | null;
    setCurrentChatId: (id: string | null) => void;

    availableModels: string[];
    currentModel: string;
    fetchModels: () => Promise<void>;
    setModel: (model: string) => Promise<void>;
    reasoningEffort: ReasoningEffort;
    setReasoningEffort: (effort: ReasoningEffort) => void;

    // History
    history: ChatSummary[];
    fetchHistory: () => Promise<void>;
    loadChat: (id: string) => Promise<void>;
    deleteChat: (id: string) => Promise<void>;
    renameChat: (id: string, newTitle: string) => Promise<void>;
    togglePin: (id: string) => Promise<void>;

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
let isSettingUp = false; // Guard against async race conditions
let listenerGeneration = 0; // Generation counter to invalidate stale setup calls

export const useChatStore = create<ChatState>((set, get) => ({
    messages: [],
    addMessage: (msg) => set((state) => ({ messages: [...state.messages, msg] })),
    input: '',
    setInput: (input) => set({ input }),
    isLoading: false,
    setIsLoading: (isLoading) => set({ isLoading }),

    currentChatId: null,
    setCurrentChatId: (id) => set({ currentChatId: id }),

    availableModels: [],
    currentModel: 'Loading...',
    reasoningEffort: 'low',
    fetchModels: async () => {
        try {
            const models = await invoke<string[]>('get_models');
            set({ availableModels: models, backendError: null });
            if (models.length > 0 && get().currentModel === 'Loading...') {
                set({ currentModel: models[0] });
            }
        } catch (e: any) {
            console.error("Failed to fetch models", e);
            set({ backendError: `Failed to connect to backend: ${e.message || e}` });
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
    fetchHistory: async () => {
        try {
            const history = await invoke<ChatSummary[]>('get_all_chats');
            set({ history, backendError: null });
        } catch (e: any) {
            console.error("Failed to fetch history", e);
            set({ backendError: `Failed to load history: ${e.message || e}` });
        }
    },
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
        try {
            await invoke('delete_chat', { id });
            if (get().currentChatId === id) {
                set({ messages: [], currentChatId: null });
            }
            await get().fetchHistory();
        } catch (e: any) {
            console.error("Failed to delete chat", e);
            set({ backendError: `Failed to delete chat: ${e.message || e}` });
        }
    },
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
            
            const chatSavedListener = await listen<string>('chat-saved', () => {
                get().fetchHistory();
            });

            // Critical check: did cleanup happen (invalidating this setup) while we were awaiting?
            if (listenerGeneration !== myGeneration) {
                console.log(`[ChatStore] Setup aborted due to generation mismatch (${myGeneration} vs ${listenerGeneration}). Cleaning up new listeners.`);
                tokenListener();
                finishedListener();
                modelSelectedListener();
                chatSavedListener();
                isSettingUp = false;
                return;
            }

            // Assign to module variables
            unlistenToken = tokenListener;
            unlistenFinished = finishedListener;
            unlistenModelSelected = modelSelectedListener;
            unlistenChatSaved = chatSavedListener;

            set({ isListening: true });
            console.log(`[ChatStore] Event listeners active (Gen: ${myGeneration}).`);
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
