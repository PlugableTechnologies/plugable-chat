import { useSettingsStore, createNewServerConfig, DEFAULT_SYSTEM_PROMPT, DEFAULT_TOOL_CALL_FORMATS, type McpServerConfig, type McpTool, type ToolCallFormatConfig, type ToolCallFormatName } from '../store/settings-store';
import { useState, useEffect, useCallback, useRef } from 'react';
import { X, Plus, Trash2, Save, Server, MessageSquare, ChevronDown, ChevronUp, Play, CheckCircle, XCircle, Loader2, Code2, Wrench, RotateCcw } from 'lucide-react';
import { invoke } from '../lib/api';
import { FALLBACK_PYTHON_ALLOWED_IMPORTS } from '../lib/python-allowed-imports';

// Python reserved keywords (cannot be used as identifiers)
const PYTHON_KEYWORDS = new Set([
    'False', 'None', 'True', 'and', 'as', 'assert', 'async', 'await',
    'break', 'class', 'continue', 'def', 'del', 'elif', 'else', 'except',
    'finally', 'for', 'from', 'global', 'if', 'import', 'in', 'is',
    'lambda', 'nonlocal', 'not', 'or', 'pass', 'raise', 'return', 'try',
    'while', 'with', 'yield'
]);

/**
 * Validate that a string is a valid Python identifier
 * - Must start with a letter or underscore
 * - Can contain letters, numbers, and underscores
 * - Cannot be a Python keyword
 */
function isValidPythonIdentifier(name: string): boolean {
    if (!name || name.length === 0) return false;
    
    // Check first character (must be letter or underscore)
    const firstChar = name[0];
    if (!/^[a-zA-Z_]$/.test(firstChar)) return false;
    
    // Check all characters (must be alphanumeric or underscore)
    if (!/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(name)) return false;
    
    // Check if it's a reserved keyword
    if (PYTHON_KEYWORDS.has(name)) return false;
    
    return true;
}

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
    
    // Check if this is the built-in test server
    const isTestServer = config.id === 'mcp-test-server';

    return (
        <div className={`border rounded-xl bg-white overflow-hidden ${isDirty ? 'border-amber-300' : 'border-gray-200'}`}>
            {/* Header */}
            <div 
                className="flex items-center gap-3 px-4 py-3 cursor-pointer hover:bg-gray-50"
                onClick={() => setExpanded(!expanded)}
            >
                {/* Status indicator */}
                <div className={`w-2.5 h-2.5 rounded-full ${
                    status?.connected ? 'bg-green-500' : 
                    status?.error ? 'bg-red-500' : 'bg-gray-300'
                }`} />
                
                {/* Enable toggle - auto-saves on change */}
                <button
                    onClick={(e) => { e.stopPropagation(); handleToggleEnabled(); }}
                    className={`relative w-10 h-5 rounded-full transition-colors ${
                        localConfig.enabled ? 'bg-blue-500' : 'bg-gray-300'
                    }`}
                >
                    <div className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${
                        localConfig.enabled ? 'translate-x-5' : ''
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
                                className={`px-3 py-1.5 text-xs rounded-lg border ${
                                    localConfig.transport.type === 'stdio'
                                        ? 'bg-blue-50 border-blue-300 text-blue-700'
                                        : 'bg-white border-gray-200 text-gray-600 hover:bg-gray-50'
                                }`}
                            >
                                Stdio (subprocess)
                            </button>
                            <button
                                onClick={() => updateTransport('sse', (localConfig.transport as any).url || '')}
                                className={`px-3 py-1.5 text-xs rounded-lg border ${
                                    localConfig.transport.type === 'sse'
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
                            className={`relative w-10 h-5 rounded-full transition-colors ${
                                localConfig.auto_approve_tools ? 'bg-blue-500' : 'bg-gray-300'
                            }`}
                        >
                            <div className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${
                                localConfig.auto_approve_tools ? 'translate-x-5' : ''
                            }`} />
                        </button>
                    </div>
                    
                    {/* Python Module Name */}
                    <div>
                        <label className="block text-xs font-medium text-gray-600 mb-1.5">
                            Python Module Name <span className="text-gray-400">(optional)</span>
                        </label>
                        <div className="relative">
                            <input
                                type="text"
                                value={localConfig.python_name || ''}
                                onChange={(e) => updateField('python_name', e.target.value || undefined)}
                                placeholder={`e.g., ${localConfig.id.replace(/-/g, '_').toLowerCase()}`}
                                className={`w-full px-3 py-2 text-sm font-mono border rounded-lg focus:outline-none focus:ring-1 ${
                                    localConfig.python_name && !isValidPythonIdentifier(localConfig.python_name)
                                        ? 'border-red-300 focus:border-red-400 focus:ring-red-400'
                                        : 'border-gray-200 focus:border-blue-400 focus:ring-blue-400'
                                }`}
                            />
                        </div>
                        {localConfig.python_name && !isValidPythonIdentifier(localConfig.python_name) && (
                            <p className="mt-1 text-xs text-red-500">
                                Must be a valid Python identifier (lowercase letters, numbers, underscores; cannot start with a number)
                            </p>
                        )}
                        <p className="mt-1 text-xs text-gray-500">
                            Name used when importing tools in Python code: <code className="bg-gray-100 px-1 rounded">from {localConfig.python_name || localConfig.id.replace(/-/g, '_').toLowerCase()} import tool_name</code>
                        </p>
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
    const { settings, updateToolCallFormats } = useSettingsStore();
    const formatConfig = settings?.tool_call_formats || DEFAULT_TOOL_CALL_FORMATS;
    const formatOptions: { id: ToolCallFormatName; label: string; description: string }[] = [
        { id: 'code_mode', label: 'Code Mode (Python)', description: 'Model returns a single Python program executed in the sandbox (primary default).' },
        { id: 'hermes', label: 'Hermes (tag-delimited)', description: '<tool_call>{"name": "...", "arguments": {...}}</tool_call>' },
        { id: 'mistral', label: 'Mistral (bracket)', description: '[TOOL_CALLS] [{"name": "...", "arguments": {...}}]' },
        { id: 'pythonic', label: 'Pythonic call', description: 'tool_name(arg1="value", arg2=123)' },
        { id: 'pure_json', label: 'Pure JSON', description: '{"tool": "name", "args": {...}} or array' },
    ];

    const [localFormats, setLocalFormats] = useState<ToolCallFormatConfig>(formatConfig);
    const [baselineFormats, setBaselineFormats] = useState<ToolCallFormatConfig>(formatConfig);
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

    const hasChanges = JSON.stringify(localFormats) !== JSON.stringify(baselineFormats);

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
            await updateToolCallFormats(localFormats);
            setBaselineFormats(localFormats);
        } finally {
            setIsSaving(false);
            onSavingChange?.(false);
        }
    }, [localFormats, onSavingChange, settings, updateToolCallFormats]);

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
        </div>
    );
}

// Built-ins Tab - python_execution and tool_search
function BuiltinsTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}) {
    const { settings, updateCodeExecutionEnabled, updateToolSystemPrompt, pythonAllowedImports } = useSettingsStore();
    const codeExecutionEnabled = settings?.python_execution_enabled ?? false;
    const allowedImports = (pythonAllowedImports && pythonAllowedImports.length > 0)
        ? pythonAllowedImports
        : FALLBACK_PYTHON_ALLOWED_IMPORTS;
    const defaultPythonPrompt = [
        "Use python_execution for calling tools, calculations, and data transforms.",
        "Tools found with tool_search will be available in the global scope, with parameters with the same name and in the same order as returned in the tool description.",
        "Do not use any imports that are not explicitly allowed.",
        `Here are the allowed imports: ${allowedImports.join(', ')}.`
    ].join(' ');
    const defaultToolSearchPrompt = "Call tool_search to discover MCP tools related to your search string. If the returned tools appear to be relevant to your goal, use them";
    const initialPythonPrompt = settings?.tool_system_prompts?.['builtin::python_execution'] || defaultPythonPrompt;
    const initialToolSearchPrompt = settings?.tool_system_prompts?.['builtin::tool_search'] || defaultToolSearchPrompt;

    const [localCodeExecutionEnabled, setLocalCodeExecutionEnabled] = useState(codeExecutionEnabled);
    const [pythonPromptDraft, setPythonPromptDraft] = useState(initialPythonPrompt);
    const [toolSearchPromptDraft, setToolSearchPromptDraft] = useState(initialToolSearchPrompt);
    const [baselineBuiltins, setBaselineBuiltins] = useState({
        codeExecutionEnabled,
        pythonPrompt: initialPythonPrompt,
        toolSearchPrompt: initialToolSearchPrompt,
    });
    const [isSaving, setIsSaving] = useState(false);

    useEffect(() => {
        const nextBaseline = {
            codeExecutionEnabled: codeExecutionEnabled,
            pythonPrompt: settings?.tool_system_prompts?.['builtin::python_execution'] || defaultPythonPrompt,
            toolSearchPrompt: settings?.tool_system_prompts?.['builtin::tool_search'] || defaultToolSearchPrompt,
        };

        const hasPending =
            localCodeExecutionEnabled !== baselineBuiltins.codeExecutionEnabled ||
            pythonPromptDraft !== baselineBuiltins.pythonPrompt ||
            toolSearchPromptDraft !== baselineBuiltins.toolSearchPrompt;

        if (!hasPending) {
            setLocalCodeExecutionEnabled(nextBaseline.codeExecutionEnabled);
            setPythonPromptDraft(nextBaseline.pythonPrompt);
            setToolSearchPromptDraft(nextBaseline.toolSearchPrompt);
            setBaselineBuiltins(nextBaseline);
        } else {
            setBaselineBuiltins(nextBaseline);
        }
    }, [
        codeExecutionEnabled,
        defaultPythonPrompt,
        defaultToolSearchPrompt,
        localCodeExecutionEnabled,
        pythonPromptDraft,
        settings?.tool_system_prompts,
        toolSearchPromptDraft,
        baselineBuiltins.codeExecutionEnabled,
        baselineBuiltins.pythonPrompt,
        baselineBuiltins.toolSearchPrompt,
    ]);

    const hasChanges =
        localCodeExecutionEnabled !== baselineBuiltins.codeExecutionEnabled ||
        pythonPromptDraft !== baselineBuiltins.pythonPrompt ||
        toolSearchPromptDraft !== baselineBuiltins.toolSearchPrompt;

    useEffect(() => {
        onDirtyChange?.(hasChanges);
    }, [hasChanges, onDirtyChange]);

    useEffect(() => {
        onSavingChange?.(isSaving);
    }, [isSaving, onSavingChange]);

    const handleToggleCodeExecution = () => {
        setLocalCodeExecutionEnabled((prev) => !prev);
    };

    const handleResetPythonPrompt = () => {
        setPythonPromptDraft(defaultPythonPrompt);
    };

    const handleResetToolSearchPrompt = () => {
        setToolSearchPromptDraft(defaultToolSearchPrompt);
    };

    const handleSave = useCallback(async () => {
        if (!settings) return;
        setIsSaving(true);
        onSavingChange?.(true);

        const saves: Promise<unknown>[] = [];
        const targetPythonPrompt = pythonPromptDraft?.trim() ? pythonPromptDraft : defaultPythonPrompt;
        const targetToolSearchPrompt = toolSearchPromptDraft?.trim() ? toolSearchPromptDraft : defaultToolSearchPrompt;

        if (localCodeExecutionEnabled !== settings.python_execution_enabled) {
            saves.push(updateCodeExecutionEnabled(localCodeExecutionEnabled));
        }

        if (targetPythonPrompt !== (settings.tool_system_prompts?.['builtin::python_execution'] || defaultPythonPrompt)) {
            saves.push(updateToolSystemPrompt('builtin', 'python_execution', targetPythonPrompt));
        }

        if (targetToolSearchPrompt !== (settings.tool_system_prompts?.['builtin::tool_search'] || defaultToolSearchPrompt)) {
            saves.push(updateToolSystemPrompt('builtin', 'tool_search', targetToolSearchPrompt));
        }

        try {
            await Promise.all(saves);
            setBaselineBuiltins({
                codeExecutionEnabled: localCodeExecutionEnabled,
                pythonPrompt: targetPythonPrompt,
                toolSearchPrompt: targetToolSearchPrompt,
            });
        } finally {
            setIsSaving(false);
            onSavingChange?.(false);
        }
    }, [
        defaultPythonPrompt,
        defaultToolSearchPrompt,
        localCodeExecutionEnabled,
        onSavingChange,
        pythonPromptDraft,
        settings,
        toolSearchPromptDraft,
        updateCodeExecutionEnabled,
        updateToolSystemPrompt,
    ]);

    useEffect(() => {
        onRegisterSave?.(handleSave);
    }, [handleSave, onRegisterSave]);

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
                            <div className="w-8 h-8 rounded-lg bg-amber-100 flex items-center justify-center">
                                <Code2 size={16} className="text-amber-600" />
                            </div>
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
                        <button
                            onClick={handleToggleCodeExecution}
                            className={`relative w-10 h-5 rounded-full transition-colors ${
                                localCodeExecutionEnabled ? 'bg-blue-500' : 'bg-gray-300'
                            }`}
                        >
                            <div
                                className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${
                                    localCodeExecutionEnabled ? 'translate-x-5' : ''
                                }`}
                            />
                        </button>
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
                            <span className="text-[11px] bg-blue-50 text-blue-700 px-2 py-0.5 rounded-full border border-blue-100">
                                builtin
                            </span>
                        </div>
                    </div>
                    <textarea
                        value={pythonPromptDraft}
                        onChange={(e) => setPythonPromptDraft(e.target.value)}
                        rows={3}
                        className="w-full px-3 py-2 text-sm font-mono border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400 bg-gray-50"
                        placeholder={defaultPythonPrompt}
                    />
                    <p className="text-[11px] text-gray-500">Appended to the system prompt when Python execution is enabled.</p>
                </div>

                {/* tool_search prompt card */}
                <div className="border border-gray-200 rounded-xl bg-white p-4 space-y-2 w-full">
                    <div className="flex items-start justify-between">
                        <div>
                            <div className="text-sm font-semibold text-gray-900">tool_search</div>
                            <p className="text-xs text-gray-500">Built-in discovery helper</p>
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
                    <p className="text-[11px] text-gray-500">Added when MCP tools are available.</p>
                </div>
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
    const { settings, addMcpServer, updateMcpServer, removeMcpServer, updateToolSystemPrompt, error, serverStatuses } = useSettingsStore();
    const servers = settings?.mcp_servers || [];

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
        </div>
    );
}

// Main Settings Modal
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

    const handleBuiltinsSavingChange = useCallback((saving: boolean) => {
        setBuiltinsSaving(saving);
    }, []);

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
            : false;

    return (
        <div className="fixed inset-0 z-50 flex items-center justify-center">
            {/* Backdrop */}
            <div 
                className="absolute inset-0 bg-black/40 backdrop-blur-sm"
                onClick={closeSettings}
            />
            
            {/* Modal */}
            <div className="relative w-full max-w-2xl max-h-[85vh] bg-white rounded-2xl shadow-2xl overflow-hidden flex flex-col m-4">
                {/* Header */}
                <div className="flex items-center justify-between px-6 py-4 border-b border-gray-100">
                    <h2 className="text-lg font-semibold text-gray-900">Settings</h2>
                    <div className="flex items-center gap-2">
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
                <div className="flex border-b border-gray-100">
                    <button
                        onClick={() => setActiveTab('system-prompt')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors ${
                            activeTab === 'system-prompt'
                                ? 'border-blue-500 text-blue-600'
                                : 'border-transparent text-gray-500 hover:text-gray-700'
                        }`}
                    >
                        <MessageSquare size={16} />
                        System Prompt
                    </button>
                    <button
                        onClick={() => setActiveTab('tools')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors ${
                            activeTab === 'tools'
                                ? 'border-blue-500 text-blue-600'
                                : 'border-transparent text-gray-500 hover:text-gray-700'
                        }`}
                    >
                        <Wrench size={16} />
                        Tools
                    </button>
                    <button
                        onClick={() => setActiveTab('interfaces')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors ${
                            activeTab === 'interfaces'
                                ? 'border-blue-500 text-blue-600'
                                : 'border-transparent text-gray-500 hover:text-gray-700'
                        }`}
                    >
                        <Wrench size={16} />
                        Interfaces
                    </button>
                    <button
                        onClick={() => setActiveTab('builtins')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors ${
                            activeTab === 'builtins'
                                ? 'border-blue-500 text-blue-600'
                                : 'border-transparent text-gray-500 hover:text-gray-700'
                        }`}
                    >
                        <Code2 size={16} />
                        Built-ins
                    </button>
                </div>
                
                {/* Content */}
                <div className="flex-1 overflow-y-auto p-6">
                    {isLoading ? (
                        <div className="flex items-center justify-center py-12">
                            <div className="w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full animate-spin" />
                        </div>
                    ) : (
                        <>
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
                                />
                            )}
                        </>
                    )}
                </div>
            </div>
        </div>
    );
}

