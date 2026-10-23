import type { ModelStateData } from './types';
import { isDownloadPending, listDownloads, type DownloadsMap } from './downloads.ts';
import type { OperationsMap } from './operations';

/** Why the chat input is disabled. `null` from `inputDisabledReason` means it is enabled. */
export type InputDisabledKind =
    | 'model-downloading'
    | 'model-loading'
    | 'model-switching'
    | 'model-unavailable'
    | 'gpu-setup'
    | 'streaming-other-chat';

export interface InputDisabledReason {
    kind: InputDisabledKind;
    /** Placeholder text for the disabled textarea. */
    placeholder: string;
}

export interface InputDisabledInputs {
    streamingInOtherChat: boolean;
    isModelReady: boolean;
    modelState: Pick<ModelStateData, 'state'>;
    downloads: DownloadsMap;
    operations: OperationsMap;
    /** Current time in ms, to show how long GPU setup has run. */
    nowMs?: number;
}

/**
 * The reason-specific placeholder for a disabled input. The model reasons come first because they
 * are the more basic block: a chat streaming elsewhere ends on its own, a model that is not loaded
 * does not. The input used to say "Response streaming in another chat..." for every one of them.
 */
export function inputDisabledReason(i: InputDisabledInputs): InputDisabledReason | null {
    if (!i.isModelReady) {
        const pending = listDownloads(i.downloads).filter(isDownloadPending);
        if (pending.length > 0) {
            const d = pending[0];
            const pct = d.state === 'active' ? ` ${Math.round(d.percent)}%` : '';
            return { kind: 'model-downloading', placeholder: `Downloading ${d.label}${pct}...` };
        }
        const gpu = i.operations['ep-registration'];
        if (gpu?.state === 'active') {
            const minutes = i.nowMs !== undefined ? Math.floor((i.nowMs - gpu.startTime) / 60000) : 0;
            return { kind: 'gpu-setup', placeholder: `Setting up GPU acceleration${minutes >= 1 ? ` (${minutes} min)` : ''}...` };
        }
        switch (i.modelState.state) {
            case 'switching_model':
            case 'unloading_model':
                return { kind: 'model-switching', placeholder: 'Switching model...' };
            case 'error':
            case 'service_unavailable':
                return { kind: 'model-unavailable', placeholder: 'No model available - see the message above' };
            default:
                return { kind: 'model-loading', placeholder: 'Model loading...' };
        }
    }
    if (i.streamingInOtherChat) {
        return { kind: 'streaming-other-chat', placeholder: 'Streaming in another chat...' };
    }
    return null;
}
