import { create } from 'zustand';

// Import slices
import { createMessageSlice, type MessageSlice } from './slices/message-slice';
import { createEditorSlice, type EditorSlice } from './slices/editor-slice';
import { createOperationStatusSlice, type OperationStatusSlice } from './slices/operation-status-slice';
import { createAttachmentSlice, type AttachmentSlice } from './slices/attachment-slice';
import { createRagSlice, type RagSlice } from './slices/rag-slice';
import { createToolExecutionSlice, type ToolExecutionSlice } from './slices/tool-execution-slice';
import { createStreamingSlice, type StreamingSlice } from './slices/streaming-slice';
import { createChatHistorySlice, type ChatHistorySlice } from './slices/chat-history-slice';
import { createModelSlice, type ModelSlice } from './slices/model-slice';
import { createStartupSlice, type StartupSlice } from './slices/startup-slice';
import { createListenerSlice, type ListenerSlice } from './listeners';
import { createSendMessageSlice, type SendMessageSlice } from './actions/send-message';

// Combined store state type
export type ChatStoreState = 
    & MessageSlice
    & EditorSlice
    & OperationStatusSlice
    & AttachmentSlice
    & RagSlice
    & ToolExecutionSlice
    & StreamingSlice
    & ChatHistorySlice
    & ModelSlice
    & StartupSlice
    & ListenerSlice
    & SendMessageSlice
    & {
        // Additional shared state
        backendError: string | null;
    };

// Create the combined store
export const useChatStore = create<ChatStoreState>()((...a) => ({
    // Message slice
    ...createMessageSlice(...a),
    
    // Editor slice
    ...createEditorSlice(...a),
    
    // Operation status slice
    ...createOperationStatusSlice(...a),
    
    // Attachment slice
    ...createAttachmentSlice(...a),
    
    // RAG slice
    ...createRagSlice(...(a as Parameters<typeof createRagSlice>)),
    
    // Tool execution slice
    ...createToolExecutionSlice(...a),
    
    // Streaming slice
    ...createStreamingSlice(...(a as Parameters<typeof createStreamingSlice>)),
    
    // Chat history slice
    ...createChatHistorySlice(...(a as Parameters<typeof createChatHistorySlice>)),
    
    // Model slice
    ...createModelSlice(...(a as Parameters<typeof createModelSlice>)),
    
    // Startup slice
    ...createStartupSlice(...(a as Parameters<typeof createStartupSlice>)),
    
    // Listener slice
    ...createListenerSlice(...(a as Parameters<typeof createListenerSlice>)),
    
    // Send message actions
    ...createSendMessageSlice(...(a as Parameters<typeof createSendMessageSlice>)),
    
    // Additional shared state
    backendError: null,
}));

// Re-export types for backward compatibility
export * from './types';
export { 
    isModelStateBlocking, 
    getModelStateMessage,
    generateClientChatIdentifier,
    deriveChatTitleFromPrompt,
    deriveChatPreviewFromMessage
} from './helpers';
