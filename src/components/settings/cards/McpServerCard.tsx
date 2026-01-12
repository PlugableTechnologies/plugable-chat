import { useState, useEffect, useCallback, useRef } from 'react';
import { ChevronDown, ChevronUp, Play, CheckCircle, XCircle, Loader2, Trash2, RotateCcw, Save } from 'lucide-react';
import { useSettingsStore, type McpServerConfig, type McpTool } from '../../../store/settings-store';
import { invoke } from '../../../lib/api';
import { SettingsTagInput } from '../common/SettingsTagInput';
import { SettingsEnvVarInput } from '../common/SettingsEnvVarInput';
import { extractToolParameters, type McpServerTestResult } from '../types';

interface McpServerCardProps {
    config: McpServerConfig;
    onSave: (config: McpServerConfig) => Promise<void>;
    onRemove: () => void;
    initialTools?: McpTool[] | undefined;
    toolPrompts: Record<string, string>;
    onSaveToolPrompt: (serverId: string, toolName: string, prompt: string) => Promise<void>;
    onDirtyChange?: (id: string, dirty: boolean) => void;
    registerSaveHandler?: (id: string, handler: () => Promise<void>) => void;
}

// Single MCP Server configuration card
export function McpServerCard({
    config,
    onSave,
    onRemove,
    initialTools,
    toolPrompts,
    onSaveToolPrompt,
    onDirtyChange,
    registerSaveHandler
}: McpServerCardProps) {
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
    const [testResult, setTestResult] = useState<McpServerTestResult | null>(null);
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
                                <SettingsTagInput
                                    tags={localConfig.args}
                                    onChange={(args) => updateField('args', args)}
                                    placeholder="Press Enter to add arguments"
                                />
                            </div>

                            <div>
                                <label className="block text-xs font-medium text-gray-600 mb-1.5">Environment Variables</label>
                                <SettingsEnvVarInput
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
