import type { StartupStateType, SubsystemStatusData } from './types';

/** How the start-up bar should look. */
export interface StartupBarModel {
    tone: 'red' | 'amber' | 'blue' | 'green';
    message: string;
    /** Only the subsystems chat cannot work without. The search model is not one of them. */
    readyCount: number;
    totalCount: number;
}

/**
 * Red "Startup failed" is only for a failure that stops chat: the engine or the chat model failed.
 * A stalled start-up with both of those still working, a failed search model, or a GPU setup that
 * has not finished never turn the bar red (rc16: a slow first-run download did).
 */
export function startupBarModel(state: StartupStateType, subsystems: SubsystemStatusData | null): StartupBarModel {
    const essential = subsystems ? [subsystems.foundry_service?.status, subsystems.model?.status] : [];
    const readyCount = essential.filter((s) => s === 'ready').length;
    const totalCount = essential.length;
    const chatBlocked = essential.some((s) => s === 'failed' || s === 'unavailable');
    if (state === 'ready') return { tone: 'green', message: 'Ready', readyCount, totalCount };
    if (state === 'failed') {
        return chatBlocked
            ? { tone: 'red', message: 'Startup failed', readyCount, totalCount }
            : { tone: 'amber', message: 'Still starting up - this is taking longer than usual', readyCount, totalCount };
    }
    const message =
        state === 'initializing' ? 'Starting up...'
        : state === 'connecting_to_foundry' ? 'Connecting to Foundry...'
        : state === 'awaiting_frontend' ? 'Synchronizing...'
        : 'Unknown state';
    return { tone: 'blue', message, readyCount, totalCount };
}

/** True when search features are off because the search model failed (chat itself may be fine). */
export function searchFeaturesUnavailable(subsystems: SubsystemStatusData | null): boolean {
    return subsystems?.cpu_embedding?.status === 'failed';
}

export const SEARCH_UNAVAILABLE_CHIP_TEXT = 'Search features unavailable - retrying';
