import type { ParsedToolCall } from '../../lib/tool-calls';

// ============ Basic Types ============

export type ReasoningEffort = 'low' | 'medium' | 'high';

// Operation status types for the status bar
export type OperationType = 'none' | 'downloading' | 'loading' | 'streaming' | 'reloading' | 'indexing' | 'error';

export interface OperationStatus {
    type: OperationType;
    message: string;
    /** For downloads: current file being downloaded */
    currentFile?: string;
    /** Progress percentage (0-100) for downloads */
    progress?: number;
    /** Whether the operation completed (shows "Complete" briefly) */
    completed?: boolean;
    /** Start time for elapsed timer */
    startTime: number;
}

export interface CachedModel {
    alias: string;
    model_id: string;
}

// Model family for format-specific handling
export type ModelFamily = 'gpt_oss' | 'gemma' | 'phi' | 'granite' | 'generic';

// Tool calling format supported by the model
export type ToolFormat = 'openai' | 'hermes' | 'gemini' | 'granite' | 'text_based';

// Reasoning/thinking output format
export type ReasoningFormat = 'none' | 'think_tags' | 'channel_based' | 'thinking_tags';

export interface ModelInfo {
    id: string;
    family: ModelFamily;
    tool_calling: boolean;
    tool_format: ToolFormat;
    vision: boolean;
    reasoning: boolean;
    reasoning_format: ReasoningFormat;
    max_input_tokens: number;
    max_output_tokens: number;
    supports_tool_calling: boolean;
    supports_temperature: boolean;
    supports_top_p: boolean;
    supports_reasoning_effort: boolean;
}

// ============ Application Startup State Machine Types ============
// These mirror the backend StartupState enum for frontend/backend coordination

export type StartupStateType =
    | 'initializing'
    | 'connecting_to_foundry'
    | 'awaiting_frontend'
    | 'ready'
    | 'failed';

export type ResourceStatusType =
    | 'pending'
    | 'initializing'
    | 'ready'
    | 'failed'
    | 'unavailable';

export interface ResourceStatusData {
    status: ResourceStatusType;
    message?: string;
}

export interface SubsystemStatusData {
    foundry_service: ResourceStatusData;
    model: ResourceStatusData;
    cpu_embedding: ResourceStatusData;
    mcp_servers: ResourceStatusData;
    settings: ResourceStatusData;
}

export interface StartupSnapshot {
    startup_state: { state: StartupStateType; message?: string };
    subsystem_status: SubsystemStatusData;
    model_state: { state: ModelStateType; model_id?: string; message?: string };
    available_models: string[];
    model_info: ModelInfo[];
    current_model: string | null;
    timestamp: number;
}

// ============ Model State Machine Types ============
// These mirror the backend ModelState enum for deterministic synchronization

export type ModelStateType =
    | 'initializing'
    | 'ready'
    | 'switching_model'
    | 'unloading_model'
    | 'loading_model'
    | 'error'
    | 'service_unavailable'
    | 'service_restarting'
    | 'reconnecting';

/**
 * Model state data from the backend state machine.
 * The backend is the single source of truth - frontend subscribes to state changes.
 */
export interface ModelStateData {
    state: ModelStateType;
    /** Current or target model ID (depending on state) */
    modelId?: string;
    /** Target model for switch operations */
    targetModel?: string;
    /** Previous model before switch/error */
    previousModel?: string;
    /** Error message when in error state */
    errorMessage?: string;
    /** Timestamp of the state change (ms since epoch) */
    timestamp?: number;
}

// ============ Chat & Message Types ============

export interface ChatSummary {
    id: string;
    title: string;
    preview: string;
    score: number;
    pinned: boolean;
    model?: string;
}

// A single tool call execution record for display
export interface ToolCallRecord {
    id: string;
    server: string;
    tool: string;
    arguments: Record<string, unknown>;
    result: string;
    isError: boolean;
    durationMs?: number;
}

// A code execution record for display
export interface CodeExecutionRecord {
    id: string;
    code: string[];
    stdout: string;
    stderr: string;
    success: boolean;
    durationMs: number;
    innerToolCalls: ToolCallRecord[];
}

export interface RagChunk {
    id: string;
    content: string;
    source_file: string;
    chunk_index: number;
    score: number;
}

export interface Message {
    id: string;
    role: 'user' | 'assistant';
    content: string;
    timestamp: number;
    /** Model ID used for this turn (only for assistant messages) */
    model?: string;
    /** System prompt string used for this assistant turn */
    systemPromptText?: string;
    /** Tool calls made during this assistant message */
    toolCalls?: ToolCallRecord[];
    /** Code execution blocks during this assistant message */
    codeExecutions?: CodeExecutionRecord[];
    /** RAG chunks used as context for this assistant message */
    ragChunks?: RagChunk[];
}

// ============ RAG Types ============

export interface FileError {
    file: string;
    error: string;
}

export interface RagIndexResult {
    total_chunks: number;
    files_processed: number;
    cache_hits: number;
    file_errors: FileError[];
}

// ============ Attachment Types ============

export interface AttachedTable {
    sourceId: string;
    sourceName: string;
    tableFqName: string;
    columnCount: number;
}

export interface AttachedTool {
    key: string;        // "builtin::python_execution" or "mcp-server-id::tool_name"
    name: string;
    server: string;     // "builtin" or MCP server name
    isBuiltin: boolean;
}

// ============ Tool Execution Types ============

// Tool execution state for UI display
export interface PendingToolApproval {
    approvalKey: string;
    calls: ParsedToolCall[];
    iteration: number;
}

export interface ToolExecutionState {
    currentTool: { 
        server: string; 
        tool: string; 
        arguments?: Record<string, unknown>;
        startTime?: number;
    } | null;
    lastResult: { server: string; tool: string; result: string; isError: boolean } | null;
    totalIterations: number;
    hadToolCalls: boolean;
    /** Last heartbeat timestamp (ms since epoch) while tool runs */
    lastHeartbeatTs?: number;
}

// Code execution state for code_execution tool
export interface CodeExecutionState {
    /** Whether code is currently running */
    isRunning: boolean;
    /** The code being executed */
    currentCode: string[];
    /** Number of inner tool calls made during execution */
    innerToolCalls: number;
    /** Stdout from the execution */
    stdout: string;
    /** Stderr from the execution */
    stderr: string;
    /** Whether the execution succeeded */
    success: boolean | null;
    /** Duration of execution in milliseconds */
    durationMs: number;
}

// Tool search result from tool_search tool
export interface ToolSearchResult {
    name: string;
    description?: string;
    score: number;
    server_id: string;
}

// Tool search state
export interface ToolSearchState {
    /** Whether a search is in progress */
    isSearching: boolean;
    /** The queries used for the search */
    queries: string[];
    /** Results from the search */
    results: ToolSearchResult[];
}
