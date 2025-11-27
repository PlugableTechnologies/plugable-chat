import { invoke } from './api';

// Parsed tool call from backend
export interface ParsedToolCall {
    server: string;
    tool: string;
    arguments: Record<string, unknown>;
    raw: string;
}

// Tool call execution status
export type ToolCallStatus = 'pending' | 'approved' | 'rejected' | 'executing' | 'completed' | 'error';

// Tool call with execution state
export interface ToolCallState extends ParsedToolCall {
    id: string;
    approvalKey?: string;
    status: ToolCallStatus;
    result?: string;
    error?: string;
}

// Event payloads from backend
export interface ToolCallsPendingEvent {
    approval_key: string;
    calls: ParsedToolCall[];
    iteration: number;
}

export interface ToolExecutingEvent {
    server: string;
    tool: string;
    arguments: Record<string, unknown>;
}

export interface ToolResultEvent {
    server: string;
    tool: string;
    result: string;
    is_error: boolean;
}

export interface ToolLoopFinishedEvent {
    iterations: number;
    had_tool_calls: boolean;
}

// Detect tool calls in content
export async function detectToolCalls(content: string): Promise<ParsedToolCall[]> {
    try {
        const calls = await invoke<ParsedToolCall[]>('detect_tool_calls', { content });
        return calls;
    } catch (e) {
        console.error('[ToolCalls] Failed to detect tool calls:', e);
        return [];
    }
}

// Execute a tool call
export async function executeToolCall(
    serverId: string,
    toolName: string,
    args: Record<string, unknown>
): Promise<string> {
    const result = await invoke<string>('execute_tool_call', {
        serverId,
        toolName,
        arguments: args,
    });
    return result;
}

// Parse tool calls from content (client-side fallback)
export function parseToolCallsLocal(content: string): ParsedToolCall[] {
    const calls: ParsedToolCall[] = [];
    const regex = /<tool_call>(.*?)<\/tool_call>/gs;
    
    let match;
    while ((match = regex.exec(content)) !== null) {
        try {
            const parsed = JSON.parse(match[1].trim());
            if (parsed.server && parsed.tool) {
                calls.push({
                    server: parsed.server,
                    tool: parsed.tool,
                    arguments: parsed.arguments || {},
                    raw: match[0],
                });
            }
        } catch (e) {
            // Invalid JSON, skip
        }
    }
    
    return calls;
}

// Check if content contains tool calls
export function hasToolCalls(content: string): boolean {
    return /<tool_call>.*?<\/tool_call>/s.test(content);
}

// Format tool result for injection into chat
export function formatToolResult(call: ParsedToolCall, result: string, isError: boolean): string {
    return `<tool_result server="${call.server}" tool="${call.tool}" ${isError ? 'error="true"' : ''}>
${result}
</tool_result>`;
}

// Approve a pending tool call
export async function approveToolCall(approvalKey: string): Promise<boolean> {
    try {
        const result = await invoke<boolean>('approve_tool_call', { approvalKey });
        return result;
    } catch (e) {
        console.error('[ToolCalls] Failed to approve tool call:', e);
        return false;
    }
}

// Reject a pending tool call
export async function rejectToolCall(approvalKey: string): Promise<boolean> {
    try {
        const result = await invoke<boolean>('reject_tool_call', { approvalKey });
        return result;
    } catch (e) {
        console.error('[ToolCalls] Failed to reject tool call:', e);
        return false;
    }
}

// Get list of pending tool approvals
export async function getPendingToolApprovals(): Promise<string[]> {
    try {
        return await invoke<string[]>('get_pending_tool_approvals');
    } catch (e) {
        console.error('[ToolCalls] Failed to get pending approvals:', e);
        return [];
    }
}

