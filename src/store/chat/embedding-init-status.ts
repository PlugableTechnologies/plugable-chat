import type { OperationStatus } from './types';

/** Payload of the backend `embedding-init-progress` event. */
export interface EmbeddingInitEvent {
    message: string;
    is_complete: boolean;
    error?: boolean;
}

/**
 * Status-bar state for an embedding-model init event. A failure must not be `completed`:
 * the status bar paints completed operations green with a check mark.
 */
export function nextOperationStatusForEmbeddingInit(
    prev: OperationStatus | null,
    event: EmbeddingInitEvent,
    now: number,
): OperationStatus {
    return {
        type: event.error ? 'error' : 'loading',
        message: event.message,
        completed: event.error ? false : event.is_complete,
        startTime: prev?.startTime ?? now,
    };
}
