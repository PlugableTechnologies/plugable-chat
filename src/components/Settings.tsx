import { useSettingsStore, createNewServerConfig, DEFAULT_SYSTEM_PROMPT, DEFAULT_TOOL_CALL_FORMATS, type McpServerConfig, type McpTool, type ToolCallFormatConfig, type ToolCallFormatName, type DatabaseSourceConfig, type SupportedDatabaseKind, type DatabaseToolboxConfig, type ChatFormatName } from '../store/settings-store';
import { useState, useEffect, useCallback, useRef } from 'react';
import { X, Plus, Trash2, Save, Server, MessageSquare, ChevronDown, ChevronUp, Play, CheckCircle, XCircle, Loader2, Code2, Wrench, RotateCcw, RefreshCw, AlertCircle, Download, Cpu, HardDrive, ExternalLink, Zap, GitBranch, Database } from 'lucide-react';
import { invoke, listen, type FoundryCatalogModel, type FoundryServiceStatus } from '../lib/api';
import { FALLBACK_PYTHON_ALLOWED_IMPORTS } from '../lib/python-allowed-imports';
import { useChatStore } from '../store/chat-store';

// Tag input component for args - auto-splits on spaces
function TagInput({
    tags,
    onChange,
    placeholder
}: {
    tags: string[];
    onChange: (tags: string[]) => void;
    placeholder?: string;
}) {
    const [input, setInput] = useState('');

    const addTags = useCallback(() => {
        const trimmed = input.trim();
        if (!trimmed) {
            setInput('');
            return;
        }

        // Split on spaces, but preserve quoted strings
        const parts: string[] = [];
        let current = '';
        let inQuote = false;
        let quoteChar = '';

        for (let i = 0; i < trimmed.length; i++) {
            const char = trimmed[i];

            if ((char === '"' || char === "'") && !inQuote) {
                inQuote = true;
                quoteChar = char;
            } else if (char === quoteChar && inQuote) {
                inQuote = false;
                quoteChar = '';
            } else if (char === ' ' && !inQuote) {
                if (current) {
                    parts.push(current);
                    current = '';
                }
            } else {
                current += char;
            }
        }
        if (current) {
            parts.push(current);
        }

        // Filter out duplicates and empty strings
        const newParts = parts.filter(p => p && !tags.includes(p));
        if (newParts.length > 0) {
            onChange([...tags, ...newParts]);
        }
        setInput('');
    }, [input, tags, onChange]);

    const handleKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === 'Enter' || e.key === ',') {
            e.preventDefault();
            addTags();
        } else if (e.key === 'Backspace' && !input && tags.length > 0) {
            const newTags = tags.slice(0, -1);
            onChange(newTags);
        }
    };

    const handlePaste = (e: React.ClipboardEvent) => {
        e.preventDefault();
        const pasted = e.clipboardData.getData('text');
        // Set input and immediately trigger split
        setInput(prev => prev + pasted);
        // Use setTimeout to process after state update
        setTimeout(() => {
            const trimmed = (input + pasted).trim();
            if (!trimmed) return;

            // Split on spaces, but preserve quoted strings
            const parts: string[] = [];
            let current = '';
            let inQuote = false;
            let quoteChar = '';

            for (let i = 0; i < trimmed.length; i++) {
                const char = trimmed[i];

                if ((char === '"' || char === "'") && !inQuote) {
                    inQuote = true;
                    quoteChar = char;
                } else if (char === quoteChar && inQuote) {
                    inQuote = false;
                    quoteChar = '';
                } else if (char === ' ' && !inQuote) {
                    if (current) {
                        parts.push(current);
                        current = '';
                    }
                } else {
                    current += char;
                }
            }
            if (current) {
                parts.push(current);
            }

            // Filter out duplicates and empty strings
            const newParts = parts.filter(p => p && !tags.includes(p));
            if (newParts.length > 0) {
                onChange([...tags, ...newParts]);
            }
            setInput('');
        }, 0);
    };

    const removeTag = (index: number) => {
        const newTags = tags.filter((_, i) => i !== index);
        onChange(newTags);
    };

    return (
        <div className="space-y-1">
            <div className="flex flex-wrap gap-1.5 p-2 bg-white border border-gray-200 rounded-lg min-h-[40px] focus-within:border-blue-400 focus-within:ring-1 focus-within:ring-blue-400">
                {tags.map((tag, i) => (
                    <span
                        key={i}
                        className="inline-flex items-center gap-1 px-2 py-0.5 bg-blue-100 text-blue-800 text-xs rounded-md font-mono"
                    >
                        {tag}
                        <button
                            onClick={() => removeTag(i)}
                            className="hover:text-blue-600"
                        >
                            ×
                        </button>
                    </span>
                ))}
                <input
                    type="text"
                    value={input}
                    onChange={(e) => setInput(e.target.value)}
                    onKeyDown={handleKeyDown}
                    onPaste={handlePaste}
                    onBlur={addTags}
                    placeholder={tags.length === 0 ? placeholder : ''}
                    className="flex-1 min-w-[100px] outline-none text-sm bg-transparent font-mono"
                />
            </div>
            <p className="text-[10px] text-gray-400">
                Paste or type multiple args separated by spaces. Use quotes for args with spaces.
            </p>
        </div>
    );
}

// Key-value input for environment variables
function EnvVarInput({
    env,
    onChange
}: {
    env: Record<string, string>;
    onChange: (env: Record<string, string>) => void;
}) {
    const [newKey, setNewKey] = useState('');
    const [newValue, setNewValue] = useState('');

    const addVar = () => {
        if (newKey.trim()) {
            onChange({ ...env, [newKey.trim()]: newValue });
            setNewKey('');
            setNewValue('');
        }
    };

    const removeVar = (key: string) => {
        const newEnv = { ...env };
        delete newEnv[key];
        onChange(newEnv);
    };

    return (
        <div className="space-y-2">
            {Object.entries(env).map(([key, value]) => (
                <div key={key} className="flex gap-2 items-center">
                    <span className="text-xs font-mono bg-gray-100 px-2 py-1 rounded">{key}</span>
                    <span className="text-gray-400">=</span>
                    <span className="text-xs font-mono flex-1 truncate">{value}</span>
                    <button
                        onClick={() => removeVar(key)}
                        className="text-gray-400 hover:text-red-500"
                    >
                        <Trash2 size={14} />
                    </button>
                </div>
            ))}
            <div className="flex gap-2">
                <input
                    type="text"
                    value={newKey}
                    onChange={(e) => setNewKey(e.target.value)}
                    placeholder="KEY"
                    className="flex-1 px-2 py-1 text-xs font-mono border border-gray-200 rounded focus:border-blue-400 focus:outline-none"
                />
                <input
                    type="text"
                    value={newValue}
                    onChange={(e) => setNewValue(e.target.value)}
                    placeholder="value"
                    className="flex-1 px-2 py-1 text-xs font-mono border border-gray-200 rounded focus:border-blue-400 focus:outline-none"
                    onKeyDown={(e) => e.key === 'Enter' && addVar()}
                />
                <button
                    onClick={addVar}
                    disabled={!newKey.trim()}
                    className="px-2 py-1 text-xs bg-gray-100 hover:bg-gray-200 rounded disabled:opacity-50"
                >
                    Add
                </button>
            </div>
        </div>
    );
}

// Test result type
interface TestResult {
    success: boolean;
    tools?: McpTool[];
    error?: string;
}

interface SystemPromptLayers {
    base_prompt: string;
    additions: string[];
    combined: string;
}

type ToolParameter = {
    name: string;
    type: string;
    description: string;
    required: boolean;
};

function extractToolParameters(inputSchema?: Record<string, unknown>): ToolParameter[] {
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

// Single MCP Server configuration card
function McpServerCard({
    config,
    onSave,
    onRemove,
    initialTools,
    toolPrompts,
    onSaveToolPrompt,
    onDirtyChange,
    registerSaveHandler
}: {
    config: McpServerConfig;
    onSave: (config: McpServerConfig) => Promise<void>;
    onRemove: () => void;
    initialTools?: McpTool[] | undefined;
    toolPrompts: Record<string, string>;
    onSaveToolPrompt: (serverId: string, toolName: string, prompt: string) => Promise<void>;
    onDirtyChange?: (id: string, dirty: boolean) => void;
    registerSaveHandler?: (id: string, handler: () => Promise<void>) => void;
}) {
    const [expanded, setExpanded] = useState(false);
    const [localConfig, setLocalConfig] = useState<McpServerConfig>(() => structuredClone(config));
    // Simple dirty flag - set to true on any change, reset on save or external update
    const [isDirty, setIsDirty] = useState(false);
    // Track config id to detect when we switch to a different server
    const configIdRef = useRef(config.id);
    const { serverStatuses } = useSettingsStore();
    const status = serverStatuses[config.id];

    // Test connection state
    const [isTesting, setIsTesting] = useState(false);
    const [testResult, setTestResult] = useState<TestResult | null>(null);
    const [tools, setTools] = useState<McpTool[]>([]);
    const [loadingTools, setLoadingTools] = useState(false);
    const [toolsError, setToolsError] = useState<string | null>(null);
    const [toolDrafts, setToolDrafts] = useState<Record<string, string>>({});

    // Seed tools from any cached status data so we show parameters immediately
    useEffect(() => {
        if (initialTools && initialTools.length > 0) {
            setTools(initialTools);
            setToolsError(null);
        }
    }, [initialTools]);

    // Sync with external config when it changes (e.g., after save from backend)
    useEffect(() => {
        // Only sync if the config id matches (same server) or it's a new server
        const configJson = JSON.stringify(config);
        const localJson = JSON.stringify(localConfig);

        // If external config changed and we're not dirty, or if it's a different server
        if (config.id !== configIdRef.current) {
            // Different server - reset everything
            setLocalConfig(structuredClone(config));
            setIsDirty(false);
            configIdRef.current = config.id;
        } else if (!isDirty && configJson !== localJson) {
            // Same server, not dirty, but config changed externally
            setLocalConfig(structuredClone(config));
        }
    }, [config, isDirty, localConfig]);

    // Sync tool prompt drafts with latest store state
    useEffect(() => {
        setToolDrafts(toolPrompts);
    }, [toolPrompts]);

    // Notify parent when dirty state changes
    useEffect(() => {
        onDirtyChange?.(config.id, isDirty);
    }, [config.id, isDirty, onDirtyChange]);

    const updateField = useCallback(<K extends keyof McpServerConfig>(field: K, value: McpServerConfig[K]) => {
        setLocalConfig(prev => {
            const newConfig = { ...prev, [field]: value };
            return newConfig;
        });
        setIsDirty(true);
    }, []);

    const updateTransport = useCallback((type: 'stdio' | 'sse', url?: string) => {
        if (type === 'stdio') {
            updateField('transport', { type: 'stdio' });
        } else {
            updateField('transport', { type: 'sse', url: url || '' });
        }
    }, [updateField]);

    const handleSave = useCallback(async () => {
        await onSave(localConfig);
        setIsDirty(false);

        // Only test the connection if the server is enabled
        if (localConfig.enabled) {
            setIsTesting(true);
            setTestResult(null);
            try {
                const tools = await invoke<McpTool[]>('test_mcp_server_config', { config: localConfig });
                setTestResult({ success: true, tools });
                setTools(tools);
            } catch (e: any) {
                setTestResult({ success: false, error: e.message || String(e) });
            } finally {
                setIsTesting(false);
            }
        } else {
            // Clear any previous test result when saving a disabled server
            setTestResult(null);
            setTools([]);
        }
    }, [localConfig, onSave]);

    // Expose save handler to parent so a global Save button can trigger it
    useEffect(() => {
        if (registerSaveHandler) {
            registerSaveHandler(config.id, handleSave);
        }
    }, [config.id, handleSave, registerSaveHandler]);

    // Manual test without saving
    const handleTest = useCallback(async () => {
        setIsTesting(true);
        setTestResult(null);
        try {
            const tools = await invoke<McpTool[]>('test_mcp_server_config', { config: localConfig });
            setTestResult({ success: true, tools });
            setTools(tools);
        } catch (e: any) {
            setTestResult({ success: false, error: e.message || String(e) });
        } finally {
            setIsTesting(false);
        }
    }, [localConfig]);

    const isTestServer = config.id === 'mcp-test-server';

    const handleResetToDefault = useCallback(async () => {
        if (!isTestServer) return;
        setToolsError(null);
        setTestResult(null);
        setIsTesting(false);
        try {
            const defaultConfig = await invoke<McpServerConfig>('get_default_mcp_test_server');
            setLocalConfig(structuredClone(defaultConfig));
            setIsDirty(true);
            setTools([]);
        } catch (e: any) {
            setToolsError(e.message || String(e));
        }
    }, [isTestServer]);

    // Load tools for prompt editing when expanded and enabled
    useEffect(() => {
        if (!expanded || !localConfig.enabled) return;
        if (tools.length > 0 && !toolsError) return;

        setLoadingTools(true);
        setToolsError(null);
        invoke<McpTool[]>('list_mcp_tools', { serverId: localConfig.id })
            .then(setTools)
            .catch((e: any) => setToolsError(e.message || String(e)))
            .finally(() => setLoadingTools(false));
    }, [expanded, localConfig.enabled, localConfig.id, tools.length, toolsError]);

    // Toggle enabled state and auto-save immediately
    const handleToggleEnabled = useCallback(async () => {
        const previousConfig = localConfig;
        const newEnabled = !localConfig.enabled;
        const newConfig = { ...localConfig, enabled: newEnabled };

        // Update local state immediately
        setLocalConfig(newConfig);
        // Don't mark as dirty since we're saving immediately

        // Save to backend
        try {
            await onSave(newConfig);
        } catch (e: any) {
            console.error('[McpServerCard] Failed to save server toggle:', e);
            setLocalConfig(previousConfig);
            return;
        }

        // Test connection if enabling
        if (newEnabled) {
            setIsTesting(true);
            setTestResult(null);
            try {
                const tools = await invoke<McpTool[]>('test_mcp_server_config', { config: newConfig });
                setTestResult({ success: true, tools });
                setTools(tools);
            } catch (e: any) {
                setTestResult({ success: false, error: e.message || String(e) });
            } finally {
                setIsTesting(false);
            }
        } else {
            setTestResult(null);
            setTools([]);
        }
    }, [localConfig, onSave]);

    const toolPromptKey = useCallback((toolName: string) => `${localConfig.id}::${toolName}`, [localConfig.id]);

    const handleToolPromptChange = useCallback((toolName: string, value: string) => {
        const key = toolPromptKey(toolName);
        setToolDrafts(prev => ({ ...prev, [key]: value }));
    }, [toolPromptKey]);

    const handleToolPromptSave = useCallback(async (toolName: string, value: string) => {
        await onSaveToolPrompt(localConfig.id, toolName, value);
    }, [localConfig.id, onSaveToolPrompt]);

    return (
        <div className={`border rounded-xl bg-white overflow-hidden ${isDirty ? 'border-amber-300' : 'border-gray-200'}`}>
            {/* Header */}
            <div
                className="flex items-center gap-3 px-4 py-3 cursor-pointer hover:bg-gray-50"
                onClick={() => setExpanded(!expanded)}
            >
                {/* Status indicator */}
                <div className={`w-2.5 h-2.5 rounded-full ${status?.connected ? 'bg-green-500' :
                    status?.error ? 'bg-red-500' : 'bg-gray-300'
                    }`} />

                {/* Enable toggle - auto-saves on change */}
                <button
                    onClick={(e) => { e.stopPropagation(); handleToggleEnabled(); }}
                    className={`relative w-10 h-5 rounded-full transition-colors ${localConfig.enabled ? 'bg-blue-500' : 'bg-gray-300'
                        }`}
                >
                    <div className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${localConfig.enabled ? 'translate-x-5' : ''
                        }`} />
                </button>

                {/* Name */}
                <div className="flex-1 flex items-center gap-2">
                    <span className="font-medium text-gray-900 truncate">{localConfig.name || 'Unnamed MCP server'}</span>
                    {isTestServer && (
                        <span className="text-xs bg-purple-100 text-purple-700 px-2 py-0.5 rounded-full">Built-in</span>
                    )}
                </div>

                {/* Unsaved indicator */}
                {isDirty && (
                    <span className="text-xs text-amber-600 font-medium">Unsaved</span>
                )}

                {/* Expand/collapse */}
                {expanded ? <ChevronUp size={18} /> : <ChevronDown size={18} />}
            </div>

            {/* Expanded details */}
            {expanded && (
                <div className="px-4 pb-4 pt-2 border-t border-gray-100 space-y-4">
                    {/* Name (now editable at top of fields) */}
                    <div>
                        <label className="block text-xs font-medium text-gray-600 mb-1.5">Server Name</label>
                        <input
                            type="text"
                            value={localConfig.name}
                            onChange={(e) => updateField('name', e.target.value)}
                            placeholder="Enter a name for this server"
                            className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                        />
                    </div>

                    {/* Transport type */}
                    <div>
                        <label className="block text-xs font-medium text-gray-600 mb-1.5">Transport</label>
                        <div className="flex gap-2">
                            <button
                                onClick={() => updateTransport('stdio')}
                                className={`px-3 py-1.5 text-xs rounded-lg border ${localConfig.transport.type === 'stdio'
                                    ? 'bg-blue-50 border-blue-300 text-blue-700'
                                    : 'bg-white border-gray-200 text-gray-600 hover:bg-gray-50'
                                    }`}
                            >
                                Stdio (subprocess)
                            </button>
                            <button
                                onClick={() => updateTransport('sse', (localConfig.transport as any).url || '')}
                                className={`px-3 py-1.5 text-xs rounded-lg border ${localConfig.transport.type === 'sse'
                                    ? 'bg-blue-50 border-blue-300 text-blue-700'
                                    : 'bg-white border-gray-200 text-gray-600 hover:bg-gray-50'
                                    }`}
                            >
                                SSE (HTTP)
                            </button>
                        </div>
                    </div>

                    {/* Stdio-specific fields */}
                    {localConfig.transport.type === 'stdio' && (
                        <>
                            <div>
                                <label className="block text-xs font-medium text-gray-600 mb-1.5">Command</label>
                                <input
                                    type="text"
                                    value={localConfig.command || ''}
                                    onChange={(e) => updateField('command', e.target.value || null)}
                                    placeholder="e.g., node, python, npx"
                                    className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                                />
                            </div>

                            <div>
                                <label className="block text-xs font-medium text-gray-600 mb-1.5">Arguments</label>
                                <TagInput
                                    tags={localConfig.args}
                                    onChange={(args) => updateField('args', args)}
                                    placeholder="Press Enter to add arguments"
                                />
                            </div>

                            <div>
                                <label className="block text-xs font-medium text-gray-600 mb-1.5">Environment Variables</label>
                                <EnvVarInput
                                    env={localConfig.env}
                                    onChange={(env) => updateField('env', env)}
                                />
                            </div>
                        </>
                    )}

                    {/* SSE-specific fields */}
                    {localConfig.transport.type === 'sse' && (
                        <div>
                            <label className="block text-xs font-medium text-gray-600 mb-1.5">Server URL</label>
                            <input
                                type="text"
                                value={(localConfig.transport as { type: 'sse'; url: string }).url}
                                onChange={(e) => updateTransport('sse', e.target.value)}
                                placeholder="http://localhost:3000/sse"
                                className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                            />
                        </div>
                    )}

                    {/* Auto-approve toggle */}
                    <div className="flex items-center justify-between py-2">
                        <div>
                            <div className="text-sm font-medium text-gray-700">Auto-approve tool calls</div>
                            <div className="text-xs text-gray-500">Execute tools without user confirmation</div>
                        </div>
                        <button
                            onClick={() => updateField('auto_approve_tools', !localConfig.auto_approve_tools)}
                            className={`relative w-10 h-5 rounded-full transition-colors ${localConfig.auto_approve_tools ? 'bg-blue-500' : 'bg-gray-300'
                                }`}
                        >
                            <div className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${localConfig.auto_approve_tools ? 'translate-x-5' : ''
                                }`} />
                        </button>
                    </div>

                    {/* Tool system prompts */}
                    <div className="space-y-2">
                        <div className="flex items-center justify-between">
                            <label className="text-xs font-medium text-gray-600">Tool system prompts</label>
                            {!localConfig.enabled && (
                                <span className="text-[11px] text-gray-500">Enable this server to load tools</span>
                            )}
                        </div>
                        {!localConfig.enabled && (
                            <p className="text-xs text-gray-500">Turn on this server to view its tools and edit their prompts.</p>
                        )}
                        {localConfig.enabled && loadingTools && (
                            <div className="flex items-center gap-2 text-xs text-gray-600 bg-gray-50 px-3 py-2 rounded-lg">
                                <Loader2 size={14} className="animate-spin" />
                                Loading tools...
                            </div>
                        )}
                        {localConfig.enabled && toolsError && (
                            <div className="text-xs text-red-700 bg-red-50 px-3 py-2 rounded-lg">
                                {toolsError}
                            </div>
                        )}
                        {localConfig.enabled && !loadingTools && !toolsError && tools.length === 0 && (
                            <p className="text-xs text-gray-500">No tools reported yet from this server.</p>
                        )}
                        {localConfig.enabled && !loadingTools && tools.length > 0 && (
                            <div className="space-y-3">
                                {tools.map(tool => {
                                    const key = toolPromptKey(tool.name);
                                    const value = toolDrafts[key] ?? '';
                                    const parameters = extractToolParameters(tool.inputSchema as Record<string, unknown> | undefined);
                                    return (
                                        <div key={tool.name} className="border border-gray-200 rounded-lg p-3 bg-gray-50">
                                            <div className="flex items-start justify-between gap-2">
                                                <div>
                                                    <div className="text-sm font-medium text-gray-900">{tool.name}</div>
                                                    {tool.description && (
                                                        <p className="text-xs text-gray-600 mt-0.5">{tool.description}</p>
                                                    )}
                                                </div>
                                                <span className="text-[11px] bg-white text-gray-600 px-2 py-0.5 rounded border border-gray-200">MCP tool</span>
                                            </div>
                                            {parameters.length > 0 && (
                                                <div className="mt-2 space-y-1">
                                                    <div className="text-[11px] font-semibold text-gray-600">Parameters</div>
                                                    <div className="space-y-1">
                                                        {parameters.map((param) => (
                                                            <div
                                                                key={param.name}
                                                                className="flex flex-wrap items-start gap-2 text-xs text-gray-800"
                                                            >
                                                                <span className="font-mono px-2 py-0.5 bg-white border border-gray-200 rounded">
                                                                    {param.name}
                                                                </span>
                                                                <span className="text-[11px] text-gray-500">{param.type}</span>
                                                                <span className={`text-[11px] ${param.required ? 'text-red-600' : 'text-gray-500'}`}>
                                                                    {param.required ? 'required' : 'optional'}
                                                                </span>
                                                                {param.description && (
                                                                    <span className="text-gray-600">{param.description}</span>
                                                                )}
                                                            </div>
                                                        ))}
                                                    </div>
                                                </div>
                                            )}
                                            <label className="block text-xs font-medium text-gray-600 mt-3 mb-1">System prompt (optional)</label>
                                            <textarea
                                                value={value}
                                                onChange={(e) => handleToolPromptChange(tool.name, e.target.value)}
                                                onBlur={(e) => handleToolPromptSave(tool.name, e.target.value)}
                                                rows={3}
                                                className="w-full px-3 py-2 text-sm font-mono border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400 bg-white"
                                                placeholder="Add extra instructions for this tool"
                                            />
                                            <p className="text-[11px] text-gray-500 mt-1">
                                                Appended to the system prompt when this tool is enabled.
                                            </p>
                                        </div>
                                    );
                                })}
                            </div>
                        )}
                    </div>

                    {/* Status message from sync */}
                    {status?.error && !testResult && (
                        <div className="text-xs text-red-700 bg-red-50 px-3 py-2 rounded-lg max-h-48 overflow-y-auto">
                            <pre className="font-mono whitespace-pre-wrap">
                                {status.error}
                            </pre>
                        </div>
                    )}

                    {/* Test result display */}
                    {testResult && (
                        <div className={`rounded-lg p-3 ${testResult.success ? 'bg-green-50 border border-green-200' : 'bg-red-50 border border-red-200'}`}>
                            <div className="flex items-center gap-2 mb-2">
                                {testResult.success ? (
                                    <>
                                        <CheckCircle size={16} className="text-green-600" />
                                        <span className="text-sm font-medium text-green-700">Connection Successful</span>
                                    </>
                                ) : (
                                    <>
                                        <XCircle size={16} className="text-red-600" />
                                        <span className="text-sm font-medium text-red-700">Connection Failed</span>
                                    </>
                                )}
                            </div>
                            {testResult.success && testResult.tools && (
                                <div className="text-xs text-green-700">
                                    <span className="font-medium">{testResult.tools.length} tool{testResult.tools.length !== 1 ? 's' : ''} available:</span>
                                    <ul className="mt-1 space-y-0.5 ml-2">
                                        {testResult.tools.map((tool, i) => (
                                            <li key={i} className="font-mono">
                                                • {tool.name}
                                                {tool.description && (
                                                    <span className="text-green-600 font-sans"> - {tool.description}</span>
                                                )}
                                            </li>
                                        ))}
                                    </ul>
                                </div>
                            )}
                            {testResult.error && (
                                <div className="text-xs text-red-700 max-h-48 overflow-y-auto">
                                    <pre className="font-mono whitespace-pre-wrap bg-red-100/50 p-2 rounded mt-1">
                                        {testResult.error}
                                    </pre>
                                </div>
                            )}
                        </div>
                    )}

                    {/* Testing indicator */}
                    {isTesting && (
                        <div className="flex items-center gap-2 text-xs text-blue-600 bg-blue-50 px-3 py-2 rounded-lg">
                            <Loader2 size={14} className="animate-spin" />
                            Testing connection...
                        </div>
                    )}

                    {/* Actions */}
                    <div className="flex justify-between pt-2 border-t border-gray-100">
                        <button
                            onClick={onRemove}
                            disabled={isTestServer}
                            className="flex items-center gap-1.5 px-3 py-1.5 text-xs text-red-600 hover:bg-red-50 rounded-lg disabled:opacity-50 disabled:cursor-not-allowed"
                            title={isTestServer ? "Cannot remove built-in test server" : "Remove server"}
                        >
                            <Trash2 size={14} />
                            Remove
                        </button>
                        <div className="flex gap-2">
                            <button
                                onClick={handleTest}
                                disabled={isTesting || !localConfig.command}
                                className="flex items-center gap-1.5 px-3 py-1.5 text-xs border border-gray-300 text-gray-700 rounded-lg hover:bg-gray-50 disabled:opacity-50 disabled:cursor-not-allowed"
                                title="Test connection without saving"
                            >
                                {isTesting ? <Loader2 size={14} className="animate-spin" /> : <Play size={14} />}
                                Test
                            </button>
                            {isTestServer && (
                                <button
                                    onClick={handleResetToDefault}
                                    disabled={isTesting}
                                    className="flex items-center gap-1.5 px-3 py-1.5 text-xs border border-gray-300 text-gray-700 rounded-lg hover:bg-gray-50 disabled:opacity-50 disabled:cursor-not-allowed"
                                    title="Reset built-in server to defaults"
                                >
                                    <RotateCcw size={14} />
                                    Reset
                                </button>
                            )}
                            <button
                                onClick={handleSave}
                                disabled={!isDirty || isTesting}
                                className="flex items-center gap-1.5 px-3 py-1.5 text-xs bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                            >
                                <Save size={14} />
                                Save
                            </button>
                        </div>
                    </div>
                </div>
            )}
        </div>
    );
}

// ============ Models Tab ============

type DeviceFilter = 'Auto' | 'CPU' | 'GPU' | 'NPU';

interface ModelCardProps {
    model: FoundryCatalogModel;
    isCached: boolean;
    isLoaded: boolean;
    isDownloading: boolean;
    downloadProgress?: { file: string; progress: number };
    onDownload: () => void;
    onUnload: () => void;
    onRemove: () => void;
}

function ModelCard({
    model,
    isCached,
    isLoaded,
    isDownloading,
    downloadProgress,
    onDownload,
    onUnload,
    onRemove,
}: ModelCardProps) {
    const deviceType = model.runtime?.deviceType || 'CPU';
    const fileSizeGB = (model.fileSizeMb / 1024).toFixed(2);
    const tasks = model.task?.replace('-completion', '') || 'chat';
    const supportsTools = model.supportsToolCalling;

    // Device badge colors
    const deviceBadgeClass = deviceType === 'GPU'
        ? 'bg-emerald-100 text-emerald-700'
        : deviceType === 'NPU'
            ? 'bg-purple-100 text-purple-700'
            : 'bg-gray-100 text-gray-600';

    // Card border for loaded state
    const cardBorderClass = isLoaded
        ? 'border-2 border-blue-400 shadow-lg shadow-blue-100'
        : 'border border-gray-200';

    return (
        <div className={`model-card bg-white rounded-xl p-4 ${cardBorderClass} transition-all duration-200 hover:shadow-md`}>
            {/* Header: Device badge, alias, license */}
            <div className="flex items-center justify-between mb-2">
                <div className="flex items-center gap-2">
                    <span className={`text-xs font-semibold px-2 py-0.5 rounded ${deviceBadgeClass}`}>
                        {deviceType}
                    </span>
                    <h3 className="font-semibold text-gray-900">{model.alias || model.displayName}</h3>
                </div>
                <span className="text-xs text-gray-500 bg-gray-50 px-2 py-0.5 rounded">
                    {model.license || 'Unknown'}
                </span>
            </div>

            {/* Model ID subtitle */}
            <p className="text-xs text-gray-400 font-mono mb-3 truncate" title={model.name}>
                {model.name}
            </p>

            {/* Details row */}
            <div className="flex items-center gap-3 text-sm text-gray-600 mb-3">
                <div className="flex items-center gap-1">
                    <HardDrive size={14} className="text-gray-400" />
                    <span>{fileSizeGB} GB</span>
                </div>
                <span className="text-gray-300">|</span>
                <span className="capitalize">{tasks}</span>
                {supportsTools && (
                    <>
                        <span className="text-gray-300">|</span>
                        <span className="flex items-center gap-1 text-blue-600">
                            <Wrench size={12} />
                            tools
                        </span>
                    </>
                )}
            </div>

            {/* Status badges */}
            <div className="flex items-center gap-2 mb-4">
                {isCached && (
                    <span className="text-xs px-2 py-0.5 rounded bg-green-100 text-green-700 flex items-center gap-1">
                        <CheckCircle size={12} />
                        Downloaded
                    </span>
                )}
                {isLoaded && (
                    <span className="text-xs px-2 py-0.5 rounded bg-blue-100 text-blue-700 flex items-center gap-1">
                        <Zap size={12} />
                        Loaded
                    </span>
                )}
                {!isCached && !isDownloading && (
                    <span className="text-xs px-2 py-0.5 rounded bg-gray-100 text-gray-500">
                        Not Downloaded
                    </span>
                )}
            </div>

            {/* Download progress */}
            {isDownloading && downloadProgress && (
                <div className="mb-4">
                    <div className="flex items-center justify-between text-xs text-gray-500 mb-1">
                        <span className="truncate max-w-[60%]">{downloadProgress.file}</span>
                        <span>{downloadProgress.progress.toFixed(1)}%</span>
                    </div>
                    <div className="w-full bg-gray-200 rounded-full h-2">
                        <div
                            className="bg-blue-500 h-2 rounded-full transition-all duration-300"
                            style={{ width: `${downloadProgress.progress}%` }}
                        />
                    </div>
                </div>
            )}

            {/* Action buttons */}
            <div className="flex items-center gap-2">
                {!isCached && !isDownloading && (
                    <button
                        onClick={onDownload}
                        className="flex-1 flex items-center justify-center gap-2 px-3 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors text-sm font-medium"
                    >
                        <Download size={16} />
                        Download
                    </button>
                )}
                {isCached && !isLoaded && (
                    <button
                        onClick={onRemove}
                        className="flex items-center justify-center gap-2 px-3 py-2 bg-red-50 text-red-600 rounded-lg hover:bg-red-100 transition-colors text-sm"
                    >
                        <Trash2 size={16} />
                        Remove from Cache
                    </button>
                )}
                {isLoaded && (
                    <button
                        onClick={onUnload}
                        className="flex-1 flex items-center justify-center gap-2 px-3 py-2 bg-gray-100 text-gray-700 rounded-lg hover:bg-gray-200 transition-colors text-sm font-medium"
                    >
                        Unload
                    </button>
                )}
                {isDownloading && (
                    <div className="flex-1 flex items-center justify-center gap-2 px-3 py-2 bg-gray-100 text-gray-500 rounded-lg text-sm">
                        <Loader2 size={16} className="animate-spin" />
                        Downloading...
                    </div>
                )}
            </div>
        </div>
    );
}

function ModelsTab() {
    const [catalogModels, setCatalogModels] = useState<FoundryCatalogModel[]>([]);
    const [cachedModelIds, setCachedModelIds] = useState<string[]>([]);
    const [loadedModelIds, setLoadedModelIds] = useState<string[]>([]);
    const [serviceStatus, setServiceStatus] = useState<FoundryServiceStatus | null>(null);
    const [deviceFilter, setDeviceFilter] = useState<DeviceFilter>('GPU');
    const [isLoading, setIsLoading] = useState(true);
    const [downloadingModel, setDownloadingModel] = useState<string | null>(null);
    const [downloadProgress, setDownloadProgress] = useState<{ file: string; progress: number } | null>(null);
    const [error, setError] = useState<string | null>(null);

    const { operationStatus, setOperationStatus } = useChatStore();

    // Fetch all data on mount
    useEffect(() => {
        fetchAllData();
    }, []);

    // Track download progress from operationStatus
    useEffect(() => {
        if (operationStatus?.type === 'downloading') {
            setDownloadProgress({
                file: operationStatus.currentFile || '',
                progress: operationStatus.progress || 0,
            });
            if (operationStatus.completed) {
                setDownloadingModel(null);
                setDownloadProgress(null);
                // Refresh cached models after download
                fetchCachedModels();
            }
        }
    }, [operationStatus]);

    const fetchAllData = async () => {
        setIsLoading(true);
        setError(null);
        try {
            await Promise.all([
                fetchCatalogModels(),
                fetchCachedModels(),
                fetchLoadedModels(),
                fetchServiceStatus(),
            ]);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to load model data');
        } finally {
            setIsLoading(false);
        }
    };

    const fetchCatalogModels = async () => {
        try {
            const models = await invoke<FoundryCatalogModel[]>('get_catalog_models');
            setCatalogModels(models);
        } catch (err) {
            console.error('Failed to fetch catalog models:', err);
        }
    };

    const fetchCachedModels = async () => {
        try {
            const models = await invoke<string[]>('get_models');
            setCachedModelIds(models);
        } catch (err) {
            console.error('Failed to fetch cached models:', err);
        }
    };

    const fetchLoadedModels = async () => {
        try {
            const models = await invoke<string[]>('get_loaded_models');
            setLoadedModelIds(models);
        } catch (err) {
            console.error('Failed to fetch loaded models:', err);
        }
    };

    const fetchServiceStatus = async () => {
        try {
            const status = await invoke<FoundryServiceStatus>('get_foundry_service_status');
            setServiceStatus(status);
        } catch (err) {
            console.error('Failed to fetch service status:', err);
        }
    };

    const handleDownload = async (model: FoundryCatalogModel) => {
        setDownloadingModel(model.name);
        setDownloadProgress({ file: 'Starting...', progress: 0 });
        // Set operation status so the chat-store listener tracks progress
        setOperationStatus({
            type: 'downloading',
            message: `Downloading ${model.alias || model.name}...`,
            progress: 0,
            currentFile: 'Starting...',
            startTime: Date.now(),
        });
        try {
            await invoke('download_model', { modelName: model.name });
            await fetchCachedModels();
            setOperationStatus({
                type: 'downloading',
                message: `${model.alias || model.name} downloaded successfully`,
                completed: true,
                startTime: Date.now(),
            });
        } catch (err) {
            console.error('Download failed:', err);
            const errorMessage = `Failed to download ${model.alias || model.name}:\n\n${err}`;
            alert(errorMessage);
            setError(`Download failed: ${err}`);
            setOperationStatus(null);
        } finally {
            setDownloadingModel(null);
            setDownloadProgress(null);
            // Clear operation status after a delay
            setTimeout(() => {
                setOperationStatus(null);
            }, 3000);
        }
    };

    const handleUnload = async (model: FoundryCatalogModel) => {
        try {
            await invoke('unload_model', { modelName: model.name });
            await fetchLoadedModels();
        } catch (err) {
            console.error('Unload failed:', err);
            const errorMessage = `Failed to unload ${model.alias || model.name}:\n\n${err}`;
            alert(errorMessage);
            setError(`Unload failed: ${err}`);
        }
    };

    const handleRemove = async (model: FoundryCatalogModel) => {
        if (!confirm(`Remove ${model.alias || model.name} from cache?\n\nThis will delete the downloaded model files from disk.`)) {
            return;
        }
        try {
            await invoke('remove_cached_model', { modelName: model.name });
            // Refresh the cached models list to reflect the removal
            await fetchCachedModels();
            // Also refresh loaded models in case it was loaded
            await fetchLoadedModels();
        } catch (err) {
            console.error('Remove failed:', err);
            const errorMessage = `Failed to remove ${model.alias || model.name} from cache:\n\n${err}`;
            alert(errorMessage);
            setError(`Remove failed: ${err}`);
        }
    };

    const handleOpenProductLink = () => {
        window.open('https://plugable.com/products/tbt5-ai', '_blank');
    };

    // Filter and sort models
    const filteredModels = catalogModels
        .filter((model) => {
            if (deviceFilter === 'Auto') return true;
            return model.runtime?.deviceType === deviceFilter;
        })
        .sort((a, b) => {
            // Sort: 1. Tools support first, 2. By size ascending (smaller first)
            const aTools = a.supportsToolCalling ? 1 : 0;
            const bTools = b.supportsToolCalling ? 1 : 0;
            if (aTools !== bTools) return bTools - aTools; // Tools support first

            // Then by size ascending (smaller models first)
            return (a.fileSizeMb || 0) - (b.fileSizeMb || 0);
        });

    const isModelCached = (model: FoundryCatalogModel) => {
        return cachedModelIds.some(id => 
            id.toLowerCase().includes(model.name.toLowerCase()) || 
            model.name.toLowerCase().includes(id.toLowerCase())
        );
    };

    const isModelLoaded = (model: FoundryCatalogModel) => {
        return loadedModelIds.some(id => 
            id.toLowerCase().includes(model.name.toLowerCase()) || 
            model.name.toLowerCase().includes(id.toLowerCase())
        );
    };

    const serviceEndpoint = serviceStatus?.endpoints?.[0] || 'Not available';

    return (
        <div className="models-tab flex flex-col h-full">
            {/* Header */}
            <div className="flex items-center justify-between mb-4">
                <div className="flex items-center gap-4">
                    {/* Device Filter */}
                    <div className="flex items-center gap-2">
                        <Cpu size={16} className="text-gray-500" />
                        <select
                            value={deviceFilter}
                            onChange={(e) => setDeviceFilter(e.target.value as DeviceFilter)}
                            className="text-sm border border-gray-200 rounded-lg px-3 py-1.5 bg-white focus:outline-none focus:ring-2 focus:ring-blue-400"
                        >
                            <option value="Auto">All Devices</option>
                            <option value="GPU">GPU</option>
                            <option value="CPU">CPU</option>
                            <option value="NPU">NPU</option>
                        </select>
                    </div>

                    {/* Service Status */}
                    <div className="flex items-center gap-2 text-sm text-gray-500">
                        <span className={`w-2 h-2 rounded-full ${serviceStatus ? 'bg-green-500' : 'bg-red-500'}`} />
                        <span>{serviceStatus ? `Service: ${serviceEndpoint}` : 'Service not available'}</span>
                    </div>
                </div>

                {/* Refresh Button */}
                <button
                    onClick={fetchAllData}
                    disabled={isLoading}
                    className="flex items-center gap-2 px-3 py-1.5 text-sm text-gray-600 bg-gray-100 rounded-lg hover:bg-gray-200 transition-colors disabled:opacity-50"
                >
                    <RefreshCw size={14} className={isLoading ? 'animate-spin' : ''} />
                    Refresh
                </button>
            </div>

            {/* Error display */}
            {error && (
                <div className="mb-4 p-3 bg-red-50 border border-red-200 rounded-lg text-red-700 text-sm flex items-center gap-2">
                    <AlertCircle size={16} />
                    {error}
                    <button onClick={() => setError(null)} className="ml-auto text-red-500 hover:text-red-700">
                        <X size={14} />
                    </button>
                </div>
            )}

            {/* Scrollable content area */}
            <div className="flex-1 overflow-y-auto">
                {/* Promotional Banner */}
                <div className="mb-4 p-3 bg-gradient-to-r from-blue-50 to-indigo-50 border border-blue-100 rounded-lg">
                    <p className="text-sm text-gray-700">
                        Enable more models with the{' '}
                        <button
                            onClick={handleOpenProductLink}
                            className="text-blue-600 hover:text-blue-800 font-medium inline-flex items-center gap-1 hover:underline"
                        >
                            Plugable TBT5-AI
                            <ExternalLink size={12} />
                        </button>
                    </p>
                </div>

                {/* Loading state */}
                {isLoading && (
                    <div className="flex items-center justify-center py-12">
                        <Loader2 size={24} className="animate-spin text-blue-500" />
                    </div>
                )}

                {/* Model Cards Grid */}
                {!isLoading && (
                    <div className="model-card-grid grid grid-cols-1 gap-4">
                        {filteredModels.map((model) => (
                            <ModelCard
                                key={model.name}
                                model={model}
                                isCached={isModelCached(model)}
                                isLoaded={isModelLoaded(model)}
                                isDownloading={downloadingModel === model.name}
                                downloadProgress={downloadingModel === model.name ? downloadProgress ?? undefined : undefined}
                                onDownload={() => handleDownload(model)}
                                onUnload={() => handleUnload(model)}
                                onRemove={() => handleRemove(model)}
                            />
                        ))}
                    </div>
                )}

                {/* Empty state */}
                {!isLoading && filteredModels.length === 0 && (
                    <div className="text-center py-12 text-gray-500">
                        <Cpu size={48} className="mx-auto mb-4 text-gray-300" />
                        <p>No models found for {deviceFilter} device type.</p>
                        <p className="text-sm mt-2">Try selecting a different device filter.</p>
                    </div>
                )}
            </div>

            {/* Footer */}
            <div className="mt-4 pt-4 border-t border-gray-100 text-xs text-gray-400">
                <div className="flex items-center gap-2">
                    <HardDrive size={12} />
                    <span>Cache: {serviceStatus?.modelDirPath || 'Unknown location'}</span>
                </div>
            </div>
        </div>
    );
}

// State Preview interface
interface StatePreview {
    name: string;
    description: string;
    available_tools: string[];
    prompt_additions: string[];
    is_possible: boolean;
}

// State Machine Preview Component
function StateMachinePreview() {
    const [states, setStates] = useState<StatePreview[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [expanded, setExpanded] = useState(false);

    const fetchStates = useCallback(() => {
        setLoading(true);
        setError(null);
        invoke<StatePreview[]>('get_state_machine_preview')
            .then((data) => {
                setStates(data);
            })
            .catch((e) => {
                console.error('Failed to get state machine preview:', e);
                setError(e.message || String(e));
            })
            .finally(() => setLoading(false));
    }, []);

    useEffect(() => {
        fetchStates();
    }, [fetchStates]);

    return (
        <div className="border border-gray-200 rounded-xl overflow-hidden">
            <button
                onClick={() => setExpanded(!expanded)}
                className="w-full flex items-center justify-between px-4 py-3 bg-gray-50 hover:bg-gray-100 text-sm font-medium text-gray-700"
            >
                <span className="flex items-center gap-2">
                    <GitBranch size={16} />
                    State Machine Preview
                    <span className="text-xs bg-purple-100 text-purple-700 px-2 py-0.5 rounded-full">
                        {states.length} states
                    </span>
                </span>
                {expanded ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
            </button>

            {expanded && (
                <div className="p-4 bg-white border-t border-gray-200 space-y-3">
                    {loading ? (
                        <div className="flex items-center gap-2 text-sm text-gray-600">
                            <Loader2 size={16} className="animate-spin" />
                            Loading states...
                        </div>
                    ) : error ? (
                        <div className="text-sm text-red-700 bg-red-50 px-3 py-2 rounded-lg">{error}</div>
                    ) : states.length > 0 ? (
                        <div className="space-y-2">
                            {states.map((state, idx) => (
                                <div
                                    key={idx}
                                    className={`p-3 rounded-lg border ${state.is_possible ? 'border-green-200 bg-green-50/50' : 'border-gray-200 bg-gray-50'}`}
                                >
                                    <div className="flex items-center justify-between mb-1">
                                        <span className="font-medium text-sm text-gray-900">{state.name}</span>
                                        {state.is_possible && (
                                            <span className="text-xs bg-green-100 text-green-700 px-2 py-0.5 rounded-full">
                                                possible
                                            </span>
                                        )}
                                    </div>
                                    <p className="text-xs text-gray-600 mb-2">{state.description}</p>
                                    {state.available_tools.length > 0 && (
                                        <div className="flex flex-wrap gap-1">
                                            {state.available_tools.map((tool, tidx) => (
                                                <span
                                                    key={tidx}
                                                    className="text-xs bg-blue-100 text-blue-700 px-2 py-0.5 rounded"
                                                >
                                                    {tool}
                                                </span>
                                            ))}
                                        </div>
                                    )}
                                </div>
                            ))}
                        </div>
                    ) : (
                        <p className="text-sm text-gray-500">No states available.</p>
                    )}
                    <button
                        onClick={fetchStates}
                        className="text-xs text-blue-600 hover:text-blue-700"
                    >
                        Refresh
                    </button>
                </div>
            )}
        </div>
    );
}

// System Prompt Tab
function SystemPromptTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}) {
    const { settings, updateSystemPrompt, error, promptRefreshTick } = useSettingsStore();
    const [localPrompt, setLocalPrompt] = useState(settings?.system_prompt || '');
    const [hasChanges, setHasChanges] = useState(false);
    const [preview, setPreview] = useState<string | null>(null);
    const [showPreview, setShowPreview] = useState(false);
    const [loadingPreview, setLoadingPreview] = useState(false);
    const [layers, setLayers] = useState<SystemPromptLayers | null>(null);
    const [layersError, setLayersError] = useState<string | null>(null);
    const [isSaving, setIsSaving] = useState(false);

    useEffect(() => {
        if (settings?.system_prompt) {
            setLocalPrompt(settings.system_prompt);
            setHasChanges(false);
        }
    }, [settings?.system_prompt]);

    const fetchLayers = useCallback(() => {
        setLoadingPreview(true);
        setLayersError(null);
        invoke<SystemPromptLayers>('get_system_prompt_layers')
            .then((data) => {
                setLayers(data);
                setPreview(data.combined);
            })
            .catch((e) => {
                console.error('Failed to get system prompt layers:', e);
                setLayersError(e.message || String(e));
                setPreview('Failed to load preview');
            })
            .finally(() => setLoadingPreview(false));
    }, []);

    // Keep layers in sync with saved settings
    useEffect(() => {
        fetchLayers();
    }, [fetchLayers, settings?.mcp_servers, settings?.python_execution_enabled, settings?.system_prompt]);

    // Refresh when prompt refresh tick changes (e.g., MCP config saved)
    useEffect(() => {
        fetchLayers();
    }, [fetchLayers, promptRefreshTick]);

    // Fetch preview when toggling view
    useEffect(() => {
        if (showPreview) {
            fetchLayers();
        }
    }, [showPreview, fetchLayers]);

    const handleSave = async () => {
        setIsSaving(true);
        onSavingChange?.(true);
        try {
            await updateSystemPrompt(localPrompt);
            setHasChanges(false);
            fetchLayers();
        } finally {
            setIsSaving(false);
            onSavingChange?.(false);
        }
    };

    const handleChange = (value: string) => {
        setLocalPrompt(value);
        setHasChanges(value !== settings?.system_prompt);
    };

    const handleReset = () => {
        setLocalPrompt(DEFAULT_SYSTEM_PROMPT);
        setHasChanges(DEFAULT_SYSTEM_PROMPT !== settings?.system_prompt);
    };

    useEffect(() => {
        onDirtyChange?.(hasChanges);
    }, [hasChanges, onDirtyChange]);

    useEffect(() => {
        onSavingChange?.(isSaving);
    }, [isSaving, onSavingChange]);

    useEffect(() => {
        onRegisterSave?.(handleSave);
    }, [handleSave, onRegisterSave]);

    // Check if current prompt matches default
    const isDefault = localPrompt === DEFAULT_SYSTEM_PROMPT;

    // Count enabled MCP servers
    const enabledServers = settings?.mcp_servers?.filter(s => s.enabled).length || 0;

    return (
        <div className="space-y-4">
            <div>
                <div className="flex items-center justify-between mb-2">
                    <label className="text-sm font-medium text-gray-700">Base System Prompt</label>
                    {hasChanges && (
                        <span className="text-xs text-amber-600">Unsaved changes</span>
                    )}
                </div>
                <textarea
                    value={localPrompt}
                    onChange={(e) => handleChange(e.target.value)}
                    rows={8}
                    className="w-full px-4 py-3 text-sm font-mono border border-gray-200 rounded-xl focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400 resize-none bg-gray-50"
                    placeholder="Enter your system prompt..."
                />
                <p className="mt-2 text-xs text-gray-500">
                    This is the base prompt. MCP tool descriptions are appended automatically based on enabled servers.
                </p>
            </div>

            {/* Tool prompt breakdown */}
            <div className="border border-gray-200 rounded-xl overflow-hidden">
                <div className="flex items-center justify-between px-4 py-3 bg-gray-50">
                    <div className="flex items-center gap-2">
                        <Wrench size={16} />
                        <span className="text-sm font-medium text-gray-700">Additional prompts from tools</span>
                    </div>
                    <button
                        onClick={fetchLayers}
                        className="text-xs text-blue-600 hover:text-blue-700"
                    >
                        Refresh
                    </button>
                </div>
                <div className="p-4 bg-white space-y-3">
                    {loadingPreview ? (
                        <div className="flex items-center gap-2 text-sm text-gray-600">
                            <Loader2 size={16} className="animate-spin" />
                            Loading tool prompts...
                        </div>
                    ) : layersError ? (
                        <div className="text-sm text-red-700 bg-red-50 px-3 py-2 rounded-lg">{layersError}</div>
                    ) : layers && layers.additions.length > 0 ? (
                        layers.additions.map((block, idx) => (
                            <pre
                                key={idx}
                                className="text-xs font-mono whitespace-pre-wrap text-gray-700 bg-gray-50 p-3 rounded-lg border border-gray-100"
                            >
                                {block}
                            </pre>
                        ))
                    ) : (
                        <p className="text-sm text-gray-500">No tool-specific prompts active.</p>
                    )}
                </div>
            </div>

            {/* Preview toggle */}
            <div className="border border-gray-200 rounded-xl overflow-hidden">
                <button
                    onClick={() => setShowPreview(!showPreview)}
                    className="w-full flex items-center justify-between px-4 py-3 bg-gray-50 hover:bg-gray-100 text-sm font-medium text-gray-700"
                >
                    <span className="flex items-center gap-2">
                        <MessageSquare size={16} />
                        Full System Prompt Preview
                        {enabledServers > 0 && (
                            <span className="text-xs bg-blue-100 text-blue-700 px-2 py-0.5 rounded-full">
                                {enabledServers} MCP server{enabledServers !== 1 ? 's' : ''} enabled
                            </span>
                        )}
                    </span>
                    {showPreview ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
                </button>

                {showPreview && (
                    <div className="p-4 bg-white border-t border-gray-200">
                        {loadingPreview ? (
                            <div className="flex items-center justify-center py-8">
                                <div className="w-5 h-5 border-2 border-blue-500 border-t-transparent rounded-full animate-spin" />
                            </div>
                        ) : (
                            <pre className="text-xs font-mono whitespace-pre-wrap text-gray-700 max-h-80 overflow-y-auto bg-gray-50 p-3 rounded-lg">
                                {preview || 'No preview available'}
                            </pre>
                        )}
                        <p className="mt-2 text-xs text-gray-500">
                            This is exactly what will be sent to the model as the system prompt.
                        </p>
                    </div>
                )}
            </div>

            {/* State Machine Preview */}
            <StateMachinePreview />

            {error && (
                <div className="text-sm text-red-600 bg-red-50 px-3 py-2 rounded-lg">
                    {error}
                </div>
            )}

            <div className="flex justify-between items-center">
                <button
                    onClick={handleReset}
                    disabled={isDefault}
                    className="flex items-center gap-2 px-4 py-2 text-gray-600 text-sm font-medium rounded-lg border border-gray-200 hover:bg-gray-50 disabled:opacity-50 disabled:cursor-not-allowed"
                    title={isDefault ? "Already using default prompt" : "Reset to default prompt"}
                >
                    <RotateCcw size={16} />
                    Reset to Default
                </button>
                <div className="text-xs text-gray-500">{hasChanges ? 'Pending changes' : 'No changes'}</div>
            </div>
        </div>
    );
}

// Interfaces Tab - tool calling formats
function InterfacesTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}) {
    const { settings, updateToolCallFormats, updateChatFormat } = useSettingsStore();
    const currentModel = useChatStore((state) => state.currentModel);
    const availableModels = useChatStore((state) => state.availableModels);
    const formatConfig = settings?.tool_call_formats || DEFAULT_TOOL_CALL_FORMATS;
    const formatOptions: { id: ToolCallFormatName; label: string; description: string }[] = [
        { id: 'native', label: 'Native (OpenAI API)', description: 'Use the model\'s native tool calling via the `tools` API parameter (recommended for supported models).' },
        { id: 'code_mode', label: 'Code Mode (Python)', description: 'Model returns a single Python program executed in the sandbox.' },
        { id: 'hermes', label: 'Hermes (tag-delimited)', description: '<tool_call>{"name": "...", "arguments": {...}}</tool_call>' },
        { id: 'mistral', label: 'Mistral (bracket)', description: '[TOOL_CALLS] [{"name": "...", "arguments": {...}}]' },
        { id: 'pythonic', label: 'Pythonic call', description: 'tool_name(arg1="value", arg2=123)' },
        { id: 'pure_json', label: 'Pure JSON', description: '{"tool": "name", "args": {...}} or array' },
    ];
    const chatFormatOptions: { id: ChatFormatName; label: string; description: string }[] = [
        { id: 'openai_completions', label: 'OpenAI Chat Completions', description: 'POST /v1/chat/completions (messages array).' },
        { id: 'openai_responses', label: 'OpenAI Responses', description: 'POST /v1/responses (input blocks). Requires endpoint/model support.' },
    ];

    const chatFormatDefault = settings?.chat_format_default ?? 'openai_completions';
    const chatFormatOverrides = settings?.chat_format_overrides ?? {};
    const currentModelChatFormat: ChatFormatName = currentModel
        ? chatFormatOverrides[currentModel] ?? chatFormatDefault
        : chatFormatDefault;
    const [localFormats, setLocalFormats] = useState<ToolCallFormatConfig>(formatConfig);
    const [baselineFormats, setBaselineFormats] = useState<ToolCallFormatConfig>(formatConfig);
    const [localChatFormat, setLocalChatFormat] = useState<ChatFormatName>(currentModelChatFormat);
    const [baselineChatFormat, setBaselineChatFormat] = useState<ChatFormatName>(currentModelChatFormat);
    const [isSaving, setIsSaving] = useState(false);

    useEffect(() => {
        const nextBaseline = formatConfig;
        const hasPending = JSON.stringify(localFormats) !== JSON.stringify(baselineFormats);
        if (!hasPending) {
            setLocalFormats(nextBaseline);
            setBaselineFormats(nextBaseline);
        } else {
            setBaselineFormats(nextBaseline);
        }
    }, [formatConfig, localFormats, baselineFormats]);

    useEffect(() => {
        const next = currentModelChatFormat;
        // If the local matches baseline, keep in sync with latest data
        if (localChatFormat === baselineChatFormat) {
            setLocalChatFormat(next);
            setBaselineChatFormat(next);
        } else {
            setBaselineChatFormat(next);
        }
    }, [baselineChatFormat, chatFormatDefault, chatFormatOverrides, currentModel, currentModelChatFormat, localChatFormat]);

    // Native tool calling state is now derived from localFormats.enabled.includes('native')
    // No need for separate sync effect

    const hasChanges =
        JSON.stringify(localFormats) !== JSON.stringify(baselineFormats) ||
        (currentModel ? localChatFormat !== baselineChatFormat : false);

    useEffect(() => {
        onDirtyChange?.(hasChanges);
    }, [hasChanges, onDirtyChange]);

    useEffect(() => {
        onSavingChange?.(isSaving);
    }, [isSaving, onSavingChange]);

    const toggleFormat = useCallback((format: ToolCallFormatName) => {
        setLocalFormats((prev) => {
            const nextEnabled = prev.enabled.includes(format)
                ? prev.enabled.filter((f) => f !== format)
                : [...prev.enabled, format];
            const deduped = Array.from(new Set(nextEnabled));
            const ensuredEnabled = deduped.length > 0 ? deduped : [...DEFAULT_TOOL_CALL_FORMATS.enabled];
            const nextPrimary = ensuredEnabled.includes(prev.primary) ? prev.primary : ensuredEnabled[0];
            return { enabled: ensuredEnabled, primary: nextPrimary };
        });
    }, []);

    const setPrimaryFormat = useCallback((format: ToolCallFormatName) => {
        setLocalFormats((prev) => {
            if (!prev.enabled.includes(format)) return prev;
            return { ...prev, primary: format };
        });
    }, []);

    const handleSave = useCallback(async () => {
        if (!settings) return;
        setIsSaving(true);
        onSavingChange?.(true);
        try {
            // Save tool call formats (includes Native format toggle)
            await updateToolCallFormats(localFormats);
            setBaselineFormats(localFormats);
            if (currentModel && localChatFormat !== baselineChatFormat) {
                await updateChatFormat(currentModel, localChatFormat);
                setBaselineChatFormat(localChatFormat);
            }
        } finally {
            setIsSaving(false);
            onSavingChange?.(false);
        }
    }, [
        baselineChatFormat,
        currentModel,
        localChatFormat,
        localFormats,
        onSavingChange,
        settings,
        updateChatFormat,
        updateToolCallFormats,
    ]);

    useEffect(() => {
        onRegisterSave?.(handleSave);
    }, [handleSave, onRegisterSave]);

    return (
        <div className="space-y-3">
            <div>
                <h3 className="text-sm font-medium text-gray-700">Tool calling formats</h3>
                <p className="text-xs text-gray-500">Primary is advertised in system prompts; others stay enabled for parsing/execution.</p>
            </div>
            <div className="border border-gray-200 rounded-xl bg-white overflow-hidden w-full">
                <div className="divide-y divide-gray-100">
                    {formatOptions.map((option) => {
                        const enabled = localFormats.enabled.includes(option.id);
                        const isPrimary = localFormats.primary === option.id;
                        return (
                            <div key={option.id} className="flex items-center justify-between px-4 py-3 gap-3">
                                <div className="flex items-start gap-3">
                                    <input
                                        type="checkbox"
                                        checked={enabled}
                                        onChange={() => toggleFormat(option.id)}
                                        className="mt-1 h-4 w-4 text-blue-600 border-gray-300 rounded"
                                    />
                                    <div>
                                        <div className="text-sm font-medium text-gray-900">{option.label}</div>
                                        <p className="text-xs text-gray-500 font-mono">{option.description}</p>
                                    </div>
                                </div>
                                <label className={`flex items-center gap-2 text-xs ${enabled ? 'text-gray-700' : 'text-gray-400'}`}>
                                    <input
                                        type="radio"
                                        name="primary-format"
                                        disabled={!enabled}
                                        checked={isPrimary}
                                        onChange={() => setPrimaryFormat(option.id)}
                                        className="h-3.5 w-3.5 text-blue-600 border-gray-300"
                                    />
                                    Primary
                                </label>
                            </div>
                        );
                    })}
                </div>
            </div>

            <div className="pt-1 space-y-2">
                <div className="flex items-start justify-between gap-3">
                    <div>
                        <h3 className="text-sm font-medium text-gray-700">Chat format (per model)</h3>
                        <p className="text-xs text-gray-500">
                            Default: <span className="font-mono">{chatFormatDefault}</span>. Overrides apply to the active model only.
                        </p>
                    </div>
                    <div className="text-xs text-gray-500 text-right">
                        {currentModel ? (
                            <>
                                <div className="font-medium text-gray-800">{currentModel}</div>
                                <div>{chatFormatOverrides[currentModel] ? 'Override applied' : 'Using default'}</div>
                            </>
                        ) : (
                            <div>Load a model to set per-model chat format.</div>
                        )}
                        {availableModels.length > 0 && (
                            <div className="mt-1 text-[11px] text-gray-400">Available: {availableModels.join(', ')}</div>
                        )}
                    </div>
                </div>
                <div className="border border-gray-200 rounded-xl bg-white overflow-hidden w-full">
                    <div className="divide-y divide-gray-100">
                        {chatFormatOptions.map((option) => {
                            const isSelected = localChatFormat === option.id;
                            return (
                                <label
                                    key={option.id}
                                    className={`flex items-center justify-between px-4 py-3 gap-3 cursor-pointer ${
                                        currentModel ? 'opacity-100' : 'opacity-60'
                                    }`}
                                >
                                    <div className="flex items-start gap-3">
                                        <input
                                            type="radio"
                                            name="chat-format"
                                            disabled={!currentModel}
                                            checked={isSelected}
                                            onChange={() => setLocalChatFormat(option.id)}
                                            className="mt-1 h-4 w-4 text-blue-600 border-gray-300"
                                        />
                                        <div>
                                            <div className="text-sm font-medium text-gray-900">{option.label}</div>
                                            <p className="text-xs text-gray-500 font-mono">{option.description}</p>
                                        </div>
                                    </div>
                                    {option.id === 'openai_responses' && (
                                        <span className="text-[11px] text-amber-600 bg-amber-50 border border-amber-100 px-2 py-1 rounded-full">
                                            Requires endpoint support (e.g., gpt-oss-20b)
                                        </span>
                                    )}
                                </label>
                            );
                        })}
                    </div>
                </div>
            </div>

        </div>
    );
}

// Built-ins Tab - python_execution and tool_search
function BuiltinsTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
    onRegisterReset,
}: {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
    onRegisterReset?: (handler: () => void) => void;
}) {
    const {
        settings,
        updateToolSearchMaxResults,
        updateToolExamplesEnabled,
        updateToolExamplesMax,
        updateToolSystemPrompt,
        updateRagChunkMinRelevancy,
        updateSchemaRelevancyThreshold,
        updateRagDominantThreshold,
        pythonAllowedImports,
    } = useSettingsStore();
    const allowedImports = (pythonAllowedImports && pythonAllowedImports.length > 0)
        ? pythonAllowedImports
        : FALLBACK_PYTHON_ALLOWED_IMPORTS;
    
    // Enablement toggles were removed - all built-ins are now enabled via + Attach Tool in chat
    const [localToolSearchMaxResults, setLocalToolSearchMaxResults] = useState(settings?.tool_search_max_results ?? 3);
    const [localToolExamplesEnabled, setLocalToolExamplesEnabled] = useState(settings?.tool_use_examples_enabled ?? false);
    const [localToolExamplesMax, setLocalToolExamplesMax] = useState(settings?.tool_use_examples_max ?? 2);
    
    // Relevancy thresholds for state machine
    const [localRagChunkMinRelevancy, setLocalRagChunkMinRelevancy] = useState(settings?.rag_chunk_min_relevancy ?? 0.3);
    const [localSchemaRelevancyThreshold, setLocalSchemaRelevancyThreshold] = useState(settings?.schema_relevancy_threshold ?? 0.4);
    const [localRagDominantThreshold, setLocalRagDominantThreshold] = useState(settings?.rag_dominant_threshold ?? 0.6);

    const defaultPythonPrompt = [
        "Use python_execution for calling tools, calculations, and data transforms.",
        "Tools found with tool_search will be available in the global scope, with parameters with the same name and in the same order as returned in the tool description.",
        "Do not use any imports that are not explicitly allowed.",
        `Here are the allowed imports: ${allowedImports.join(', ')}.`
    ].join(' ');
    const defaultToolSearchPrompt = "Call tool_search to discover MCP tools related to your search string. If the returned tools appear to be relevant to your goal, use them";
    const defaultSchemaSearchPrompt = "Use `schema_search` to discover database tables and their structure when you need to write SQL queries. Returns table names, column information, and SQL dialect hints.";
    const defaultSqlSelectPrompt = "Use `sql_select` to run SQL queries against configured database sources. NEVER make up or guess data values - always execute queries to get factual information. Do NOT return SQL code to the user; only show results.";

    const [pythonPromptDraft, setPythonPromptDraft] = useState(settings?.tool_system_prompts?.['builtin::python_execution'] || defaultPythonPrompt);
    const [toolSearchPromptDraft, setToolSearchPromptDraft] = useState(settings?.tool_system_prompts?.['builtin::tool_search'] || defaultToolSearchPrompt);
    const [schemaSearchPromptDraft, setSchemaSearchPromptDraft] = useState(settings?.tool_system_prompts?.['builtin::schema_search'] || defaultSchemaSearchPrompt);
    const [sqlSelectPromptDraft, setSqlSelectPromptDraft] = useState(settings?.tool_system_prompts?.['builtin::sql_select'] || defaultSqlSelectPrompt);

    const [baselineBuiltins, setBaselineBuiltins] = useState({
        pythonPrompt: settings?.tool_system_prompts?.['builtin::python_execution'] || defaultPythonPrompt,
        toolSearchPrompt: settings?.tool_system_prompts?.['builtin::tool_search'] || defaultToolSearchPrompt,
        schemaSearchPrompt: settings?.tool_system_prompts?.['builtin::schema_search'] || defaultSchemaSearchPrompt,
        sqlSelectPrompt: settings?.tool_system_prompts?.['builtin::sql_select'] || defaultSqlSelectPrompt,
        toolSearchMaxResults: settings?.tool_search_max_results ?? 3,
        toolExamplesEnabled: settings?.tool_use_examples_enabled ?? false,
        toolExamplesMax: settings?.tool_use_examples_max ?? 2,
        ragChunkMinRelevancy: settings?.rag_chunk_min_relevancy ?? 0.3,
        schemaRelevancyThreshold: settings?.schema_relevancy_threshold ?? 0.4,
        ragDominantThreshold: settings?.rag_dominant_threshold ?? 0.6,
    });
    const [isSaving, setIsSaving] = useState(false);

    useEffect(() => {
        const nextBaseline = {
            pythonPrompt: settings?.tool_system_prompts?.['builtin::python_execution'] || defaultPythonPrompt,
            toolSearchPrompt: settings?.tool_system_prompts?.['builtin::tool_search'] || defaultToolSearchPrompt,
            schemaSearchPrompt: settings?.tool_system_prompts?.['builtin::schema_search'] || defaultSchemaSearchPrompt,
            sqlSelectPrompt: settings?.tool_system_prompts?.['builtin::sql_select'] || defaultSqlSelectPrompt,
            toolSearchMaxResults: settings?.tool_search_max_results ?? 3,
            toolExamplesEnabled: settings?.tool_use_examples_enabled ?? false,
            toolExamplesMax: settings?.tool_use_examples_max ?? 2,
            ragChunkMinRelevancy: settings?.rag_chunk_min_relevancy ?? 0.3,
            schemaRelevancyThreshold: settings?.schema_relevancy_threshold ?? 0.4,
            ragDominantThreshold: settings?.rag_dominant_threshold ?? 0.6,
        };

        const hasPending =
            pythonPromptDraft !== baselineBuiltins.pythonPrompt ||
            toolSearchPromptDraft !== baselineBuiltins.toolSearchPrompt ||
            schemaSearchPromptDraft !== baselineBuiltins.schemaSearchPrompt ||
            sqlSelectPromptDraft !== baselineBuiltins.sqlSelectPrompt ||
            localToolSearchMaxResults !== baselineBuiltins.toolSearchMaxResults ||
            localToolExamplesEnabled !== baselineBuiltins.toolExamplesEnabled ||
            localToolExamplesMax !== baselineBuiltins.toolExamplesMax ||
            localRagChunkMinRelevancy !== baselineBuiltins.ragChunkMinRelevancy ||
            localSchemaRelevancyThreshold !== baselineBuiltins.schemaRelevancyThreshold ||
            localRagDominantThreshold !== baselineBuiltins.ragDominantThreshold;

        if (!hasPending) {
            setPythonPromptDraft(nextBaseline.pythonPrompt);
            setToolSearchPromptDraft(nextBaseline.toolSearchPrompt);
            setSchemaSearchPromptDraft(nextBaseline.schemaSearchPrompt);
            setSqlSelectPromptDraft(nextBaseline.sqlSelectPrompt);
            setLocalToolSearchMaxResults(nextBaseline.toolSearchMaxResults);
            setLocalToolExamplesEnabled(nextBaseline.toolExamplesEnabled);
            setLocalToolExamplesMax(nextBaseline.toolExamplesMax);
            setLocalRagChunkMinRelevancy(nextBaseline.ragChunkMinRelevancy);
            setLocalSchemaRelevancyThreshold(nextBaseline.schemaRelevancyThreshold);
            setLocalRagDominantThreshold(nextBaseline.ragDominantThreshold);
            setBaselineBuiltins(nextBaseline);
        } else {
            setBaselineBuiltins(nextBaseline);
        }
    }, [
        settings?.tool_search_max_results,
        settings?.tool_use_examples_enabled,
        settings?.tool_use_examples_max,
        settings?.rag_chunk_min_relevancy,
        settings?.schema_relevancy_threshold,
        settings?.rag_dominant_threshold,
        settings?.tool_system_prompts,
        defaultPythonPrompt,
        defaultToolSearchPrompt,
        defaultSchemaSearchPrompt,
        defaultSqlSelectPrompt,
    ]);

    const hasChanges =
        pythonPromptDraft !== baselineBuiltins.pythonPrompt ||
        toolSearchPromptDraft !== baselineBuiltins.toolSearchPrompt ||
        schemaSearchPromptDraft !== baselineBuiltins.schemaSearchPrompt ||
        sqlSelectPromptDraft !== baselineBuiltins.sqlSelectPrompt ||
        localToolSearchMaxResults !== baselineBuiltins.toolSearchMaxResults ||
        localToolExamplesEnabled !== baselineBuiltins.toolExamplesEnabled ||
        localToolExamplesMax !== baselineBuiltins.toolExamplesMax ||
        localRagChunkMinRelevancy !== baselineBuiltins.ragChunkMinRelevancy ||
        localSchemaRelevancyThreshold !== baselineBuiltins.schemaRelevancyThreshold ||
        localRagDominantThreshold !== baselineBuiltins.ragDominantThreshold;

    useEffect(() => {
        onDirtyChange?.(hasChanges);
    }, [hasChanges, onDirtyChange]);

    useEffect(() => {
        onSavingChange?.(isSaving);
    }, [isSaving, onSavingChange]);

    const handleResetAll = useCallback(() => {
        setPythonPromptDraft(defaultPythonPrompt);
        setToolSearchPromptDraft(defaultToolSearchPrompt);
        setSchemaSearchPromptDraft(defaultSchemaSearchPrompt);
        setSqlSelectPromptDraft(defaultSqlSelectPrompt);
        setLocalToolSearchMaxResults(3);
        setLocalToolExamplesEnabled(false);
        setLocalToolExamplesMax(2);
        setLocalRagChunkMinRelevancy(0.3);
        setLocalSchemaRelevancyThreshold(0.4);
        setLocalRagDominantThreshold(0.6);
    }, [
        defaultPythonPrompt,
        defaultToolSearchPrompt,
        defaultSchemaSearchPrompt,
        defaultSqlSelectPrompt,
    ]);

    const handleResetPythonPrompt = () => {
        setPythonPromptDraft(defaultPythonPrompt);
    };

    const handleResetToolSearchPrompt = () => {
        setToolSearchPromptDraft(defaultToolSearchPrompt);
    };

    const handleResetSchemaSearchPrompt = () => {
        setSchemaSearchPromptDraft(defaultSchemaSearchPrompt);
    };

    const handleResetSqlSelectPrompt = () => {
        setSqlSelectPromptDraft(defaultSqlSelectPrompt);
    };

    const handleSave = useCallback(async () => {
        if (!settings) return;
        setIsSaving(true);
        onSavingChange?.(true);

        const saves: Promise<unknown>[] = [];
        const targetPythonPrompt = pythonPromptDraft?.trim() ? pythonPromptDraft : defaultPythonPrompt;
        const targetToolSearchPrompt = toolSearchPromptDraft?.trim() ? toolSearchPromptDraft : defaultToolSearchPrompt;
        const targetSchemaSearchPrompt = schemaSearchPromptDraft?.trim() ? schemaSearchPromptDraft : defaultSchemaSearchPrompt;
        const targetSqlSelectPrompt = sqlSelectPromptDraft?.trim() ? sqlSelectPromptDraft : defaultSqlSelectPrompt;

        if (localToolSearchMaxResults !== (settings.tool_search_max_results ?? 3)) {
            saves.push(updateToolSearchMaxResults(localToolSearchMaxResults));
        }

        if (localToolExamplesEnabled !== (settings.tool_use_examples_enabled ?? false)) {
            saves.push(updateToolExamplesEnabled(localToolExamplesEnabled));
        }

        if (localToolExamplesMax !== (settings.tool_use_examples_max ?? 2)) {
            saves.push(updateToolExamplesMax(localToolExamplesMax));
        }

        if (localRagChunkMinRelevancy !== settings.rag_chunk_min_relevancy) {
            saves.push(updateRagChunkMinRelevancy(localRagChunkMinRelevancy));
        }

        if (localSchemaRelevancyThreshold !== settings.schema_relevancy_threshold) {
            saves.push(updateSchemaRelevancyThreshold(localSchemaRelevancyThreshold));
        }

        if (localRagDominantThreshold !== settings.rag_dominant_threshold) {
            saves.push(updateRagDominantThreshold(localRagDominantThreshold));
        }

        if (targetPythonPrompt !== (settings.tool_system_prompts?.['builtin::python_execution'] || defaultPythonPrompt)) {
            saves.push(updateToolSystemPrompt('builtin', 'python_execution', targetPythonPrompt));
        }

        if (targetToolSearchPrompt !== (settings.tool_system_prompts?.['builtin::tool_search'] || defaultToolSearchPrompt)) {
            saves.push(updateToolSystemPrompt('builtin', 'tool_search', targetToolSearchPrompt));
        }

        if (targetSchemaSearchPrompt !== (settings.tool_system_prompts?.['builtin::schema_search'] || defaultSchemaSearchPrompt)) {
            saves.push(updateToolSystemPrompt('builtin', 'schema_search', targetSchemaSearchPrompt));
        }

        if (targetSqlSelectPrompt !== (settings.tool_system_prompts?.['builtin::sql_select'] || defaultSqlSelectPrompt)) {
            saves.push(updateToolSystemPrompt('builtin', 'sql_select', targetSqlSelectPrompt));
        }

        try {
            await Promise.all(saves);
            setBaselineBuiltins({
                pythonPrompt: targetPythonPrompt,
                toolSearchPrompt: targetToolSearchPrompt,
                schemaSearchPrompt: targetSchemaSearchPrompt,
                sqlSelectPrompt: targetSqlSelectPrompt,
                toolSearchMaxResults: localToolSearchMaxResults,
                toolExamplesEnabled: localToolExamplesEnabled,
                toolExamplesMax: localToolExamplesMax,
                ragChunkMinRelevancy: localRagChunkMinRelevancy,
                schemaRelevancyThreshold: localSchemaRelevancyThreshold,
                ragDominantThreshold: localRagDominantThreshold,
            });
        } finally {
            setIsSaving(false);
            onSavingChange?.(false);
        }
    }, [
        pythonPromptDraft,
        toolSearchPromptDraft,
        schemaSearchPromptDraft,
        sqlSelectPromptDraft,
        localToolSearchMaxResults,
        localToolExamplesEnabled,
        localToolExamplesMax,
        localRagChunkMinRelevancy,
        localSchemaRelevancyThreshold,
        localRagDominantThreshold,
        settings,
        defaultPythonPrompt,
        defaultToolSearchPrompt,
        defaultSchemaSearchPrompt,
        defaultSqlSelectPrompt,
        updateToolSearchMaxResults,
        updateToolExamplesEnabled,
        updateToolExamplesMax,
        updateRagChunkMinRelevancy,
        updateSchemaRelevancyThreshold,
        updateRagDominantThreshold,
        updateToolSystemPrompt,
        onSavingChange,
    ]);

    useEffect(() => {
        onRegisterSave?.(handleSave);
    }, [handleSave, onRegisterSave]);

    useEffect(() => {
        onRegisterReset?.(handleResetAll);
    }, [handleResetAll, onRegisterReset]);

    return (
        <div className="space-y-3">
            <div>
                <h3 className="text-sm font-medium text-gray-700">Built-in tools</h3>
                <p className="text-xs text-gray-500">Core tools that run locally within the app.</p>
            </div>

            <div className="flex flex-col gap-3">
                {/* python_execution (combined card) */}
                <div className="border border-gray-200 rounded-xl bg-white p-4 space-y-3 w-full">
                    <div className="flex items-start justify-between gap-3">
                        <div className="flex items-center gap-3">
                            <div>
                                <div className="flex items-center gap-2">
                                    <span className="font-medium text-gray-900">python_execution</span>
                                    <span className="text-xs bg-amber-100 text-amber-700 px-2 py-0.5 rounded-full">builtin</span>
                                </div>
                                <p className="text-xs text-gray-500 mt-0.5">
                                    Run Python code for calculations, data processing, and transformations
                                </p>
                            </div>
                        </div>
                    </div>
                    <div className="flex items-center justify-between gap-2">
                        <div className="text-xs font-semibold text-gray-900">System prompt (optional)</div>
                        <div className="flex items-center gap-2">
                            <button
                                onClick={handleResetPythonPrompt}
                                className="text-[11px] text-gray-600 px-2 py-0.5 rounded border border-gray-200 hover:bg-gray-50"
                                title="Reset to default prompt"
                            >
                                Reset
                            </button>
                        </div>
                    </div>
                    <textarea
                        value={pythonPromptDraft}
                        onChange={(e) => setPythonPromptDraft(e.target.value)}
                        rows={3}
                        className="w-full px-3 py-2 text-sm font-mono border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400 bg-gray-50"
                        placeholder={defaultPythonPrompt}
                    />
                    <p className="text-[11px] text-gray-500">Appended to the system prompt when Python execution is attached to a chat.</p>
                </div>

                {/* tool_search prompt card */}
                <div className="border border-gray-200 rounded-xl bg-white p-4 space-y-2 w-full">
                    <div className="flex items-start justify-between">
                        <div className="flex items-center gap-3">
                            <div>
                                <div className="text-sm font-semibold text-gray-900">tool_search (Configuration)</div>
                                <p className="text-xs text-gray-500">
                                    Discover MCP tools related to your search query.
                                </p>
                            </div>
                        </div>
                        <div className="flex items-center gap-2">
                            <button
                                onClick={handleResetToolSearchPrompt}
                                className="text-[11px] text-gray-600 px-2 py-0.5 rounded border border-gray-200 hover:bg-gray-50"
                                title="Reset to default prompt"
                            >
                                Reset
                            </button>
                            <span className="text-[11px] bg-blue-50 text-blue-700 px-2 py-0.5 rounded-full border border-blue-100">builtin</span>
                        </div>
                    </div>
                    <label className="text-xs font-medium text-gray-600">System prompt (optional)</label>
                    <textarea
                        value={toolSearchPromptDraft}
                        onChange={(e) => setToolSearchPromptDraft(e.target.value)}
                        rows={3}
                        className="w-full px-3 py-2 text-sm font-mono border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400 bg-gray-50"
                        placeholder={defaultToolSearchPrompt}
                    />
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 pt-1">
                        <label className="flex flex-col text-xs text-gray-600">
                            <span className="font-semibold text-gray-800 mb-1">Max results per search</span>
                            <input
                                type="number"
                                min={1}
                                max={20}
                                value={localToolSearchMaxResults}
                                onChange={(e) => {
                                    const next = Math.min(20, Math.max(1, Number(e.target.value) || 1));
                                    setLocalToolSearchMaxResults(next);
                                }}
                                className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                            />
                            <span className="text-[11px] text-gray-500 mt-1">Caps tool_search and auto-discovery results.</span>
                        </label>
                        <div className="flex flex-col gap-2 border border-gray-100 rounded-lg p-3 bg-gray-50">
                            <div className="flex items-center justify-between">
                                <div>
                                    <div className="text-xs font-semibold text-gray-800">Tool examples</div>
                                    <p className="text-[11px] text-gray-500">Include input_examples in prompts.</p>
                                </div>
                                <button
                                    onClick={() => setLocalToolExamplesEnabled((prev) => !prev)}
                                    className={`relative w-9 h-5 rounded-full transition-colors ${localToolExamplesEnabled ? 'bg-blue-500' : 'bg-gray-300'
                                        }`}
                                    title="Toggle tool input_examples in prompts"
                                >
                                    <div
                                        className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${localToolExamplesEnabled ? 'translate-x-4' : ''
                                            }`}
                                    />
                                </button>
                            </div>
                            {localToolExamplesEnabled && (
                                <div className="flex items-center gap-2">
                                    <span className="text-[11px] text-gray-600">Max per tool:</span>
                                    <input
                                        type="number"
                                        min={1}
                                        max={5}
                                        value={localToolExamplesMax}
                                        onChange={(e) => {
                                            const next = Math.min(5, Math.max(1, Number(e.target.value) || 1));
                                            setLocalToolExamplesMax(next);
                                        }}
                                        className="w-20 px-2 py-1 text-sm border border-gray-200 rounded-md focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                                    />
                                </div>
                            )}
                        </div>
                    </div>
                </div>

                {/* schema_search card */}
                <div className="border border-gray-200 rounded-xl bg-white p-4 space-y-2 w-full">
                    <div className="flex items-start justify-between">
                        <div className="flex items-center gap-3">
                            <div>
                                <div className="text-sm font-semibold text-gray-900">schema_search</div>
                                <p className="text-xs text-gray-500">
                                    Discover database tables and their structure.
                                </p>
                            </div>
                        </div>
                        <div className="flex items-center gap-2">
                            <button
                                onClick={handleResetSchemaSearchPrompt}
                                className="text-[11px] text-gray-600 px-2 py-0.5 rounded border border-gray-200 hover:bg-gray-50"
                                title="Reset to default prompt"
                            >
                                Reset
                            </button>
                            <span className="text-[11px] bg-blue-50 text-blue-700 px-2 py-0.5 rounded-full border border-blue-100">builtin</span>
                        </div>
                    </div>
                    <label className="text-xs font-medium text-gray-600">System prompt (optional)</label>
                    <textarea
                        value={schemaSearchPromptDraft}
                        onChange={(e) => setSchemaSearchPromptDraft(e.target.value)}
                        rows={3}
                        className="w-full px-3 py-2 text-sm font-mono border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400 bg-gray-50"
                        placeholder={defaultSchemaSearchPrompt}
                    />
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 pt-1">
                        <label className="flex flex-col text-xs text-gray-600">
                            <span className="font-semibold text-gray-800 mb-1">Schema Relevancy Threshold</span>
                            <input
                                type="number"
                                step={0.1}
                                min={0}
                                max={1}
                                value={localSchemaRelevancyThreshold}
                                onChange={(e) => setLocalSchemaRelevancyThreshold(Number(e.target.value))}
                                className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                            />
                            <span className="text-[11px] text-gray-500 mt-1">Minimum similarity to include a table.</span>
                        </label>
                    </div>
                </div>

                {/* sql_select card */}
                <div className="border border-gray-200 rounded-xl bg-white p-4 space-y-2 w-full">
                    <div className="flex items-start justify-between">
                        <div className="flex items-center gap-3">
                            <div>
                                <div className="text-sm font-semibold text-gray-900">sql_select</div>
                                <p className="text-xs text-gray-500">
                                    Execute SQL queries against attached tables.
                                </p>
                            </div>
                        </div>
                        <div className="flex items-center gap-2">
                            <button
                                onClick={handleResetSqlSelectPrompt}
                                className="text-[11px] text-gray-600 px-2 py-0.5 rounded border border-gray-200 hover:bg-gray-50"
                                title="Reset to default prompt"
                            >
                                Reset
                            </button>
                            <span className="text-[11px] bg-blue-50 text-blue-700 px-2 py-0.5 rounded-full border border-blue-100">builtin</span>
                        </div>
                    </div>
                    <label className="text-xs font-medium text-gray-600">System prompt (optional)</label>
                    <textarea
                        value={sqlSelectPromptDraft}
                        onChange={(e) => setSqlSelectPromptDraft(e.target.value)}
                        rows={3}
                        className="w-full px-3 py-2 text-sm font-mono border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400 bg-gray-50"
                        placeholder={defaultSqlSelectPrompt}
                    />
                </div>

                {/* Shared Relevancy thresholds */}
                <div className="border border-gray-200 rounded-xl bg-white p-4 space-y-3 w-full">
                    <div className="text-sm font-semibold text-gray-900">General Retrieval Configuration</div>
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                        <label className="flex flex-col text-xs text-gray-600">
                            <span className="font-semibold text-gray-800 mb-1">RAG Relevancy Threshold</span>
                            <input
                                type="number"
                                step={0.1}
                                min={0}
                                max={1}
                                value={localRagChunkMinRelevancy}
                                onChange={(e) => setLocalRagChunkMinRelevancy(Number(e.target.value))}
                                className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                            />
                            <span className="text-[11px] text-gray-500 mt-1">Minimum similarity to include a document chunk.</span>
                        </label>
                        <label className="flex flex-col text-xs text-gray-600">
                            <span className="font-semibold text-gray-800 mb-1">RAG Dominant Threshold</span>
                            <input
                                type="number"
                                step={0.1}
                                min={0}
                                max={1}
                                value={localRagDominantThreshold}
                                onChange={(e) => setLocalRagDominantThreshold(Number(e.target.value))}
                                className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                            />
                            <span className="text-[11px] text-gray-500 mt-1">Similarity above which SQL is suppressed to focus context.</span>
                        </label>
                    </div>
                </div>
            </div>

            <div className="mt-6 pt-4 border-t border-gray-100">
                <p className="text-sm text-gray-600 text-center italic">
                    Select <strong>+ Attach Tool</strong> in chat to use an enabled tool
                </p>
            </div>
        </div>
    );
}

// Tools Tab - MCP servers only
function ToolsTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}) {
    const { settings, addMcpServer, updateMcpServer, removeMcpServer, updateToolSystemPrompt, error, serverStatuses, addAlwaysOnBuiltinTool, removeAlwaysOnBuiltinTool } = useSettingsStore();
    const servers = settings?.mcp_servers || [];
    const alwaysOnBuiltins = settings?.always_on_builtin_tools || [];

    // Built-in tools that can be set to always-on
    const builtinTools = [
        { name: 'python_execution', description: 'Execute Python code in a sandboxed environment' },
        { name: 'sql_select', description: 'Execute SQL SELECT queries on configured databases' },
        { name: 'schema_search', description: 'Search for relevant database tables by description' },
        { name: 'tool_search', description: 'Discover MCP tools relevant to the current task' },
    ];

    const isBuiltinAlwaysOn = (name: string) => alwaysOnBuiltins.includes(name);

    const toggleBuiltinAlwaysOn = async (name: string) => {
        if (isBuiltinAlwaysOn(name)) {
            await removeAlwaysOnBuiltinTool(name);
        } else {
            await addAlwaysOnBuiltinTool(name);
        }
    };

    const [serverDirtyMap, setServerDirtyMap] = useState<Record<string, boolean>>({});
    const serverSaveHandlers = useRef<Record<string, () => Promise<void>>>({});
    const [isSaving, setIsSaving] = useState(false);

    useEffect(() => {
        setServerDirtyMap((prev) => {
            const next: Record<string, boolean> = {};
            servers.forEach((s) => {
                next[s.id] = prev[s.id] ?? false;
            });
            return next;
        });
    }, [servers]);

    const markServerDirty = useCallback((id: string, dirty: boolean) => {
        setServerDirtyMap((prev) => ({
            ...prev,
            [id]: dirty,
        }));
    }, []);

    const hasServerChanges = Object.values(serverDirtyMap).some(Boolean);

    useEffect(() => {
        onDirtyChange?.(hasServerChanges);
    }, [hasServerChanges, onDirtyChange]);

    useEffect(() => {
        onSavingChange?.(isSaving);
    }, [isSaving, onSavingChange]);

    const handleAddServer = () => {
        const newConfig = createNewServerConfig();
        addMcpServer(newConfig);
    };

    const handleSaveAll = useCallback(async () => {
        if (!settings) return;
        setIsSaving(true);
        onSavingChange?.(true);

        const saves: Promise<unknown>[] = [];

        const dirtyServerIds = Object.entries(serverDirtyMap)
            .filter(([, dirty]) => dirty)
            .map(([id]) => id);

        dirtyServerIds.forEach((id) => {
            const saveFn = serverSaveHandlers.current[id];
            if (saveFn) {
                saves.push(
                    saveFn().catch((err) => {
                        console.error(`Failed to save MCP server ${id}:`, err);
                        throw err;
                    })
                );
            }
        });

        try {
            await Promise.all(saves);
            setServerDirtyMap((prev) => {
                const next: Record<string, boolean> = {};
                Object.keys(prev).forEach((id) => {
                    next[id] = prev[id] && dirtyServerIds.includes(id) ? false : prev[id];
                });
                return next;
            });
        } finally {
            setIsSaving(false);
            onSavingChange?.(false);
        }
    }, [onSavingChange, serverDirtyMap, settings]);

    useEffect(() => {
        onRegisterSave?.(handleSaveAll);
    }, [handleSaveAll, onRegisterSave]);

    return (
        <div className="space-y-6">
            {/* Built-in Tools Always-On Section */}
            <div className="space-y-3">
                <div>
                    <h3 className="text-sm font-medium text-gray-700">Built-in Tools</h3>
                    <p className="text-xs text-gray-500">Mark built-in tools as "Always On" to include them in every chat</p>
                </div>
                <div className="grid gap-2">
                    {builtinTools.map((tool) => {
                        const isOn = isBuiltinAlwaysOn(tool.name);
                        return (
                            <div 
                                key={tool.name}
                                className={`flex items-center justify-between p-3 rounded-lg border transition-colors ${
                                    isOn ? 'bg-purple-50 border-purple-200' : 'bg-white border-gray-200'
                                }`}
                            >
                                <div>
                                    <div className="text-sm font-medium text-gray-900">{tool.name}</div>
                                    <div className="text-xs text-gray-500">{tool.description}</div>
                                </div>
                                <button
                                    onClick={() => toggleBuiltinAlwaysOn(tool.name)}
                                    className={`px-3 py-1 text-xs font-medium rounded-full transition-colors ${
                                        isOn 
                                            ? 'bg-purple-500 text-white hover:bg-purple-600' 
                                            : 'bg-gray-100 text-gray-600 hover:bg-gray-200'
                                    }`}
                                >
                                    {isOn ? 'Always On' : 'Off'}
                                </button>
                            </div>
                        );
                    })}
                </div>
            </div>

            {/* MCP Servers Section */}
            <div className="space-y-3">
                <div className="flex items-center justify-between">
                    <div>
                        <h3 className="text-sm font-medium text-gray-700">MCP Servers</h3>
                        <p className="text-xs text-gray-500">External tools via Model Context Protocol</p>
                    </div>
                    <button
                        onClick={handleAddServer}
                        className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-600 text-white text-xs font-medium rounded-lg hover:bg-blue-700"
                    >
                        <Plus size={14} />
                        Add Server
                    </button>
                </div>

                {error && (
                    <div className="text-sm text-red-600 bg-red-50 px-3 py-2 rounded-lg">
                        {error}
                    </div>
                )}

                <div className="space-y-3">
                    {servers.length === 0 ? (
                        <div className="text-center py-8 text-gray-500 border border-dashed border-gray-200 rounded-xl">
                            <Server size={32} className="mx-auto mb-2 opacity-30" />
                            <p className="text-sm">No MCP servers configured</p>
                            <p className="text-xs mt-1">Add a server to enable external tool capabilities</p>
                        </div>
                    ) : (
                        servers.map((server) => (
                            <McpServerCard
                                key={server.id}
                                config={server}
                                onSave={updateMcpServer}
                                onRemove={() => removeMcpServer(server.id)}
                                initialTools={serverStatuses?.[server.id]?.tools}
                                toolPrompts={settings?.tool_system_prompts || {}}
                                onSaveToolPrompt={updateToolSystemPrompt}
                                onDirtyChange={markServerDirty}
                                registerSaveHandler={(id, handler) => {
                                    serverSaveHandlers.current[id] = handler;
                                }}
                            />
                        ))
                    )}
                </div>
            </div>

            <div className="mt-6 pt-4 border-t border-gray-100 text-center italic">
                <p className="text-sm text-gray-600">
                    Select <strong>+ Attach Tool</strong> in chat to use an enabled tool
                </p>
            </div>
        </div>
    );
}

// Schemas Tab - manage always-on database tables
function SchemasTab() {
    const { settings, addAlwaysOnTable, removeAlwaysOnTable } = useSettingsStore();
    const [cachedTables, setCachedTables] = useState<any[]>([]);
    const [loading, setLoading] = useState(false);
    const [searchQuery, setSearchQuery] = useState('');

    // Fetch cached tables from backend
    useEffect(() => {
        const fetchTables = async () => {
            setLoading(true);
            try {
                const tables = await invoke<any[]>('get_cached_database_schemas');
                setCachedTables(tables);
            } catch (e: any) {
                console.error('[SchemasTab] Failed to fetch tables:', e);
            } finally {
                setLoading(false);
            }
        };
        fetchTables();
    }, []);

    const alwaysOnTables = settings?.always_on_tables || [];
    
    const isAlwaysOn = (sourceId: string, tableFqName: string) => 
        alwaysOnTables.some(t => t.source_id === sourceId && t.table_fq_name === tableFqName);

    const toggleAlwaysOn = async (sourceId: string, tableFqName: string) => {
        if (isAlwaysOn(sourceId, tableFqName)) {
            await removeAlwaysOnTable(sourceId, tableFqName);
        } else {
            await addAlwaysOnTable(sourceId, tableFqName);
        }
    };

    // Filter tables by search query
    const filteredTables = cachedTables.filter(table => {
        if (!searchQuery.trim()) return true;
        const query = searchQuery.toLowerCase();
        return table.fully_qualified_name?.toLowerCase().includes(query) ||
               table.source_id?.toLowerCase().includes(query);
    });

    return (
        <div className="space-y-6">
            <div>
                <h3 className="text-sm font-medium text-gray-700">Always-On Tables</h3>
                <p className="text-xs text-gray-500 mt-1">
                    Tables marked as "Always On" will automatically have their schemas included in every chat.
                    They appear as locked pills in the chat input area.
                </p>
            </div>

            {/* Search */}
            <div className="relative">
                <input
                    type="text"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    placeholder="Search tables..."
                    className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                />
            </div>

            {loading ? (
                <div className="flex items-center justify-center py-12">
                    <Loader2 className="animate-spin text-gray-400" size={24} />
                </div>
            ) : filteredTables.length === 0 ? (
                <div className="text-center py-8 text-gray-500 border border-dashed border-gray-200 rounded-xl">
                    <Database size={32} className="mx-auto mb-2 opacity-30" />
                    <p className="text-sm">No cached database tables</p>
                    <p className="text-xs mt-1">Go to Databases tab and click "Refresh Schemas" to index your tables</p>
                </div>
            ) : (
                <div className="space-y-2 max-h-[400px] overflow-y-auto">
                    {filteredTables.map((table) => {
                        const isOn = isAlwaysOn(table.source_id, table.fully_qualified_name);
                        return (
                            <div 
                                key={`${table.source_id}::${table.fully_qualified_name}`}
                                className={`flex items-center justify-between p-3 rounded-lg border transition-colors ${
                                    isOn ? 'bg-amber-50 border-amber-200' : 'bg-white border-gray-200'
                                }`}
                            >
                                <div className="flex-1 min-w-0">
                                    <div className="text-sm font-medium text-gray-900 truncate">
                                        {table.fully_qualified_name}
                                    </div>
                                    <div className="text-xs text-gray-500">
                                        Source: {table.source_id} | {table.column_count || 0} columns
                                    </div>
                                </div>
                                <button
                                    onClick={() => toggleAlwaysOn(table.source_id, table.fully_qualified_name)}
                                    className={`ml-4 px-3 py-1 text-xs font-medium rounded-full transition-colors ${
                                        isOn 
                                            ? 'bg-amber-500 text-white hover:bg-amber-600' 
                                            : 'bg-gray-100 text-gray-600 hover:bg-gray-200'
                                    }`}
                                >
                                    {isOn ? 'Always On' : 'Off'}
                                </button>
                            </div>
                        );
                    })}
                </div>
            )}

            {alwaysOnTables.length > 0 && (
                <div className="pt-4 border-t border-gray-100">
                    <p className="text-xs text-gray-500">
                        {alwaysOnTables.length} table{alwaysOnTables.length !== 1 ? 's' : ''} set to always-on
                    </p>
                </div>
            )}
        </div>
    );
}

// Files Tab - manage always-on RAG paths
function FilesTab() {
    const { settings, addAlwaysOnRagPath, removeAlwaysOnRagPath } = useSettingsStore();
    const alwaysOnPaths = settings?.always_on_rag_paths || [];

    const handleAddFiles = async () => {
        try {
            const paths = await invoke<string[]>('select_files');
            for (const path of paths) {
                if (!alwaysOnPaths.includes(path)) {
                    await addAlwaysOnRagPath(path);
                }
            }
        } catch (e: any) {
            console.error('[FilesTab] Failed to select files:', e);
        }
    };

    const handleAddFolder = async () => {
        try {
            const path = await invoke<string | null>('select_folder');
            if (path && !alwaysOnPaths.includes(path)) {
                await addAlwaysOnRagPath(path);
            }
        } catch (e: any) {
            console.error('[FilesTab] Failed to select folder:', e);
        }
    };

    return (
        <div className="space-y-6">
            <div>
                <h3 className="text-sm font-medium text-gray-700">Always-On Files</h3>
                <p className="text-xs text-gray-500 mt-1">
                    Files and folders marked as "Always On" will be automatically indexed and searched in every chat.
                    They appear as locked pills in the chat input area.
                </p>
            </div>

            {/* Add buttons */}
            <div className="flex gap-2">
                <button
                    onClick={handleAddFiles}
                    className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-600 text-white text-xs font-medium rounded-lg hover:bg-blue-700"
                >
                    <Plus size={14} />
                    Add Files
                </button>
                <button
                    onClick={handleAddFolder}
                    className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-600 text-white text-xs font-medium rounded-lg hover:bg-blue-700"
                >
                    <Plus size={14} />
                    Add Folder
                </button>
            </div>

            {alwaysOnPaths.length === 0 ? (
                <div className="text-center py-8 text-gray-500 border border-dashed border-gray-200 rounded-xl">
                    <HardDrive size={32} className="mx-auto mb-2 opacity-30" />
                    <p className="text-sm">No always-on files configured</p>
                    <p className="text-xs mt-1">Add files or folders to have them automatically available in every chat</p>
                </div>
            ) : (
                <div className="space-y-2">
                    {alwaysOnPaths.map((path) => (
                        <div 
                            key={path}
                            className="flex items-center justify-between p-3 bg-emerald-50 border border-emerald-200 rounded-lg"
                        >
                            <div className="flex-1 min-w-0">
                                <div className="text-sm font-medium text-gray-900 truncate" title={path}>
                                    {path.split(/[/\\]/).pop() || path}
                                </div>
                                <div className="text-xs text-gray-500 truncate" title={path}>
                                    {path}
                                </div>
                            </div>
                            <button
                                onClick={() => removeAlwaysOnRagPath(path)}
                                className="ml-4 p-1.5 text-gray-400 hover:text-red-500 hover:bg-red-50 rounded transition-colors"
                                title="Remove from always-on"
                            >
                                <Trash2 size={14} />
                            </button>
                        </div>
                    ))}
                </div>
            )}

            {alwaysOnPaths.length > 0 && (
                <div className="pt-4 border-t border-gray-100">
                    <p className="text-xs text-gray-500">
                        {alwaysOnPaths.length} path{alwaysOnPaths.length !== 1 ? 's' : ''} set to always-on
                    </p>
                </div>
            )}
        </div>
    );
}

// Main Settings Modal

interface SchemaRefreshProgress {
    message: string;
    source_name: string;
    current_table: string | null;
    tables_done: number;
    tables_total: number;
    is_complete: boolean;
    error: string | null;
}

interface SchemaRefreshStatus extends SchemaRefreshProgress {
    startTime: number;
}

function SchemaRefreshStatusBar({ status }: { status: SchemaRefreshStatus }) {
    const [elapsed, setElapsed] = useState(0);

    useEffect(() => {
        if (status.is_complete) return;
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - status.startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [status.startTime, status.is_complete]);

    const formatElapsedTime = (seconds: number): string => {
        if (seconds < 60) return `${seconds}s`;
        const mins = Math.floor(seconds / 60);
        const secs = seconds % 60;
        return `${mins}m ${secs}s`;
    };

    return (
        <div className={`flex items-center justify-between px-4 py-2 border rounded-lg transition-colors ${status.error ? 'bg-red-50 border-red-200 text-red-800' : 'bg-blue-50 border-blue-200 text-blue-800'}`}>
            <div className="flex items-center gap-3 min-w-0">
                {!status.is_complete && !status.error && <Loader2 className="animate-spin text-blue-600" size={16} />}
                {status.is_complete && <CheckCircle className="text-green-600" size={16} />}
                {status.error && <XCircle className="text-red-600" size={16} />}
                <div className="flex flex-col min-w-0">
                    <span className="text-sm font-medium truncate">
                        {status.error ? `Error: ${status.error}` : status.message}
                    </span>
                    {status.current_table && !status.is_complete && (
                        <span className="text-[10px] opacity-70 truncate">{status.current_table}</span>
                    )}
                </div>
            </div>
            {!status.is_complete && !status.error && (
                <span className="font-mono text-xs opacity-70 ml-4 flex-shrink-0">
                    {formatElapsedTime(elapsed)}
                </span>
            )}
        </div>
    );
}

// Databases Tab - manage database sources and toolbox config
function DatabasesTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}) {
    const { settings, updateDatabaseToolboxConfig } = useSettingsStore();
    const DEFAULT_TOOLBOX_COMMAND = '/opt/homebrew/bin/toolbox';
    const DEFAULT_BIGQUERY_ARGS = ['--stdio', '--prebuilt', 'bigquery'];
    const [toolboxConfig, setToolboxConfig] = useState(settings?.database_toolbox || {
        enabled: false,
        sources: [],
    });
    const [saveError, setSaveError] = useState<string | null>(null);
    const [refreshingSources, setRefreshingSources] = useState<Record<string, boolean>>({});
    const [refreshStatus, setRefreshStatus] = useState<SchemaRefreshStatus | null>(null);

    useEffect(() => {
        const unlistenPromise = listen<SchemaRefreshProgress>('schema-refresh-progress', (event) => {
            setRefreshStatus(prev => ({
                ...event.payload,
                startTime: prev?.startTime || Date.now(),
            }));
            
            if (event.payload.is_complete || event.payload.error) {
                setTimeout(() => setRefreshStatus(null), 5000);
            }
        });
        
        return () => {
            unlistenPromise.then(unlisten => unlisten());
        };
    }, []);

    // Simple deep erase of internal properties for checking dirty state if needed
    // For now simplistic dirty check
    const isDirty = JSON.stringify(settings?.database_toolbox) !== JSON.stringify(toolboxConfig);

    useEffect(() => {
        onDirtyChange?.(isDirty);
    }, [isDirty, onDirtyChange]);

    const handleSave = useCallback(async () => {
        onSavingChange?.(true);
        setSaveError(null);
        const validationErrors: string[] = [];
        const sanitizedSources = toolboxConfig.sources.map((src) => {
            const trimmedCommand = src.command?.trim() || '';
            const trimmedProject = src.project_id?.trim() || '';
            const commandWithDefault =
                src.transport.type === 'stdio' && !trimmedCommand
                    ? DEFAULT_TOOLBOX_COMMAND
                    : trimmedCommand;

            const argsWithDefault =
                (src.args || []).filter(Boolean).length === 0 && src.kind === 'bigquery'
                    ? DEFAULT_BIGQUERY_ARGS
                    : (src.args || []).filter(Boolean);

            const requiresCommand = src.enabled && src.transport.type === 'stdio';
            const requiresProject =
                src.enabled &&
                src.kind === 'bigquery' &&
                !(trimmedProject || src.env?.BIGQUERY_PROJECT?.trim());

            if (requiresCommand && !commandWithDefault) {
                validationErrors.push(
                    `${src.name || src.id}: command is required for stdio transport.`
                );
            }
            if (requiresProject) {
                validationErrors.push(
                    `${src.name || src.id}: set Project ID or BIGQUERY_PROJECT env for BigQuery.`
                );
            }

            return {
                ...src,
                command: commandWithDefault || null,
                args: argsWithDefault,
                project_id: trimmedProject || undefined,
            };
        });

        if (validationErrors.length > 0) {
            setSaveError(validationErrors.join(' '));
            onSavingChange?.(false);
            return;
        }

        const sanitizedConfig: DatabaseToolboxConfig = {
            ...toolboxConfig,
            sources: sanitizedSources,
        };
        try {
            setToolboxConfig(sanitizedConfig);
            await updateDatabaseToolboxConfig(sanitizedConfig);
        } catch (err: any) {
            const message = err?.message || String(err);
            setSaveError(message);
        } finally {
            onSavingChange?.(false);
        }
    }, [toolboxConfig, updateDatabaseToolboxConfig, onSavingChange]);

    useEffect(() => {
        onRegisterSave?.(handleSave);
    }, [handleSave, onRegisterSave]);

    const handleRefreshSchemas = async (sourceId: string) => {
        setRefreshingSources(prev => ({ ...prev, [sourceId]: true }));
        setSaveError(null);
        try {
            // Backend refresh_database_schemas refreshes all enabled sources
            // For now we'll just call it and it will refresh all enabled ones
            // In the future we might want a per-source refresh command
            await invoke('refresh_database_schemas');
        } catch (err: any) {
            setSaveError(`Refresh failed: ${err?.message || String(err)}`);
        } finally {
            setRefreshingSources(prev => ({ ...prev, [sourceId]: false }));
        }
    };

    // Handle adding a new database source
    const addSource = (kind: SupportedDatabaseKind) => {
        const newSource: DatabaseSourceConfig = {
            id: `db-${Date.now()}`,
            name: `New ${kind} Source`,
            kind,
            enabled: true,
            transport: { type: 'stdio' },
            command: null,
            args: [],
            env: {},
            auto_approve_tools: true,
            defer_tools: true,
            project_id: '',
        };
        setToolboxConfig(prev => ({
            ...prev,
            sources: [...prev.sources, newSource]
        }));
    };

    const updateSource = (index: number, updates: Partial<DatabaseSourceConfig>) => {
        setToolboxConfig(prev => {
            const newSources = [...prev.sources];
            newSources[index] = { ...newSources[index], ...updates };
            return { ...prev, sources: newSources };
        });
    };

    const removeSource = (index: number) => {
        setToolboxConfig(prev => {
            const newSources = [...prev.sources];
            newSources.splice(index, 1);
            return { ...prev, sources: newSources };
        });
    };

    return (
        <div className="space-y-6">
            <div>
                <h3 className="text-lg font-medium text-gray-900">Database Sources</h3>
                <p className="text-sm text-gray-500">Configure database MCP servers (stdio or SSE).</p>
            </div>

            {saveError && (
                <div className="database-config-error-alert flex items-start gap-2 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
                    <AlertCircle size={16} />
                    <div className="flex-1">
                        <div className="font-semibold text-red-800">Schema refresh failed</div>
                        <p className="text-xs text-red-700">
                            Fix the MCP database configuration here and try again. Details: {saveError}
                        </p>
                    </div>
                </div>
            )}

            {refreshStatus && (
                <SchemaRefreshStatusBar status={refreshStatus} />
            )}

            <div className="database-toolbox-toggle flex items-start gap-2 rounded-lg border border-gray-200 bg-white px-4 py-3">
                <input
                    type="checkbox"
                    className="mt-1 h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                    checked={toolboxConfig.enabled}
                    onChange={(e) => setToolboxConfig(prev => ({ ...prev, enabled: e.target.checked }))}
                />
                <div>
                    <div className="text-sm font-medium text-gray-900">Enable Database Toolbox</div>
                    <div className="text-xs text-gray-500">
                        Required for schema refresh and SQL execution via the MCP toolbox.
                    </div>
                </div>
            </div>

            <div className="space-y-4">
                {toolboxConfig.sources.map((source, idx) => (
                    <div key={source.id} className="database-source-card border border-gray-200 rounded-lg p-4 space-y-3 bg-white">
                        <div className="flex justify-between items-start">
                            <div className="flex items-center gap-2">
                                <span className="text-xs font-semibold bg-blue-100 text-blue-700 px-2 py-0.5 rounded uppercase">
                                    {source.kind}
                                </span>
                                <input
                                    type="text"
                                    value={source.name}
                                    onChange={(e) => updateSource(idx, { name: e.target.value })}
                                    className="font-medium text-gray-900 border-b border-transparent hover:border-gray-300 focus:border-blue-500 focus:outline-none px-1"
                                />
                                <select
                                    value={source.kind}
                                    onChange={(e) => updateSource(idx, { kind: e.target.value as SupportedDatabaseKind })}
                                    className="text-xs border border-gray-200 rounded px-2 py-1 text-gray-700 bg-white"
                                >
                                    <option value="bigquery">BigQuery</option>
                                    <option value="postgres">PostgreSQL</option>
                                    <option value="mysql">MySQL</option>
                                    <option value="sqlite">SQLite</option>
                                    <option value="spanner">Spanner</option>
                                </select>
                            </div>
                            <div className="flex items-center gap-3">
                                <label className="flex items-center gap-2 text-sm text-gray-600">
                                    <input
                                        type="checkbox"
                                        checked={source.enabled}
                                        onChange={(e) => updateSource(idx, { enabled: e.target.checked })}
                                        className="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                                    />
                                    Enabled
                                </label>
                                {source.enabled && (
                                    <button
                                        onClick={() => handleRefreshSchemas(source.id)}
                                        disabled={refreshingSources[source.id]}
                                        className="inline-flex items-center gap-1.5 px-2 py-1 text-xs font-medium rounded bg-blue-50 text-blue-600 hover:bg-blue-100 disabled:opacity-50 transition-colors"
                                        title="Refresh table schemas from this database"
                                    >
                                        {refreshingSources[source.id] ? (
                                            <Loader2 size={12} className="animate-spin" />
                                        ) : (
                                            <RefreshCw size={12} />
                                        )}
                                        Refresh
                                    </button>
                                )}
                                <button onClick={() => removeSource(idx)} className="text-gray-400 hover:text-red-500">
                                    <Trash2 size={16} />
                                </button>
                            </div>
                        </div>

                        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                            <div>
                                <label className="block text-xs font-medium text-gray-700 mb-1.5">Transport</label>
                                <div className="flex gap-2">
                                    <button
                                        onClick={() => updateSource(idx, { transport: { type: 'stdio' } })}
                                        className={`px-3 py-1.5 text-xs rounded-lg border ${source.transport.type === 'stdio'
                                            ? 'bg-blue-50 border-blue-300 text-blue-700'
                                            : 'bg-white border-gray-200 text-gray-600 hover:bg-gray-50'
                                            }`}
                                    >
                                        Stdio (subprocess)
                                    </button>
                                    <button
                                        onClick={() => updateSource(idx, { transport: { type: 'sse', url: (source.transport as any).url || '' } })}
                                        className={`px-3 py-1.5 text-xs rounded-lg border ${source.transport.type === 'sse'
                                            ? 'bg-blue-50 border-blue-300 text-blue-700'
                                            : 'bg-white border-gray-200 text-gray-600 hover:bg-gray-50'
                                            }`}
                                    >
                                        SSE (HTTP)
                                    </button>
                                </div>
                            </div>

                            {source.transport.type === 'sse' && (
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1.5">Server URL</label>
                                    <input
                                        type="text"
                                        value={(source.transport as any).url || ''}
                                        onChange={(e) => updateSource(idx, { transport: { type: 'sse', url: e.target.value } })}
                                        placeholder="http://localhost:3000/sse"
                                        className="w-full text-sm border-gray-300 rounded-md shadow-sm focus:border-blue-500 focus:ring-blue-500"
                                    />
                                </div>
                            )}
                        </div>

                            {source.kind === 'bigquery' && (
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1.5">Dataset allowlist (CSV, optional)</label>
                                    <input
                                        type="text"
                                        value={source.dataset_allowlist || ''}
                                        onChange={(e) => updateSource(idx, { dataset_allowlist: e.target.value })}
                                        placeholder="dataset_a,dataset_b"
                                        className="w-full text-sm border-gray-300 rounded-md shadow-sm focus:border-blue-500 focus:ring-blue-500"
                                    />
                                    <p className="text-[11px] text-gray-500 mt-1">
                                        If set, only these datasets are cached. Leave blank to enumerate all.
                                    </p>
                                </div>
                            )}

                        {source.transport.type === 'stdio' && (
                            <>
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1.5">Command</label>
                                    <input
                                        type="text"
                                        value={source.command || ''}
                                        onChange={(e) => updateSource(idx, { command: e.target.value })}
                                        placeholder="/opt/homebrew/bin/toolbox"
                                        className={`w-full text-sm rounded-md shadow-sm focus:ring-1 ${
                                            source.enabled && source.transport.type === 'stdio' && !(source.command?.trim())
                                                ? 'border-red-300 focus:border-red-500 focus:ring-red-500'
                                                : 'border-gray-300 focus:border-blue-500 focus:ring-blue-500'
                                        }`}
                                    />
                                </div>
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1.5">Arguments</label>
                                    <TagInput
                                        tags={source.args}
                                        onChange={(args) => updateSource(idx, { args })}
                                        placeholder="--stdio --prebuilt bigquery"
                                    />
                                </div>
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1.5">Environment Variables</label>
                                    <EnvVarInput
                                        env={source.env}
                                        onChange={(env) => updateSource(idx, { env })}
                                    />
                                </div>
                            </>
                        )}

                        {source.kind === 'bigquery' && (
                            <div className="space-y-3">
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1">
                                        Project ID
                                    </label>
                                    <input
                                        type="text"
                                        value={source.project_id || ''}
                                        onChange={(e) => updateSource(idx, { project_id: e.target.value })}
                                        placeholder="gcp-project-id"
                                        className="w-full text-sm border-gray-300 rounded-md shadow-sm focus:border-blue-500 focus:ring-blue-500"
                                    />
                                </div>
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1">
                                        Dataset allowlist (CSV, optional)
                                    </label>
                                    <input
                                        type="text"
                                        value={source.dataset_allowlist || ''}
                                        onChange={(e) => updateSource(idx, { dataset_allowlist: e.target.value })}
                                        placeholder="dataset_a,dataset_b"
                                        className="w-full text-sm border-gray-300 rounded-md shadow-sm focus:border-blue-500 focus:ring-blue-500"
                                    />
                                    <p className="text-[11px] text-gray-500 mt-1">
                                        If set, only these datasets are cached. Leave blank to enumerate all.
                                    </p>
                                </div>
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1">
                                        Table allowlist (CSV, optional)
                                    </label>
                                    <input
                                        type="text"
                                        value={source.table_allowlist || ''}
                                        onChange={(e) => updateSource(idx, { table_allowlist: e.target.value })}
                                        placeholder="table_a,table_b"
                                        className="w-full text-sm border-gray-300 rounded-md shadow-sm focus:border-blue-500 focus:ring-blue-500"
                                    />
                                    <p className="text-[11px] text-gray-500 mt-1">
                                        If set, only these tables are cached within allowed datasets. Leave blank to include all tables.
                                    </p>
                                </div>
                            </div>
                        )}

                        {/* Auto-approve is now implicit for database sources */}
                    </div>
                ))}

                {toolboxConfig.sources.length === 0 && (
                    <div className="text-center py-8 bg-gray-50 rounded-lg border border-dashed border-gray-300">
                        <p className="text-sm text-gray-500">No database sources configured.</p>
                    </div>
                )}

                <div className="flex gap-2 flex-wrap">
                    <button
                        onClick={() => addSource('bigquery')}
                        className="flex items-center gap-2 px-3 py-2 text-sm font-medium text-blue-600 bg-blue-50 rounded-lg hover:bg-blue-100"
                    >
                        <Plus size={16} />
                        Add BigQuery
                    </button>
                    <button
                        onClick={() => addSource('postgres')}
                        className="flex items-center gap-2 px-3 py-2 text-sm font-medium text-blue-600 bg-blue-50 rounded-lg hover:bg-blue-100"
                    >
                        <Plus size={16} />
                        Add PostgreSQL
                    </button>
                    <button
                        onClick={() => addSource('mysql')}
                        className="flex items-center gap-2 px-3 py-2 text-sm font-medium text-blue-600 bg-blue-50 rounded-lg hover:bg-blue-100"
                    >
                        <Plus size={16} />
                        Add MySQL
                    </button>
                    <button
                        onClick={() => addSource('sqlite')}
                        className="flex items-center gap-2 px-3 py-2 text-sm font-medium text-blue-600 bg-blue-50 rounded-lg hover:bg-blue-100"
                    >
                        <Plus size={16} />
                        Add SQLite
                    </button>
                    <button
                        onClick={() => addSource('spanner')}
                        className="flex items-center gap-2 px-3 py-2 text-sm font-medium text-blue-600 bg-blue-50 rounded-lg hover:bg-blue-100"
                    >
                        <Plus size={16} />
                        Add Spanner
                    </button>
                </div>
            </div>
        </div>
    );
}

export function SettingsModal() {
    const { isSettingsOpen, closeSettings, activeTab, setActiveTab, isLoading } = useSettingsStore();
    const [systemDirty, setSystemDirty] = useState(false);
    const [systemSaving, setSystemSaving] = useState(false);
    const systemSaveHandlerRef = useRef<(() => Promise<void>) | null>(null);
    const [toolsDirty, setToolsDirty] = useState(false);
    const [toolsSaving, setToolsSaving] = useState(false);
    const toolsSaveHandlerRef = useRef<(() => Promise<void>) | null>(null);
    const [interfacesDirty, setInterfacesDirty] = useState(false);
    const [interfacesSaving, setInterfacesSaving] = useState(false);
    const interfacesSaveHandlerRef = useRef<(() => Promise<void>) | null>(null);
    const [builtinsDirty, setBuiltinsDirty] = useState(false);
    const [builtinsSaving, setBuiltinsSaving] = useState(false);
    const builtinsSaveHandlerRef = useRef<(() => Promise<void>) | null>(null);
    const builtinsResetHandlerRef = useRef<(() => void) | null>(null);
    const [databasesDirty, setDatabasesDirty] = useState(false);
    const [databasesSaving, setDatabasesSaving] = useState(false);
    const databasesSaveHandlerRef = useRef<(() => Promise<void>) | null>(null);

    const { settings } = useSettingsStore();

    const handleRegisterSystemSave = useCallback((handler: () => Promise<void>) => {
        systemSaveHandlerRef.current = handler;
    }, []);

    const handleSystemSavingChange = useCallback((saving: boolean) => {
        setSystemSaving(saving);
    }, []);

    const handleRegisterToolsSave = useCallback((handler: () => Promise<void>) => {
        toolsSaveHandlerRef.current = handler;
    }, []);

    const handleToolsSavingChange = useCallback((saving: boolean) => {
        setToolsSaving(saving);
    }, []);

    const handleRegisterInterfacesSave = useCallback((handler: () => Promise<void>) => {
        interfacesSaveHandlerRef.current = handler;
    }, []);

    const handleInterfacesSavingChange = useCallback((saving: boolean) => {
        setInterfacesSaving(saving);
    }, []);

    const handleRegisterBuiltinsSave = useCallback((handler: () => Promise<void>) => {
        builtinsSaveHandlerRef.current = handler;
    }, []);

    const handleRegisterBuiltinsReset = useCallback((handler: () => void) => {
        builtinsResetHandlerRef.current = handler;
    }, []);

    const handleBuiltinsSavingChange = useCallback((saving: boolean) => {
        setBuiltinsSaving(saving);
    }, []);

    const handleRegisterDatabasesSave = useCallback((handler: () => Promise<void>) => {
        databasesSaveHandlerRef.current = handler;
    }, []);

    const handleDatabasesSavingChange = useCallback((saving: boolean) => {
        setDatabasesSaving(saving);
    }, []);

    const handleHeaderReset = useCallback(() => {
        if (activeTab === 'builtins') {
            builtinsResetHandlerRef.current?.();
        }
    }, [activeTab]);

    const handleHeaderSave = useCallback(async () => {
        let handler: (() => Promise<void>) | null = null;
        if (activeTab === 'system-prompt') {
            handler = systemSaveHandlerRef.current;
        } else if (activeTab === 'tools') {
            handler = toolsSaveHandlerRef.current;
        } else if (activeTab === 'interfaces') {
            handler = interfacesSaveHandlerRef.current;
        } else if (activeTab === 'builtins') {
            handler = builtinsSaveHandlerRef.current;
        } else if (activeTab === 'databases') {
            handler = databasesSaveHandlerRef.current;
        }
        if (!handler) return;
        await handler();
    }, [activeTab]);

    if (!isSettingsOpen) return null;

    const isCurrentTabDirty =
        activeTab === 'system-prompt'
            ? systemDirty
            : activeTab === 'tools'
                ? toolsDirty
                : activeTab === 'interfaces'
                    ? interfacesDirty
                    : activeTab === 'builtins'
                        ? builtinsDirty
                    : activeTab === 'databases'
                        ? databasesDirty
                        : false;

    const isCurrentTabSaving =
        activeTab === 'system-prompt'
            ? systemSaving
            : activeTab === 'tools'
                ? toolsSaving
                : activeTab === 'interfaces'
                    ? interfacesSaving
                    : activeTab === 'builtins'
                        ? builtinsSaving
                        : activeTab === 'databases'
                            ? databasesSaving
                            : false;

    // Conditions for showing database tabs
    const showDatabasesTab = settings?.schema_search_enabled || settings?.sql_select_enabled;

    return (
        <div id="settings-modal" className="settings-modal fixed inset-0 z-50 flex items-center justify-center">
            {/* Backdrop */}
            <div
                className="settings-backdrop absolute inset-0 bg-black/40 backdrop-blur-sm"
                onClick={closeSettings}
            />

            {/* Modal */}
            <div className="settings-surface relative w-full max-w-2xl max-h-[85vh] bg-white rounded-2xl shadow-2xl overflow-hidden flex flex-col m-4">
                {/* Header */}
                <div className="settings-header flex items-center justify-between px-6 py-4 border-b border-gray-100">
                    <h2 className="settings-title text-lg font-semibold text-gray-900">Settings</h2>
                    <div className="settings-header-actions flex items-center gap-2">
                        {activeTab === 'builtins' && (
                            <button
                                onClick={handleHeaderReset}
                                className="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium rounded-lg border border-gray-200 text-gray-700 hover:bg-gray-50"
                                title="Reset built-ins to defaults"
                            >
                                <RotateCcw size={16} />
                                Reset
                            </button>
                        )}
                        {activeTab && (
                            <button
                                onClick={handleHeaderSave}
                                disabled={!isCurrentTabDirty || isCurrentTabSaving}
                                className="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium rounded-lg bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                            >
                                {isCurrentTabSaving ? <Loader2 size={16} className="animate-spin" /> : <Save size={16} />}
                                Save
                            </button>
                        )}
                        <button
                            onClick={closeSettings}
                            className="p-1.5 hover:bg-gray-100 rounded-lg text-gray-500"
                        >
                            <X size={20} />
                        </button>
                    </div>
                </div>

                {/* Tabs */}
                <div className="settings-tablist flex items-center border-b border-gray-100 overflow-x-auto min-h-[56px] pb-2">
                    <button
                        onClick={() => setActiveTab('models')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors whitespace-nowrap ${activeTab === 'models'
                            ? 'border-blue-500 text-blue-600'
                            : 'border-transparent text-gray-500 hover:text-gray-700'
                            }`}
                    >
                        <Cpu size={16} />
                        Models
                    </button>
                    {showDatabasesTab && (
                        <button
                            onClick={() => setActiveTab('databases')}
                            className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors whitespace-nowrap ${activeTab === 'databases'
                                ? 'border-blue-500 text-blue-600'
                                : 'border-transparent text-gray-500 hover:text-gray-700'
                                }`}
                        >
                            <Server size={16} />
                            Databases
                        </button>
                    )}
                    <button
                        onClick={() => setActiveTab('builtins')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors whitespace-nowrap ${activeTab === 'builtins'
                            ? 'border-blue-500 text-blue-600'
                            : 'border-transparent text-gray-500 hover:text-gray-700'
                            }`}
                    >
                        <Code2 size={16} />
                        Built-ins
                    </button>
                    <button
                        onClick={() => setActiveTab('tools')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors whitespace-nowrap ${activeTab === 'tools'
                            ? 'border-blue-500 text-blue-600'
                            : 'border-transparent text-gray-500 hover:text-gray-700'
                            }`}
                    >
                        <Wrench size={16} />
                        Tools
                    </button>
                    <button
                        onClick={() => setActiveTab('system-prompt')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors whitespace-nowrap ${activeTab === 'system-prompt'
                            ? 'border-blue-500 text-blue-600'
                            : 'border-transparent text-gray-500 hover:text-gray-700'
                            }`}
                    >
                        <MessageSquare size={16} />
                        System Prompt
                    </button>
                    <button
                        onClick={() => setActiveTab('interfaces')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors whitespace-nowrap ${activeTab === 'interfaces'
                            ? 'border-blue-500 text-blue-600'
                            : 'border-transparent text-gray-500 hover:text-gray-700'
                            }`}
                    >
                        <Wrench size={16} />
                        Interfaces
                    </button>
                    <button
                        onClick={() => setActiveTab('schemas')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors whitespace-nowrap ${activeTab === 'schemas'
                            ? 'border-blue-500 text-blue-600'
                            : 'border-transparent text-gray-500 hover:text-gray-700'
                            }`}
                    >
                        <Database size={16} />
                        Schemas
                    </button>
                    <button
                        onClick={() => setActiveTab('files')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors whitespace-nowrap ${activeTab === 'files'
                            ? 'border-blue-500 text-blue-600'
                            : 'border-transparent text-gray-500 hover:text-gray-700'
                            }`}
                    >
                        <HardDrive size={16} />
                        Files
                    </button>
                </div>

                {/* Content */}
                <div className="settings-content flex-1 overflow-y-auto p-6">
                    {isLoading ? (
                        <div className="flex items-center justify-center py-12">
                            <div className="w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full animate-spin" />
                        </div>
                    ) : (
                        <>
                            {activeTab === 'models' && (
                                <ModelsTab />
                            )}
                            {activeTab === 'system-prompt' && (
                                <SystemPromptTab
                                    onDirtyChange={setSystemDirty}
                                    onRegisterSave={handleRegisterSystemSave}
                                    onSavingChange={handleSystemSavingChange}
                                />
                            )}
                            {activeTab === 'tools' && (
                                <ToolsTab
                                    onDirtyChange={setToolsDirty}
                                    onRegisterSave={handleRegisterToolsSave}
                                    onSavingChange={handleToolsSavingChange}
                                />
                            )}
                            {activeTab === 'interfaces' && (
                                <InterfacesTab
                                    onDirtyChange={setInterfacesDirty}
                                    onRegisterSave={handleRegisterInterfacesSave}
                                    onSavingChange={handleInterfacesSavingChange}
                                />
                            )}
                            {activeTab === 'builtins' && (
                                <BuiltinsTab
                                    onDirtyChange={setBuiltinsDirty}
                                    onRegisterSave={handleRegisterBuiltinsSave}
                                    onSavingChange={handleBuiltinsSavingChange}
                                    onRegisterReset={handleRegisterBuiltinsReset}
                                />
                            )}
                            {activeTab === 'databases' && (
                                <DatabasesTab
                                    onDirtyChange={setDatabasesDirty}
                                    onRegisterSave={handleRegisterDatabasesSave}
                                    onSavingChange={handleDatabasesSavingChange}
                                />
                            )}
                            {activeTab === 'schemas' && (
                                <SchemasTab />
                            )}
                            {activeTab === 'files' && (
                                <FilesTab />
                            )}
                        </>
                    )}
                </div>
            </div>
        </div>
    );
}

