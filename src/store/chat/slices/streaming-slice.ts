import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import type { Message, OperationStatus } from '../types';

// Streaming slice needs to interact with operation status and message slices
interface StreamingSliceDeps {
    operationStatus: OperationStatus | null;
    chatGenerationCounter: number;
    chatMessages: Message[];
}

export interface StreamingSlice {
    // Active streaming state
    assistantStreamingActive: boolean;
    setAssistantStreamingActive: (streaming: boolean) => void;
    lastStreamActivityTs: number | null;
    setLastStreamActivityTs: (ts: number) => void;
    
    // Per-chat streaming tracking (streaming continues to original chat on switch)
    streamingChatId: string | null;
    streamingMessages: Message[]; // Messages for the streaming chat (if different from current)
    setStreamingChatId: (id: string | null) => void;
    
    // Stop generation
    stopActiveChatGeneration: () => Promise<void>;
}

export const createStreamingSlice: StateCreator<
    StreamingSlice & StreamingSliceDeps,
    [],
    [],
    StreamingSlice
> = (set, get) => ({
    // Active streaming state
    assistantStreamingActive: false,
    setAssistantStreamingActive: (assistantStreamingActive) => set({ assistantStreamingActive }),
    lastStreamActivityTs: null,
    setLastStreamActivityTs: (ts) => set({ lastStreamActivityTs: ts }),
    
    // Per-chat streaming tracking
    streamingChatId: null,
    streamingMessages: [],
    setStreamingChatId: (id) => set({ streamingChatId: id }),
    
    stopActiveChatGeneration: async () => {
        console.log('[ChatStore] 🛑 STOP BUTTON PRESSED by user');

        // Increment generationId to ignore any incoming tokens from the stopped generation
        const currentGenId = get().chatGenerationCounter;
        console.log('[ChatStore] Current generation to cancel:', currentGenId);

        set((state) => ({
            assistantStreamingActive: false,
            chatGenerationCounter: state.chatGenerationCounter + 1,
            streamingChatId: null,
            lastStreamActivityTs: Date.now(),
        } as any));

        try {
            // Cancel the stream - this signals both the agentic loop AND the FoundryActor to stop
            await invoke('cancel_generation', { generationId: currentGenId });
            console.log('[ChatStore] ✅ Cancel signal sent for generation', currentGenId);
        } catch (e) {
            console.error('[ChatStore] ❌ Stop failed:', e);
        }

        // Always request a Foundry service restart after a manual stop
        console.log('[ChatStore] 🔄 Requesting Foundry service restart after stop...');
        set({
            operationStatus: {
                type: 'reloading',
                message: 'Restarting Foundry service after stop...',
                startTime: Date.now(),
            },
            statusBarDismissed: false,
        } as any);

        try {
            // Restart the Foundry service
            await invoke('restart_foundry_service');
            console.log('[ChatStore] ✅ Foundry service restart initiated');
            
            // Rewarm the current model after restart
            setTimeout(async () => {
                try {
                    await invoke('rewarm_current_model');
                    console.log('[ChatStore] ✅ Model rewarm initiated');
                } catch (e) {
                    console.error('[ChatStore] ❌ Failed to rewarm model:', e);
                }
            }, 2000);
        } catch (e) {
            console.error('[ChatStore] ❌ Failed to restart Foundry service:', e);
        }
    },
});
