import type { StateCreator } from 'zustand';
import { approveToolCall, rejectToolCall } from '../../../lib/tool-calls';
import type { 
    PendingToolApproval, 
    ToolExecutionState, 
    CodeExecutionState, 
    ToolSearchState 
} from '../types';

export interface ToolExecutionSlice {
    // Tool Execution State
    pendingToolApproval: PendingToolApproval | null;
    toolExecution: ToolExecutionState;
    approveCurrentToolCall: () => Promise<void>;
    rejectCurrentToolCall: () => Promise<void>;
    
    // Code Execution State (for code_execution built-in tool)
    codeExecution: CodeExecutionState;
    
    // Tool Search State (for tool_search built-in tool)
    toolSearch: ToolSearchState;
}

export const createToolExecutionSlice: StateCreator<
    ToolExecutionSlice,
    [],
    [],
    ToolExecutionSlice
> = (set, get) => ({
    // Tool Execution State
    pendingToolApproval: null,
    toolExecution: {
        currentTool: null,
        lastResult: null,
        totalIterations: 0,
        hadToolCalls: false,
        lastHeartbeatTs: undefined,
    },
    
    approveCurrentToolCall: async () => {
        const pending = get().pendingToolApproval;
        if (!pending) {
            console.warn('[ChatStore] No pending tool approval to approve');
            return;
        }
        
        console.log(`[ChatStore] Approving tool call: ${pending.approvalKey}`);
        const success = await approveToolCall(pending.approvalKey);
        if (success) {
            set({ pendingToolApproval: null });
        }
    },
    
    rejectCurrentToolCall: async () => {
        const pending = get().pendingToolApproval;
        if (!pending) {
            console.warn('[ChatStore] No pending tool approval to reject');
            return;
        }
        
        console.log(`[ChatStore] Rejecting tool call: ${pending.approvalKey}`);
        const success = await rejectToolCall(pending.approvalKey);
        if (success) {
            set({ pendingToolApproval: null });
        }
    },
    
    // Code Execution State (for code_execution built-in tool)
    codeExecution: {
        isRunning: false,
        currentCode: [],
        innerToolCalls: 0,
        stdout: '',
        stderr: '',
        success: null,
        durationMs: 0,
    },
    
    // Tool Search State (for tool_search built-in tool)
    toolSearch: {
        isSearching: false,
        queries: [],
        results: [],
    },
});
