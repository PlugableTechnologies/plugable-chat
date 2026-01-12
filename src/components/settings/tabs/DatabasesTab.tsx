import { useState, useEffect, useCallback } from 'react';
import { Plus, Trash2, RefreshCw, Loader2, AlertCircle, X } from 'lucide-react';
import { useSettingsStore, type DatabaseSourceConfig, type SupportedDatabaseKind, type DatabaseToolboxConfig } from '../../../store/settings-store';
import { invoke, listen } from '../../../lib/api';
import { SettingsTagInput } from '../common/SettingsTagInput';
import { SettingsEnvVarInput } from '../common/SettingsEnvVarInput';
import { SchemaRefreshStatusBar } from '../preview/SchemaRefreshStatusBar';
import type { SchemaRefreshProgress, SchemaRefreshStatus, SchemaRefreshError, SchemaRefreshResult } from '../types';

interface DatabasesTabProps {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}

export function DatabasesTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: DatabasesTabProps) {
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
    // Per-source errors from the last refresh attempt
    const [sourceErrors, setSourceErrors] = useState<Record<string, SchemaRefreshError>>({});

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

    // Simple dirty check
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
        // Clear any previous error for this source
        setSourceErrors(prev => {
            const next = { ...prev };
            delete next[sourceId];
            return next;
        });
        try {
            // Use per-source refresh command to only refresh the clicked source
            const result = await invoke<SchemaRefreshResult>('refresh_database_schema_for_source', {
                sourceId
            });
            
            // Check for errors in the result
            if (result.errors && result.errors.length > 0) {
                const error = result.errors[0]; // There should be at most one for single-source refresh
                setSourceErrors(prev => ({
                    ...prev,
                    [sourceId]: error
                }));
                setSaveError(`Refresh failed for '${error.source_name}': ${error.error}`);
            }
        } catch (err: any) {
            const errorMessage = err?.message || String(err);
            setSaveError(`Refresh failed: ${errorMessage}`);
            // Also set a source-specific error
            const source = toolboxConfig.sources.find(s => s.id === sourceId);
            setSourceErrors(prev => ({
                ...prev,
                [sourceId]: {
                    source_id: sourceId,
                    source_name: source?.name || sourceId,
                    error: errorMessage,
                    details: null
                }
            }));
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
                    <div key={source.id} className={`database-source-card border rounded-lg p-4 space-y-3 ${source.id === 'embedded-demo' ? 'border-green-200 bg-green-50/30' : 'border-gray-200 bg-white'}`}>
                        <div className="flex justify-between items-start">
                            <div className="flex items-center gap-2">
                                <span className="text-xs font-semibold bg-blue-100 text-blue-700 px-2 py-0.5 rounded uppercase">
                                    {source.kind}
                                </span>
                                {source.id === 'embedded-demo' && (
                                    <span className="text-xs font-semibold bg-green-100 text-green-700 px-2 py-0.5 rounded">
                                        Built-in Demo
                                    </span>
                                )}
                                {source.id === 'embedded-demo' ? (
                                    <span className="font-medium text-gray-900 px-1">{source.name}</span>
                                ) : (
                                    <input
                                        type="text"
                                        value={source.name}
                                        onChange={(e) => updateSource(idx, { name: e.target.value })}
                                        className="font-medium text-gray-900 border-b border-transparent hover:border-gray-300 focus:border-blue-500 focus:outline-none px-1"
                                    />
                                )}
                                {source.id !== 'embedded-demo' && (
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
                                )}
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
                                {source.id !== 'embedded-demo' && (
                                    <button onClick={() => removeSource(idx)} className="text-gray-400 hover:text-red-500">
                                        <Trash2 size={16} />
                                    </button>
                                )}
                            </div>
                        </div>

                        {/* Per-source error display */}
                        {sourceErrors[source.id] && (
                            <div className="flex items-start gap-2 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm">
                                <AlertCircle className="text-red-500 flex-shrink-0 mt-0.5" size={14} />
                                <div className="flex-1 min-w-0">
                                    <div className="font-medium text-red-800 text-xs">Schema refresh failed</div>
                                    <p className="text-xs text-red-700 break-words">{sourceErrors[source.id].error}</p>
                                    {sourceErrors[source.id].details && (
                                        <p className="text-[10px] text-red-600 mt-1 break-words">{sourceErrors[source.id].details}</p>
                                    )}
                                    {sourceErrors[source.id].error.includes('not connected') && (
                                        <p className="text-[10px] text-red-600 mt-1">
                                            Tip: Make sure the toolbox command path is correct and the binary exists.
                                            Save settings first, then try refreshing again.
                                        </p>
                                    )}
                                    {sourceErrors[source.id].error.includes('No command specified') && (
                                        <p className="text-[10px] text-red-600 mt-1">
                                            Tip: Set the path to your MCP toolbox binary (e.g., /opt/homebrew/bin/toolbox).
                                        </p>
                                    )}
                                </div>
                                <button
                                    onClick={() => setSourceErrors(prev => {
                                        const next = { ...prev };
                                        delete next[source.id];
                                        return next;
                                    })}
                                    className="text-red-400 hover:text-red-600 flex-shrink-0"
                                >
                                    <X size={14} />
                                </button>
                            </div>
                        )}

                        {/* Show description and simplified config for embedded demo sources */}
                        {source.id === 'embedded-demo' && (
                            <div className="space-y-3">
                                <div className="text-sm text-gray-600 bg-green-50 rounded-lg p-3 border border-green-100">
                                    <p className="font-medium text-green-800 mb-1">Built-in Demo Database</p>
                                    <p className="text-xs text-green-700">
                                        Chicago Crimes dataset (2025) with ~23,000 records. Uses the Google MCP Database Toolbox for SQLite access.
                                        Set the path to your toolbox binary below, enable this source, and click "Refresh" to cache the schema.
                                    </p>
                                </div>
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1.5">
                                        Toolbox Binary Path <span className="text-red-500">*</span>
                                    </label>
                                    <input
                                        type="text"
                                        value={source.command || ''}
                                        onChange={(e) => updateSource(idx, { command: e.target.value })}
                                        placeholder="/opt/homebrew/bin/toolbox"
                                        className={`w-full text-sm rounded-md shadow-sm focus:ring-1 ${
                                            source.enabled && !(source.command?.trim())
                                                ? 'border-red-300 focus:border-red-500 focus:ring-red-500'
                                                : 'border-gray-300 focus:border-blue-500 focus:ring-blue-500'
                                        }`}
                                    />
                                    <p className="text-[11px] text-gray-500 mt-1">
                                        Path to the Google MCP Database Toolbox binary. Install from{' '}
                                        <a href="https://github.com/googleapis/genai-toolbox" target="_blank" rel="noopener noreferrer" className="text-blue-600 hover:underline">
                                            googleapis/genai-toolbox
                                        </a>
                                    </p>
                                </div>
                            </div>
                        )}

                        {/* Hide transport/command config for non-embedded sources */}
                        {source.id !== 'embedded-demo' && (
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
                        )}

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

                        {source.transport.type === 'stdio' && source.id !== 'embedded-demo' && (
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
                                    <SettingsTagInput
                                        tags={source.args}
                                        onChange={(args) => updateSource(idx, { args })}
                                        placeholder="--stdio --prebuilt bigquery"
                                    />
                                </div>
                                <div>
                                    <label className="block text-xs font-medium text-gray-700 mb-1.5">Environment Variables</label>
                                    <SettingsEnvVarInput
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
