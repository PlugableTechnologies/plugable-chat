/**
 * Model downloads, one entry per model. A single shared slot made a second download hide the
 * first's progress bar, and a path that forgot to mark completion left "100%" on screen until
 * the app restarted. Every download now goes through one lifecycle: start, progress, then exactly
 * one of done / error / cancelled.
 */
import { formatBytes, type OperationAction, type OperationEntry } from './operations.ts';

export type DownloadState = 'queued' | 'active' | 'done' | 'error' | 'cancelled';

export interface DownloadEntry {
    /** The name the download was requested with; the backend echoes it on every event. */
    id: string;
    /** What the user sees; replaced by the resolved variant name when the backend reports it. */
    label: string;
    sizeBytes?: number;
    state: DownloadState;
    percent: number;
    error?: string;
    startTime: number;
    /** When the percentage last went up (or the download started); drives stall detection. */
    lastProgressAt?: number;
    cancelRequested?: boolean;
}

export type DownloadsMap = Record<string, DownloadEntry>;

export type DownloadAction =
    | { type: 'start'; id: string; label?: string }
    | { type: 'started'; id: string; label?: string; sizeBytes?: number }
    | { type: 'progress'; id: string; percent: number }
    | { type: 'done'; id: string }
    | { type: 'error'; id: string; message: string }
    | { type: 'cancel-requested'; id: string }
    | { type: 'cancelled'; id: string }
    | { type: 'clear'; id: string };

const clamp = (p: number) => (Number.isNaN(p) ? 0 : Math.min(100, Math.max(0, p)));

/** True for errors raised because the user cancelled, which are not failures. */
export function isCancelError(message: string): boolean {
    return /cancel+ed/i.test(message);
}

export function reduceDownloads(map: DownloadsMap, action: DownloadAction, now: number): DownloadsMap {
    const prev = map[action.id];
    switch (action.type) {
        case 'start':
            // Waiting until the backend reports it has begun: the engine runs one download at
            // a time, so a second request really is queued.
            return {
                ...map,
                [action.id]: {
                    id: action.id,
                    label: action.label ?? action.id,
                    state: 'queued',
                    percent: 0,
                    startTime: now,
                    lastProgressAt: now,
                },
            };
        case 'started':
            if (!prev || prev.state === 'done' || prev.state === 'cancelled') return map;
            return {
                ...map,
                [action.id]: {
                    ...prev,
                    state: prev.cancelRequested ? prev.state : 'active',
                    label: action.label ?? prev.label,
                    sizeBytes: action.sizeBytes ?? prev.sizeBytes,
                    lastProgressAt: now,
                },
            };
        case 'progress':
            // A late progress event must not revive a finished or cancelled download.
            if (!prev || prev.state === 'done' || prev.state === 'cancelled' || prev.state === 'error') return map;
            return {
                ...map,
                [action.id]: {
                    ...prev,
                    state: 'active',
                    percent: clamp(action.percent),
                    lastProgressAt: clamp(action.percent) > prev.percent ? now : prev.lastProgressAt,
                },
            };
        case 'done':
            return {
                ...map,
                [action.id]: {
                    id: action.id,
                    label: prev?.label ?? action.id,
                    sizeBytes: prev?.sizeBytes,
                    state: 'done',
                    percent: 100,
                    startTime: prev?.startTime ?? now,
                },
            };
        case 'error':
            if (isCancelError(action.message) || prev?.cancelRequested) {
                return reduceDownloads(map, { type: 'cancelled', id: action.id }, now);
            }
            return {
                ...map,
                [action.id]: {
                    id: action.id,
                    label: prev?.label ?? action.id,
                    sizeBytes: prev?.sizeBytes,
                    state: 'error',
                    percent: prev?.percent ?? 0,
                    error: action.message,
                    startTime: prev?.startTime ?? now,
                },
            };
        case 'cancel-requested':
            if (!prev || (prev.state !== 'queued' && prev.state !== 'active')) return map;
            return { ...map, [action.id]: { ...prev, cancelRequested: true } };
        case 'cancelled': {
            // A cancelled download leaves no row behind.
            if (!prev) return map;
            const next = { ...map };
            delete next[action.id];
            return next;
        }
        case 'clear': {
            if (!prev) return map;
            const next = { ...map };
            delete next[action.id];
            return next;
        }
    }
}

/** Entries oldest first. */
export function listDownloads(map: DownloadsMap): DownloadEntry[] {
    return Object.values(map).sort((a, b) => a.startTime - b.startTime);
}

export function isDownloadPending(e: DownloadEntry | undefined): boolean {
    return e?.state === 'queued' || e?.state === 'active';
}

/** Only successes expire on their own; errors stay until retried or dismissed. */
export function shouldAutoClearDownload(e: DownloadEntry | undefined): boolean {
    return e?.state === 'done';
}

/**
 * The single `model-download` operation other screens already read (header text, welcome message,
 * retry), derived from the per-model entries so it can never disagree with them.
 */
export function summarizeDownloads(map: DownloadsMap): OperationAction {
    const all = listDownloads(map);
    const pending = all.filter(isDownloadPending);
    if (pending.length > 0) {
        const lead = pending.find((e) => e.state === 'active') ?? pending[0];
        const more = pending.length > 1 ? ` (+${pending.length - 1} more)` : '';
        return {
            type: 'progress',
            key: 'model-download',
            message: `Downloading ${lead.label}${more}`,
            percent: lead.percent,
        };
    }
    const failed = all.find((e) => e.state === 'error');
    if (failed) {
        return { type: 'error', key: 'model-download', message: `Failed to download ${failed.label}: ${failed.error ?? 'unknown error'}` };
    }
    const done = all.find((e) => e.state === 'done');
    if (done) {
        return { type: 'done', key: 'model-download', message: `${done.label} downloaded` };
    }
    return { type: 'clear', key: 'model-download' };
}

/** A download as a status-bar row (the row component is shared with the other operations). */
export function downloadToOperationEntry(e: DownloadEntry, stalled = false): OperationEntry {
    const size = e.sizeBytes ? ` (${formatBytes(e.sizeBytes)})` : '';
    const message =
        e.state === 'queued' ? `Queued: ${e.label}${size}`
        : e.state === 'active' ? `Downloading ${e.label}${size}${stalled ? ' - no progress for a while' : ''}`
        : e.state === 'done' ? `${e.label} downloaded`
        : `Failed to download ${e.label}: ${e.error ?? 'unknown error'}`;
    return {
        key: 'model-download',
        label: 'Chat model',
        state: e.state === 'done' ? 'done' : e.state === 'error' ? 'error' : 'active',
        message: e.cancelRequested ? `${e.label}: cancelling...` : message,
        percent: e.state === 'queued' ? undefined : Math.round(e.percent),
        startTime: e.startTime,
        retryable: e.state === 'error',
        cancellable: e.state === 'queued' || e.state === 'active',
        cancelRequested: e.cancelRequested,
        stalled,
    };
}
