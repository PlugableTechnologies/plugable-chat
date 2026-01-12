import type { StateCreator } from 'zustand';
import { invoke, listen } from '../../lib/api';
import { 
    ToolCallsPendingEvent, 
    ToolExecutingEvent, 
    ToolResultEvent, 
    ToolLoopFinishedEvent,
} from '../../lib/tool-calls';
import type { 
    ChatSummary, 
    Message, 
    ToolCallRecord,
    ModelInfo,
    OperationStatus,
} from './types';
import { parseFoundryModelStateEvent } from './helpers';
import { DEFAULT_MODEL_TO_DOWNLOAD } from './constants';

// Helper to log to backend terminal for debugging
const logToBackend = (message: string) => {
    invoke('log_to_terminal', { message }).catch(() => {});
};

// Module-level variables to hold unlisten functions
// This ensures they persist even if the store is recreated (though Zustand stores are usually singletons)
let unlistenToken: (() => void) | undefined;
let unlistenFinished: (() => void) | undefined;
let unlistenChatError: (() => void) | undefined;
let unlistenChatWarning: (() => void) | undefined;
let unlistenModelSelected: (() => void) | undefined;
let unlistenToolBlocked: (() => void) | undefined;
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
let unlistenModelStuck: (() => void) | undefined;
let unlistenModelFallback: (() => void) | undefined;
let unlistenEmbeddingInit: (() => void) | undefined;
let unlistenChatStreamStatus: (() => void) | undefined;
let unlistenAvailableModelsChanged: (() => void) | undefined;
let unlistenModelStateChanged: (() => void) | undefined;
let unlistenStartupProgress: (() => void) | undefined;
let isSettingUp = false; // Guard against async race conditions
let listenerGenerationCounter = 0; // Generation counter to invalidate stale setup calls
let hasInitializedRagContext = false; // Only clear RAG context once on true app startup
let tokenLogChatId: string | null = null;
let tokenLogRecorded = false;

// Slice dependencies (what we need from the combined state)
interface ListenerSliceDeps {
    // Message state
    chatMessages: Message[];
    streamingMessages: Message[];
    streamingChatId: string | null;
    currentChatId: string | null;
    assistantStreamingActive: boolean;
    lastStreamActivityTs: number | null;
    
    // Operation status
    operationStatus: OperationStatus | null;
    
    // Model state
    currentModel: string;
    cachedModels: { model_id: string; alias: string }[];
    
    // RAG state
    isIndexingRag: boolean;
    ragChunkCount: number;
    attachedPaths: string[];
    ragIndexedFiles: string[];
    
    // Tool execution
    toolExecution: any;
    
    // History
    isSearchingRelevance: boolean;
    
    // Startup/handshake state
    handshakeComplete: boolean;
    performHandshake: () => Promise<any>;
    
    // Actions needed
    loadModel: (model: string) => Promise<void>;
    fetchCachedModels: () => Promise<void>;
    fetchModels: () => Promise<void>;
    fetchModelInfo: () => Promise<void>;
    fetchModelState: () => Promise<void>;
    loadLaunchOverrides: () => Promise<void>;
    launchOverridesLoaded: boolean;
    launchModelOverride: string | null;
    launchInitialPrompt: string | null;
    launchPromptApplied: boolean;
    sendLaunchPrompt: () => Promise<void>;
    clearPendingSummary: (id: string) => void;
}

export interface ListenerSlice {
    isListening: boolean;
    setupListeners: () => Promise<void>;
    cleanupListeners: () => void;
}

// Helper function to initialize models on startup
async function initializeModelsOnStartup<T extends ListenerSliceDeps>(
    get: () => T,
    set: (partial: Partial<T> | ((state: T) => Partial<T>)) => void
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
        
        // Sync current model and model state with backend
        try {
            // Fetch the model state machine state first
            await get().fetchModelState();
            
            const currentBackendModel = await invoke<ModelInfo | null>('get_current_model');
            if (currentBackendModel) {
                console.log('[ChatStore] Synced current model from backend:', currentBackendModel.id);
                set({ currentModel: currentBackendModel.id } as any);
            }
        } catch (syncError) {
            console.warn('[ChatStore] Failed to sync current model from backend:', syncError);
        }

        if (cachedModels.length === 0) {
            // No models available - attempt auto-download
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
            } as any);
            
            try {
                await invoke('download_model', { modelName: DEFAULT_MODEL_TO_DOWNLOAD });
                console.log('[ChatStore] Default model download complete');
                
                // Refresh models after download
                await get().fetchCachedModels();
                await get().fetchModels();
                
                // Now load the model
                const updatedState = get();
                const DEFAULT_FALLBACK = 'phi-4-mini-instruct';
                const downloadedModel = updatedState.cachedModels.find(m => 
                    m.model_id.toLowerCase().includes(DEFAULT_FALLBACK.toLowerCase())
                );
                
                if (downloadedModel) {
                    console.log('[ChatStore] Loading downloaded model:', downloadedModel.model_id);
                    await get().loadModel(downloadedModel.model_id);
                } else if (updatedState.cachedModels.length > 0) {
                    console.warn('[ChatStore] Could not find phi-4-mini, unexpected state');
                    set({ currentModel: 'No models' } as any);
                } else {
                    set({
                        operationStatus: null,
                        currentModel: 'No models',
                    } as any);
                }
            } catch (downloadError: any) {
                console.error('[ChatStore] Failed to download default model:', downloadError);
                set({
                    operationStatus: {
                        type: 'downloading',
                        message: `Auto-download failed. Use: foundry model load ${DEFAULT_MODEL_TO_DOWNLOAD}`,
                        startTime: Date.now(),
                    },
                    currentModel: 'No models',
                } as any);
                setTimeout(() => {
                    const currentState = get();
                    if (currentState.operationStatus?.message?.includes('Auto-download failed')) {
                        set({ operationStatus: null } as any);
                    }
                }, 10000);
            }
        } else {
            console.log('[ChatStore] Found', cachedModels.length, 'cached models. Getting current model from backend...');
            
            try {
                const currentModelInfo = await invoke<{ id: string } | null>('get_current_model');
                
                if (currentModelInfo) {
                    console.log('[ChatStore] ✅ Backend selected model:', currentModelInfo.id);
                    set({ currentModel: currentModelInfo.id } as any);
                } else {
                    console.log('[ChatStore] Backend has not selected a model yet, waiting for model-selected event...');
                    set({ currentModel: 'Loading...' } as any);
                }
            } catch (loadError: any) {
                console.error('[ChatStore] Failed to get current model from backend:', loadError);
                set({ backendError: `Failed to get model: ${loadError?.message || loadError}` } as any);
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
                    set({ backendError: `Failed to load model ${launchModel}: ${e?.message || e}` } as any);
                }
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
        set({ operationStatus: null, currentModel: 'No models' } as any);
    }
}

export const createListenerSlice: StateCreator<
    ListenerSlice & ListenerSliceDeps,
    [],
    [],
    ListenerSlice
> = (set, get) => ({
    isListening: false,
    
    setupListeners: async () => {
        // Prevent duplicate listeners if already listening or currently setting up
        if (get().isListening || isSettingUp) {
            console.log("[ChatStore] Listeners already active or setting up. Skipping.");
            return;
        }

        isSettingUp = true;
        const myGeneration = listenerGenerationCounter;

        // Clear RAG context on app start to ensure fresh state
        // IMPORTANT: Only do this ONCE on true app startup, not on stall detector reconnections
        if (!hasInitializedRagContext) {
            hasInitializedRagContext = true;
            console.log('[ChatStore] Clearing RAG context on app startup...');
            set({ attachedPaths: [], ragChunkCount: 0, ragIndexedFiles: [] } as any);
            invoke<boolean>('clear_rag_context').catch(e => 
                console.error('[ChatStore] Failed to clear RAG context on startup:', e)
            );
        }

        // Clean up any existing listeners just in case (defensive)
        if (unlistenToken) { unlistenToken(); unlistenToken = undefined; }
        if (unlistenFinished) { unlistenFinished(); unlistenFinished = undefined; }

        console.log(`[ChatStore] Setting up event listeners (Gen: ${myGeneration})...`);
        logToBackend(`[FRONTEND] 🔧 Setting up event listeners (Gen: ${myGeneration})...`);

        try {
            console.log('[ChatStore] 📡 Registering chat-token listener...');
            const tokenListener = await listen<string>('chat-token', (event) => {
                const snapshot = get();
                const targetChatId = snapshot.streamingChatId || snapshot.currentChatId;

                // Log first token received - ALSO LOG TO BACKEND
                if (!tokenLogRecorded || tokenLogChatId !== targetChatId) {
                    const msgCount = snapshot.chatMessages.length;
                    const lastRole = snapshot.chatMessages[msgCount - 1]?.role || 'none';
                    const diagMsg = `[FRONTEND] 🔔 First chat-token received | chatId=${targetChatId?.slice(0,8)} | streaming=${snapshot.assistantStreamingActive} | msgCount=${msgCount} | lastRole=${lastRole} | token="${event.payload.substring(0, 30)}..."`;
                    console.log(diagMsg);
                    logToBackend(diagMsg);
                    tokenLogRecorded = true;
                    tokenLogChatId = targetChatId;
                }
                set((state) => {
                    // Ignore tokens if generation was stopped
                    if (!state.assistantStreamingActive) {
                        const warnMsg = `[FRONTEND] ⚠️ Token IGNORED: assistantStreamingActive=false | msgCount=${state.chatMessages.length}`;
                        console.warn(warnMsg);
                        logToBackend(warnMsg);
                        return state;
                    }
                    const now = Date.now();
                    
                    // Clear "Reconnecting" status if we're receiving tokens again
                    const shouldClearReconnecting = state.operationStatus?.message?.includes('Reconnecting');
                    const newOperationStatus = shouldClearReconnecting ? {
                        type: 'streaming' as const,
                        message: 'Generating response...',
                        startTime: state.operationStatus?.startTime || now,
                    } : state.operationStatus;
                    
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
                            return { streamingMessages: newStreamingMessages, lastStreamActivityTs: now, operationStatus: newOperationStatus } as any;
                        }
                        return { ...state, lastStreamActivityTs: now, operationStatus: newOperationStatus };
                    }
                    
                    // Normal case: streaming to current chat
                    const lastMsg = state.chatMessages[state.chatMessages.length - 1];
                    if (lastMsg && lastMsg.role === 'assistant') {
                        const newMessages = [...state.chatMessages];
                        const newContent = lastMsg.content + event.payload;
                        newMessages[newMessages.length - 1] = {
                            ...lastMsg,
                            content: newContent
                        };
                        // Log first successful append
                        if (lastMsg.content.length === 0 && event.payload.length > 0) {
                            logToBackend(`[FRONTEND] ✅ First token appended to assistant message | newLen=${newContent.length}`);
                        }
                        return { chatMessages: newMessages, lastStreamActivityTs: now, operationStatus: newOperationStatus } as any;
                    }
                    // Token dropped - no assistant message to append to
                    const dropMsg = `[FRONTEND] ❌ Token DROPPED: lastRole=${lastMsg?.role || 'undefined'} | msgCount=${state.chatMessages.length}`;
                    logToBackend(dropMsg);
                    return { ...state, lastStreamActivityTs: now, operationStatus: newOperationStatus };
                });
            });

            console.log('[ChatStore] 📡 Registering chat-finished listener...');
            const finishedListener = await listen('chat-finished', () => {
                const snapshot = get();
                const lastMsg = snapshot.chatMessages[snapshot.chatMessages.length - 1];
                const contentLen = lastMsg?.content?.length || 0;
                const finishMsg = `[FRONTEND] 🏁 chat-finished | msgCount=${snapshot.chatMessages.length} | lastRole=${lastMsg?.role} | contentLen=${contentLen}`;
                console.log(finishMsg);
                logToBackend(finishMsg);
                tokenLogRecorded = false;
                tokenLogChatId = null;
                set({ 
                    assistantStreamingActive: false,
                    streamingChatId: null,
                    streamingMessages: [],
                    operationStatus: null,
                    lastStreamActivityTs: Date.now(),
                } as any);
            });

            // Chat error listener - fatal errors during chat
            const chatErrorListener = await listen<{ error: string }>('chat-error', (event) => {
                const { error } = event.payload;
                console.error(`[ChatStore] ❌ chat-error: ${error}`);
                logToBackend(`[FRONTEND] ❌ chat-error: ${error}`);
                set({
                    assistantStreamingActive: false,
                    operationStatus: {
                        type: 'error',
                        message: error,
                        startTime: Date.now(),
                    },
                    statusBarDismissed: false,
                } as any);
                // Auto-dismiss after 10 seconds
                setTimeout(() => {
                    const currentState = get();
                    if (currentState.operationStatus?.type === 'error' && currentState.operationStatus?.message === error) {
                        set({ operationStatus: null } as any);
                    }
                }, 10000);
            });

            // Chat warning listener - non-fatal warnings
            const chatWarningListener = await listen<{ message: string }>('chat-warning', (event) => {
                const { message } = event.payload;
                console.warn(`[ChatStore] ⚠️ chat-warning: ${message}`);
                logToBackend(`[FRONTEND] ⚠️ chat-warning: ${message}`);
                set({
                    operationStatus: {
                        type: 'streaming',
                        message: `Warning: ${message}`,
                        startTime: Date.now(),
                    },
                    statusBarDismissed: false,
                } as any);
                // Auto-dismiss after 5 seconds
                setTimeout(() => {
                    const currentState = get();
                    if (currentState.operationStatus?.message?.includes(message)) {
                        set({ operationStatus: null } as any);
                    }
                }, 5000);
            });

            // Chat stream status listener
            const chatStreamStatusListener = await listen<{ phase: string; message: string; time_to_first_response_ms?: number }>('chat-stream-status', (event) => {
                const { phase, message } = event.payload;
                const now = Date.now();
                console.log(`[ChatStore] 📡 chat-stream-status: phase=${phase}, message=${message}`);
                
                if (phase === 'prewarming') {
                    set((state) => {
                        if (state.assistantStreamingActive) return state;
                        return {
                            operationStatus: {
                                type: 'loading',
                                message,
                                startTime: state.operationStatus?.startTime || now,
                            },
                            statusBarDismissed: false,
                        } as any;
                    });
                    return;
                }
                
                if (phase === 'prewarm_complete') {
                    set((state) => {
                        if (state.operationStatus?.type === 'loading') {
                            return { operationStatus: null } as any;
                        }
                        return state;
                    });
                    return;
                }
                
                set((state) => {
                    if (!state.assistantStreamingActive) {
                        console.log(`[ChatStore] chat-stream-status ignored: not streaming`);
                        return state;
                    }
                    return {
                        operationStatus: {
                            type: 'streaming',
                            message,
                            startTime: state.operationStatus?.startTime || now,
                        },
                        lastStreamActivityTs: now,
                    } as any;
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
                        return { streamingMessages: applyPrompt(state.streamingMessages) } as any;
                    }

                    if (!payloadChatId || payloadChatId === state.currentChatId) {
                        return { chatMessages: applyPrompt(state.chatMessages) } as any;
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
                } as any));
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
                    } as any);
                    setTimeout(() => {
                        const currentState = get();
                        if (currentState.operationStatus?.completed) {
                            set({ operationStatus: null } as any);
                        }
                    }, 3000);
                } else {
                    set({
                        operationStatus: {
                            type: 'loading',
                            message: `Failed to load ${event.payload.model}: ${event.payload.error || 'Unknown error'}`,
                            startTime: Date.now(),
                        },
                    } as any);
                }
            });

            // RAG progress listener
            const ragProgressListener = await listen<{ 
                phase: string; 
                total_files: number; 
                processed_files: number; 
                total_chunks: number; 
                processed_chunks: number; 
                current_file: string; 
                is_complete: boolean;
                extraction_progress?: number;
                extraction_total_pages?: number;
                compute_device?: string;
            }>('rag-progress', (event) => {
                const { 
                    phase, 
                    total_files, 
                    processed_files, 
                    total_chunks, 
                    processed_chunks, 
                    current_file, 
                    is_complete,
                    extraction_progress,
                    extraction_total_pages,
                    compute_device
                } = event.payload;
                
                console.log(`[ChatStore] rag-progress: phase=${phase}, is_complete=${is_complete}, device=${compute_device}`);
                
                const currentState = get();
                if (!currentState.isIndexingRag && !is_complete) {
                    console.log('[ChatStore] rag-progress: ignoring stale event (isIndexingRag=false)');
                    return;
                }
                
                let message = 'Processing documents...';
                let progress = 0;
                const deviceSuffix = compute_device ? ` [${compute_device}]` : '';

                switch (phase) {
                    case 'collecting_files':
                        message = 'Scanning directories...';
                        break;
                    case 'reading_files':
                        message = `Reading file ${processed_files + 1} of ${total_files}`;
                        progress = total_files > 0 ? (processed_files / total_files) * 100 : 0;
                        break;
                    case 'extracting_text':
                        const fileName = current_file.split('/').pop() || current_file;
                        if (extraction_progress !== undefined && extraction_total_pages !== undefined) {
                            message = `Extracting text from ${fileName} (${extraction_progress}% - page ${Math.ceil(extraction_progress * extraction_total_pages / 100)} of ${extraction_total_pages})`;
                            progress = extraction_progress;
                        } else {
                            message = `Extracting text from ${fileName}`;
                            progress = total_files > 0 ? (processed_files / total_files) * 100 : 0;
                        }
                        break;
                    case 'chunking':
                        message = `Chunking ${current_file.split('/').pop() || current_file}...`;
                        progress = total_files > 0 ? (processed_files / total_files) * 100 : 0;
                        break;
                    case 'checking_cache':
                        message = 'Checking embedding cache...';
                        break;
                    case 'generating_embeddings':
                        message = `Generating embeddings${deviceSuffix}: ${processed_chunks} / ${total_chunks}`;
                        progress = total_chunks > 0 ? (processed_chunks / total_chunks) * 100 : 0;
                        break;
                    case 'saving':
                        message = 'Saving to database...';
                        break;
                    case 'complete':
                        message = 'Indexing complete';
                        progress = 100;
                        break;
                }
                
                set((state) => ({
                    operationStatus: {
                        type: 'indexing',
                        message,
                        currentFile: current_file,
                        progress,
                        completed: is_complete,
                        startTime: state.operationStatus?.startTime || Date.now(),
                    },
                    ragChunkCount: processed_chunks || state.ragChunkCount,
                } as any));

                if (is_complete) {
                    setTimeout(() => {
                        const state = get();
                        if (state.operationStatus?.completed && state.operationStatus?.type === 'indexing') {
                            set({ operationStatus: null } as any);
                        }
                    }, 3000);
                }
            });

            // Embedding model init progress listener
            const embeddingInitListener = await listen<{ message: string; is_complete: boolean; error?: boolean }>('embedding-init-progress', (event) => {
                const { message, is_complete, error } = event.payload;
                console.log(`[ChatStore] Embedding init: ${message} (complete=${is_complete})`);
                
                set((state) => ({
                    operationStatus: {
                        type: 'loading',
                        message,
                        completed: is_complete,
                        startTime: state.operationStatus?.startTime || Date.now(),
                    }
                } as any));

                if (is_complete) {
                    setTimeout(() => {
                        const state = get();
                        if (state.operationStatus?.completed && (state.operationStatus?.message?.includes('Embedding model') || error)) {
                            set({ operationStatus: null } as any);
                        }
                    }, error ? 10000 : 3000);
                }
            });

            const modelSelectedListener = await listen<string>('model-selected', (event) => {
                set({ currentModel: event.payload } as any);
            });

            // Model state machine changes
            const modelStateChangedListener = await listen<any>('model-state-changed', (event) => {
                const parsed = parseFoundryModelStateEvent(event.payload);
                console.log(`[ChatStore] Model state changed: ${parsed.state}`, parsed);
                set({
                    modelState: parsed,
                    isModelReady: parsed.state === 'ready',
                } as any);
                
                if (parsed.state === 'ready' && parsed.modelId) {
                    set({ currentModel: parsed.modelId } as any);
                }
            });
            
            // HANDSHAKE PROTOCOL: Signal frontend is ready and receive full state snapshot.
            // This replaces the old pattern of emitting events before listeners were set up.
            // The handshake ensures we receive complete state atomically after listeners are ready.
            console.log('[ChatStore] Performing startup handshake with backend...');
            get().performHandshake().then((snapshot) => {
                if (snapshot) {
                    console.log('[ChatStore] Handshake successful, state synchronized');
                    // Fetch cached models for the UI dropdown (uses different format than snapshot)
                    get().fetchCachedModels();
                } else {
                    console.warn('[ChatStore] Handshake failed, falling back to individual fetches');
                    // Fallback: try individual fetches if handshake fails
                    get().fetchModelState().catch((e) => {
                        console.warn('[ChatStore] Fallback model state fetch failed:', e);
                    });
                    get().fetchCachedModels();
                }
            }).catch((e) => {
                console.error('[ChatStore] Handshake error:', e);
            });

            // Available models changed
            const availableModelsChangedListener = await listen<string[]>('available-models-changed', (event) => {
                const models = event.payload;
                console.log(`[ChatStore] Available models changed: ${models.length} models`);
                set((state) => ({
                    availableModels: models,
                    currentModel: state.currentModel === 'No models' && models.length > 0
                        ? models[0]
                        : state.currentModel
                } as any));
                get().fetchCachedModels();
                get().fetchModelInfo();
            });

            // Startup progress updates from backend
            const startupProgressListener = await listen<any>('startup-progress', (event) => {
                const payload = event.payload;
                console.log(`[ChatStore] Startup progress: ${payload.message}`, payload.startup_state);
                
                // Parse and update startup state
                const startupState = payload.startup_state?.state || 'initializing';
                const isAppReady = startupState === 'ready';
                
                // Update subsystem status
                const subsystemStatus = payload.subsystem_status || {};
                const isFoundryReady = subsystemStatus.foundry_service?.status === 'ready';
                const isChatReady = isFoundryReady && subsystemStatus.model?.status === 'ready';
                const isEmbeddingReady = subsystemStatus.cpu_embedding?.status === 'ready';
                
                // If we just transitioned to ready, mark handshake as complete
                // (backend became ready after frontend already called frontend_ready)
                const currentHandshakeComplete = get().handshakeComplete;
                const shouldCompleteHandshake = isAppReady && !currentHandshakeComplete;
                
                if (shouldCompleteHandshake) {
                    console.log('[ChatStore] Backend became ready via progress event, completing handshake');
                    // Refetch the full snapshot to get current model info
                    get().performHandshake().then((snapshot) => {
                        if (snapshot) {
                            console.log('[ChatStore] Re-handshake successful after backend ready');
                            get().fetchCachedModels();
                        }
                    });
                }
                
                set({
                    startupState,
                    subsystemStatus: payload.subsystem_status,
                    isAppReady,
                    isFoundryReady,
                    isChatReady,
                    isEmbeddingReady,
                } as any);
            });
            
            // Tool blocked by state machine
            const toolBlockedListener = await listen<{ tool: string; state: string; message: string }>('tool-blocked', (event) => {
                console.warn(`[ChatStore] Tool blocked: ${event.payload.tool} in state ${event.payload.state}`);
                set({
                    operationStatus: {
                        type: 'streaming',
                        message: `Tool ${event.payload.tool} blocked: ${event.payload.message}`,
                        startTime: Date.now(),
                    },
                    statusBarDismissed: false,
                } as any);
                setTimeout(() => {
                    const currentState = get();
                    if (currentState.operationStatus?.message?.includes('blocked')) {
                        set({ operationStatus: null } as any);
                    }
                }, 5000);
            });
            
            const chatSavedListener = await listen<string>('chat-saved', async (event) => {
                const chatId = event.payload;
                console.log(`[ChatStore] chat-saved event received for: ${chatId.slice(0, 8)}...`);
                get().clearPendingSummary(chatId);
                console.log(`[ChatStore] Cleared pending summary for ${chatId.slice(0, 8)}`);
            });

            const sidebarUpdateListener = await listen<ChatSummary[]>('sidebar-update', (event) => {
                if (get().isSearchingRelevance) {
                    set({ relevanceResults: event.payload, isSearchingRelevance: false } as any);
                }
            });

            // Model stuck listener
            const modelStuckListener = await listen<{ pattern: string; repetitions: number; score: number }>('model-stuck', (event) => {
                const { pattern, repetitions } = event.payload;
                console.warn(`[ChatStore] 🛑 Model stuck in loop: "${pattern}" repeated ${repetitions} times`);
                set({ 
                    modelStuckWarning: `Model appeared stuck in a loop (repeated "${pattern}"). Response was automatically cancelled.`,
                    statusBarDismissed: false
                } as any);
                
                setTimeout(() => {
                    const state = get();
                    if ((state as any).modelStuckWarning?.includes(pattern)) {
                        set({ modelStuckWarning: null } as any);
                    }
                }, 15000);
            });

            // Model fallback listener
            const modelFallbackListener = await listen<{ current_model: string; fallback_model: string; error: string }>('model-fallback-required', async (event) => {
                const { current_model, fallback_model, error } = event.payload;
                console.warn(`[ChatStore] 🔄 Model fallback required: ${current_model} -> ${fallback_model}, error: ${error}`);
                
                const state = get();
                const fallbackAvailable = state.cachedModels.some(
                    m => m.model_id.toLowerCase().includes(fallback_model.toLowerCase()) ||
                         m.alias.toLowerCase().includes(fallback_model.toLowerCase())
                );
                
                if (fallbackAvailable) {
                    const fallbackModelInfo = state.cachedModels.find(
                        m => m.model_id.toLowerCase().includes(fallback_model.toLowerCase()) ||
                             m.alias.toLowerCase().includes(fallback_model.toLowerCase())
                    );
                    
                    if (fallbackModelInfo && state.currentModel !== fallbackModelInfo.model_id) {
                        console.log(`[ChatStore] Switching to fallback model: ${fallbackModelInfo.model_id}`);
                        set({
                            operationStatus: {
                                type: 'loading',
                                message: `Switching to ${fallbackModelInfo.alias || fallbackModelInfo.model_id} due to error with current model...`,
                                startTime: Date.now(),
                            },
                            statusBarDismissed: false,
                        } as any);
                        
                        try {
                            await get().loadModel(fallbackModelInfo.model_id);
                            await invoke('set_model', { model: fallbackModelInfo.model_id });
                            console.log('[ChatStore] Fallback model selection persisted to settings:', fallbackModelInfo.model_id);
                            
                            set({
                                operationStatus: {
                                    type: 'loading',
                                    message: `Switched to ${fallbackModelInfo.alias || fallbackModelInfo.model_id}`,
                                    completed: true,
                                    startTime: Date.now(),
                                },
                            } as any);
                            setTimeout(() => {
                                const currentState = get();
                                if (currentState.operationStatus?.completed) {
                                    set({ operationStatus: null } as any);
                                }
                            }, 5000);
                        } catch (loadError: any) {
                            console.error('[ChatStore] Failed to load fallback model:', loadError);
                            set({
                                backendError: `Failed to load fallback model: ${loadError.message || loadError}`,
                                operationStatus: null,
                            } as any);
                        }
                    }
                } else {
                    console.log(`[ChatStore] Fallback model ${fallback_model} not cached. Attempting download...`);
                    set({
                        operationStatus: {
                            type: 'downloading',
                            message: `Downloading ${fallback_model} (fallback model)...`,
                            progress: 0,
                            startTime: Date.now(),
                        },
                        statusBarDismissed: false,
                    } as any);
                    
                    try {
                        await invoke('download_model', { modelName: fallback_model });
                        console.log('[ChatStore] Fallback model download complete');
                        
                        await get().fetchCachedModels();
                        
                        const updatedState = get();
                        const downloadedModel = updatedState.cachedModels.find(
                            m => m.model_id.toLowerCase().includes(fallback_model.toLowerCase()) ||
                                 m.alias.toLowerCase().includes(fallback_model.toLowerCase())
                        );
                        
                        if (downloadedModel) {
                            await get().loadModel(downloadedModel.model_id);
                            await invoke('set_model', { model: downloadedModel.model_id });
                            console.log('[ChatStore] Fallback model selection persisted to settings:', downloadedModel.model_id);
                        }
                    } catch (downloadError: any) {
                        console.error('[ChatStore] Failed to download fallback model:', downloadError);
                        set({
                            backendError: `Model error: ${error}. Fallback download failed: ${downloadError.message || downloadError}`,
                            operationStatus: null,
                        } as any);
                    }
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
                } as any);
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
                    operationStatus: {
                        type: 'streaming',
                        message: displayName,
                        startTime: state.operationStatus?.startTime || Date.now(),
                    },
                    statusBarDismissed: false,
                    lastStreamActivityTs: Date.now(),
                } as any));

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
                    } as any;
                });
            });

            const toolResultListener = await listen<ToolResultEvent>('tool-result', (event) => {
                console.log(`[ChatStore] Tool result: ${event.payload.server}::${event.payload.tool}, error=${event.payload.is_error}`);
                set((state) => {
                    const startTime = state.toolExecution.currentTool?.startTime;
                    const durationMs = startTime ? Date.now() - startTime : undefined;
                    
                    const toolCallRecord: ToolCallRecord = {
                        id: `tool-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
                        server: event.payload.server,
                        tool: event.payload.tool,
                        arguments: state.toolExecution.currentTool?.arguments || {},
                        result: event.payload.result,
                        isError: event.payload.is_error,
                        durationMs,
                    };
                    
                    const newMessages = [...state.chatMessages];
                    const lastIdx = newMessages.length - 1;
                    if (lastIdx >= 0 && newMessages[lastIdx].role === 'assistant') {
                        const existingToolCalls = newMessages[lastIdx].toolCalls || [];
                        newMessages[lastIdx] = {
                            ...newMessages[lastIdx],
                            toolCalls: [...existingToolCalls, toolCallRecord],
                        };
                    }
                    
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
                    } as any;
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
                    pendingToolApproval: null,
                    lastStreamActivityTs: Date.now(),
                } as any));
            });

            // Service restart listeners
            const serviceStopStartedListener = await listen<{ message: string }>('service-stop-started', (event) => {
                console.log(`[ChatStore] Service stop started: ${event.payload.message}`);
                set({
                    operationStatus: {
                        type: 'reloading',
                        message: event.payload.message || 'Stopping Foundry service...',
                        startTime: Date.now(),
                    },
                    statusBarDismissed: false,
                } as any);
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
                } as any);
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
                } as any);
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
                } as any);
            });

            const serviceRestartStartedListener = await listen<{ message: string }>('service-restart-started', (event) => {
                console.log(`[ChatStore] Service restart started: ${event.payload.message}`);
                set({
                    operationStatus: {
                        type: 'reloading',
                        message: event.payload.message,
                        startTime: Date.now(),
                    },
                } as any);
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
                    } as any);
                    setTimeout(() => {
                        const currentState = get();
                        if (currentState.operationStatus?.completed && currentState.operationStatus?.type === 'reloading') {
                            set({ operationStatus: null } as any);
                        }
                    }, 3000);
                } else {
                    set({
                        operationStatus: {
                            type: 'reloading',
                            message: `Service restart failed: ${event.payload.error || 'Unknown error'}`,
                            startTime: baseStart,
                        },
                    } as any);
                    setTimeout(() => {
                        const currentState = get();
                        if (currentState.operationStatus?.type === 'reloading' && !currentState.operationStatus?.completed) {
                            set({ operationStatus: null } as any);
                        }
                    }, 10000);
                }
            });

            // Critical check: did cleanup happen while we were awaiting?
            if (listenerGenerationCounter !== myGeneration) {
                console.log(`[ChatStore] Setup aborted due to generation mismatch (${myGeneration} vs ${listenerGenerationCounter}). Cleaning up new listeners.`);
                tokenListener();
                finishedListener();
                chatErrorListener();
                chatWarningListener();
                modelSelectedListener();
                modelStuckListener();
                modelFallbackListener();
                toolBlockedListener();
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
                embeddingInitListener();
                chatStreamStatusListener();
                availableModelsChangedListener();
                modelStateChangedListener();
                startupProgressListener();
                toolHeartbeatListener();
                isSettingUp = false;
                return;
            }

            // Assign to module variables
            unlistenToken = tokenListener;
            unlistenFinished = finishedListener;
            unlistenChatError = chatErrorListener;
            unlistenChatWarning = chatWarningListener;
            unlistenChatStreamStatus = chatStreamStatusListener;
            unlistenModelSelected = modelSelectedListener;
            unlistenModelStateChanged = modelStateChangedListener;
            unlistenToolBlocked = toolBlockedListener;
            unlistenToolCallsPending = toolCallsPendingListener;
            unlistenToolExecuting = toolExecutingListener;
            unlistenToolHeartbeat = toolHeartbeatListener;
            unlistenToolResult = toolResultListener;
            unlistenToolLoopFinished = toolLoopFinishedListener;
            unlistenSystemPrompt = systemPromptListener;
            unlistenChatSaved = chatSavedListener;
            unlistenSidebarUpdate = sidebarUpdateListener;
            unlistenModelStuck = modelStuckListener;
            unlistenModelFallback = modelFallbackListener;
            unlistenDownloadProgress = downloadProgressListener;
            unlistenLoadComplete = loadCompleteListener;
            unlistenRagProgress = ragProgressListener;
            unlistenEmbeddingInit = embeddingInitListener;
            unlistenServiceStopStarted = serviceStopStartedListener;
            unlistenServiceStopComplete = serviceStopCompleteListener;
            unlistenServiceStartStarted = serviceStartStartedListener;
            unlistenServiceStartComplete = serviceStartCompleteListener;
            unlistenServiceRestartStarted = serviceRestartStartedListener;
            unlistenServiceRestartComplete = serviceRestartCompleteListener;
            unlistenAvailableModelsChanged = availableModelsChangedListener;
            unlistenStartupProgress = startupProgressListener;
            
            console.log('[ChatStore] ✅ All event listeners registered successfully');
            logToBackend('[FRONTEND] ✅ All event listeners registered successfully');

            set({ isListening: true } as any);
            console.log(`[ChatStore] Event listeners active (Gen: ${myGeneration}).`);
            
            // Initialize models on startup
            initializeModelsOnStartup(get, set as any).catch((e) => {
                console.error("[ChatStore] Model initialization failed:", e);
            });
        } catch (e) {
            console.error("[ChatStore] Failed to setup listeners:", e);
        } finally {
            isSettingUp = false;
        }
    },
    
    cleanupListeners: () => {
        listenerGenerationCounter++; // Invalidate pending setups
        isSettingUp = false; // Allow new setup after cleanup
        if (unlistenToken) { unlistenToken(); unlistenToken = undefined; }
        if (unlistenFinished) { unlistenFinished(); unlistenFinished = undefined; }
        if (unlistenChatError) { unlistenChatError(); unlistenChatError = undefined; }
        if (unlistenChatWarning) { unlistenChatWarning(); unlistenChatWarning = undefined; }
        if (unlistenModelSelected) { unlistenModelSelected(); unlistenModelSelected = undefined; }
        if (unlistenModelStateChanged) { unlistenModelStateChanged(); unlistenModelStateChanged = undefined; }
        if (unlistenToolBlocked) { unlistenToolBlocked(); unlistenToolBlocked = undefined; }
        if (unlistenChatSaved) { unlistenChatSaved(); unlistenChatSaved = undefined; }
        if (unlistenSidebarUpdate) { unlistenSidebarUpdate(); unlistenSidebarUpdate = undefined; }
        if (unlistenModelStuck) { unlistenModelStuck(); unlistenModelStuck = undefined; }
        if (unlistenModelFallback) { unlistenModelFallback(); unlistenModelFallback = undefined; }
        if (unlistenToolCallsPending) { unlistenToolCallsPending(); unlistenToolCallsPending = undefined; }
        if (unlistenToolExecuting) { unlistenToolExecuting(); unlistenToolExecuting = undefined; }
        if (unlistenToolHeartbeat) { unlistenToolHeartbeat(); unlistenToolHeartbeat = undefined; }
        if (unlistenToolResult) { unlistenToolResult(); unlistenToolResult = undefined; }
        if (unlistenToolLoopFinished) { unlistenToolLoopFinished(); unlistenToolLoopFinished = undefined; }
        if (unlistenSystemPrompt) { unlistenSystemPrompt(); unlistenSystemPrompt = undefined; }
        if (unlistenDownloadProgress) { unlistenDownloadProgress(); unlistenDownloadProgress = undefined; }
        if (unlistenLoadComplete) { unlistenLoadComplete(); unlistenLoadComplete = undefined; }
        if (unlistenRagProgress) { unlistenRagProgress(); unlistenRagProgress = undefined; }
        if (unlistenEmbeddingInit) { unlistenEmbeddingInit(); unlistenEmbeddingInit = undefined; }
        if (unlistenServiceStopStarted) { unlistenServiceStopStarted(); unlistenServiceStopStarted = undefined; }
        if (unlistenServiceStopComplete) { unlistenServiceStopComplete(); unlistenServiceStopComplete = undefined; }
        if (unlistenServiceStartStarted) { unlistenServiceStartStarted(); unlistenServiceStartStarted = undefined; }
        if (unlistenServiceStartComplete) { unlistenServiceStartComplete(); unlistenServiceStartComplete = undefined; }
        if (unlistenServiceRestartStarted) { unlistenServiceRestartStarted(); unlistenServiceRestartStarted = undefined; }
        if (unlistenServiceRestartComplete) { unlistenServiceRestartComplete(); unlistenServiceRestartComplete = undefined; }
        if (unlistenChatStreamStatus) { unlistenChatStreamStatus(); unlistenChatStreamStatus = undefined; }
        if (unlistenAvailableModelsChanged) { unlistenAvailableModelsChanged(); unlistenAvailableModelsChanged = undefined; }
        if (unlistenStartupProgress) { unlistenStartupProgress(); unlistenStartupProgress = undefined; }
        
        set({ isListening: false } as any);
        console.log('[ChatStore] Listeners cleaned up');
    },
});
