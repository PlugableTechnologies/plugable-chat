import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import { DEFAULT_MODEL_TO_DOWNLOAD } from '../constants';
import type { OperationStatus } from '../types';
import { reduceOperations, shouldAutoClear, type OperationAction, type OperationKey, type OperationsMap } from '../operations';
import { runRetry } from '../retry';
import {
    isDownloadPending,
    listDownloads,
    reduceDownloads,
    shouldAutoClearDownload,
    summarizeDownloads,
    type DownloadAction,
    type DownloadsMap,
} from '../downloads';
import type { GpuAssessment } from '../../../lib/gpu-assessment';

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

/** A replacement model that needs a download; nothing is fetched until the user agrees. */
export interface PendingFallbackDownload {
    failedModel: string;
    model: string;
    label: string;
    sizeBytes: number | null;
}

/** UI-only message from the backend. Never added to chatMessages, so no model ever reads it. */
export interface ChatNotice {
    level: 'warning' | 'error';
    text: string;
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
    /** One entry per model download; `operations['model-download']` is derived from it */
    downloads: DownloadsMap;
    dispatchDownload: (action: DownloadAction) => void;
    /** Stops a queued or running download; the entry disappears when the backend confirms */
    cancelModelDownload: (id: string) => Promise<void>;
    retryOperation: (key: OperationKey) => Promise<void>;
    setOperationStatus: (status: OperationStatus | null) => void;
    dismissStatusBar: () => void;
    showStatusBar: () => void;
    
    /** Why the GPU is or is not in use (backend `gpu-assessment` events); null until the first answer */
    gpuAssessment: GpuAssessment | null;
    /** Headline the user dismissed, so the same card does not return until the situation changes */
    gpuAssessmentDismissed: string | null;
    setGpuAssessment: (a: GpuAssessment | null) => void;
    dismissGpuAssessment: () => void;

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

    // Fallback model waiting for the user's OK to download
    pendingFallbackDownload: PendingFallbackDownload | null;
    setPendingFallbackDownload: (p: PendingFallbackDownload | null) => void;

    // Transient app notice (switching/overflow/error); shown in the status bar only
    chatNotice: ChatNotice | null;
    setChatNotice: (notice: ChatNotice | null) => void;

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
    downloads: {},
    downloadingModelName: null,
    downloadingModelSizeBytes: null,
    dispatchDownload: (action) => {
        const next = reduceDownloads(get().downloads, action, Date.now());
        if (next === get().downloads) return;
        set({ downloads: next, statusBarDismissed: false });
        // Keep the single `model-download` summary (header text, welcome message) in step.
        get().dispatchOperation(summarizeDownloads(next));
        const pending = listDownloads(next).filter(isDownloadPending);
        const lead = pending.find((e) => e.state === 'active') ?? pending[0];
        set({ downloadingModelName: lead?.label ?? null, downloadingModelSizeBytes: lead?.sizeBytes ?? null });
        if (action.type === 'done') {
            setTimeout(() => {
                if (shouldAutoClearDownload(get().downloads[action.id])) {
                    get().dispatchDownload({ type: 'clear', id: action.id });
                }
            }, 3000);
        }
    },
    cancelModelDownload: async (id) => {
        get().dispatchDownload({ type: 'cancel-requested', id });
        try {
            await invoke('cancel_model_download', { modelName: id });
        } catch (e) {
            console.warn('[ChatStore] cancel_model_download failed:', e);
        }
    },
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
        await runRetry(key, RETRY_COMMANDS[key], {
            invoke,
            dispatch: get().dispatchOperation,
            refreshModels: async () => {
                await (get() as any).fetchCachedModels?.();
                await (get() as any).fetchModels?.();
            },
        });
    },
    setOperationStatus: (status) => set({ operationStatus: status, statusBarDismissed: false }),
    dismissStatusBar: () => set({ statusBarDismissed: true }),
    showStatusBar: () => set({ statusBarDismissed: false }),
    
    gpuAssessment: null,
    gpuAssessmentDismissed: null,
    setGpuAssessment: (a) => set({ gpuAssessment: a, statusBarDismissed: false }),
    dismissGpuAssessment: () => set((state) => ({ gpuAssessmentDismissed: state.gpuAssessment?.headline ?? null })),

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

    pendingFallbackDownload: null,
    setPendingFallbackDownload: (p) => set({ pendingFallbackDownload: p, statusBarDismissed: false }),

    chatNotice: null,
    setChatNotice: (notice) => set({ chatNotice: notice, statusBarDismissed: false }),

    backendError: null,
    clearError: () => set({ backendError: null }),
});
