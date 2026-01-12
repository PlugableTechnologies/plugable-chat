import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import type { 
    StartupStateType, 
    SubsystemStatusData, 
    StartupSnapshot,
    ModelStateData,
    ModelInfo,
} from '../types';
import { parseFoundryModelStateEvent } from '../helpers';

/**
 * Parse the backend SubsystemStatus into frontend format
 */
function parseSubsystemStatus(raw: any): SubsystemStatusData {
    const parseResourceStatus = (rs: any) => {
        if (!rs) return { status: 'pending' as const };
        if (typeof rs === 'string') return { status: rs as any };
        if (rs.status) return { status: rs.status, message: rs.message };
        return { status: 'pending' as const };
    };

    return {
        foundry_service: parseResourceStatus(raw?.foundry_service),
        model: parseResourceStatus(raw?.model),
        cpu_embedding: parseResourceStatus(raw?.cpu_embedding),
        mcp_servers: parseResourceStatus(raw?.mcp_servers),
        settings: parseResourceStatus(raw?.settings),
    };
}

/**
 * Parse the backend StartupState into frontend format
 */
function parseStartupState(raw: any): StartupStateType {
    if (!raw) return 'initializing';
    if (typeof raw === 'string') return raw as StartupStateType;
    if (raw.state) return raw.state as StartupStateType;
    return 'initializing';
}

// Dependencies from other slices that we need to update after handshake
interface StartupSliceDeps {
    // Model state
    modelState: ModelStateData;
    isModelReady: boolean;
    // Available models
    availableModels: string[];
    modelInfo: ModelInfo[];
    currentModel: string;
}

export interface StartupSlice {
    // Startup state machine
    startupState: StartupStateType;
    subsystemStatus: SubsystemStatusData;
    isAppReady: boolean;
    handshakeComplete: boolean;
    
    // Computed properties
    isFoundryReady: boolean;
    isChatReady: boolean;
    isEmbeddingReady: boolean;
    
    // Actions
    performHandshake: () => Promise<StartupSnapshot | null>;
    applyStartupSnapshot: (snapshot: StartupSnapshot) => void;
}

const defaultSubsystemStatus: SubsystemStatusData = {
    foundry_service: { status: 'pending' },
    model: { status: 'pending' },
    cpu_embedding: { status: 'pending' },
    mcp_servers: { status: 'pending' },
    settings: { status: 'pending' },
};

export const createStartupSlice: StateCreator<
    StartupSlice & StartupSliceDeps,
    [],
    [],
    StartupSlice
> = (set, get) => ({
    // Initial state
    startupState: 'initializing',
    subsystemStatus: defaultSubsystemStatus,
    isAppReady: false,
    handshakeComplete: false,
    
    // Computed properties (derived from subsystem status)
    isFoundryReady: false,
    isChatReady: false,
    isEmbeddingReady: false,
    
    performHandshake: async () => {
        console.log('[StartupSlice] Performing handshake with backend...');
        try {
            const snapshot = await invoke<StartupSnapshot>('frontend_ready');
            console.log('[StartupSlice] Handshake complete, received snapshot:', snapshot);
            
            // Apply the snapshot
            get().applyStartupSnapshot(snapshot);
            
            return snapshot;
        } catch (e: any) {
            console.error('[StartupSlice] Handshake failed:', e);
            set({
                startupState: 'failed',
                handshakeComplete: false,
            });
            return null;
        }
    },
    
    applyStartupSnapshot: (snapshot: StartupSnapshot) => {
        console.log('[StartupSlice] Applying startup snapshot...');
        
        // Parse startup state
        const startupState = parseStartupState(snapshot.startup_state);
        
        // Parse subsystem status
        const subsystemStatus = parseSubsystemStatus(snapshot.subsystem_status);
        
        // Parse model state using existing helper
        const modelStateData = parseFoundryModelStateEvent({ 
            state: snapshot.model_state, 
            timestamp: snapshot.timestamp 
        });
        
        // Compute derived booleans
        const isFoundryReady = subsystemStatus.foundry_service.status === 'ready';
        const isChatReady = isFoundryReady && subsystemStatus.model.status === 'ready';
        const isEmbeddingReady = subsystemStatus.cpu_embedding.status === 'ready';
        const isAppReady = startupState === 'ready';
        
        // Apply all state atomically
        set({
            // Startup state
            startupState,
            subsystemStatus,
            isAppReady,
            handshakeComplete: true,
            isFoundryReady,
            isChatReady,
            isEmbeddingReady,
            
            // Model state (from model-slice)
            modelState: modelStateData,
            isModelReady: modelStateData.state === 'ready',
            
            // Available models (from model-slice)
            availableModels: snapshot.available_models,
            modelInfo: snapshot.model_info,
            currentModel: snapshot.current_model || 'Loading...',
        } as any);
        
        console.log('[StartupSlice] Snapshot applied. App ready:', isAppReady, 'Chat ready:', isChatReady);
    },
});
