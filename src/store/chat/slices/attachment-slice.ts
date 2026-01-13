import type { StateCreator } from 'zustand';
import type { AttachedTable, AttachedTool, AttachedTabularFile } from '../types';

export interface AttachmentSlice {
    // Per-chat attached database tables
    attachedDatabaseTables: AttachedTable[];
    addAttachedTable: (table: AttachedTable) => void;
    removeAttachedTable: (tableFqName: string) => void;
    clearAttachedTables: () => void;
    
    // Per-chat attached tools (built-in + MCP)
    attachedTools: AttachedTool[];
    addAttachedTool: (tool: AttachedTool) => void;
    removeAttachedTool: (toolKey: string) => void;
    clearAttachedTools: () => void;
    
    // Per-chat attached tabular files (CSV, TSV, XLS, XLSX)
    attachedTabularFiles: AttachedTabularFile[];
    addTabularFile: (file: AttachedTabularFile) => void;
    removeTabularFile: (filePath: string) => void;
    clearTabularFiles: () => void;
    
    // Always-on configuration (synced from settings store)
    // These are displayed as locked pills and always sent with chat requests
    alwaysOnTools: AttachedTool[];
    alwaysOnTables: AttachedTable[];
    alwaysOnRagPaths: string[];
    syncAlwaysOnFromSettings: () => void;
}

export const createAttachmentSlice: StateCreator<
    AttachmentSlice,
    [],
    [],
    AttachmentSlice
> = (set) => ({
    // Per-chat attached database tables
    attachedDatabaseTables: [],
    addAttachedTable: (table) => set((s) => ({
        attachedDatabaseTables: [...s.attachedDatabaseTables.filter(t => t.tableFqName !== table.tableFqName), table]
    })),
    removeAttachedTable: (tableFqName) => set((s) => ({
        attachedDatabaseTables: s.attachedDatabaseTables.filter(t => t.tableFqName !== tableFqName)
    })),
    clearAttachedTables: () => set({ attachedDatabaseTables: [] }),
    
    // Per-chat attached tools
    attachedTools: [],
    addAttachedTool: (tool) => set((s) => ({
        attachedTools: [...s.attachedTools.filter(t => t.key !== tool.key), tool]
    })),
    removeAttachedTool: (toolKey) => set((s) => ({
        attachedTools: s.attachedTools.filter(t => t.key !== toolKey)
    })),
    clearAttachedTools: () => set({ attachedTools: [] }),
    
    // Per-chat attached tabular files
    attachedTabularFiles: [],
    addTabularFile: (file) => set((s) => {
        // Avoid duplicates and assign variable index
        const existing = s.attachedTabularFiles.filter(f => f.filePath !== file.filePath);
        const newFile = { ...file, variableIndex: existing.length + 1 };
        return { attachedTabularFiles: [...existing, newFile] };
    }),
    removeTabularFile: (filePath) => set((s) => {
        // Remove file and reassign variable indices
        const remaining = s.attachedTabularFiles
            .filter(f => f.filePath !== filePath)
            .map((f, idx) => ({ ...f, variableIndex: idx + 1 }));
        return { attachedTabularFiles: remaining };
    }),
    clearTabularFiles: () => set({ attachedTabularFiles: [] }),
    
    // Always-on configuration (synced from settings store)
    alwaysOnTools: [],
    alwaysOnTables: [],
    alwaysOnRagPaths: [],
    syncAlwaysOnFromSettings: () => {
        // This function syncs always-on items from the settings store
        // Called when settings change or on mount
        // Import settings store dynamically to avoid circular dependency
        import('../../settings-store').then(({ useSettingsStore }) => {
            const settings = useSettingsStore.getState().settings;
            if (!settings) return;
            
            // Convert always-on builtin tools to AttachedTool format
            const builtinTools: AttachedTool[] = (settings.always_on_builtin_tools || []).map(name => ({
                key: `builtin::${name}`,
                name,
                server: 'builtin',
                isBuiltin: true,
            }));
            
            // Convert always-on MCP tools to AttachedTool format
            const mcpTools: AttachedTool[] = (settings.always_on_mcp_tools || []).map(toolKey => {
                const parts = toolKey.split('::');
                const serverId = parts[0] || 'unknown';
                const toolName = parts.slice(1).join('::') || toolKey;
                return {
                    key: toolKey,
                    name: toolName,
                    server: serverId,
                    isBuiltin: false,
                };
            });
            
            // Convert always-on tables to AttachedTable format
            const tables: AttachedTable[] = (settings.always_on_tables || []).map(t => ({
                sourceId: t.source_id,
                sourceName: t.source_id, // We don't have the name here, use ID
                tableFqName: t.table_fq_name,
                columnCount: 0, // Unknown at sync time
            }));
            
            set({
                alwaysOnTools: [...builtinTools, ...mcpTools],
                alwaysOnTables: tables,
                alwaysOnRagPaths: settings.always_on_rag_paths || [],
            });
        });
    },
});
