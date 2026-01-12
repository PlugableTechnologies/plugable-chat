import { useState, useEffect, useCallback, useRef } from 'react';
import { Plus, Server } from 'lucide-react';
import { useSettingsStore, createNewServerConfig } from '../../../store/settings-store';
import { McpServerCard } from '../cards/McpServerCard';

interface ToolsTabProps {
    onDirtyChange?: (dirty: boolean) => void;
    onRegisterSave?: (handler: () => Promise<void>) => void;
    onSavingChange?: (saving: boolean) => void;
}

export function ToolsTab({
    onDirtyChange,
    onRegisterSave,
    onSavingChange,
}: ToolsTabProps) {
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
