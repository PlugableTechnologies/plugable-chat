import type { FoundryCatalogModel } from '../../lib/api';
import type { McpTool } from '../../store/settings-store';

// Test result for MCP server connection testing
export interface McpServerTestResult {
    success: boolean;
    tools?: McpTool[];
    error?: string;
}

// Tool parameter extracted from JSON schema
export type ToolParameter = {
    name: string;
    type: string;
    description: string;
    required: boolean;
};

// Device filter for model catalog
export type ModelDeviceFilter = 'Auto' | 'CPU' | 'GPU' | 'NPU';

// Props for the Foundry model card component
export interface FoundryModelCardProps {
    model: FoundryCatalogModel;
    isCached: boolean;
    isLoaded: boolean;
    isDownloading: boolean;
    downloadProgress?: { file: string; progress: number };
    onDownload: () => void;
    onUnload: () => void;
    onRemove: () => void;
}

// State preview for debugging state machine
export interface StatePreview {
    name: string;
    description: string;
    available_tools: string[];
    prompt_additions: string[];
    is_possible: boolean;
}

// Schema refresh progress event
export interface SchemaRefreshProgress {
    message: string;
    source_name: string;
    current_table: string | null;
    tables_done: number;
    tables_total: number;
    is_complete: boolean;
    error: string | null;
}

// Schema refresh status with timing
export interface SchemaRefreshStatus extends SchemaRefreshProgress {
    startTime: number;
}

// Per-source error information from schema refresh
export interface SchemaRefreshError {
    source_id: string;
    source_name: string;
    error: string;
    details: string | null;
}

// Schema refresh result with detailed per-source status
export interface SchemaRefreshResult {
    sources: unknown[];
    errors: SchemaRefreshError[];
}

// Helper function to extract tool parameters from JSON schema
export function extractToolParameters(inputSchema?: Record<string, unknown>): ToolParameter[] {
    if (!inputSchema) return [];
    const propertiesRaw = (inputSchema as any).properties;
    if (!propertiesRaw || typeof propertiesRaw !== 'object') return [];

    const requiredList = Array.isArray((inputSchema as any).required)
        ? (inputSchema as any).required.filter((item: unknown): item is string => typeof item === 'string')
        : [];

    const params: ToolParameter[] = Object.entries(propertiesRaw)
        .filter(([, value]) => value && typeof value === 'object')
        .map(([name, value]) => {
            const schema = value as Record<string, any>;
            const type = typeof schema.type === 'string' ? schema.type : 'any';
            const description = typeof schema.description === 'string' ? schema.description : '';
            const required = requiredList.includes(name);
            return { name, type, description, required };
        });

    return params.sort((a, b) => {
        if (a.required !== b.required) {
            return a.required ? -1 : 1;
        }
        return a.name.localeCompare(b.name);
    });
}
