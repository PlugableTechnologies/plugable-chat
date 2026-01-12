/**
 * DEPRECATED: This file is maintained for backward compatibility.
 * Import from './chat' directly for the modular store implementation.
 * 
 * The store has been split into slices following the Zustand slices pattern.
 * See src/store/chat/index.ts for the combined store.
 */

// Re-export everything from the new modular structure
export { 
    useChatStore,
    type ChatStoreState,
} from './chat';

// Re-export all types for backward compatibility
export {
    type ReasoningEffort,
    type OperationType,
    type OperationStatus,
    type CachedModel,
    type ModelFamily,
    type ToolFormat,
    type ReasoningFormat,
    type ModelInfo,
    type ModelStateType,
    type ModelStateData,
    type StartupStateType,
    type ResourceStatusType,
    type ResourceStatusData,
    type SubsystemStatusData,
    type StartupSnapshot,
    type ChatSummary,
    type ToolCallRecord,
    type CodeExecutionRecord,
    type RagChunk,
    type Message,
    type FileError,
    type RagIndexResult,
    type AttachedTable,
    type AttachedTool,
    type PendingToolApproval,
    type ToolExecutionState,
    type CodeExecutionState,
    type ToolSearchResult,
    type ToolSearchState,
    isModelStateBlocking,
    getModelStateMessage,
    generateClientChatIdentifier,
    deriveChatTitleFromPrompt,
    deriveChatPreviewFromMessage,
} from './chat';
