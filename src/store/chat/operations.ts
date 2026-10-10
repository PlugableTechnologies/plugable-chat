/**
 * Concurrent long-running operations shown in the status bar, keyed so that GPU component
 * registration, embedding init, model download and toolbox download never overwrite each other
 * (they all share the single `operationStatus` slot otherwise).
 */

export type OperationKey = 'ep-registration' | 'embedding' | 'model-download' | 'toolbox-download';

export type OperationState = 'active' | 'done' | 'error';

export interface OperationEntry {
    key: OperationKey;
    label: string;
    state: OperationState;
    message: string;
    /** 0-100; undefined means indeterminate */
    percent?: number;
    file?: string;
    startTime: number;
    /** Errors offer a Retry button wired to this key */
    retryable?: boolean;
    cancellable?: boolean;
    cancelRequested?: boolean;
}

export type OperationsMap = Partial<Record<OperationKey, OperationEntry>>;

export const OPERATION_LABELS: Record<OperationKey, string> = {
    'ep-registration': 'GPU components',
    'embedding': 'Embedding model',
    'model-download': 'Chat model',
    'toolbox-download': 'Database toolbox',
};

export type OperationAction =
    | { type: 'progress'; key: OperationKey; message?: string; percent?: number; file?: string; cancellable?: boolean }
    | { type: 'done'; key: OperationKey; message?: string }
    | { type: 'error'; key: OperationKey; message: string }
    | { type: 'cancel-requested'; key: OperationKey }
    | { type: 'clear'; key: OperationKey };

export function clampPercent(p: number | undefined): number | undefined {
    if (p === undefined || p === null || Number.isNaN(p)) return undefined;
    return Math.min(100, Math.max(0, Math.round(p)));
}

/** Pure reducer. Returns the same map object when nothing changes. */
export function reduceOperations(ops: OperationsMap, action: OperationAction, now: number): OperationsMap {
    const prev = ops[action.key];
    switch (action.type) {
        case 'progress': {
            // A cancelled operation keeps saying "cancelling" until the backend reports done.
            if (prev?.cancelRequested) return ops;
            return {
                ...ops,
                [action.key]: {
                    key: action.key,
                    label: OPERATION_LABELS[action.key],
                    state: 'active',
                    message: action.message ?? prev?.message ?? OPERATION_LABELS[action.key],
                    percent: clampPercent(action.percent) ?? prev?.percent,
                    file: action.file ?? prev?.file,
                    startTime: prev && prev.state === 'active' ? prev.startTime : now,
                    cancellable: action.cancellable ?? prev?.cancellable,
                },
            };
        }
        case 'done':
            return {
                ...ops,
                [action.key]: {
                    key: action.key,
                    label: OPERATION_LABELS[action.key],
                    state: 'done',
                    message: action.message ?? `${OPERATION_LABELS[action.key]} ready`,
                    percent: 100,
                    startTime: prev?.startTime ?? now,
                },
            };
        case 'error':
            return {
                ...ops,
                [action.key]: {
                    key: action.key,
                    label: OPERATION_LABELS[action.key],
                    state: 'error',
                    message: action.message,
                    startTime: prev?.startTime ?? now,
                    retryable: true,
                },
            };
        case 'cancel-requested':
            if (!prev || prev.state !== 'active') return ops;
            return { ...ops, [action.key]: { ...prev, cancelRequested: true, message: `${prev.label}: cancelling...` } };
        case 'clear': {
            if (!prev) return ops;
            const next = { ...ops };
            delete next[action.key];
            return next;
        }
    }
}

/** Entries in a stable display order. */
export function listOperations(ops: OperationsMap): OperationEntry[] {
    const order: OperationKey[] = ['ep-registration', 'embedding', 'model-download', 'toolbox-download'];
    return order.map((k) => ops[k]).filter((e): e is OperationEntry => !!e);
}

export function hasActiveOperations(ops: OperationsMap): boolean {
    return listOperations(ops).some((e) => e.state === 'active');
}

/** Only successes expire on their own; errors stay until retried or dismissed. */
export function shouldAutoClear(entry: OperationEntry | undefined): boolean {
    return entry?.state === 'done';
}

/** Interpret the backend embedding-init event. */
export function embeddingEventToAction(e: { message: string; is_complete: boolean; error?: boolean }): OperationAction {
    if (e.error) return { type: 'error', key: 'embedding', message: e.message };
    if (e.is_complete) return { type: 'done', key: 'embedding', message: e.message };
    return { type: 'progress', key: 'embedding', message: e.message };
}

export function epEventToAction(e: { phase: string; ep?: string; percent?: number; message?: string; failures?: unknown[] }): OperationAction | null {
    if (e.phase === 'downloading') {
        return {
            type: 'progress', key: 'ep-registration', percent: e.percent, cancellable: true,
            message: `Preparing GPU acceleration${e.ep ? `: ${e.ep}` : ''}`,
        };
    }
    if (e.phase === 'done') {
        // `done` always carries a message (success, cancel and failure alike); only a non-empty
        // `failures` list means something went wrong and deserves a persistent error row.
        return e.failures && e.failures.length > 0 && e.message
            ? { type: 'error', key: 'ep-registration', message: e.message }
            : { type: 'clear', key: 'ep-registration' };
    }
    return null;
}

/** Strings the backend prefixes onto database errors; each maps to a distinct user-facing cause. */
export type DatabaseErrorKind = 'embedding-not-ready' | 'toolbox-missing' | 'toolbox-failed' | 'other';

export function classifyDatabaseError(message: string): DatabaseErrorKind {
    const m = message.toLowerCase();
    if (m.includes('embedding model not ready')) return 'embedding-not-ready';
    if (m.includes('toolbox not downloaded')) return 'toolbox-missing';
    if (m.includes('toolbox failed to start')) return 'toolbox-failed';
    return 'other';
}

export function describeDatabaseError(message: string): string {
    switch (classifyDatabaseError(message)) {
        case 'embedding-not-ready':
            return 'The embedding model is not ready yet. It downloads on first run; schema refresh will work once it finishes.';
        case 'toolbox-missing':
            return 'The database toolbox is not downloaded yet. It downloads automatically; you can also start it manually below.';
        case 'toolbox-failed':
            return 'The database toolbox could not start. Details: ' + message.replace(/^.*toolbox failed to start:?\s*/i, '');
        default:
            return message;
    }
}

export function formatBytes(bytes: number): string {
    if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
    if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
    return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}
