import { create } from 'zustand';
import { invoke } from '../lib/api';
import { FALLBACK_PYTHON_ALLOWED_IMPORTS } from '../lib/python-allowed-imports';

// Transport type matching Rust backend
export type Transport = 
    | { type: 'stdio' }
    | { type: 'sse'; url: string };

// MCP Server configuration
export interface McpServerConfig {
    id: string;
    name: string;
    enabled: boolean;
    transport: Transport;
    command: string | null;
    args: string[];
    env: Record<string, string>;
    auto_approve_tools: boolean;
    python_name?: string;  // Python module name for imports (must be valid Python identifier)
}

// Shared tool-calling format names (must match Rust)
export type ToolCallFormatName = 'hermes' | 'mistral' | 'pythonic' | 'pure_json' | 'code_mode';

export interface ToolCallFormatConfig {
    enabled: ToolCallFormatName[];
    primary: ToolCallFormatName;
}

// Application settings
export interface AppSettings {
    system_prompt: string;
    mcp_servers: McpServerConfig[];
    tool_call_formats: ToolCallFormatConfig;
    tool_system_prompts: Record<string, string>;
    python_execution_enabled: boolean;
    python_tool_calling_enabled: boolean;
    legacy_tool_call_format_enabled: boolean;
}

// Connection status for MCP servers
export interface McpServerStatus {
    connected: boolean;
    error?: string;
    tools?: McpTool[];
}

// MCP Tool definition
export interface McpTool {
    name: string;
    description?: string;
    inputSchema?: Record<string, unknown>;
}

interface SettingsState {
    // Settings data
    settings: AppSettings | null;
    isLoading: boolean;
    error: string | null;
    pythonAllowedImports: string[];
    promptRefreshTick: number;
    
    // MCP Server statuses
    serverStatuses: Record<string, McpServerStatus>;
    
    // Modal state
    isSettingsOpen: boolean;
    activeTab: 'system-prompt' | 'interfaces' | 'tools';
    
    // Actions
    openSettings: () => void;
    closeSettings: () => void;
    setActiveTab: (tab: 'system-prompt' | 'interfaces' | 'tools') => void;
    
    // Settings CRUD
    fetchSettings: () => Promise<void>;
    updateSystemPrompt: (prompt: string) => Promise<void>;
    updateToolCallFormats: (config: ToolCallFormatConfig) => Promise<void>;
    updateCodeExecutionEnabled: (enabled: boolean) => Promise<void>;
    addMcpServer: (config: McpServerConfig) => Promise<void>;
    updateMcpServer: (config: McpServerConfig) => Promise<void>;
    removeMcpServer: (serverId: string) => Promise<void>;
    updateToolSystemPrompt: (serverId: string, toolName: string, prompt: string) => Promise<void>;
    bumpPromptRefresh: () => void;
    refreshMcpTools: (serverId: string) => Promise<McpTool[]>;
    
    // MCP Server operations
    connectServer: (serverId: string) => Promise<void>;
    disconnectServer: (serverId: string) => Promise<void>;
    testConnection: (serverId: string) => Promise<boolean>;
}

// Default system prompt - exported so UI can offer reset
export const DEFAULT_SYSTEM_PROMPT = "You are a helpful AI assistant. Be direct and concise in your responses. When you don't know something, say so rather than guessing.";

export const DEFAULT_TOOL_CALL_FORMATS: ToolCallFormatConfig = {
    enabled: ['hermes', 'code_mode'],
    primary: 'code_mode',
};

function normalizeToolCallFormats(config: ToolCallFormatConfig): ToolCallFormatConfig {
    const dedupedEnabled: ToolCallFormatName[] = [];
    for (const fmt of config.enabled || []) {
        if (!dedupedEnabled.includes(fmt)) {
            dedupedEnabled.push(fmt);
        }
    }
    const enabled = dedupedEnabled.length > 0 ? dedupedEnabled : [...DEFAULT_TOOL_CALL_FORMATS.enabled];
    const primary = enabled.includes(config.primary) ? config.primary : enabled[0];
    return { enabled, primary };
}

// Helper to create a new server config
export function createNewServerConfig(): McpServerConfig {
    return {
        id: `mcp-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
        name: 'New MCP Server',
        enabled: false,
        transport: { type: 'stdio' },
        command: null,
        args: [],
        env: {},
        auto_approve_tools: false,
    };
}

function toPythonIdentifier(name: string): string {
    let result = '';
    let lastUnderscore = false;
    for (const ch of name) {
        if (/[a-zA-Z0-9]/.test(ch)) {
            const lower = ch.toLowerCase();
            result += lower;
            lastUnderscore = false;
        } else if (ch === ' ' || ch === '-' || ch === '_' || ch === '.') {
            if (!lastUnderscore && result.length > 0) {
                result += '_';
                lastUnderscore = true;
            }
        }
    }
    while (result.endsWith('_')) {
        result = result.slice(0, -1);
    }
    if (!result) {
        result = 'module';
    }
    if (/^[0-9]/.test(result)) {
        result = `_${result}`;
    }
    const PYTHON_KEYWORDS = new Set([
        'false','none','true','and','as','assert','async','await','break','class','continue','def','del','elif','else','except','finally','for','from','global','if','import','in','is','lambda','nonlocal','not','or','pass','raise','return','try','while','with','yield'
    ]);
    if (PYTHON_KEYWORDS.has(result)) {
        result = `${result}_`;
    }
    return result;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
    settings: null,
    isLoading: false,
    error: null,
    pythonAllowedImports: FALLBACK_PYTHON_ALLOWED_IMPORTS,
    promptRefreshTick: 0,
    serverStatuses: {},
    isSettingsOpen: false,
    activeTab: 'system-prompt',
    bumpPromptRefresh: () => set(state => ({ promptRefreshTick: state.promptRefreshTick + 1 })),
    
    openSettings: () => {
        set({ isSettingsOpen: true });
        // Fetch latest settings when opening
        get().fetchSettings();
    },
    
    closeSettings: () => {
        set({ isSettingsOpen: false });
    },
    
    setActiveTab: (tab) => {
        set({ activeTab: tab });
    },
    
    fetchSettings: async () => {
        set({ isLoading: true, error: null });
        try {
            const [settings, allowedImportsRaw] = await Promise.all([
                invoke<AppSettings>('get_settings'),
                invoke<string[]>('get_python_allowed_imports').catch((err) => {
                    console.error('[SettingsStore] Failed to fetch allowed imports:', err);
                    return FALLBACK_PYTHON_ALLOWED_IMPORTS;
                }),
            ]);
            const normalizedFormats = normalizeToolCallFormats(settings.tool_call_formats || DEFAULT_TOOL_CALL_FORMATS);
            const mergedSettings: AppSettings = { ...settings, tool_call_formats: normalizedFormats };
            const allowedImports = (allowedImportsRaw && allowedImportsRaw.length > 0)
                ? allowedImportsRaw
                : FALLBACK_PYTHON_ALLOWED_IMPORTS;
            console.log('[SettingsStore] Fetched settings:', settings);
            set({ settings: mergedSettings, pythonAllowedImports: allowedImports, isLoading: false });
            
            // Sync MCP servers after fetching settings
            try {
                console.log('[SettingsStore] Syncing MCP servers...');
                const results = await invoke<{ server_id: string; success: boolean; error: string | null }[]>('sync_mcp_servers');
                console.log('[SettingsStore] MCP server sync results:', results);
                
                // Update server statuses based on sync results
                const newStatuses: Record<string, McpServerStatus> = {};
                for (const result of results) {
                    newStatuses[result.server_id] = { 
                        connected: result.success,
                        error: result.error || undefined,
                    };
                }
                set(state => ({
                    serverStatuses: { ...state.serverStatuses, ...newStatuses }
                }));
            } catch (syncError: any) {
                console.error('[SettingsStore] MCP server sync failed:', syncError);
            }
        } catch (e: any) {
            console.error('[SettingsStore] Failed to fetch settings:', e);
            set({ 
                error: `Failed to load settings: ${e.message || e}`,
                isLoading: false,
                // Provide defaults on error
                settings: {
                    system_prompt: DEFAULT_SYSTEM_PROMPT,
                    mcp_servers: [],
                    tool_call_formats: DEFAULT_TOOL_CALL_FORMATS,
                    tool_system_prompts: {},
                    python_execution_enabled: false,
                    python_tool_calling_enabled: true,
                    legacy_tool_call_format_enabled: false,
                },
                pythonAllowedImports: FALLBACK_PYTHON_ALLOWED_IMPORTS,
            });
        }
    },
    
    updateSystemPrompt: async (prompt: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        
        // Optimistic update
        set({ 
            settings: { ...currentSettings, system_prompt: prompt },
            error: null 
        });
        
        try {
            await invoke('update_system_prompt', { prompt });
            console.log('[SettingsStore] System prompt updated');
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update system prompt:', e);
            // Revert on error
            set({ 
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },

    updateToolCallFormats: async (config: ToolCallFormatConfig) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;

        const normalized = normalizeToolCallFormats(config);

        // Optimistic update
        set({
            settings: { ...currentSettings, tool_call_formats: normalized },
            error: null,
        });

        try {
            await invoke('update_tool_call_formats', { config: normalized });
            console.log('[SettingsStore] Tool call formats updated', normalized);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update tool call formats:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`,
            });
        }
    },
    
    updateToolSystemPrompt: async (serverId: string, toolName: string, prompt: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        
        const key = `${serverId}::${toolName}`;
        const newPrompts = { ...currentSettings.tool_system_prompts };
        if (prompt.trim()) {
            newPrompts[key] = prompt;
        } else {
            delete newPrompts[key];
        }
        
        // Optimistic update
        set({ 
            settings: { ...currentSettings, tool_system_prompts: newPrompts },
            error: null 
        });
        
        try {
            await invoke('update_tool_system_prompt', { serverId, toolName, prompt });
            console.log('[SettingsStore] Tool system prompt updated:', key);
            get().bumpPromptRefresh();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update tool system prompt:', e);
            set({ 
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },
    
    updateCodeExecutionEnabled: async (enabled: boolean) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        
        // Optimistic update
        set({ 
            settings: { ...currentSettings, python_execution_enabled: enabled },
            error: null 
        });
        
        try {
            await invoke('update_python_execution_enabled', { enabled });
            console.log('[SettingsStore] Code execution enabled updated:', enabled);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update code execution enabled:', e);
            // Revert on error
            set({ 
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },
    
    addMcpServer: async (config: McpServerConfig) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const sanitizedName = toPythonIdentifier(config.name);
        const newConfig = { ...config, name: sanitizedName, python_name: sanitizedName };
        
        // Optimistic update
        const newSettings = {
            ...currentSettings,
            mcp_servers: [...currentSettings.mcp_servers, newConfig],
        };
        set({ settings: newSettings, error: null });
        
        try {
            await invoke('add_mcp_server', { config: newConfig });
            console.log('[SettingsStore] MCP server added:', newConfig.id);
            get().bumpPromptRefresh();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to add MCP server:', e);
            // Revert on error
            set({ 
                settings: currentSettings,
                error: `Failed to add server: ${e.message || e}`
            });
        }
    },
    
    updateMcpServer: async (config: McpServerConfig) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const sanitizedName = toPythonIdentifier(config.name);
        const newConfig = { ...config, name: sanitizedName, python_name: sanitizedName };
        
        // Optimistic update
        const newSettings = {
            ...currentSettings,
            mcp_servers: currentSettings.mcp_servers.map(s => 
                s.id === config.id ? newConfig : s
            ),
        };
        set({ settings: newSettings, error: null });
        
        try {
            // Backend will sync servers automatically after update
            await invoke('update_mcp_server', { config: newConfig });
            console.log('[SettingsStore] MCP server updated:', newConfig.id);
            
            // Update connection status
            const isConnected = newConfig.enabled;
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [newConfig.id]: { connected: isConnected }
                }
            }));

            // Refresh tool list when the server is enabled so UI + prompts are current
            if (isConnected) {
                try {
                    await get().refreshMcpTools(newConfig.id);
                } catch (toolErr: any) {
                    console.error('[SettingsStore] Failed to refresh MCP tools after enable:', toolErr);
                }
            } else {
                // Clear any cached tools when disabling
                set(state => ({
                    serverStatuses: {
                        ...state.serverStatuses,
                        [newConfig.id]: { connected: false, tools: [] }
                    }
                }));
            }

            get().bumpPromptRefresh();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update MCP server:', e);
            // Revert on error
            set({ 
                settings: currentSettings,
                error: `Failed to update server: ${e.message || e}`
            });
        }
    },
    
    removeMcpServer: async (serverId: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        
        // Optimistic update
        const newSettings = {
            ...currentSettings,
            mcp_servers: currentSettings.mcp_servers.filter(s => s.id !== serverId),
        };
        set({ settings: newSettings, error: null });
        
        try {
            await invoke('remove_mcp_server', { serverId });
            console.log('[SettingsStore] MCP server removed:', serverId);
            get().bumpPromptRefresh();
            
            // Clean up status
            const newStatuses = { ...get().serverStatuses };
            delete newStatuses[serverId];
            set({ serverStatuses: newStatuses });
        } catch (e: any) {
            console.error('[SettingsStore] Failed to remove MCP server:', e);
            // Revert on error
            set({ 
                settings: currentSettings,
                error: `Failed to remove server: ${e.message || e}`
            });
        }
    },
    
    connectServer: async (serverId: string) => {
        // Update status to connecting
        set(state => ({
            serverStatuses: {
                ...state.serverStatuses,
                [serverId]: { connected: false, error: undefined }
            }
        }));
        
        try {
            await invoke('connect_mcp_server', { serverId });
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [serverId]: { connected: true }
                }
            }));
            console.log('[SettingsStore] MCP server connected:', serverId);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to connect MCP server:', e);
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [serverId]: { connected: false, error: e.message || String(e) }
                }
            }));
        }
    },
    
    disconnectServer: async (serverId: string) => {
        try {
            await invoke('disconnect_mcp_server', { serverId });
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [serverId]: { connected: false }
                }
            }));
            console.log('[SettingsStore] MCP server disconnected:', serverId);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to disconnect MCP server:', e);
        }
    },
    
    testConnection: async (serverId: string) => {
        try {
            const result = await invoke<boolean>('test_mcp_connection', { serverId });
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [serverId]: { connected: result }
                }
            }));
            return result;
        } catch (e: any) {
            console.error('[SettingsStore] Connection test failed:', e);
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [serverId]: { connected: false, error: e.message || String(e) }
                }
            }));
            return false;
        }
    },

    refreshMcpTools: async (serverId: string) => {
        try {
            const tools = await invoke<McpTool[]>('list_mcp_tools', { serverId });
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [serverId]: { ...(state.serverStatuses[serverId] || { connected: true }), tools, error: undefined }
                }
            }));
            return tools;
        } catch (e: any) {
            console.error('[SettingsStore] Failed to refresh MCP tools:', e);
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [serverId]: { ...(state.serverStatuses[serverId] || { connected: false }), error: e.message || String(e) }
                }
            }));
            throw e;
        }
    },
}));

