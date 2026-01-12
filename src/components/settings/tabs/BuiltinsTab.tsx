import { useState, useEffect, useCallback } from 'react';
import { useSettingsStore } from '../../../store/settings-store';
import { FALLBACK_PYTHON_ALLOWED_IMPORTS } from '../../../lib/python-allowed-imports';

interface BuiltinsTabProps {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
    onRegisterReset?: (handler: () => void) => void;
}

export function BuiltinsTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
    onRegisterReset,
}: BuiltinsTabProps) {
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
        addAlwaysOnBuiltinTool,
        removeAlwaysOnBuiltinTool,
    } = useSettingsStore();
    const alwaysOnBuiltins = settings?.always_on_builtin_tools || [];
    const isBuiltinAlwaysOn = (name: string) => alwaysOnBuiltins.includes(name);

    const toggleBuiltinAlwaysOn = async (name: string) => {
        if (isBuiltinAlwaysOn(name)) {
            await removeAlwaysOnBuiltinTool(name);
        } else {
            await addAlwaysOnBuiltinTool(name);
        }
    };

    const allowedImports = (pythonAllowedImports && pythonAllowedImports.length > 0)
        ? pythonAllowedImports
        : FALLBACK_PYTHON_ALLOWED_IMPORTS;
    
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
                        <div className="flex items-center gap-2 pr-1">
                            <span className="text-[11px] font-medium text-gray-500 uppercase tracking-wider">Always On</span>
                            <button
                                onClick={() => toggleBuiltinAlwaysOn('python_execution')}
                                className={`relative w-9 h-5 rounded-full transition-colors ${isBuiltinAlwaysOn('python_execution') ? 'bg-blue-500' : 'bg-gray-300'
                                    }`}
                                title="Keep this tool enabled for every chat"
                            >
                                <div
                                    className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${isBuiltinAlwaysOn('python_execution') ? 'translate-x-4' : ''
                                        }`}
                                />
                            </button>
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
                        <div className="flex items-center gap-3">
                            <div className="flex items-center gap-2 pr-3 border-r border-gray-100">
                                <span className="text-[11px] font-medium text-gray-500 uppercase tracking-wider">Always On</span>
                                <button
                                    onClick={() => toggleBuiltinAlwaysOn('tool_search')}
                                    className={`relative w-9 h-5 rounded-full transition-colors ${isBuiltinAlwaysOn('tool_search') ? 'bg-blue-500' : 'bg-gray-300'
                                        }`}
                                    title="Keep this tool enabled for every chat"
                                >
                                    <div
                                        className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${isBuiltinAlwaysOn('tool_search') ? 'translate-x-4' : ''
                                            }`}
                                    />
                                </button>
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
                        <div className="flex items-center gap-3">
                            <div className="flex items-center gap-2 pr-3 border-r border-gray-100">
                                <span className="text-[11px] font-medium text-gray-500 uppercase tracking-wider">Always On</span>
                                <button
                                    onClick={() => toggleBuiltinAlwaysOn('schema_search')}
                                    className={`relative w-9 h-5 rounded-full transition-colors ${isBuiltinAlwaysOn('schema_search') ? 'bg-blue-500' : 'bg-gray-300'
                                        }`}
                                    title="Keep this tool enabled for every chat"
                                >
                                    <div
                                        className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${isBuiltinAlwaysOn('schema_search') ? 'translate-x-4' : ''
                                            }`}
                                    />
                                </button>
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
                        <div className="flex items-center gap-3">
                            <div className="flex items-center gap-2 pr-3 border-r border-gray-100">
                                <span className="text-[11px] font-medium text-gray-500 uppercase tracking-wider">Always On</span>
                                <button
                                    onClick={() => toggleBuiltinAlwaysOn('sql_select')}
                                    className={`relative w-9 h-5 rounded-full transition-colors ${isBuiltinAlwaysOn('sql_select') ? 'bg-blue-500' : 'bg-gray-300'
                                        }`}
                                    title="Keep this tool enabled for every chat"
                                >
                                    <div
                                        className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform ${isBuiltinAlwaysOn('sql_select') ? 'translate-x-4' : ''
                                            }`}
                                    />
                                </button>
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
