import type { ModelStateData, ModelStateType } from './types';

/**
 * Parse the backend ModelState enum into a flat ModelStateData object
 */
export function parseFoundryModelStateEvent(payload: any): ModelStateData {
    if (!payload || !payload.state) {
        return { state: 'initializing' };
    }

    const stateObj = payload.state;
    const timestamp = payload.timestamp;

    // Handle tagged enum format from Rust: { state: "ready", model_id: "..." }
    if (typeof stateObj === 'object' && stateObj.state) {
        const stateName = stateObj.state as string;
        switch (stateName) {
            case 'initializing':
                return { state: 'initializing', timestamp };
            case 'ready':
                return { state: 'ready', modelId: stateObj.model_id, timestamp };
            case 'switching_model':
                return {
                    state: 'switching_model',
                    previousModel: stateObj.from,
                    targetModel: stateObj.to,
                    timestamp
                };
            case 'unloading_model':
                return {
                    state: 'unloading_model',
                    modelId: stateObj.model_id,
                    targetModel: stateObj.next_model,
                    timestamp
                };
            case 'loading_model':
                return { state: 'loading_model', modelId: stateObj.model_id, timestamp };
            case 'error':
                return {
                    state: 'error',
                    errorMessage: stateObj.message,
                    previousModel: stateObj.last_model,
                    timestamp
                };
            case 'service_unavailable':
                return { state: 'service_unavailable', errorMessage: stateObj.message, timestamp };
            case 'service_restarting':
                return { state: 'service_restarting', timestamp };
            case 'reconnecting':
                return { state: 'reconnecting', timestamp };
            default:
                console.warn('[ChatStore] Unknown model state:', stateName);
                return { state: 'initializing', timestamp };
        }
    }

    // Fallback: assume it's a simple string state name
    if (typeof stateObj === 'string') {
        return { state: stateObj as ModelStateType, timestamp };
    }

    return { state: 'initializing', timestamp };
}

/**
 * Check if prompts should be blocked based on the current model state
 */
export function isModelStateBlocking(modelState: ModelStateData): boolean {
    return modelState.state !== 'ready';
}

/**
 * Get a user-friendly message for the current model state
 */
export function getModelStateMessage(modelState: ModelStateData): string {
    switch (modelState.state) {
        case 'initializing':
            return 'Initializing...';
        case 'ready':
            return modelState.modelId ? `Ready: ${modelState.modelId}` : 'Ready';
        case 'switching_model':
            return `Switching to ${modelState.targetModel || 'new model'}...`;
        case 'unloading_model':
            return `Unloading ${modelState.modelId || 'model'} from VRAM...`;
        case 'loading_model':
            return `Loading ${modelState.modelId || 'model'} into VRAM...`;
        case 'error':
            return modelState.errorMessage || 'Model error';
        case 'service_unavailable':
            return modelState.errorMessage || 'Foundry service unavailable';
        case 'service_restarting':
            return 'Restarting Foundry service...';
        case 'reconnecting':
            return 'Reconnecting to Foundry...';
        default:
            return 'Unknown state';
    }
}

/**
 * Generate a unique client-side chat identifier
 */
export function generateClientChatIdentifier(): string {
    const cryptoObj = typeof globalThis !== 'undefined' ? (globalThis as any).crypto : undefined;
    if (cryptoObj && typeof cryptoObj.randomUUID === 'function') {
        return cryptoObj.randomUUID();
    }
    return `chat-${Date.now()}-${Math.floor(Math.random() * 1000)}`;
}

/**
 * Derive a chat title from the first user prompt
 */
export function deriveChatTitleFromPrompt(prompt: string): string {
    const cleaned = prompt.trim().replace(/\s+/g, ' ');
    if (!cleaned) {
        return "Untitled Chat";
    }
    const sentenceEnd = cleaned.search(/[.!?]/);
    const base = sentenceEnd > 0 ? cleaned.substring(0, sentenceEnd).trim() : cleaned;
    if (base.length <= 40) {
        return base;
    }
    return `${base.substring(0, 37).trim()}...`;
}

/**
 * Derive a chat preview from a message content
 */
export function deriveChatPreviewFromMessage(message: string): string {
    const cleaned = message.trim().replace(/\s+/g, ' ');
    if (!cleaned) return "";
    if (cleaned.length <= 80) {
        return cleaned;
    }
    return `${cleaned.substring(0, 77)}...`;
}
