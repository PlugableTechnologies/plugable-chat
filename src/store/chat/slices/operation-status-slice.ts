import type { StateCreator } from 'zustand';
import type { OperationStatus } from '../types';

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
> = (set) => ({
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
    
    modelStuckWarning: null,
    setModelStuck: (message) => set({ modelStuckWarning: message, statusBarDismissed: false }),
    
    modelIncompatibleNotice: null,
    setModelIncompatibleNotice: (notice) => set({ modelIncompatibleNotice: notice, statusBarDismissed: false }),

    backendError: null,
    clearError: () => set({ backendError: null }),
});
