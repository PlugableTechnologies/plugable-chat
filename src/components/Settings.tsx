import { useSettingsStore, createNewServerConfig, DEFAULT_SYSTEM_PROMPT, type McpServerConfig, type McpTool } from '../store/settings-store';
import { useState, useEffect, useCallback, useRef } from 'react';
import { X, Plus, Trash2, Save, Server, MessageSquare, ChevronDown, ChevronUp, Play, CheckCircle, XCircle, Loader2, Code2, Wrench, RotateCcw } from 'lucide-react';
import { invoke } from '../lib/api';

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

// Single MCP Server configuration card
function McpServerCard({ 
    config, 
    onSave, 
    onRemove 
}: { 
    config: McpServerConfig;
    onSave: (config: McpServerConfig) => void;
    onRemove: () => void;
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
        onSave(localConfig);
        setIsDirty(false);
        
        // Only test the connection if the server is enabled
        if (localConfig.enabled) {
            setIsTesting(true);
            setTestResult(null);
            try {
                const tools = await invoke<McpTool[]>('test_mcp_server_config', { config: localConfig });
                setTestResult({ success: true, tools });
            } catch (e: any) {
                setTestResult({ success: false, error: e.message || String(e) });
            } finally {
                setIsTesting(false);
            }
        } else {
            // Clear any previous test result when saving a disabled server
            setTestResult(null);
        }
    }, [localConfig, onSave]);
    
    // Manual test without saving
    const handleTest = useCallback(async () => {
        setIsTesting(true);
        setTestResult(null);
        try {
            const tools = await invoke<McpTool[]>('test_mcp_server_config', { config: localConfig });
            setTestResult({ success: true, tools });
        } catch (e: any) {
            setTestResult({ success: false, error: e.message || String(e) });
        } finally {
            setIsTesting(false);
        }
    }, [localConfig]);
    
    // Toggle enabled state and auto-save immediately
    const handleToggleEnabled = useCallback(async () => {
        const newEnabled = !localConfig.enabled;
        const newConfig = { ...localConfig, enabled: newEnabled };
        
        // Update local state immediately
        setLocalConfig(newConfig);
        // Don't mark as dirty since we're saving immediately
        
        // Save to backend
        onSave(newConfig);
        
        // Test connection if enabling
        if (newEnabled) {
            setIsTesting(true);
            setTestResult(null);
            try {
                const tools = await invoke<McpTool[]>('test_mcp_server_config', { config: newConfig });
                setTestResult({ success: true, tools });
            } catch (e: any) {
                setTestResult({ success: false, error: e.message || String(e) });
            } finally {
                setIsTesting(false);
            }
        } else {
            setTestResult(null);
        }
    }, [localConfig, onSave]);
    
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
                    <input
                        type="text"
                        value={localConfig.name}
                        onChange={(e) => updateField('name', e.target.value)}
                        onClick={(e) => e.stopPropagation()}
                        className="flex-1 font-medium text-gray-900 bg-transparent focus:outline-none focus:bg-gray-50 rounded px-1"
                    />
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
function SystemPromptTab() {
    const { settings, updateSystemPrompt, error } = useSettingsStore();
    const [localPrompt, setLocalPrompt] = useState(settings?.system_prompt || '');
    const [hasChanges, setHasChanges] = useState(false);
    const [preview, setPreview] = useState<string | null>(null);
    const [showPreview, setShowPreview] = useState(false);
    const [loadingPreview, setLoadingPreview] = useState(false);
    
    useEffect(() => {
        if (settings?.system_prompt) {
            setLocalPrompt(settings.system_prompt);
            setHasChanges(false);
        }
    }, [settings?.system_prompt]);
    
    // Fetch preview when showing it
    useEffect(() => {
        if (showPreview) {
            setLoadingPreview(true);
            invoke<string>('get_system_prompt_preview')
                .then(setPreview)
                .catch(e => {
                    console.error('Failed to get preview:', e);
                    setPreview('Failed to load preview');
                })
                .finally(() => setLoadingPreview(false));
        }
    }, [showPreview, settings?.mcp_servers]);
    
    const handleSave = async () => {
        await updateSystemPrompt(localPrompt);
        setHasChanges(false);
        // Refresh preview after save
        if (showPreview) {
            setLoadingPreview(true);
            invoke<string>('get_system_prompt_preview')
                .then(setPreview)
                .catch(() => setPreview('Failed to load preview'))
                .finally(() => setLoadingPreview(false));
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
                <button
                    onClick={handleSave}
                    disabled={!hasChanges}
                    className="flex items-center gap-2 px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                    <Save size={16} />
                    Save Changes
                </button>
            </div>
        </div>
    );
}

// Tools Tab - combines built-in tools and MCP servers
function ToolsTab() {
    const { settings, updateCodeExecutionEnabled, addMcpServer, updateMcpServer, removeMcpServer, error } = useSettingsStore();
    const servers = settings?.mcp_servers || [];
    const codeExecutionEnabled = settings?.python_execution_enabled ?? false;
    
    const handleAddServer = () => {
        const newConfig = createNewServerConfig();
        addMcpServer(newConfig);
    };
    
    const handleToggleCodeExecution = () => {
        updateCodeExecutionEnabled(!codeExecutionEnabled);
    };
    
    return (
        <div className="space-y-6">
            {/* Built-in Tools Section */}
            <div className="space-y-3">
                <div>
                    <h3 className="text-sm font-medium text-gray-700">Built-in Tools</h3>
                    <p className="text-xs text-gray-500">Core tools that run locally within the app</p>
                </div>
                
                {/* Code Execution Tool Card */}
                <div className="border border-gray-200 rounded-xl bg-white overflow-hidden">
                    <div className="flex items-center gap-3 px-4 py-3">
                        {/* Icon */}
                        <div className="w-8 h-8 rounded-lg bg-amber-100 flex items-center justify-center">
                            <Code2 size={16} className="text-amber-600" />
                        </div>
                        
                        {/* Info */}
                        <div className="flex-1">
                            <div className="flex items-center gap-2">
                                <span className="font-medium text-gray-900">Code Execution</span>
                                <span className="text-xs bg-amber-100 text-amber-700 px-2 py-0.5 rounded-full">Python Sandbox</span>
                            </div>
                            <p className="text-xs text-gray-500 mt-0.5">
                                Run Python code for calculations, data processing, and transformations
                            </p>
                        </div>
                        
                        {/* Toggle */}
                        <button
                            onClick={handleToggleCodeExecution}
                            className={`relative w-10 h-5 rounded-full transition-colors ${
                                codeExecutionEnabled ? 'bg-blue-500' : 'bg-gray-300'
                            }`}
                        >
                            <div className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${
                                codeExecutionEnabled ? 'translate-x-5' : ''
                            }`} />
                        </button>
                    </div>
                </div>
            </div>
            
            {/* Divider */}
            <div className="border-t border-gray-200" />
            
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
    
    if (!isSettingsOpen) return null;
    
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
                    <button
                        onClick={closeSettings}
                        className="p-1.5 hover:bg-gray-100 rounded-lg text-gray-500"
                    >
                        <X size={20} />
                    </button>
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
                </div>
                
                {/* Content */}
                <div className="flex-1 overflow-y-auto p-6">
                    {isLoading ? (
                        <div className="flex items-center justify-center py-12">
                            <div className="w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full animate-spin" />
                        </div>
                    ) : (
                        <>
                            {activeTab === 'system-prompt' && <SystemPromptTab />}
                            {activeTab === 'tools' && <ToolsTab />}
                        </>
                    )}
                </div>
            </div>
        </div>
    );
}

