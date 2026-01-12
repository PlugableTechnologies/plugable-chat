import { useState, useCallback, useRef } from 'react';
import { X, Save, Loader2, RotateCcw, Cpu, Server, Code2, Wrench, MessageSquare, Database, HardDrive } from 'lucide-react';
import { useSettingsStore } from '../../store/settings-store';
import { ModelsTab } from './tabs/ModelsTab';
import { SystemPromptTab } from './tabs/SystemPromptTab';
import { InterfacesTab } from './tabs/InterfacesTab';
import { BuiltinsTab } from './tabs/BuiltinsTab';
import { ToolsTab } from './tabs/ToolsTab';
import { SchemasTab } from './tabs/SchemasTab';
import { FilesTab } from './tabs/FilesTab';
import { DatabasesTab } from './tabs/DatabasesTab';

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
