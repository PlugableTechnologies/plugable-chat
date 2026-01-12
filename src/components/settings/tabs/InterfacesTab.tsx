import { useState, useEffect, useCallback } from 'react';
import { useSettingsStore, DEFAULT_TOOL_CALL_FORMATS, type ToolCallFormatConfig, type ToolCallFormatName, type ChatFormatName } from '../../../store/settings-store';
import { useChatStore } from '../../../store/chat-store';

interface InterfacesTabProps {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}

export function InterfacesTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: InterfacesTabProps) {
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
