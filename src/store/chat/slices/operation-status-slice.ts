import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import { DEFAULT_MODEL_TO_DOWNLOAD } from '../constants';
import type { OperationStatus } from '../types';
import { reduceOperations, shouldAutoClear, type OperationAction, type OperationKey, type OperationsMap } from '../operations';

const RETRY_COMMANDS: Record<OperationKey, { command: string; args?: Record<string, unknown> }> = {
    'ep-registration': { command: 'retry_gpu_registration' },
    'embedding': { command: 'retry_embedding_init' },
    'model-download': { command: 'download_model', args: { modelName: DEFAULT_MODEL_TO_DOWNLOAD } },
    'toolbox-download': { command: 'install_toolbox' },
};


/** Shown after a model is blocklisted as incompatible with the installed Foundry runtime. */
export interface ModelIncompatibleNotice {
    model: string;
    reason: string;
    /** Cached CPU build of the same alias, if any; the banner offers to switch to it. */
    alternativeModel: string | null;
}

export interface OperationStatusSlice {
    // Operation status for status bar (downloads, loads, streaming)
    operationStatus: OperationStatus | null;
    statusBarDismissed: boolean;
    /** Concurrent keyed operations (GPU components, embedding, model and toolbox downloads) */
    operations: OperationsMap;
    /** Real name/size of the model being downloaded (from `model-download-started`) */
    downloadingModelName: string | null;
    downloadingModelSizeBytes: number | null;
    dispatchOperation: (action: OperationAction) => void;
    retryOperation: (key: OperationKey) => Promise<void>;
    setOperationStatus: (status: OperationStatus | null) => void;
    dismissStatusBar: () => void;
    showStatusBar: () => void;
    
    // Heartbeat warning (frontend cannot reach backend)
    heartbeatWarningStart: number | null;
    heartbeatWarningMessage: string | null;
    setHeartbeatWarning: (startTime: number | null, message?: string | null) => void;
    
    // Model stuck warning
    modelStuckWarning: string | null;
    setModelStuck: (message: string | null) => void;
    
    // Model marked incompatible: why, and the CPU build to offer instead
    modelIncompatibleNotice: ModelIncompatibleNotice | null;
    setModelIncompatibleNotice: (notice: ModelIncompatibleNotice | null) => void;

    // Error handling
    backendError: string | null;
    clearError: () => void;
}

export const createOperationStatusSlice: StateCreator<
    OperationStatusSlice,
    [],
    [],
    OperationStatusSlice
> = (set, get) => ({
    operationStatus: null,
    statusBarDismissed: false,
    operations: {},
    downloadingModelName: null,
    downloadingModelSizeBytes: null,
    dispatchOperation: (action) => {
        set((state) => {
            const next = reduceOperations(state.operations, action, Date.now());
            return next === state.operations ? state : { operations: next, statusBarDismissed: false };
        });
        if (action.key === 'model-download' && action.type === 'done') {
            set({ downloadingModelName: null, downloadingModelSizeBytes: null });
        }
        if (action.type === 'done') {
            setTimeout(() => {
                if (shouldAutoClear(get().operations[action.key])) {
                    set((state) => ({ operations: reduceOperations(state.operations, { type: 'clear', key: action.key }, Date.now()) }));
                }
            }, 3000);
        }
    },
    retryOperation: async (key) => {
        const { command, args } = RETRY_COMMANDS[key];
        get().dispatchOperation({ type: 'progress', key, message: 'Retrying...', percent: 0 });
        try {
            await invoke(command, args);
            if (key === 'model-download' || key === 'toolbox-download') {
                get().dispatchOperation({ type: 'done', key });
                if (key === 'model-download') await (get() as any).fetchCachedModels?.();
            }
        } catch (e: any) {
            get().dispatchOperation({ type: 'error', key, message: `Retry failed: ${e?.message ?? e}` });
        }
    },
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
    
    modelStuckWarning: null,
    setModelStuck: (message) => set({ modelStuckWarning: message, statusBarDismissed: false }),
    
    modelIncompatibleNotice: null,
    setModelIncompatibleNotice: (notice) => set({ modelIncompatibleNotice: notice, statusBarDismissed: false }),

    backendError: null,
    clearError: () => set({ backendError: null }),
});
