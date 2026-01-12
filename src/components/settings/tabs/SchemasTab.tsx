import { useState, useEffect } from 'react';
import { Database, Loader2 } from 'lucide-react';
import { useSettingsStore } from '../../../store/settings-store';
import { invoke } from '../../../lib/api';

export function SchemasTab() {
    const { settings, addAlwaysOnTable, removeAlwaysOnTable } = useSettingsStore();
    const [cachedTables, setCachedTables] = useState<any[]>([]);
    const [loading, setLoading] = useState(false);
    const [searchQuery, setSearchQuery] = useState('');

    // Fetch cached tables from backend
    useEffect(() => {
        const fetchTables = async () => {
            setLoading(true);
            try {
                const tables = await invoke<any[]>('get_cached_database_schemas');
                setCachedTables(tables);
            } catch (e: any) {
                console.error('[SchemasTab] Failed to fetch tables:', e);
            } finally {
                setLoading(false);
            }
        };
        fetchTables();
    }, []);

    const alwaysOnTables = settings?.always_on_tables || [];
    
    const isAlwaysOn = (sourceId: string, tableFqName: string) => 
        alwaysOnTables.some(t => t.source_id === sourceId && t.table_fq_name === tableFqName);

    const toggleAlwaysOn = async (sourceId: string, tableFqName: string) => {
        if (isAlwaysOn(sourceId, tableFqName)) {
            await removeAlwaysOnTable(sourceId, tableFqName);
        } else {
            await addAlwaysOnTable(sourceId, tableFqName);
        }
    };

    // Filter tables by search query
    const filteredTables = cachedTables.filter(table => {
        if (!searchQuery.trim()) return true;
        const query = searchQuery.toLowerCase();
        return table.fully_qualified_name?.toLowerCase().includes(query) ||
               table.source_id?.toLowerCase().includes(query);
    });

    return (
        <div className="space-y-6">
            <div>
                <h3 className="text-sm font-medium text-gray-700">Always-On Tables</h3>
                <p className="text-xs text-gray-500 mt-1">
                    Tables marked as "Always On" will automatically have their schemas included in every chat.
                    They appear as locked pills in the chat input area.
                </p>
            </div>

            {/* Search */}
            <div className="relative">
                <input
                    type="text"
                    value={searchQuery}
                    onChange={(e) => setSearchQuery(e.target.value)}
                    placeholder="Search tables..."
                    className="w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:border-blue-400 focus:outline-none focus:ring-1 focus:ring-blue-400"
                />
            </div>

            {loading ? (
                <div className="flex items-center justify-center py-12">
                    <Loader2 className="animate-spin text-gray-400" size={24} />
                </div>
            ) : filteredTables.length === 0 ? (
                <div className="text-center py-8 text-gray-500 border border-dashed border-gray-200 rounded-xl">
                    <Database size={32} className="mx-auto mb-2 opacity-30" />
                    <p className="text-sm">No cached database tables</p>
                    <p className="text-xs mt-1">Go to Databases tab and click "Refresh Schemas" to index your tables</p>
                </div>
            ) : (
                <div className="space-y-2 max-h-[400px] overflow-y-auto">
                    {filteredTables.map((table) => {
                        const isOn = isAlwaysOn(table.source_id, table.fully_qualified_name);
                        return (
                            <div 
                                key={`${table.source_id}::${table.fully_qualified_name}`}
                                className={`flex items-center justify-between p-3 rounded-lg border transition-colors ${
                                    isOn ? 'bg-amber-50 border-amber-200' : 'bg-white border-gray-200'
                                }`}
                            >
                                <div className="flex-1 min-w-0">
                                    <div className="text-sm font-medium text-gray-900 truncate">
                                        {table.fully_qualified_name}
                                    </div>
                                    <div className="text-xs text-gray-500">
                                        Source: {table.source_id} | {table.column_count || 0} columns
                                    </div>
                                </div>
                                <button
                                    onClick={() => toggleAlwaysOn(table.source_id, table.fully_qualified_name)}
                                    className={`ml-4 px-3 py-1 text-xs font-medium rounded-full transition-colors ${
                                        isOn 
                                            ? 'bg-amber-500 text-white hover:bg-amber-600' 
                                            : 'bg-gray-100 text-gray-600 hover:bg-gray-200'
                                    }`}
                                >
                                    {isOn ? 'Always On' : 'Off'}
                                </button>
                            </div>
                        );
                    })}
                </div>
            )}

            {alwaysOnTables.length > 0 && (
                <div className="pt-4 border-t border-gray-100">
                    <p className="text-xs text-gray-500">
                        {alwaysOnTables.length} table{alwaysOnTables.length !== 1 ? 's' : ''} set to always-on
                    </p>
                </div>
            )}
        </div>
    );
}
