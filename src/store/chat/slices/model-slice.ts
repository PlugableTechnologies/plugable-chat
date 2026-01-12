import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import type { 
    CachedModel, 
    ModelInfo, 
    ModelStateData, 
    ReasoningEffort, 
    OperationStatus,
    Message,
} from '../types';
import { parseFoundryModelStateEvent } from '../helpers';
import { MODEL_FETCH_MAX_RETRIES, MODEL_FETCH_INITIAL_DELAY_MS } from '../constants';

// Module-level state for model fetching
let modelFetchPromise: Promise<void> | null = null;
let modelFetchRetryTimer: ReturnType<typeof setTimeout> | null = null;

// Dependencies from other slices
interface ModelSliceDeps {
    operationStatus: OperationStatus | null;
    currentChatId: string | null;
    chatMessages: Message[];
    // For clearing attachments on model switch
    attachedDatabaseTables: any[];
    attachedTools: any[];
    attachedPaths: string[];
    ragIndexedFiles: string[];
    ragChunkCount: number;
}

export interface ModelSlice {
    // Model state machine (deterministic sync with backend)
    modelState: ModelStateData;
    isModelReady: boolean;
    fetchModelState: () => Promise<void>;
    
    // Available models
    availableModels: string[];
    cachedModels: CachedModel[];
    modelInfo: ModelInfo[];
    currentModel: string;
    isConnecting: boolean;
    hasFetchedCachedModels: boolean;
    reasoningEffort: ReasoningEffort;
    
    // Model operations
    fetchModels: () => Promise<void>;
    retryConnection: () => Promise<void>;
    fetchCachedModels: () => Promise<void>;
    fetchModelInfo: () => Promise<void>;
    setModel: (model: string) => Promise<void>;
    loadModel: (modelName: string) => Promise<void>;
    downloadModel: (modelName: string) => Promise<void>;
    getLoadedModels: () => Promise<string[]>;
    setReasoningEffort: (effort: ReasoningEffort) => void;
    
    // Launch overrides (from CLI)
    launchOverridesLoaded: boolean;
    launchModelOverride: string | null;
    launchInitialPrompt: string | null;
    launchPromptApplied: boolean;
    markLaunchPromptApplied: () => void;
    loadLaunchOverrides: () => Promise<void>;
    
    // Foundry service management
    reloadFoundry: () => Promise<void>;
}

export const createModelSlice: StateCreator<
    ModelSlice & ModelSliceDeps,
    [],
    [],
    ModelSlice
> = (set, get) => ({
    // Model state machine (deterministic sync with backend)
    modelState: { state: 'initializing' } as ModelStateData,
    isModelReady: false,
    fetchModelState: async () => {
        try {
            console.log('[ChatStore] Fetching model state from backend...');
            const state = await invoke<any>('get_model_state');
            console.log('[ChatStore] Raw model state from backend:', JSON.stringify(state));
            const parsed = parseFoundryModelStateEvent({ state, timestamp: Date.now() });
            console.log('[ChatStore] Parsed model state:', JSON.stringify(parsed));
            set({
                modelState: parsed,
                isModelReady: parsed.state === 'ready',
            });
            console.log('[ChatStore] ✅ Model state updated to:', parsed.state);
        } catch (e: any) {
            console.error('[ChatStore] ❌ Failed to fetch model state:', e);
            // Don't update state on error - keep existing
        }
    },

    // Available models
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
                        set({ availableModels: models, backendError: null, isConnecting: false } as any);
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
                set({ availableModels: [], backendError: null, isConnecting: false, currentModel: 'No models' } as any);
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
                    set({ availableModels: [], backendError: null, isConnecting: false, currentModel: 'No models' } as any);
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
            } as any);
        })();

        try {
            await modelFetchPromise;
        } finally {
            modelFetchPromise = null;
        }
    },
    
    retryConnection: async () => {
        // Reset state and try again
        set({ currentModel: 'Loading...', backendError: null } as any);
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
        // Delegate to loadModel which handles UI feedback and persistence
        await get().loadModel(model);
    },
    
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
            set({ 
                currentChatId: null, 
                chatMessages: [],
                // Clear all per-chat attachments
                attachedDatabaseTables: [],
                attachedTools: [],
                attachedPaths: [],
                ragIndexedFiles: [],
                ragChunkCount: 0,
            } as any);
            // Fire-and-forget clear of backend RAG context
            invoke<boolean>('clear_rag_context').catch(e => 
                console.error('[ChatStore] Failed to clear RAG context on model switch:', e)
            );
        }

        set({
            operationStatus: {
                type: 'loading',
                message: `Loading ${modelName} into VRAM...`,
                startTime: Date.now(),
            },
            statusBarDismissed: false,
        } as any);
        try {
            // Use set_model which BOTH loads the model AND persists to settings
            // This ensures the selection survives app restart
            await invoke('set_model', { model: modelName });
            set({
                operationStatus: {
                    type: 'loading',
                    message: `${modelName} loaded successfully`,
                    completed: true,
                    startTime: Date.now(),
                },
                currentModel: modelName,
            } as any);
            console.log('[ChatStore] Model loaded and persisted to settings:', modelName);
            // Auto-dismiss after 3 seconds
            setTimeout(() => {
                const currentState = get();
                if (currentState.operationStatus?.completed) {
                    set({ operationStatus: null } as any);
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
            } as any);
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
        } as any);
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
            } as any);
            // Refresh cached models
            await get().fetchCachedModels();
            // After download, auto-load if no model is currently loaded
            if (get().currentModel === 'No models') {
                await get().loadModel(modelName);
            }
        } catch (e: any) {
            console.error('[ChatStore] Failed to download model:', e);
            set({
                operationStatus: {
                    type: 'downloading',
                    message: `Failed to download ${modelName}: ${e.message || e}`,
                    startTime: Date.now(),
                },
                backendError: `Failed to download model: ${e.message || e}`,
            } as any);
        }
    },
    
    getLoadedModels: async () => {
        try {
            const models = await invoke<string[]>('get_loaded_models');
            return models;
        } catch (e: any) {
            console.error('[ChatStore] Failed to get loaded models:', e);
            return [];
        }
    },
    
    setReasoningEffort: (effort: ReasoningEffort) => set({ reasoningEffort: effort }),
    
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
    
    // Foundry service management
    reloadFoundry: async () => {
        console.log('[ChatStore] 🔄 Reloading Foundry service...');
        set({
            operationStatus: {
                type: 'reloading',
                message: 'Restarting Foundry service...',
                startTime: Date.now(),
            },
            statusBarDismissed: false,
        } as any);
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
            } as any);
            setTimeout(() => {
                const currentState = get();
                if (currentState.operationStatus?.type === 'reloading' && !currentState.operationStatus?.completed) {
                    set({ operationStatus: null } as any);
                }
            }, 10000);
        }
    },
});
