import { create } from 'zustand';
import { invoke } from '../lib/api';

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
}

// Application settings
export interface AppSettings {
    system_prompt: string;
    mcp_servers: McpServerConfig[];
    code_execution_enabled: boolean;
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
    
    // MCP Server statuses
    serverStatuses: Record<string, McpServerStatus>;
    
    // Modal state
    isSettingsOpen: boolean;
    activeTab: 'system-prompt' | 'tools';
    
    // Actions
    openSettings: () => void;
    closeSettings: () => void;
    setActiveTab: (tab: 'system-prompt' | 'tools') => void;
    
    // Settings CRUD
    fetchSettings: () => Promise<void>;
    updateSystemPrompt: (prompt: string) => Promise<void>;
    updateCodeExecutionEnabled: (enabled: boolean) => Promise<void>;
    addMcpServer: (config: McpServerConfig) => Promise<void>;
    updateMcpServer: (config: McpServerConfig) => Promise<void>;
    removeMcpServer: (serverId: string) => Promise<void>;
    
    // MCP Server operations
    connectServer: (serverId: string) => Promise<void>;
    disconnectServer: (serverId: string) => Promise<void>;
    testConnection: (serverId: string) => Promise<boolean>;
}

// Default system prompt
const DEFAULT_SYSTEM_PROMPT = "You are a helpful AI assistant. Be direct and concise in your responses. When you don't know something, say so rather than guessing.";

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

export const useSettingsStore = create<SettingsState>((set, get) => ({
    settings: null,
    isLoading: false,
    error: null,
    serverStatuses: {},
    isSettingsOpen: false,
    activeTab: 'system-prompt',
    
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
            const settings = await invoke<AppSettings>('get_settings');
            console.log('[SettingsStore] Fetched settings:', settings);
            set({ settings, isLoading: false });
            
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
                    code_execution_enabled: false,
                }
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
    
    updateCodeExecutionEnabled: async (enabled: boolean) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        
        // Optimistic update
        set({ 
            settings: { ...currentSettings, code_execution_enabled: enabled },
            error: null 
        });
        
        try {
            await invoke('update_code_execution_enabled', { enabled });
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
        
        // Optimistic update
        const newSettings = {
            ...currentSettings,
            mcp_servers: [...currentSettings.mcp_servers, config],
        };
        set({ settings: newSettings, error: null });
        
        try {
            await invoke('add_mcp_server', { config });
            console.log('[SettingsStore] MCP server added:', config.id);
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
        
        // Optimistic update
        const newSettings = {
            ...currentSettings,
            mcp_servers: currentSettings.mcp_servers.map(s => 
                s.id === config.id ? config : s
            ),
        };
        set({ settings: newSettings, error: null });
        
        try {
            // Backend will sync servers automatically after update
            await invoke('update_mcp_server', { config });
            console.log('[SettingsStore] MCP server updated:', config.id);
            
            // Update connection status
            const isConnected = config.enabled;
            set(state => ({
                serverStatuses: {
                    ...state.serverStatuses,
                    [config.id]: { connected: isConnected }
                }
            }));
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
}));

