import { useState, useEffect } from 'react';
import { RotateCcw } from 'lucide-react';
import { useSettingsStore, DEFAULT_SYSTEM_PROMPT } from '../../../store/settings-store';
import { StateMachinePreview } from '../preview/StateMachinePreview';

interface SystemPromptTabProps {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}

export function SystemPromptTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: SystemPromptTabProps) {
    const { settings, updateSystemPrompt, error } = useSettingsStore();
    const [localPrompt, setLocalPrompt] = useState(settings?.system_prompt || '');
    const [hasChanges, setHasChanges] = useState(false);
    const [isSaving, setIsSaving] = useState(false);

    useEffect(() => {
        if (settings?.system_prompt) {
            setLocalPrompt(settings.system_prompt);
            setHasChanges(false);
        }
    }, [settings?.system_prompt]);

    const handleSave = async () => {
        setIsSaving(true);
        onSavingChange?.(true);
        try {
            await updateSystemPrompt(localPrompt);
            setHasChanges(false);
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
                    This is the base prompt. Tool descriptions and instructions are appended automatically during chat based on your configuration and attachments.
                </p>
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
