import { useSettingsStore, createNewServerConfig, type McpServerConfig } from '../store/settings-store';
import { useState, useEffect, useCallback, useRef } from 'react';
import { X, Plus, Trash2, Save, Server, MessageSquare, ChevronDown, ChevronUp, PlugZap } from 'lucide-react';

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
    
    const handleSave = useCallback(() => {
        onSave(localConfig);
        setIsDirty(false);
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
                
                {/* Enable toggle */}
                <button
                    onClick={(e) => { e.stopPropagation(); updateField('enabled', !localConfig.enabled); }}
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
                    
                    {/* Status message */}
                    {status?.error && (
                        <div className="text-xs text-red-600 bg-red-50 px-3 py-2 rounded-lg">
                            {status.error}
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
                        <button
                            onClick={handleSave}
                            disabled={!isDirty}
                            className="flex items-center gap-1.5 px-3 py-1.5 text-xs bg-blue-600 text-white rounded-lg hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            <Save size={14} />
                            Save
                        </button>
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
    
    useEffect(() => {
        if (settings?.system_prompt) {
            setLocalPrompt(settings.system_prompt);
            setHasChanges(false);
        }
    }, [settings?.system_prompt]);
    
    const handleSave = async () => {
        await updateSystemPrompt(localPrompt);
        setHasChanges(false);
    };
    
    const handleChange = (value: string) => {
        setLocalPrompt(value);
        setHasChanges(value !== settings?.system_prompt);
    };
    
    return (
        <div className="space-y-4">
            <div>
                <div className="flex items-center justify-between mb-2">
                    <label className="text-sm font-medium text-gray-700">System Prompt</label>
                    {hasChanges && (
                        <span className="text-xs text-amber-600">Unsaved changes</span>
                    )}
                </div>
                <textarea
                    value={localPrompt}
                    onChange={(e) => handleChange(e.target.value)}
                    rows={12}
                    className="w-full px-4 py-3 text-sm font-mono border border-gray-200 rounded-xl focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400 resize-none bg-gray-50"
                    placeholder="Enter your system prompt..."
                />
                <p className="mt-2 text-xs text-gray-500">
                    This prompt is sent at the beginning of every conversation. MCP tool descriptions will be appended automatically.
                </p>
            </div>
            
            {error && (
                <div className="text-sm text-red-600 bg-red-50 px-3 py-2 rounded-lg">
                    {error}
                </div>
            )}
            
            <div className="flex justify-end">
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

// MCP Servers Tab
function McpServersTab() {
    const { settings, addMcpServer, updateMcpServer, removeMcpServer, error } = useSettingsStore();
    const servers = settings?.mcp_servers || [];
    
    const handleAddServer = () => {
        const newConfig = createNewServerConfig();
        addMcpServer(newConfig);
    };
    
    return (
        <div className="space-y-4">
            <div className="flex items-center justify-between">
                <div>
                    <h3 className="text-sm font-medium text-gray-700">MCP Servers</h3>
                    <p className="text-xs text-gray-500">Configure Model Context Protocol servers for tool access</p>
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
                    <div className="text-center py-12 text-gray-500">
                        <Server size={40} className="mx-auto mb-3 opacity-30" />
                        <p className="text-sm">No MCP servers configured</p>
                        <p className="text-xs mt-1">Add a server to enable tool capabilities</p>
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
                        onClick={() => setActiveTab('mcp-servers')}
                        className={`flex items-center gap-2 px-6 py-3 text-sm font-medium border-b-2 transition-colors ${
                            activeTab === 'mcp-servers'
                                ? 'border-blue-500 text-blue-600'
                                : 'border-transparent text-gray-500 hover:text-gray-700'
                        }`}
                    >
                        <PlugZap size={16} />
                        MCP Servers
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
                            {activeTab === 'mcp-servers' && <McpServersTab />}
                        </>
                    )}
                </div>
            </div>
        </div>
    );
}

