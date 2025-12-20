import { create } from 'zustand'
import { invoke, listen } from '../lib/api';
import { 
    ToolCallsPendingEvent, 
    ToolExecutingEvent, 
    ToolResultEvent, 
    ToolLoopFinishedEvent,
    ParsedToolCall,
    approveToolCall,
    rejectToolCall
} from '../lib/tool-calls';

export type ReasoningEffort = 'low' | 'medium' | 'high';

// Operation status types for the status bar
export type OperationType = 'none' | 'downloading' | 'loading' | 'streaming' | 'reloading' | 'indexing';

export interface OperationStatus {
    type: OperationType;
    message: string;
    /** For downloads: current file being downloaded */
    currentFile?: string;
    /** Progress percentage (0-100) for downloads */
    progress?: number;
    /** Whether the operation completed (shows "Complete" briefly) */
    completed?: boolean;
    /** Start time for elapsed timer */
    startTime: number;
}

export interface CachedModel {
    alias: string;
    model_id: string;
}

// Model family for format-specific handling
export type ModelFamily = 'gpt_oss' | 'gemma' | 'phi' | 'granite' | 'generic';

// Tool calling format supported by the model
export type ToolFormat = 'openai' | 'hermes' | 'gemini' | 'granite' | 'text_based';

// Reasoning/thinking output format
export type ReasoningFormat = 'none' | 'think_tags' | 'channel_based' | 'thinking_tags';

export interface ModelInfo {
    id: string;
    family: ModelFamily;
    tool_calling: boolean;
    tool_format: ToolFormat;
    vision: boolean;
    reasoning: boolean;
    reasoning_format: ReasoningFormat;
    max_input_tokens: number;
    max_output_tokens: number;
    supports_tool_calling: boolean;
    supports_temperature: boolean;
    supports_top_p: boolean;
    supports_reasoning_effort: boolean;
}

export interface ChatSummary {
    id: string;
    title: string;
    preview: string;
    score: number;
    pinned: boolean;
    model?: string;
}

// A single tool call execution record for display
export interface ToolCallRecord {
    id: string;
    server: string;
    tool: string;
    arguments: Record<string, unknown>;
    result: string;
    isError: boolean;
    durationMs?: number;
}

// A code execution record for display
export interface CodeExecutionRecord {
    id: string;
    code: string[];
    stdout: string;
    stderr: string;
    success: boolean;
    durationMs: number;
    innerToolCalls: ToolCallRecord[];
}

export interface Message {
    id: string;
    role: 'user' | 'assistant';
    content: string;
    timestamp: number;
    /** Model ID used for this turn (only for assistant messages) */
    model?: string;
    /** System prompt string used for this assistant turn */
    systemPromptText?: string;
    /** Tool calls made during this assistant message */
    toolCalls?: ToolCallRecord[];
    /** Code execution blocks during this assistant message */
    codeExecutions?: CodeExecutionRecord[];
    /** RAG chunks used as context for this assistant message */
    ragChunks?: RagChunk[];
}

export interface RagChunk {
    id: string;
    content: string;
    source_file: string;
    chunk_index: number;
    score: number;
}

export interface RagIndexResult {
    total_chunks: number;
    files_processed: number;
    cache_hits: number;
}

// Tool execution state for UI display
export interface PendingToolApproval {
    approvalKey: string;
    calls: ParsedToolCall[];
    iteration: number;
}

export interface ToolExecutionState {
    currentTool: { 
        server: string; 
        tool: string; 
        arguments?: Record<string, unknown>;
        startTime?: number;
    } | null;
    lastResult: { server: string; tool: string; result: string; isError: boolean } | null;
    totalIterations: number;
    hadToolCalls: boolean;
    /** Last heartbeat timestamp (ms since epoch) while tool runs */
    lastHeartbeatTs?: number;
}

// Code execution state for code_execution tool
export interface CodeExecutionState {
    /** Whether code is currently running */
    isRunning: boolean;
    /** The code being executed */
    currentCode: string[];
    /** Number of inner tool calls made during execution */
    innerToolCalls: number;
    /** Stdout from the execution */
    stdout: string;
    /** Stderr from the execution */
    stderr: string;
    /** Whether the execution succeeded */
    success: boolean | null;
    /** Duration of execution in milliseconds */
    durationMs: number;
}

// Tool search result from tool_search tool
export interface ToolSearchResult {
    name: string;
    description?: string;
    score: number;
    server_id: string;
}

// Tool search state
export interface ToolSearchState {
    /** Whether a search is in progress */
    isSearching: boolean;
    /** The queries used for the search */
    queries: string[];
    /** Results from the search */
    results: ToolSearchResult[];
}

interface ChatState {
    chatMessages: Message[];
    appendChatMessage: (msg: Message) => void;
    chatInputValue: string;
    setChatInputValue: (s: string) => void;
    assistantStreamingActive: boolean;
    setAssistantStreamingActive: (loading: boolean) => void;
    lastStreamActivityTs: number | null;
    setLastStreamActivityTs: (ts: number) => void;
    stopActiveChatGeneration: () => Promise<void>;
    chatGenerationCounter: number;

    currentChatId: string | null;
    setCurrentChatId: (id: string | null) => void;

    // Operation status for status bar (downloads, loads, streaming)
    operationStatus: OperationStatus | null;
    statusBarDismissed: boolean;
    setOperationStatus: (status: OperationStatus | null) => void;
    dismissStatusBar: () => void;
    showStatusBar: () => void;
    // Heartbeat warning (frontend cannot reach backend)
    heartbeatWarningStart: number | null;
    heartbeatWarningMessage: string | null;
    setHeartbeatWarning: (startTime: number | null, message?: string | null) => void;

    // Per-chat streaming tracking (streaming continues to original chat on switch)
    streamingChatId: string | null;
    streamingMessages: Message[]; // Messages for the streaming chat (if different from current)
    setStreamingChatId: (id: string | null) => void;

    // Model loading operations
    loadModel: (modelName: string) => Promise<void>;
    downloadModel: (modelName: string) => Promise<void>;
    getLoadedModels: () => Promise<string[]>;
    loadLaunchOverrides: () => Promise<void>;
    launchOverridesLoaded: boolean;
    launchModelOverride: string | null;
    launchInitialPrompt: string | null;
    launchPromptApplied: boolean;
    markLaunchPromptApplied: () => void;
    sendLaunchPrompt: () => Promise<void>;

    availableModels: string[];
    cachedModels: CachedModel[];
    modelInfo: ModelInfo[];
    currentModel: string;
    isConnecting: boolean;
    hasFetchedCachedModels: boolean; // True after first fetchCachedModels completes
    fetchModels: () => Promise<void>;
    fetchCachedModels: () => Promise<void>;
    fetchModelInfo: () => Promise<void>;
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

    // RAG (Retrieval Augmented Generation) State
    attachedPaths: string[];
    ragIndexedFiles: string[];
    isIndexingRag: boolean;
    isSearchingRag: boolean;
    ragChunkCount: number;
    addAttachment: (path: string) => Promise<void>;
    removeAttachment: (path: string) => void;
    clearAttachments: () => void;
    clearAttachedPaths: () => void;
    processRagDocuments: () => Promise<RagIndexResult | null>;
    searchRagContext: (query: string, limit?: number) => Promise<RagChunk[]>;
    clearRagContext: () => Promise<void>;
    fetchRagIndexedFiles: () => Promise<void>;
    removeRagFile: (sourceFile: string) => Promise<void>;

    // Tool Execution State
    pendingToolApproval: PendingToolApproval | null;
    toolExecution: ToolExecutionState;
    approveCurrentToolCall: () => Promise<void>;
    rejectCurrentToolCall: () => Promise<void>;

    // Code Execution State (for code_execution built-in tool)
    codeExecution: CodeExecutionState;
    
    // Tool Search State (for tool_search built-in tool)
    toolSearch: ToolSearchState;

    // System-initiated chat (for help messages, onboarding, etc.)
    startSystemChat: (assistantMessage: string, title?: string) => void;
}

// Module-level variables to hold unlisten functions
// This ensures they persist even if the store is recreated (though Zustand stores are usually singletons)
let unlistenToken: (() => void) | undefined;
let unlistenFinished: (() => void) | undefined;
let unlistenModelSelected: (() => void) | undefined;
let unlistenChatSaved: (() => void) | undefined;
let unlistenSidebarUpdate: (() => void) | undefined;
let unlistenToolCallsPending: (() => void) | undefined;
let unlistenToolExecuting: (() => void) | undefined;
let unlistenToolHeartbeat: (() => void) | undefined;
let unlistenToolResult: (() => void) | undefined;
let unlistenToolLoopFinished: (() => void) | undefined;
let unlistenDownloadProgress: (() => void) | undefined;
let unlistenLoadComplete: (() => void) | undefined;
let unlistenServiceStopStarted: (() => void) | undefined;
let unlistenServiceStopComplete: (() => void) | undefined;
let unlistenServiceStartStarted: (() => void) | undefined;
let unlistenServiceStartComplete: (() => void) | undefined;
let unlistenServiceRestartStarted: (() => void) | undefined;
let unlistenServiceRestartComplete: (() => void) | undefined;
let unlistenSystemPrompt: (() => void) | undefined;
let unlistenRagProgress: (() => void) | undefined;
let isSettingUp = false; // Guard against async race conditions
let listenerGeneration = 0; // Generation counter to invalidate stale setup calls
let modelFetchPromise: Promise<void> | null = null;
let modelFetchRetryTimer: ReturnType<typeof setTimeout> | null = null;
const MODEL_FETCH_MAX_RETRIES = 3;
const MODEL_FETCH_INITIAL_DELAY_MS = 1000;
let tokenLogChatId: string | null = null;
let tokenLogRecorded = false;
const DEBUG_LOG_ENDPOINT = 'http://127.0.0.1:7242/ingest/ed1dd551-d0f1-4880-9a65-c463a4dc7c0d';

// Relevance search debounce/cancellation state
let relevanceSearchTimeout: ReturnType<typeof setTimeout> | null = null;
let relevanceSearchGeneration = 0; // Incremented on each new search to cancel stale results
const RELEVANCE_SEARCH_DEBOUNCE_MS = 400; // Wait 400ms after typing stops
const RELEVANCE_SEARCH_MIN_LENGTH = 3; // Minimum chars before searching

// Default model to download if no models are available
// Using 'phi-4-mini-instruct' to specifically match the instruct version (not reasoning)
// This matches the alias 'Phi-4-mini-instruct-generic-gpu:5' in the Foundry catalog
const DEFAULT_MODEL_TO_DOWNLOAD = 'phi-4-mini-instruct';

const generateClientChatId = () => {
    const cryptoObj = typeof globalThis !== 'undefined' ? (globalThis as any).crypto : undefined;
    if (cryptoObj && typeof cryptoObj.randomUUID === 'function') {
        return cryptoObj.randomUUID();
    }
    return `chat-${Date.now()}-${Math.floor(Math.random() * 1000)}`;
};

const createChatTitleFromPrompt = (prompt: string) => {
    const cleaned = prompt.trim().replace(/\s+/g, ' ');
    if (!cleaned) {
        return "Untitled Chat";
    }
    const sentenceEnd = cleaned.search(/[.!?]/);
    const base = sentenceEnd > 0 ? cleaned.substring(0, sentenceEnd).trim() : cleaned;
    if (base.length <= 40) {
        return base;
    }
    return `${base.substring(0, 37).trim()}...`;
};

const createChatPreviewFromMessage = (message: string) => {
    const cleaned = message.trim().replace(/\s+/g, ' ');
    if (!cleaned) return "";
    if (cleaned.length <= 80) {
        return cleaned;
    }
    return `${cleaned.substring(0, 77)}...`;
};

// Helper function to initialize models on startup
async function initializeModelsOnStartup(
    get: () => ChatState,
    set: (partial: Partial<ChatState> | ((state: ChatState) => Partial<ChatState>)) => void
) {
    console.log('[ChatStore] Starting model initialization...');
    
    try {
        // Load launch overrides (model / initial prompt) once
        if (!get().launchOverridesLoaded) {
            await get().loadLaunchOverrides();
        }

        // Step 1: Fetch available/cached models
        console.log('[ChatStore] Fetching cached models...');
        await get().fetchCachedModels();
        await get().fetchModels();
        await get().fetchModelInfo();
        
        const state = get();
        const cachedModels = state.cachedModels;
        
        // Sync current model with backend if possible
        try {
            const currentBackendModel = await invoke<ModelInfo | null>('get_current_model');
            if (currentBackendModel) {
                console.log('[ChatStore] Synced current model from backend:', currentBackendModel.id);
                set({ currentModel: currentBackendModel.id });
            }
        } catch (syncError) {
            console.warn('[ChatStore] Failed to sync current model from backend:', syncError);
        }

        if (cachedModels.length === 0) {
            // No models available - attempt auto-download
            // The App.tsx will also show the help chat so users understand what's happening
            console.log('[ChatStore] No cached models found. Attempting auto-download of:', DEFAULT_MODEL_TO_DOWNLOAD);
            
            set({
                operationStatus: {
                    type: 'downloading',
                    message: `Downloading ${DEFAULT_MODEL_TO_DOWNLOAD}...`,
                    progress: 0,
                    startTime: Date.now(),
                },
                statusBarDismissed: false,
                currentModel: 'Downloading...',
            });
            
            try {
                await invoke('download_model', { modelName: DEFAULT_MODEL_TO_DOWNLOAD });
                console.log('[ChatStore] Default model download complete');
                
                // Refresh models after download
                await get().fetchCachedModels();
                await get().fetchModels();
                
                // Now load the model
                const updatedState = get();
                if (updatedState.cachedModels.length > 0) {
                    const modelToLoad = updatedState.cachedModels[0].model_id;
                    console.log('[ChatStore] Loading downloaded model:', modelToLoad);
                    await get().loadModel(modelToLoad);
                } else {
                    // Download succeeded but no models found - unusual
                    set({
                        operationStatus: null,
                        currentModel: 'No models',
                    });
                }
            } catch (downloadError: any) {
                console.error('[ChatStore] Failed to download default model:', downloadError);
                // Clear loading state but show the error in status bar briefly
                set({
                    operationStatus: {
                        type: 'downloading',
                        message: `Auto-download failed. Use: foundry model load ${DEFAULT_MODEL_TO_DOWNLOAD}`,
                        startTime: Date.now(),
                    },
                    currentModel: 'No models',
                });
                // Auto-dismiss error after 10 seconds
                setTimeout(() => {
                    const currentState = get();
                    if (currentState.operationStatus?.message?.includes('Auto-download failed')) {
                        set({ operationStatus: null });
                    }
                }, 10000);
            }
        } else {
            // Models are available - check if any are loaded into VRAM
            console.log('[ChatStore] Found', cachedModels.length, 'cached models. Checking loaded models...');
            
            try {
                const loadedModels = await get().getLoadedModels();
                
                if (loadedModels.length > 0) {
                    // Use the first loaded model
                    const activeModel = loadedModels[0];
                    console.log('[ChatStore] Found loaded model:', activeModel);
                    set({ currentModel: activeModel });
                } else {
                    // No models loaded - load the first cached model
                    const modelToLoad = cachedModels[0].model_id;
                    console.log('[ChatStore] No models loaded. Loading first cached model:', modelToLoad);
                    await get().loadModel(modelToLoad);
                }
            } catch (loadError: any) {
                console.error('[ChatStore] Failed to check/load models:', loadError);
                // Still set the first cached model as current (even if not loaded into VRAM)
                set({ currentModel: cachedModels[0].model_id });
            }
        }

        // Apply model override if provided
        const launchModel = get().launchModelOverride;
        if (launchModel) {
            const current = get().currentModel;
            if (current !== launchModel) {
                console.log('[ChatStore] Applying launch model override:', launchModel);
                try {
                    await get().loadModel(launchModel);
                } catch (e: any) {
                    console.error('[ChatStore] Failed to load launch override model:', e);
                    set({ backendError: `Failed to load model ${launchModel}: ${e?.message || e}` });
                }
            } else {
                console.log('[ChatStore] Launch model override already active:', launchModel);
            }
        }

        // Auto-send initial prompt if provided and not yet applied
        if (get().launchInitialPrompt && !get().launchPromptApplied) {
            try {
                await get().sendLaunchPrompt();
            } catch (e: any) {
                console.error('[ChatStore] Failed to send launch initial prompt:', e);
            }
        }
    } catch (e: any) {
        console.error('[ChatStore] Model initialization error:', e);
        set({ operationStatus: null, currentModel: 'No models' });
    }
}

export const useChatStore = create<ChatState>((set, get) => ({
    chatMessages: [],
    appendChatMessage: (msg) =>
        set((state) => ({ chatMessages: [...state.chatMessages, msg] })),
    chatInputValue: '',
    setChatInputValue: (chatInputValue) => set({ chatInputValue }),
    assistantStreamingActive: false,
    setAssistantStreamingActive: (assistantStreamingActive) =>
        set({ assistantStreamingActive }),
    lastStreamActivityTs: null,
    setLastStreamActivityTs: (ts) => set({ lastStreamActivityTs: ts }),
    chatGenerationCounter: 0,
    stopActiveChatGeneration: async () => {
        console.log('[ChatStore] 🛑 STOP BUTTON PRESSED by user');
        
        // Increment generationId to ignore any incoming tokens from the stopped generation
        const currentGenId = get().chatGenerationCounter;
        console.log('[ChatStore] Current generation to cancel:', currentGenId);
        
        set((state) => ({ 
            assistantStreamingActive: false, 
            chatGenerationCounter: state.chatGenerationCounter + 1,
            streamingChatId: null,
            lastStreamActivityTs: Date.now(),
        }));
        
        try {
            // Cancel the stream - this signals both the agentic loop AND the FoundryActor to stop
            await invoke('cancel_generation', { generationId: currentGenId });
            console.log('[ChatStore] ✅ Cancel signal sent for generation', currentGenId);
        } catch (e) {
            console.error('[ChatStore] ❌ Stop failed:', e);
        }

        // Always request a Foundry service restart after a manual stop
        console.log('[ChatStore] 🔄 Requesting Foundry service restart after stop...');
        set({
            operationStatus: {
                type: 'reloading',
                message: 'Restarting Foundry service after stop...',
                startTime: Date.now(),
            },
            statusBarDismissed: false,
        });

        try {
            await invoke('reload_foundry');
            console.log('[ChatStore] ✅ Reload request sent to backend (waiting for restart events)');
        } catch (e) {
            console.error('[ChatStore] ❌ Failed to request Foundry restart:', e);
            set({
                operationStatus: {
                    type: 'reloading',
                    message: `Failed to restart Foundry service: ${ (e as any)?.message || e }`,
                    startTime: Date.now(),
                },
            });
            setTimeout(() => {
                const currentState = get();
                if (currentState.operationStatus?.type === 'reloading' && !currentState.operationStatus?.completed) {
                    set({ operationStatus: null });
                }
            }, 10000);
        }
    },

    currentChatId: null,
    setCurrentChatId: (id) => set({ currentChatId: id }),

    // Operation status for status bar
    operationStatus: null,
    statusBarDismissed: false,
    setOperationStatus: (status) => set({ operationStatus: status, statusBarDismissed: false }),
    dismissStatusBar: () => set({ statusBarDismissed: true }),
    showStatusBar: () => set({ statusBarDismissed: false }),
    heartbeatWarningStart: null,
    heartbeatWarningMessage: null,
    setHeartbeatWarning: (startTime, message) => set({
        heartbeatWarningStart: startTime,
        heartbeatWarningMessage: message ?? (startTime ? 'Backend unresponsive' : null),
        statusBarDismissed: false,
    }),

    // Per-chat streaming tracking
    streamingChatId: null,
    streamingMessages: [],
    setStreamingChatId: (id) => set({ streamingChatId: id }),

    // Launch overrides (from CLI)
    launchOverridesLoaded: false,
    launchModelOverride: null,
    launchInitialPrompt: null,
    launchPromptApplied: false,
    markLaunchPromptApplied: () => set({ launchPromptApplied: true }),
    loadLaunchOverrides: async () => {
        if (get().launchOverridesLoaded) return;
        try {
            const payload = await invoke<{ model?: string | null; initial_prompt?: string | null }>('get_launch_overrides');
            set({
                launchOverridesLoaded: true,
                launchModelOverride: payload?.model ?? null,
                launchInitialPrompt: payload?.initial_prompt ?? null,
            });
            console.log('[ChatStore] Launch overrides loaded:', payload);
        } catch (e: any) {
            console.error('[ChatStore] Failed to load launch overrides:', e);
            // Mark as loaded to avoid retry loops
            set({ launchOverridesLoaded: true });
        }
    },

    // Model loading operations
    loadModel: async (modelName: string) => {
        const state = get();
        if (state.currentModel === modelName) {
            console.log('[ChatStore] Model already active:', modelName);
            return;
        }

        console.log('[ChatStore] Loading model:', modelName);
        
        // If we have an active chat, start a new one when switching models
        if (state.currentChatId || state.chatMessages.length > 0) {
            console.log('[ChatStore] Switching models, starting new chat');
            set({ currentChatId: null, chatMessages: [] });
        }

        set({
            operationStatus: {
                type: 'loading',
                message: `Loading ${modelName} into VRAM...`,
                startTime: Date.now(),
            },
            statusBarDismissed: false,
        });
        try {
            await invoke('load_model', { modelName });
            set({
                operationStatus: {
                    type: 'loading',
                    message: `${modelName} loaded successfully`,
                    completed: true,
                    startTime: Date.now(),
                },
                currentModel: modelName,
            });
            // Auto-dismiss after 3 seconds
            setTimeout(() => {
                const state = get();
                if (state.operationStatus?.completed) {
                    set({ operationStatus: null });
                }
            }, 3000);
        } catch (e: any) {
            console.error('[ChatStore] Failed to load model:', e);
            set({
                operationStatus: {
                    type: 'loading',
                    message: `Failed to load ${modelName}: ${e.message || e}`,
                    startTime: Date.now(),
                },
                backendError: `Failed to load model: ${e.message || e}`,
            });
        }
    },

    downloadModel: async (modelName: string) => {
        console.log('[ChatStore] Downloading model:', modelName);
        set({
            operationStatus: {
                type: 'downloading',
                message: `Downloading ${modelName}...`,
                progress: 0,
                startTime: Date.now(),
            },
            statusBarDismissed: false,
        });
        try {
            await invoke('download_model', { modelName });
            set({
                operationStatus: {
                    type: 'downloading',
                    message: `${modelName} downloaded successfully`,
                    completed: true,
                    progress: 100,
                    startTime: Date.now(),
                },
            });
            // Refresh cached models
            await get().fetchCachedModels();
            // Auto-dismiss after 3 seconds
            setTimeout(() => {
                const state = get();
                if (state.operationStatus?.completed) {
                    set({ operationStatus: null });
                }
            }, 3000);
        } catch (e: any) {
            console.error('[ChatStore] Failed to download model:', e);
            set({
                operationStatus: {
                    type: 'downloading',
                    message: `Failed to download ${modelName}: ${e.message || e}`,
                    startTime: Date.now(),
                },
                backendError: `Failed to download model: ${e.message || e}`,
            });
        }
    },

    getLoadedModels: async () => {
        try {
            const models = await invoke<string[]>('get_loaded_models');
            console.log('[ChatStore] Loaded models:', models);
            return models;
        } catch (e: any) {
            console.error('[ChatStore] Failed to get loaded models:', e);
            return [];
        }
    },

    sendLaunchPrompt: async () => {
        const state = get();
        if (state.launchPromptApplied) return;
        const rawPrompt = state.launchInitialPrompt;
        if (!rawPrompt || state.chatMessages.length > 0) {
            set({ launchPromptApplied: true });
            return;
        }
        const text = rawPrompt.trim();
        if (!text) {
            set({ launchPromptApplied: true });
            return;
        }

        const chatId = generateClientChatId();
        const derivedTitle = createChatTitleFromPrompt(text);
        const preview = createChatPreviewFromMessage(text);
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
        });

        try {
            const returnedChatId = await invoke<string>('chat', {
                chatId,
                title: derivedTitle,
                message: text,
                history: [],
                reasoningEffort: state.reasoningEffort,
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
                };
            });
        } finally {
            set({ launchPromptApplied: true });
        }
    },

    availableModels: [],
    cachedModels: [],
    modelInfo: [],
    currentModel: 'Loading...',
    isConnecting: false,
    hasFetchedCachedModels: false,
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
            let lastConnectionError: string | null = null;

            // Returns: 'success' | 'empty' | 'error'
            const attemptFetch = async (): Promise<'success' | 'empty' | 'error'> => {
                try {
                    console.log(`[ChatStore] Fetching models (attempt ${retryCount + 1}/${MODEL_FETCH_MAX_RETRIES})...`);
                    const models = await invoke<string[]>('get_models');
                    
                    if (models.length > 0) {
                        set({ availableModels: models, backendError: null, isConnecting: false });
                        if (get().currentModel === 'Loading...' || get().currentModel === 'Unavailable') {
                            set({ currentModel: models[0] });
                        }
                        console.log(`[ChatStore] Successfully fetched ${models.length} model(s)`);
                        return 'success';
                    } else {
                        // Connection succeeded, but no models available - this is NOT a connection error
                        console.log("[ChatStore] Connected to Foundry, but no models available");
                        return 'empty';
                    }
                } catch (e: any) {
                    console.error(`[ChatStore] Fetch models attempt ${retryCount + 1} failed:`, e);
                    lastConnectionError = e.message || String(e);
                    return 'error';
                }
            };

            // Initial attempt
            const initialResult = await attemptFetch();
            if (initialResult === 'success') {
                return;
            }
            if (initialResult === 'empty') {
                // Connected but no models - don't retry, just set empty state
                console.log("[ChatStore] Foundry connected but no models - download required");
                set({ availableModels: [], backendError: null, isConnecting: false, currentModel: 'No models' });
                return;
            }

            // Only retry on actual connection errors
            while (retryCount < MODEL_FETCH_MAX_RETRIES - 1) {
                retryCount++;
                console.log(`[ChatStore] Retrying in ${delay}ms...`);
                
                await new Promise(resolve => {
                    modelFetchRetryTimer = setTimeout(resolve, delay);
                });
                modelFetchRetryTimer = null;

                const result = await attemptFetch();
                if (result === 'success') {
                    return;
                }
                if (result === 'empty') {
                    // Connected now - no need to keep retrying
                    console.log("[ChatStore] Foundry connected but no models - download required");
                    set({ availableModels: [], backendError: null, isConnecting: false, currentModel: 'No models' });
                    return;
                }

                // Exponential backoff with max of 10 seconds
                delay = Math.min(delay * 1.5, 10000);
            }

            // All retries failed with actual connection errors
            console.error(`[ChatStore] Failed to connect to Foundry after ${MODEL_FETCH_MAX_RETRIES} attempts`);
            set({ 
                backendError: `Failed to connect to Foundry. Please ensure Foundry is running and try again.${lastConnectionError ? ` (${lastConnectionError})` : ''}`,
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
            set({ cachedModels: cached, hasFetchedCachedModels: true });
            console.log(`[ChatStore] Found ${cached.length} cached model(s)`);
        } catch (e: any) {
            console.error('[ChatStore] Failed to fetch cached models:', e);
            // Still mark as fetched so we don't block forever
            set({ hasFetchedCachedModels: true });
        }
    },
    fetchModelInfo: async () => {
        try {
            console.log('[ChatStore] Fetching model info with capabilities...');
            const info = await invoke<ModelInfo[]>('get_model_info');
            set({ modelInfo: info });
            console.log(`[ChatStore] Found ${info.length} model(s) with capabilities:`, 
                info.map(m => `${m.id}: toolCalling=${m.tool_calling}`).join(', '));
        } catch (e: any) {
            console.error('[ChatStore] Failed to fetch model info:', e);
        }
    },
    setModel: async (model) => {
        try {
            const state = get();
            if (state.currentModel === model) return;

            await invoke('set_model', { model });
            set({ currentModel: model });

            // Start new chat on model change
            if (state.currentChatId || state.chatMessages.length > 0) {
                console.log('[ChatStore] Model set, starting new chat');
                set({ currentChatId: null, chatMessages: [] });
            }
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
            const state = get();
            const currentChatId = state.currentChatId;
            const streamingChatId = state.streamingChatId;
            
            // If we're switching away from a streaming chat, save current messages to streamingMessages
            if (streamingChatId && streamingChatId === currentChatId && id !== currentChatId) {
                console.log(`[ChatStore] Switching away from streaming chat ${currentChatId?.slice(0, 8)}, saving messages`);
                set({ streamingMessages: [...state.chatMessages] });
            }
            
            // If we're switching to the streaming chat, restore messages from streamingMessages
            if (streamingChatId && streamingChatId === id && state.streamingMessages.length > 0) {
                console.log(`[ChatStore] Switching to streaming chat ${id.slice(0, 8)}, restoring messages`);
                set({ 
                    chatMessages: state.streamingMessages, 
                    currentChatId: id, 
                    streamingMessages: [], 
                    backendError: null 
                });
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
                set({ chatMessages: processedMessages, currentChatId: id, backendError: null });
            } else {
                set({ chatMessages: [], currentChatId: id });
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
                set({ chatMessages: [], currentChatId: null });
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
                const snapshot = get();
                const targetChatId = snapshot.streamingChatId || snapshot.currentChatId;
                if (targetChatId && (!tokenLogRecorded || tokenLogChatId !== targetChatId)) {
                    tokenLogRecorded = true;
                    tokenLogChatId = targetChatId;
                    // #region agent log
                    fetch(DEBUG_LOG_ENDPOINT,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H1',location:'chat-store.ts:1122',message:'first_token_for_stream',data:{chatId:targetChatId,assistantStreamingActive:snapshot.assistantStreamingActive,generationCounter:snapshot.chatGenerationCounter},timestamp:Date.now()})}).catch(()=>{});
                    // #endregion
                }
                set((state) => {
                    // Ignore tokens if generation was stopped
                    if (!state.assistantStreamingActive) {
                        return state;
                    }
                    const now = Date.now();
                    
                    // Check if we're streaming to a different chat than currently displayed
                    const targetChatId = state.streamingChatId;
                    const isStreamingToOtherChat = targetChatId && targetChatId !== state.currentChatId;
                    
                    if (isStreamingToOtherChat) {
                        // Append token to streamingMessages instead of current messages
                        const lastMsg = state.streamingMessages[state.streamingMessages.length - 1];
                        if (lastMsg && lastMsg.role === 'assistant') {
                            const newStreamingMessages = [...state.streamingMessages];
                            newStreamingMessages[newStreamingMessages.length - 1] = {
                                ...lastMsg,
                                content: lastMsg.content + event.payload
                            };
                            return { streamingMessages: newStreamingMessages, lastStreamActivityTs: now };
                        }
                        return { ...state, lastStreamActivityTs: now };
                    }
                    
                    // Normal case: streaming to current chat
                    const lastMsg = state.chatMessages[state.chatMessages.length - 1];
                    // Only append if the last message is from assistant
                    if (lastMsg && lastMsg.role === 'assistant') {
                        const newMessages = [...state.chatMessages];
                        newMessages[newMessages.length - 1] = {
                            ...lastMsg,
                            content: lastMsg.content + event.payload
                        };
                        return { chatMessages: newMessages, lastStreamActivityTs: now };
                    }
                    return { ...state, lastStreamActivityTs: now };
                });
            });

            const finishedListener = await listen('chat-finished', () => {
                const snapshot = get();
                tokenLogRecorded = false;
                tokenLogChatId = null;
                // #region agent log
                fetch(DEBUG_LOG_ENDPOINT,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H1',location:'chat-store.ts:1163',message:'chat_finished_event',data:{streamingChatId:snapshot.streamingChatId,assistantStreamingActive:snapshot.assistantStreamingActive,toolCurrent:snapshot.toolExecution.currentTool ? {server:snapshot.toolExecution.currentTool.server,tool:snapshot.toolExecution.currentTool.tool} : null,pendingToolApproval:!!snapshot.pendingToolApproval,lastStreamActivityTs:snapshot.lastStreamActivityTs},timestamp:Date.now()})}).catch(()=>{});
                // #endregion
                // If we were streaming to a different chat, the messages are in streamingMessages
                // They should have been saved to LanceDB by the backend, so we don't need to do anything special
                set({ 
                    assistantStreamingActive: false,
                    streamingChatId: null,
                    streamingMessages: [],
                    operationStatus: null,
                    lastStreamActivityTs: Date.now(),
                });
            });
            
            const systemPromptListener = await listen<{ chat_id?: string; generation_id?: number; prompt: string }>('system-prompt', (event) => {
                set((state) => {
                    const prompt = event.payload?.prompt;
                    if (!prompt) return state;

                    const payloadChatId = event.payload?.chat_id;
                    const streamingTarget = state.streamingChatId;
                    const streamingMatches = streamingTarget && payloadChatId && streamingTarget === payloadChatId;

                    const applyPrompt = (messages: Message[]) => {
                        if (!messages.length) return messages;
                        const last = messages[messages.length - 1];
                        if (last.role !== 'assistant') return messages;
                        const updated = [...messages];
                        updated[updated.length - 1] = { ...last, systemPromptText: prompt };
                        return updated;
                    };

                    if (streamingMatches && state.streamingMessages.length > 0) {
                        return { streamingMessages: applyPrompt(state.streamingMessages) };
                    }

                    if (!payloadChatId || payloadChatId === state.currentChatId) {
                        return { chatMessages: applyPrompt(state.chatMessages) };
                    }

                    return state;
                });
            });
            
            // Model download progress listener
            const downloadProgressListener = await listen<{ file: string; progress: number }>('model-download-progress', (event) => {
                console.log(`[ChatStore] Download progress: ${event.payload.file} - ${event.payload.progress}%`);
                set((state) => ({
                    operationStatus: state.operationStatus?.type === 'downloading' ? {
                        ...state.operationStatus,
                        currentFile: event.payload.file,
                        progress: event.payload.progress,
                    } : state.operationStatus,
                }));
            });
            
            // Model load complete listener
            const loadCompleteListener = await listen<{ model: string; success: boolean; error?: string }>('model-load-complete', (event) => {
                console.log(`[ChatStore] Model load complete: ${event.payload.model}, success=${event.payload.success}`);
                if (event.payload.success) {
                    set({
                        operationStatus: {
                            type: 'loading',
                            message: `${event.payload.model} loaded successfully`,
                            completed: true,
                            startTime: Date.now(),
                        },
                        currentModel: event.payload.model,
                    });
                    // Auto-dismiss after 3 seconds
                    setTimeout(() => {
                        const currentState = get();
                        if (currentState.operationStatus?.completed) {
                            set({ operationStatus: null });
                        }
                    }, 3000);
                } else {
                    set({
                        operationStatus: {
                            type: 'loading',
                            message: `Failed to load ${event.payload.model}: ${event.payload.error || 'Unknown error'}`,
                            startTime: Date.now(),
                        },
                    });
                }
            });

            // RAG progress listener
            const ragProgressListener = await listen<{ total_chunks: number; processed_chunks: number; current_file: string; is_complete: boolean }>('rag-progress', (event) => {
                const { total_chunks, processed_chunks, current_file, is_complete } = event.payload;
                const progress = total_chunks > 0 ? (processed_chunks / total_chunks) * 100 : 0;
                
                set((state) => ({
                    operationStatus: {
                        type: 'indexing',
                        message: `Processing Chunk ${processed_chunks} / ${total_chunks}`,
                        currentFile: current_file,
                        progress,
                        completed: is_complete,
                        startTime: state.operationStatus?.startTime || Date.now(),
                    },
                    ragChunkCount: processed_chunks,
                }));

                if (is_complete) {
                    setTimeout(() => {
                        const state = get();
                        if (state.operationStatus?.completed && state.operationStatus?.type === 'indexing') {
                            set({ operationStatus: null });
                        }
                    }, 3000);
                }
            });

            const modelSelectedListener = await listen<string>('model-selected', (event) => {
                set({ currentModel: event.payload });
            });
            
            const chatSavedListener = await listen<string>('chat-saved', async (event) => {
                const chatId = event.payload;
                console.log(`[ChatStore] chat-saved event received for: ${chatId.slice(0, 8)}...`);
                // #region agent log
                fetch(DEBUG_LOG_ENDPOINT,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H2',location:'chat-store.ts:chatSavedListener',message:'chat_saved_event_start',data:{chatId:chatId.slice(0,8)},timestamp:Date.now()})}).catch(()=>{});
                // #endregion
                
                // The chat is already in history via upsertHistoryEntry() called when the message was sent.
                // We just need to clear the pending flag - no need to re-fetch everything from the backend.
                get().clearPendingSummary(chatId);
                // #region agent log
                fetch(DEBUG_LOG_ENDPOINT,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H2',location:'chat-store.ts:chatSavedListener',message:'chat_saved_event_end',data:{chatId:chatId.slice(0,8)},timestamp:Date.now()})}).catch(()=>{});
                // #endregion
                console.log(`[ChatStore] Cleared pending summary for ${chatId.slice(0, 8)}`);
            });

            const sidebarUpdateListener = await listen<ChatSummary[]>('sidebar-update', (event) => {
                // Only apply if we're still searching (not cancelled)
                if (get().isSearchingRelevance) {
                    set({ relevanceResults: event.payload, isSearchingRelevance: false });
                }
            });

            // Tool execution event listeners
            const toolCallsPendingListener = await listen<ToolCallsPendingEvent>('tool-calls-pending', (event) => {
                console.log(`[ChatStore] Tool calls pending: ${event.payload.approval_key}`, event.payload.calls);
                set({
                    pendingToolApproval: {
                        approvalKey: event.payload.approval_key,
                        calls: event.payload.calls,
                        iteration: event.payload.iteration,
                    }
                });
            });

            const toolExecutingListener = await listen<ToolExecutingEvent>('tool-executing', (event) => {
                const { server, tool, arguments: payloadArgs } = event.payload;
                const toolName = tool;
                if (toolName === 'python_execution') {
                    const codeLines = Array.isArray((payloadArgs as any)?.code)
                        ? (payloadArgs as any).code.length
                        : undefined;
                    console.info(`[ChatStore] 🐍 python_execution triggered on ${server} (code_lines=${codeLines ?? 'unknown'})`);
                } else {
                    console.log(`[ChatStore] Tool executing: ${server}::${toolName}`);
                }
                const displayName = toolName === 'python_execution' 
                    ? 'Running Python code...' 
                    : toolName === 'tool_search'
                    ? 'Searching for tools...'
                    : `Executing ${toolName}...`;
                // #region agent log
                fetch(DEBUG_LOG_ENDPOINT,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H2',location:'chat-store.ts:1291',message:'tool_executing',data:{server,tool:toolName,argumentKeys:Object.keys(payloadArgs || {}),streamingChatId:get().streamingChatId,assistantStreamingActive:get().assistantStreamingActive},timestamp:Date.now()})}).catch(()=>{});
                // #endregion
                const scheduleUpdate = () => set((state) => ({
                    toolExecution: {
                        ...state.toolExecution,
                        currentTool: { 
                            server, 
                            tool: toolName,
                            arguments: payloadArgs,
                            startTime: Date.now(),
                        },
                    },
                    // Update operation status to show tool execution
                    operationStatus: {
                        type: 'streaming',
                        message: displayName,
                        startTime: state.operationStatus?.startTime || Date.now(),
                    },
                    statusBarDismissed: false,
                    lastStreamActivityTs: Date.now(),
                }));

                if (typeof queueMicrotask === 'function') {
                    queueMicrotask(scheduleUpdate);
                } else {
                    setTimeout(scheduleUpdate, 0);
                }
            });

            const toolHeartbeatListener = await listen<{ server: string; tool: string; elapsed_ms: number; beat: number }>('tool-heartbeat', (event) => {
                set((state) => {
                    const current = state.toolExecution.currentTool;
                    if (!current) return state;
                    if (current.server !== event.payload.server || current.tool !== event.payload.tool) {
                        return state;
                    }
                    return {
                        toolExecution: {
                            ...state.toolExecution,
                            lastHeartbeatTs: Date.now(),
                        },
                        lastStreamActivityTs: Date.now(),
                    };
                });
            });

            const toolResultListener = await listen<ToolResultEvent>('tool-result', (event) => {
                console.log(`[ChatStore] Tool result: ${event.payload.server}::${event.payload.tool}, error=${event.payload.is_error}`);
                // #region agent log
                fetch(DEBUG_LOG_ENDPOINT,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H2',location:'chat-store.ts:1341',message:'tool_result',data:{server:event.payload.server,tool:event.payload.tool,isError:event.payload.is_error},timestamp:Date.now()})}).catch(()=>{});
                // #endregion
                set((state) => {
                    // Calculate duration if we have a start time
                    const startTime = state.toolExecution.currentTool?.startTime;
                    const durationMs = startTime ? Date.now() - startTime : undefined;
                    
                    // Create tool call record
                    const toolCallRecord: ToolCallRecord = {
                        id: `tool-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
                        server: event.payload.server,
                        tool: event.payload.tool,
                        arguments: state.toolExecution.currentTool?.arguments || {},
                        result: event.payload.result,
                        isError: event.payload.is_error,
                        durationMs,
                    };
                    
                    // Append to last assistant message's toolCalls
                    const newMessages = [...state.chatMessages];
                    const lastIdx = newMessages.length - 1;
                    if (lastIdx >= 0 && newMessages[lastIdx].role === 'assistant') {
                        const existingToolCalls = newMessages[lastIdx].toolCalls || [];
                        newMessages[lastIdx] = {
                            ...newMessages[lastIdx],
                            toolCalls: [...existingToolCalls, toolCallRecord],
                        };
                    }
                    
                    // Update status bar: show error briefly or revert to generating
                    const newOperationStatus = event.payload.is_error
                        ? {
                            type: 'streaming' as const,
                            message: `Tool error: ${event.payload.tool} - retrying...`,
                            startTime: state.operationStatus?.startTime || Date.now(),
                        }
                        : {
                            type: 'streaming' as const,
                            message: 'Generating response...',
                            startTime: state.operationStatus?.startTime || Date.now(),
                        };
                    
                    return {
                        chatMessages: newMessages,
                        operationStatus: newOperationStatus,
                        toolExecution: {
                            ...state.toolExecution,
                            currentTool: null,
                            lastResult: {
                                server: event.payload.server,
                                tool: event.payload.tool,
                                result: event.payload.result,
                                isError: event.payload.is_error,
                            },
                        },
                        lastStreamActivityTs: Date.now(),
                    };
                });
            });

            const toolLoopFinishedListener = await listen<ToolLoopFinishedEvent>('tool-loop-finished', (event) => {
                console.log(`[ChatStore] Tool loop finished: ${event.payload.iterations} iterations, hadToolCalls=${event.payload.had_tool_calls}`);
                set((state) => ({
                    toolExecution: {
                        ...state.toolExecution,
                        currentTool: null,
                        totalIterations: event.payload.iterations,
                        hadToolCalls: event.payload.had_tool_calls,
                    },
                    pendingToolApproval: null, // Clear any pending approval
                    lastStreamActivityTs: Date.now(),
                }));
            });

            // Service restart listeners (stop/start granularity)
            const serviceStopStartedListener = await listen<{ message: string }>('service-stop-started', (event) => {
                console.log(`[ChatStore] Service stop started: ${event.payload.message}`);
                set({
                    operationStatus: {
                        type: 'reloading',
                        message: event.payload.message || 'Stopping Foundry service...',
                        startTime: Date.now(),
                    },
                    statusBarDismissed: false,
                });
            });

            const serviceStopCompleteListener = await listen<{ success: boolean; message?: string; error?: string }>('service-stop-complete', (event) => {
                console.log(`[ChatStore] Service stop complete: success=${event.payload.success}`);
                const baseStart = get().operationStatus?.startTime || Date.now();
                set({
                    operationStatus: {
                        type: 'reloading',
                        message: event.payload.success
                            ? (event.payload.message || 'Service stopped')
                            : `Service stop failed: ${event.payload.error || 'Unknown error'}`,
                        startTime: baseStart,
                        completed: event.payload.success ? false : undefined,
                    },
                    statusBarDismissed: false,
                });
            });

            const serviceStartStartedListener = await listen<{ message: string }>('service-start-started', (event) => {
                console.log(`[ChatStore] Service start started: ${event.payload.message}`);
                const baseStart = get().operationStatus?.startTime || Date.now();
                set({
                    operationStatus: {
                        type: 'reloading',
                        message: event.payload.message || 'Starting Foundry service...',
                        startTime: baseStart,
                    },
                    statusBarDismissed: false,
                });
            });

            const serviceStartCompleteListener = await listen<{ success: boolean; message?: string; error?: string }>('service-start-complete', (event) => {
                console.log(`[ChatStore] Service start complete: success=${event.payload.success}`);
                const baseStart = get().operationStatus?.startTime || Date.now();
                set({
                    operationStatus: {
                        type: 'reloading',
                        message: event.payload.success
                            ? (event.payload.message || 'Service started')
                            : `Service start failed: ${event.payload.error || 'Unknown error'}`,
                        startTime: baseStart,
                        completed: event.payload.success ? false : undefined,
                    },
                    statusBarDismissed: false,
                });
            });

            const serviceRestartStartedListener = await listen<{ message: string }>('service-restart-started', (event) => {
                console.log(`[ChatStore] Service restart started: ${event.payload.message}`);
                set({
                    operationStatus: {
                        type: 'reloading',
                        message: event.payload.message,
                        startTime: Date.now(),
                    },
                });
            });

            const serviceRestartCompleteListener = await listen<{ success: boolean; message?: string; error?: string }>('service-restart-complete', (event) => {
                console.log(`[ChatStore] Service restart complete: success=${event.payload.success}`);
                const baseStart = get().operationStatus?.startTime || Date.now();
                if (event.payload.success) {
                    set({
                        operationStatus: {
                            type: 'reloading',
                            message: event.payload.message || 'Service restarted successfully',
                            completed: true,
                            startTime: baseStart,
                        },
                    });
                    // Auto-dismiss after 3 seconds
                    setTimeout(() => {
                        const currentState = get();
                        if (currentState.operationStatus?.completed && currentState.operationStatus?.type === 'reloading') {
                            set({ operationStatus: null });
                        }
                    }, 3000);
                } else {
                    set({
                        operationStatus: {
                            type: 'reloading',
                            message: `Service restart failed: ${event.payload.error || 'Unknown error'}`,
                            startTime: baseStart,
                        },
                    });
                    // Auto-dismiss errors after 10 seconds
                    setTimeout(() => {
                        const currentState = get();
                        if (currentState.operationStatus?.type === 'reloading' && !currentState.operationStatus?.completed) {
                            set({ operationStatus: null });
                        }
                    }, 10000);
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
                toolCallsPendingListener();
                toolExecutingListener();
                toolResultListener();
                toolLoopFinishedListener();
                systemPromptListener();
                downloadProgressListener();
                loadCompleteListener();
                ragProgressListener();
                serviceStopStartedListener();
                serviceStopCompleteListener();
                serviceStartStartedListener();
                serviceStartCompleteListener();
                serviceRestartStartedListener();
                serviceRestartCompleteListener();
                isSettingUp = false;
                return;
            }

            // Assign to module variables
            unlistenToken = tokenListener;
            unlistenFinished = finishedListener;
            unlistenModelSelected = modelSelectedListener;
            unlistenToolCallsPending = toolCallsPendingListener;
            unlistenToolExecuting = toolExecutingListener;
            unlistenToolHeartbeat = toolHeartbeatListener;
            unlistenToolResult = toolResultListener;
            unlistenToolLoopFinished = toolLoopFinishedListener;
            unlistenSystemPrompt = systemPromptListener;
            unlistenChatSaved = chatSavedListener;
            unlistenSidebarUpdate = sidebarUpdateListener;
            unlistenDownloadProgress = downloadProgressListener;
            unlistenLoadComplete = loadCompleteListener;
            unlistenRagProgress = ragProgressListener;
            unlistenServiceStopStarted = serviceStopStartedListener;
            unlistenServiceStopComplete = serviceStopCompleteListener;
            unlistenServiceStartStarted = serviceStartStartedListener;
            unlistenServiceStartComplete = serviceStartCompleteListener;
            unlistenServiceRestartStarted = serviceRestartStartedListener;
            unlistenServiceRestartComplete = serviceRestartCompleteListener;

            set({ isListening: true });
            console.log(`[ChatStore] Event listeners active (Gen: ${myGeneration}).`);
            
            // Initialize models on startup
            initializeModelsOnStartup(get, set).catch((e) => {
                console.error("[ChatStore] Model initialization failed:", e);
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
        if (unlistenToolCallsPending) {
            unlistenToolCallsPending();
            unlistenToolCallsPending = undefined;
        }
        if (unlistenToolExecuting) {
            unlistenToolExecuting();
            unlistenToolExecuting = undefined;
        }
        if (unlistenToolHeartbeat) {
            unlistenToolHeartbeat();
            unlistenToolHeartbeat = undefined;
        }
        if (unlistenToolResult) {
            unlistenToolResult();
            unlistenToolResult = undefined;
        }
        if (unlistenToolLoopFinished) {
            unlistenToolLoopFinished();
            unlistenToolLoopFinished = undefined;
        }
        if (unlistenSystemPrompt) {
            unlistenSystemPrompt();
            unlistenSystemPrompt = undefined;
        }
        if (unlistenDownloadProgress) {
            unlistenDownloadProgress();
            unlistenDownloadProgress = undefined;
        }
        if (unlistenLoadComplete) {
            unlistenLoadComplete();
            unlistenLoadComplete = undefined;
        }
        if (unlistenRagProgress) {
            unlistenRagProgress();
            unlistenRagProgress = undefined;
        }
        if (unlistenServiceStopStarted) {
            unlistenServiceStopStarted();
            unlistenServiceStopStarted = undefined;
        }
        if (unlistenServiceStopComplete) {
            unlistenServiceStopComplete();
            unlistenServiceStopComplete = undefined;
        }
        if (unlistenServiceStartStarted) {
            unlistenServiceStartStarted();
            unlistenServiceStartStarted = undefined;
        }
        if (unlistenServiceStartComplete) {
            unlistenServiceStartComplete();
            unlistenServiceStartComplete = undefined;
        }
        if (unlistenServiceRestartStarted) {
            unlistenServiceRestartStarted();
            unlistenServiceRestartStarted = undefined;
        }
        if (unlistenServiceRestartComplete) {
            unlistenServiceRestartComplete();
            unlistenServiceRestartComplete = undefined;
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
    setEditorContent: (content, language) => set({ editorContent: content, editorLanguage: language, isEditorOpen: true }),

    // RAG (Retrieval Augmented Generation) State
    attachedPaths: [],
    ragIndexedFiles: [],
    isIndexingRag: false,
    isSearchingRag: false,
    ragChunkCount: 0,
    
    addAttachment: async (path: string) => {
        const state = get();
        // Avoid duplicates
        if (state.attachedPaths.includes(path)) {
            return;
        }
        console.log(`[ChatStore] Adding attachment and indexing immediately: ${path}`);
        
        // Add path to attachedPaths
        set((s) => ({ attachedPaths: [...s.attachedPaths, path] }));
        
        // Immediately trigger indexing
        set({ 
            isIndexingRag: true,
            operationStatus: {
                type: 'indexing',
                message: 'Preparing documents...',
                startTime: Date.now(),
            }
        });
        try {
            const result = await invoke<RagIndexResult>('process_rag_documents', { paths: get().attachedPaths });
            console.log(`[ChatStore] RAG indexing complete: ${result.total_chunks} chunks from ${result.files_processed} files`);
            set({ ragChunkCount: result.total_chunks, isIndexingRag: false, attachedPaths: [] });
            
            // Fetch updated list of indexed files
            await get().fetchRagIndexedFiles();
        } catch (e: any) {
            console.error('[ChatStore] RAG processing failed:', e);
            set({ isIndexingRag: false });
        }
    },
    
    removeAttachment: (path: string) => set((state) => {
        console.log(`[ChatStore] Removing attachment: ${path}`);
        return { attachedPaths: state.attachedPaths.filter(p => p !== path) };
    }),
    
    clearAttachments: () => {
        console.log('[ChatStore] Clearing all attachments');
        set({ attachedPaths: [], ragChunkCount: 0, ragIndexedFiles: [] });
    },
    
    clearAttachedPaths: () => {
        console.log('[ChatStore] Clearing attachment paths (preserving RAG context)');
        set({ attachedPaths: [] });
    },
    
    processRagDocuments: async () => {
        const paths = get().attachedPaths;
        if (paths.length === 0) {
            return null;
        }
        
        console.log(`[ChatStore] Processing ${paths.length} RAG documents...`);
        set({ 
            isIndexingRag: true,
            operationStatus: {
                type: 'indexing',
                message: 'Preparing documents...',
                startTime: Date.now(),
            }
        });
        
        try {
            const result = await invoke<RagIndexResult>('process_rag_documents', { paths });
            console.log(`[ChatStore] RAG indexing complete: ${result.total_chunks} chunks from ${result.files_processed} files`);
            set({ ragChunkCount: result.total_chunks, isIndexingRag: false });
            return result;
        } catch (e: any) {
            console.error('[ChatStore] RAG processing failed:', e);
            set({ isIndexingRag: false });
            return null;
        }
    },
    
    searchRagContext: async (query: string, limit: number = 5) => {
        console.log(`[ChatStore] Searching RAG context for: "${query.slice(0, 50)}..."`);
        set({ isSearchingRag: true });
        
        try {
            const chunks = await invoke<RagChunk[]>('search_rag_context', { query, limit });
            console.log(`[ChatStore] Found ${chunks.length} relevant chunks`);
            set({ isSearchingRag: false });
            return chunks;
        } catch (e: any) {
            console.error('[ChatStore] RAG search failed:', e);
            set({ isSearchingRag: false });
            return [];
        }
    },
    
    clearRagContext: async () => {
        console.log('[ChatStore] Clearing RAG context');
        try {
            await invoke<boolean>('clear_rag_context');
            set({ attachedPaths: [], ragChunkCount: 0, ragIndexedFiles: [] });
        } catch (e: any) {
            console.error('[ChatStore] Failed to clear RAG context:', e);
        }
    },
    
    fetchRagIndexedFiles: async () => {
        try {
            const files = await invoke<string[]>('get_rag_indexed_files');
            console.log(`[ChatStore] Fetched ${files.length} indexed RAG files`);
            set({ ragIndexedFiles: files });
        } catch (e: any) {
            console.error('[ChatStore] Failed to fetch RAG indexed files:', e);
        }
    },
    
    removeRagFile: async (sourceFile: string) => {
        console.log(`[ChatStore] Removing RAG file: ${sourceFile}`);
        try {
            const result = await invoke<{ chunks_removed: number; remaining_chunks: number }>('remove_rag_file', { sourceFile });
            console.log(`[ChatStore] Removed ${result.chunks_removed} chunks, ${result.remaining_chunks} remaining`);
            set({ ragChunkCount: result.remaining_chunks });
            // Refresh the indexed files list
            await get().fetchRagIndexedFiles();
        } catch (e: any) {
            console.error('[ChatStore] Failed to remove RAG file:', e);
        }
    },

    // Tool Execution State
    pendingToolApproval: null,
    toolExecution: {
        currentTool: null,
        lastResult: null,
        totalIterations: 0,
        hadToolCalls: false,
        lastHeartbeatTs: undefined,
    },
    
    approveCurrentToolCall: async () => {
        const pending = get().pendingToolApproval;
        if (!pending) {
            console.warn('[ChatStore] No pending tool approval to approve');
            return;
        }
        
        console.log(`[ChatStore] Approving tool call: ${pending.approvalKey}`);
        const success = await approveToolCall(pending.approvalKey);
        if (success) {
            set({ pendingToolApproval: null });
        }
    },
    
    rejectCurrentToolCall: async () => {
        const pending = get().pendingToolApproval;
        if (!pending) {
            console.warn('[ChatStore] No pending tool approval to reject');
            return;
        }
        
        console.log(`[ChatStore] Rejecting tool call: ${pending.approvalKey}`);
        const success = await rejectToolCall(pending.approvalKey);
        if (success) {
            set({ pendingToolApproval: null });
        }
    },

    // Code Execution State (for code_execution built-in tool)
    codeExecution: {
        isRunning: false,
        currentCode: [],
        innerToolCalls: 0,
        stdout: '',
        stderr: '',
        success: null,
        durationMs: 0,
    },
    
    // Tool Search State (for tool_search built-in tool)
    toolSearch: {
        isSearching: false,
        queries: [],
        results: [],
    },

    // System-initiated chat for help messages
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
        });
    }
}))
