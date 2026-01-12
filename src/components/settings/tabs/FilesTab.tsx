import { Plus, Trash2, HardDrive } from 'lucide-react';
import { useSettingsStore } from '../../../store/settings-store';
import { invoke } from '../../../lib/api';

export function FilesTab() {
    const { settings, addAlwaysOnRagPath, removeAlwaysOnRagPath } = useSettingsStore();
    const alwaysOnPaths = settings?.always_on_rag_paths || [];

    const handleAddFiles = async () => {
        try {
            const paths = await invoke<string[]>('select_files');
            for (const path of paths) {
                if (!alwaysOnPaths.includes(path)) {
                    await addAlwaysOnRagPath(path);
                }
            }
        } catch (e: any) {
            console.error('[FilesTab] Failed to select files:', e);
        }
    };

    const handleAddFolder = async () => {
        try {
            const path = await invoke<string | null>('select_folder');
            if (path && !alwaysOnPaths.includes(path)) {
                await addAlwaysOnRagPath(path);
            }
        } catch (e: any) {
            console.error('[FilesTab] Failed to select folder:', e);
        }
    };

    return (
        <div className="space-y-6">
            <div>
                <h3 className="text-sm font-medium text-gray-700">Always-On Files</h3>
                <p className="text-xs text-gray-500 mt-1">
                    Files and folders marked as "Always On" will be automatically indexed and searched in every chat.
                    They appear as locked pills in the chat input area.
                </p>
            </div>

            {/* Add buttons */}
            <div className="flex gap-2">
                <button
                    onClick={handleAddFiles}
                    className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-600 text-white text-xs font-medium rounded-lg hover:bg-blue-700"
                >
                    <Plus size={14} />
                    Add Files
                </button>
                <button
                    onClick={handleAddFolder}
                    className="flex items-center gap-1.5 px-3 py-1.5 bg-blue-600 text-white text-xs font-medium rounded-lg hover:bg-blue-700"
                >
                    <Plus size={14} />
                    Add Folder
                </button>
            </div>

            {alwaysOnPaths.length === 0 ? (
                <div className="text-center py-8 text-gray-500 border border-dashed border-gray-200 rounded-xl">
                    <HardDrive size={32} className="mx-auto mb-2 opacity-30" />
                    <p className="text-sm">No always-on files configured</p>
                    <p className="text-xs mt-1">Add files or folders to have them automatically available in every chat</p>
                </div>
            ) : (
                <div className="space-y-2">
                    {alwaysOnPaths.map((path) => (
                        <div 
                            key={path}
                            className="flex items-center justify-between p-3 bg-emerald-50 border border-emerald-200 rounded-lg"
                        >
                            <div className="flex-1 min-w-0">
                                <div className="text-sm font-medium text-gray-900 truncate" title={path}>
                                    {path.split(/[/\\]/).pop() || path}
                                </div>
                                <div className="text-xs text-gray-500 truncate" title={path}>
                                    {path}
                                </div>
                            </div>
                            <button
                                onClick={() => removeAlwaysOnRagPath(path)}
                                className="ml-4 p-1.5 text-gray-400 hover:text-red-500 hover:bg-red-50 rounded transition-colors"
                                title="Remove from always-on"
                            >
                                <Trash2 size={14} />
                            </button>
                        </div>
                    ))}
                </div>
            )}

            {alwaysOnPaths.length > 0 && (
                <div className="pt-4 border-t border-gray-100">
                    <p className="text-xs text-gray-500">
                        {alwaysOnPaths.length} path{alwaysOnPaths.length !== 1 ? 's' : ''} set to always-on
                    </p>
                </div>
            )}
        </div>
    );
}
