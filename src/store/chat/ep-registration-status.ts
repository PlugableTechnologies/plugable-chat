import type { OperationStatus } from './types';

/** Payload of the backend `ep-registration-progress` event. */
export interface EpRegistrationProgressEvent {
    phase: string;
    ep?: string;
    percent?: number;
    message?: string;
}

export const EP_REGISTRATION_STATUS_LABEL = 'Preparing GPU acceleration';

const EP_REGISTRATION_STATUS_NOTE =
    'first run only; the app finishes starting when this ends, or cancel to use CPU models';

/** True while `status` is the "Preparing GPU acceleration" bar (as opposed to any other operation). */
export function isEpRegistrationStatus(status: OperationStatus | null | undefined): boolean {
    return status?.cancelAction === 'ep-registration';
}

/**
 * Next operation status for a backend `ep-registration-progress` event.
 *
 * `downloading` shows provider and percent with a Cancel button. `done` (finished, failed or
 * cancelled, all of which let the app continue on the providers that did register) clears our
 * bar and leaves the outcome message briefly. Other operations' status is never touched.
 */
export function nextOperationStatusForEpRegistration(
    current: OperationStatus | null,
    event: EpRegistrationProgressEvent,
    now: number,
): OperationStatus | null {
    if (event.phase === 'downloading') {
        // Do not let a late progress tick overwrite an unrelated operation (model load, streaming).
        if (current && !isEpRegistrationStatus(current) && !current.completed && current.type !== 'none') {
            return current;
        }
        // Progress ticks can still arrive after Cancel; keep saying "cancelling".
        if (current?.cancelRequested && isEpRegistrationStatus(current)) {
            return current;
        }
        const percent = Math.min(100, Math.max(0, Math.round(event.percent ?? 0)));
        return {
            type: 'downloading',
            message: `${EP_REGISTRATION_STATUS_LABEL}${event.ep ? `: ${event.ep}` : ''} ${percent}% (${EP_REGISTRATION_STATUS_NOTE})`,
            progress: percent,
            startTime: isEpRegistrationStatus(current) ? current!.startTime : now,
            cancelAction: 'ep-registration',
        };
    }
    if (!isEpRegistrationStatus(current)) {
        return current;
    }
    return event.message ? { type: 'none', message: event.message, startTime: now } : null;
}

/** Status shown between clicking Cancel and the backend's `done` event. */
export function cancellingEpRegistrationStatus(current: OperationStatus): OperationStatus {
    return {
        type: 'downloading',
        message: `${EP_REGISTRATION_STATUS_LABEL}: cancelling, continuing with CPU models...`,
        startTime: current.startTime,
        cancelAction: 'ep-registration',
        cancelRequested: true,
    };
}
