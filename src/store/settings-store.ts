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
    defer_tools?: boolean;
    python_name?: string;  // Derived from server name for Python imports
}

// Shared tool-calling format names (must match Rust)
export type ToolCallFormatName = 'native' | 'hermes' | 'mistral' | 'pythonic' | 'pure_json' | 'code_mode';

export interface ToolCallFormatConfig {
    enabled: ToolCallFormatName[];
    primary: ToolCallFormatName;
}

// Chat formats (per-model)
export type ChatFormatName = 'openai_completions' | 'openai_responses';

// Database source kinds (must match Rust SupportedDatabaseKind)
export type SupportedDatabaseKind = 'bigquery' | 'postgres' | 'mysql' | 'sqlite' | 'spanner';

// Individual database source configuration
export interface DatabaseSourceConfig {
    id: string;
    name: string;
    kind: SupportedDatabaseKind;
    enabled: boolean;
    transport: Transport;
    command: string | null;
    args: string[];
    env: Record<string, string>;
    auto_approve_tools: boolean;
    defer_tools: boolean;
    project_id?: string; // Optional for BigQuery or other sources
    dataset_allowlist?: string; // Comma-separated dataset list (BigQuery only)
    table_allowlist?: string; // Comma-separated table list (BigQuery only)
}

// Database Toolbox configuration
export interface DatabaseToolboxConfig {
    enabled: boolean;
    sources: DatabaseSourceConfig[];
}

// Application settings
export interface AppSettings {
    system_prompt: string;
    mcp_servers: McpServerConfig[];
    chat_format_default: ChatFormatName;
    chat_format_overrides: Record<string, ChatFormatName>;
    tool_call_formats: ToolCallFormatConfig;
    tool_system_prompts: Record<string, string>;
    tool_search_max_results: number;
    tool_search_enabled: boolean;
    python_execution_enabled: boolean;
    python_tool_calling_enabled: boolean;
    legacy_tool_call_format_enabled: boolean;
    tool_use_examples_enabled: boolean;
    tool_use_examples_max: number;
    // Database built-ins
    database_toolbox: DatabaseToolboxConfig;
    schema_search_enabled: boolean;
    sql_select_enabled: boolean;
    // Note: schema_search_internal_only was removed - it's now auto-derived
    // when sql_select is enabled but schema_search is not
    // Relevancy thresholds for state machine
    rag_chunk_min_relevancy: number;
    schema_relevancy_threshold: number;
    rag_dominant_threshold: number;
    // Always-on configuration
    always_on_builtin_tools: string[];
    always_on_mcp_tools: string[];
    always_on_tables: AlwaysOnTableConfig[];
    always_on_rag_paths: string[];
}

// Always-on table configuration
export interface AlwaysOnTableConfig {
    source_id: string;
    table_fq_name: string;
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
    inputExamples?: any[];
    allowed_callers?: string[];
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
    activeTab: 'models' | 'system-prompt' | 'interfaces' | 'builtins' | 'tools' | 'databases' | 'schemas' | 'files';

    // Actions
    openSettings: () => void;
    closeSettings: () => void;
    setActiveTab: (tab: 'models' | 'system-prompt' | 'interfaces' | 'builtins' | 'tools' | 'databases' | 'schemas' | 'files') => void;

    // Settings CRUD
    fetchSettings: () => Promise<void>;
    updateSystemPrompt: (prompt: string) => Promise<void>;
    updateToolCallFormats: (config: ToolCallFormatConfig) => Promise<void>;
    updateChatFormat: (modelId: string, format: ChatFormatName) => Promise<void>;
    updateCodeExecutionEnabled: (enabled: boolean) => Promise<void>;
    updateNativeToolCallingEnabled: (enabled: boolean) => Promise<void>;
    updateToolSearchEnabled: (enabled: boolean) => Promise<void>;
    updateToolSearchMaxResults: (maxResults: number) => Promise<void>;
    updateToolExamplesEnabled: (enabled: boolean) => Promise<void>;
    updateToolExamplesMax: (maxExamples: number) => Promise<void>;
    updateSchemaSearchEnabled: (enabled: boolean) => Promise<void>;
    updateSqlSelectEnabled: (enabled: boolean) => Promise<void>;
    // Relevancy thresholds for state machine
    updateRagChunkMinRelevancy: (value: number) => Promise<void>;
    updateSchemaRelevancyThreshold: (value: number) => Promise<void>;
    updateRagDominantThreshold: (value: number) => Promise<void>;
    updateDatabaseToolboxConfig: (config: DatabaseToolboxConfig) => Promise<void>;
    addMcpServer: (config: McpServerConfig) => Promise<void>;
    updateMcpServer: (config: McpServerConfig) => Promise<void>;
    removeMcpServer: (serverId: string) => Promise<void>;
    updateToolSystemPrompt: (serverId: string, toolName: string, prompt: string) => Promise<void>;
    bumpPromptRefresh: () => void;
    refreshMcpTools: (serverId: string) => Promise<McpTool[]>;

    // Always-on configuration
    addAlwaysOnBuiltinTool: (toolName: string) => Promise<void>;
    removeAlwaysOnBuiltinTool: (toolName: string) => Promise<void>;
    addAlwaysOnMcpTool: (toolKey: string) => Promise<void>;
    removeAlwaysOnMcpTool: (toolKey: string) => Promise<void>;
    addAlwaysOnTable: (sourceId: string, tableFqName: string) => Promise<void>;
    removeAlwaysOnTable: (sourceId: string, tableFqName: string) => Promise<void>;
    addAlwaysOnRagPath: (path: string) => Promise<void>;
    removeAlwaysOnRagPath: (path: string) => Promise<void>;

    // MCP Server operations
    connectServer: (serverId: string) => Promise<void>;
    disconnectServer: (serverId: string) => Promise<void>;
    testConnection: (serverId: string) => Promise<boolean>;
}

// Default system prompt - exported so UI can offer reset
export const DEFAULT_SYSTEM_PROMPT = "You are a helpful AI assistant. Be direct and concise in your responses. When you don't know something, say so rather than guessing.";

export const DEFAULT_TOOL_CALL_FORMATS: ToolCallFormatConfig = {
    enabled: ['native', 'hermes', 'code_mode'],
    primary: 'native',
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
    const defaultName = 'New MCP Server';
    return {
        id: `mcp-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
        name: defaultName,
        enabled: false,
        transport: { type: 'stdio' },
        command: null,
        args: [],
        env: {},
        auto_approve_tools: false,
        defer_tools: true,
        python_name: toPythonIdentifier(defaultName),
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
        'false', 'none', 'true', 'and', 'as', 'assert', 'async', 'await', 'break', 'class', 'continue', 'def', 'del', 'elif', 'else', 'except', 'finally', 'for', 'from', 'global', 'if', 'import', 'in', 'is', 'lambda', 'nonlocal', 'not', 'or', 'pass', 'raise', 'return', 'try', 'while', 'with', 'yield'
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
    activeTab: 'models',
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
            const normalizedServers = (settings.mcp_servers || []).map((server) => {
                const isTestServer = server.id === 'mcp-test-server';
                const name = isTestServer ? 'mcp_test_server' : server.name;
                return {
                    ...server,
                    name,
                    defer_tools: isTestServer ? false : (server.defer_tools ?? true),
                    python_name: server.python_name ?? toPythonIdentifier(name || server.id),
                };
            });
            const normalizedDbSources: DatabaseSourceConfig[] = (settings.database_toolbox?.sources || []).map((source) => ({
                ...source,
                transport: source.transport ?? { type: 'stdio' },
                command: source.command ?? null,
                args: source.args ?? [],
                env: source.env ?? {},
                auto_approve_tools: true, // Always true for database sources
                defer_tools: source.defer_tools ?? true,
                dataset_allowlist: source.dataset_allowlist ?? '',
                table_allowlist: source.table_allowlist ?? '',
            }));
            const mergedSettings: AppSettings = {
                ...settings,
                mcp_servers: normalizedServers,
                tool_call_formats: normalizedFormats,
                chat_format_default: settings.chat_format_default ?? 'openai_completions',
                chat_format_overrides: settings.chat_format_overrides ?? {},
                tool_search_enabled: settings.tool_search_enabled ?? false,
                tool_search_max_results: settings.tool_search_max_results ?? 3,
                tool_use_examples_enabled: settings.tool_use_examples_enabled ?? false,
                tool_use_examples_max: settings.tool_use_examples_max ?? 2,
                database_toolbox: {
                    enabled: settings.database_toolbox?.enabled ?? false,
                    sources: normalizedDbSources,
                },
                // Always-on configuration defaults
                always_on_builtin_tools: settings.always_on_builtin_tools ?? [],
                always_on_mcp_tools: settings.always_on_mcp_tools ?? [],
                always_on_tables: settings.always_on_tables ?? [],
                always_on_rag_paths: settings.always_on_rag_paths ?? [],
            };
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
                    chat_format_default: 'openai_completions',
                    chat_format_overrides: {},
                    tool_call_formats: DEFAULT_TOOL_CALL_FORMATS,
                    tool_system_prompts: {},
                    tool_search_max_results: 3,
                    tool_search_enabled: false,
                    python_execution_enabled: false,
                    python_tool_calling_enabled: true,
                    legacy_tool_call_format_enabled: false,
                    tool_use_examples_enabled: false,
                    tool_use_examples_max: 2,
                    database_toolbox: {
                        enabled: false,
                        sources: [],
                    },
                    schema_search_enabled: false,
                    sql_select_enabled: false,
                    // Relevancy thresholds defaults
                    rag_chunk_min_relevancy: 0.3,
                    schema_relevancy_threshold: 0.4,
                    rag_dominant_threshold: 0.6,
                    // Always-on configuration defaults
                    always_on_builtin_tools: [],
                    always_on_mcp_tools: [],
                    always_on_tables: [],
                    always_on_rag_paths: [],
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
            // Re-fetch to ensure local state stays in sync with backend normalization/persistence
            await get().fetchSettings();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update tool call formats:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`,
            });
        }
    },

    updateChatFormat: async (modelId: string, format: ChatFormatName) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;

        const overrides = { ...(currentSettings.chat_format_overrides || {}) };
        if (format === currentSettings.chat_format_default) {
            delete overrides[modelId];
        } else {
            overrides[modelId] = format;
        }

        const nextSettings = { ...currentSettings, chat_format_overrides: overrides };

        // Optimistic update
        set({ settings: nextSettings, error: null });

        try {
            await invoke('update_chat_format', { modelId, format });
            console.log('[SettingsStore] Chat format updated', { modelId, format });
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update chat format:', e);
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

    updateNativeToolCallingEnabled: async (enabled: boolean) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;

        // Compute new format config with native enabled/disabled
        const currentFormats = currentSettings.tool_call_formats;
        let newEnabled = [...currentFormats.enabled];
        let newPrimary = currentFormats.primary;

        if (enabled) {
            // Add native if not already present
            if (!newEnabled.includes('native')) {
                newEnabled = ['native', ...newEnabled];
            }
            // Set native as primary
            newPrimary = 'native';
        } else {
            // Remove native from enabled
            newEnabled = newEnabled.filter(f => f !== 'native');
            // If native was primary, fall back to first available
            if (newPrimary === 'native') {
                newPrimary = newEnabled[0] || 'hermes';
            }
        }

        const newFormats: ToolCallFormatConfig = {
            enabled: newEnabled,
            primary: newPrimary,
        };

        // Optimistic update
        set({
            settings: { ...currentSettings, tool_call_formats: newFormats },
            error: null
        });

        try {
            await invoke('update_native_tool_calling_enabled', { enabled });
            console.log('[SettingsStore] Native tool calling updated:', enabled);
            // Re-fetch to sync with backend
            await get().fetchSettings();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update native tool calling:', e);
            // Revert on error
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },

    updateToolSearchEnabled: async (enabled: boolean) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;

        // Optimistic update
        set({
            settings: { ...currentSettings, tool_search_enabled: enabled },
            error: null,
        });

        try {
            await invoke('update_tool_search_enabled', { enabled });
            console.log('[SettingsStore] tool_search_enabled updated:', enabled);
            get().bumpPromptRefresh();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update tool_search_enabled:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`,
            });
        }
    },
    updateToolSearchMaxResults: async (maxResults: number) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const capped = Math.min(Math.max(Math.floor(maxResults), 1), 20);
        const newSettings = { ...currentSettings, tool_search_max_results: capped };
        set({ settings: newSettings, error: null });
        try {
            await invoke('save_app_settings', { newSettings });
            console.log('[SettingsStore] tool_search_max_results updated:', capped);
            get().bumpPromptRefresh();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update tool_search_max_results:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`,
            });
        }
    },
    updateToolExamplesEnabled: async (enabled: boolean) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const newSettings = { ...currentSettings, tool_use_examples_enabled: enabled };
        set({ settings: newSettings, error: null });
        try {
            await invoke('save_app_settings', { newSettings });
            console.log('[SettingsStore] tool_use_examples_enabled updated:', enabled);
            get().bumpPromptRefresh();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update tool_use_examples_enabled:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`,
            });
        }
    },
    updateToolExamplesMax: async (maxExamples: number) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const capped = Math.min(Math.max(Math.floor(maxExamples), 1), 5);
        const newSettings = { ...currentSettings, tool_use_examples_max: capped };
        set({ settings: newSettings, error: null });
        try {
            await invoke('save_app_settings', { newSettings });
            console.log('[SettingsStore] tool_use_examples_max updated:', capped);
            get().bumpPromptRefresh();
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update tool_use_examples_max:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`,
            });
        }
    },

    updateSchemaSearchEnabled: async (enabled: boolean) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        set({
            settings: { ...currentSettings, schema_search_enabled: enabled },
            error: null
        });
        try {
            await invoke('update_schema_search_enabled', { enabled });
            console.log('[SettingsStore] schema_search_enabled updated:', enabled);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update schema_search_enabled:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },

    // Note: updateSchemaSearchInternalOnly was removed - internal schema search
    // is now auto-derived when sql_select is enabled but schema_search is not

    updateSqlSelectEnabled: async (enabled: boolean) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        set({
            settings: { ...currentSettings, sql_select_enabled: enabled },
            error: null
        });
        try {
            await invoke('update_sql_select_enabled', { enabled });
            console.log('[SettingsStore] sql_select_enabled updated:', enabled);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update sql_select_enabled:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },

    // Relevancy threshold updates
    updateRagChunkMinRelevancy: async (value: number) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        set({
            settings: { ...currentSettings, rag_chunk_min_relevancy: value },
            error: null
        });
        try {
            await invoke('update_rag_chunk_min_relevancy', { value });
            console.log('[SettingsStore] rag_chunk_min_relevancy updated:', value);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update rag_chunk_min_relevancy:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },

    updateSchemaRelevancyThreshold: async (value: number) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        set({
            settings: { ...currentSettings, schema_relevancy_threshold: value },
            error: null
        });
        try {
            await invoke('update_schema_relevancy_threshold', { value });
            console.log('[SettingsStore] schema_relevancy_threshold updated:', value);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update schema_relevancy_threshold:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },

    updateRagDominantThreshold: async (value: number) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        set({
            settings: { ...currentSettings, rag_dominant_threshold: value },
            error: null
        });
        try {
            await invoke('update_rag_dominant_threshold', { value });
            console.log('[SettingsStore] rag_dominant_threshold updated:', value);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update rag_dominant_threshold:', e);
            set({
                settings: currentSettings,
                error: `Failed to save: ${e.message || e}`
            });
        }
    },

    updateDatabaseToolboxConfig: async (config: DatabaseToolboxConfig) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        set({
            settings: { ...currentSettings, database_toolbox: config },
            error: null
        });
        try {
            await invoke('update_database_toolbox_config', { config });
            console.log('[SettingsStore] database_toolbox config updated');
        } catch (e: any) {
            console.error('[SettingsStore] Failed to update database_toolbox config:', e);
            const message = `Failed to save: ${e?.message || e}`;
            set({
                settings: get().settings,
                error: message,
            });
            throw new Error(message);
        }
    },

    addMcpServer: async (config: McpServerConfig) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const isTestServer = config.id === 'mcp-test-server';
        const name = isTestServer ? 'mcp_test_server' : config.name;
        const pythonName = toPythonIdentifier(name || config.id);
        const newConfig: McpServerConfig = {
            ...config,
            name,
            defer_tools: isTestServer ? false : (config.defer_tools ?? true),
            python_name: pythonName,
        };

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
        const isTestServer = config.id === 'mcp-test-server';
        const name = isTestServer ? 'mcp_test_server' : config.name;
        const pythonName = toPythonIdentifier(name || config.id);
        const newConfig: McpServerConfig = {
            ...config,
            name,
            defer_tools: isTestServer ? false : (config.defer_tools ?? true),
            python_name: pythonName,
        };

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

    // ============ Always-On Configuration Methods ============

    addAlwaysOnBuiltinTool: async (toolName: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const newList = [...currentSettings.always_on_builtin_tools];
        if (!newList.includes(toolName)) {
            newList.push(toolName);
        }
        set({
            settings: { ...currentSettings, always_on_builtin_tools: newList },
            error: null
        });
        try {
            await invoke('update_always_on_builtin_tools', { tools: newList });
            console.log('[SettingsStore] Always-on builtin tool added:', toolName);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to add always-on builtin tool:', e);
            set({ settings: currentSettings, error: `Failed to save: ${e.message || e}` });
        }
    },

    removeAlwaysOnBuiltinTool: async (toolName: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const newList = currentSettings.always_on_builtin_tools.filter(t => t !== toolName);
        set({
            settings: { ...currentSettings, always_on_builtin_tools: newList },
            error: null
        });
        try {
            await invoke('update_always_on_builtin_tools', { tools: newList });
            console.log('[SettingsStore] Always-on builtin tool removed:', toolName);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to remove always-on builtin tool:', e);
            set({ settings: currentSettings, error: `Failed to save: ${e.message || e}` });
        }
    },

    addAlwaysOnMcpTool: async (toolKey: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const newList = [...currentSettings.always_on_mcp_tools];
        if (!newList.includes(toolKey)) {
            newList.push(toolKey);
        }
        set({
            settings: { ...currentSettings, always_on_mcp_tools: newList },
            error: null
        });
        try {
            await invoke('update_always_on_mcp_tools', { tools: newList });
            console.log('[SettingsStore] Always-on MCP tool added:', toolKey);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to add always-on MCP tool:', e);
            set({ settings: currentSettings, error: `Failed to save: ${e.message || e}` });
        }
    },

    removeAlwaysOnMcpTool: async (toolKey: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const newList = currentSettings.always_on_mcp_tools.filter(t => t !== toolKey);
        set({
            settings: { ...currentSettings, always_on_mcp_tools: newList },
            error: null
        });
        try {
            await invoke('update_always_on_mcp_tools', { tools: newList });
            console.log('[SettingsStore] Always-on MCP tool removed:', toolKey);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to remove always-on MCP tool:', e);
            set({ settings: currentSettings, error: `Failed to save: ${e.message || e}` });
        }
    },

    addAlwaysOnTable: async (sourceId: string, tableFqName: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const exists = currentSettings.always_on_tables.some(
            t => t.source_id === sourceId && t.table_fq_name === tableFqName
        );
        if (exists) return;
        const newList = [...currentSettings.always_on_tables, { source_id: sourceId, table_fq_name: tableFqName }];
        set({
            settings: { ...currentSettings, always_on_tables: newList },
            error: null
        });
        try {
            await invoke('update_always_on_tables', { tables: newList });
            console.log('[SettingsStore] Always-on table added:', sourceId, tableFqName);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to add always-on table:', e);
            set({ settings: currentSettings, error: `Failed to save: ${e.message || e}` });
        }
    },

    removeAlwaysOnTable: async (sourceId: string, tableFqName: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const newList = currentSettings.always_on_tables.filter(
            t => !(t.source_id === sourceId && t.table_fq_name === tableFqName)
        );
        set({
            settings: { ...currentSettings, always_on_tables: newList },
            error: null
        });
        try {
            await invoke('update_always_on_tables', { tables: newList });
            console.log('[SettingsStore] Always-on table removed:', sourceId, tableFqName);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to remove always-on table:', e);
            set({ settings: currentSettings, error: `Failed to save: ${e.message || e}` });
        }
    },

    addAlwaysOnRagPath: async (path: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const newList = [...currentSettings.always_on_rag_paths];
        if (!newList.includes(path)) {
            newList.push(path);
        }
        set({
            settings: { ...currentSettings, always_on_rag_paths: newList },
            error: null
        });
        try {
            await invoke('update_always_on_rag_paths', { paths: newList });
            console.log('[SettingsStore] Always-on RAG path added:', path);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to add always-on RAG path:', e);
            set({ settings: currentSettings, error: `Failed to save: ${e.message || e}` });
        }
    },

    removeAlwaysOnRagPath: async (path: string) => {
        const currentSettings = get().settings;
        if (!currentSettings) return;
        const newList = currentSettings.always_on_rag_paths.filter(p => p !== path);
        set({
            settings: { ...currentSettings, always_on_rag_paths: newList },
            error: null
        });
        try {
            await invoke('update_always_on_rag_paths', { paths: newList });
            console.log('[SettingsStore] Always-on RAG path removed:', path);
        } catch (e: any) {
            console.error('[SettingsStore] Failed to remove always-on RAG path:', e);
            set({ settings: currentSettings, error: `Failed to save: ${e.message || e}` });
        }
    },
}));

