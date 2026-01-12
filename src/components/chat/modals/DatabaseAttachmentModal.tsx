import { useEffect, useState } from 'react';
import { Database, X, Loader2, AlertCircle, Layout, Check, ChevronRight } from 'lucide-react';
import { invoke } from '../../../lib/api';
import { useChatStore } from '../../../store/chat-store';

interface DatabaseAttachmentModalProps {
    isOpen: boolean;
    onClose: () => void;
    chatPrompt?: string;
}

/**
 * Database Attachment Modal - allows selecting database tables to attach
 */
export const DatabaseAttachmentModal = ({
    isOpen,
    onClose,
    chatPrompt = ""
}: DatabaseAttachmentModalProps) => {
    const { attachedDatabaseTables, addAttachedTable, removeAttachedTable } = useChatStore();
    const [results, setResults] = useState<any[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);

    // Fetch tables when modal opens, ordered by relevance to chat prompt (if any)
    useEffect(() => {
        if (!isOpen) return;

        const fetchTables = async () => {
            setLoading(true);
            setError(null);
            try {
                // If there's a chat prompt, use semantic search to order by relevance
                // Otherwise, get all tables (backend returns all when query is empty)
                const searchResults = await invoke<any[]>('search_database_tables', {
                    query: chatPrompt.trim(),
                    limit: 50
                });
                setResults(searchResults);
            } catch (err: any) {
                setError(err?.message || String(err));
            } finally {
                setLoading(false);
            }
        };

        fetchTables();
    }, [isOpen, chatPrompt]);

    if (!isOpen) return null;

    const isAttached = (fqName: string) =>
        attachedDatabaseTables.some(t => t.tableFqName === fqName);

    const hasPrompt = chatPrompt.trim().length > 0;

    return (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/50 backdrop-blur-sm p-4">
            <div className="bg-white rounded-2xl shadow-xl w-full max-w-2xl max-h-[80vh] flex flex-col overflow-hidden">
                <div className="px-6 py-4 border-b border-gray-100 flex items-center justify-between">
                    <div className="flex items-center gap-2">
                        <Database className="text-amber-500" size={20} />
                        <h2 className="text-lg font-semibold text-gray-900">Attach Database Tables</h2>
                    </div>
                    <button onClick={onClose} className="text-gray-400 hover:text-gray-600">
                        <X size={20} />
                    </button>
                </div>

                <div className="p-6 space-y-4 flex-1 overflow-hidden flex flex-col">
                    {/* Show context about ordering */}
                    {hasPrompt && (
                        <div className="text-xs text-gray-500 bg-amber-50 px-3 py-2 rounded-lg border border-amber-100">
                            <span className="font-medium text-amber-700">Ordered by relevance to:</span>{' '}
                            <span className="italic">"{chatPrompt.length > 60 ? chatPrompt.slice(0, 60) + '...' : chatPrompt}"</span>
                        </div>
                    )}

                    {error && (
                        <div className="flex items-center gap-2 p-3 bg-red-50 text-red-700 rounded-lg text-sm">
                            <AlertCircle size={16} />
                            <span>{error}</span>
                        </div>
                    )}

                    <div className="flex-1 overflow-y-auto space-y-2 pr-2 custom-scrollbar">
                        {loading ? (
                            <div className="flex flex-col items-center justify-center py-12 text-gray-400 gap-3">
                                <Loader2 className="animate-spin" size={32} />
                                <p className="text-sm">Loading tables...</p>
                            </div>
                        ) : results.length === 0 ? (
                            <div className="text-center py-12 text-gray-500 italic">
                                No database tables available. Configure database sources in Settings.
                            </div>
                        ) : (
                            results.map((res) => {
                                const table = res.table;
                                const attached = isAttached(table.table_fq_name);
                                // Parse fully qualified name into components (e.g., project.dataset.table)
                                const nameParts = table.table_fq_name.split('.');
                                const tableName = nameParts.pop() || table.table_fq_name;
                                const pathParts = nameParts; // remaining parts (project, dataset, etc.)

                                return (
                                    <div
                                        key={table.table_fq_name}
                                        onClick={() => {
                                            if (attached) {
                                                removeAttachedTable(table.table_fq_name);
                                            } else {
                                                addAttachedTable({
                                                    sourceId: table.source_id,
                                                    sourceName: table.source_name,
                                                    tableFqName: table.table_fq_name,
                                                    columnCount: table.column_count
                                                });
                                            }
                                        }}
                                        className={`group cursor-pointer p-4 rounded-xl border transition-all flex items-start gap-4 ${attached
                                                ? 'bg-amber-50 border-amber-200 ring-1 ring-amber-200'
                                                : 'bg-white border-gray-100 hover:border-amber-200 hover:bg-gray-50'
                                            }`}
                                    >
                                        <div className={`mt-0.5 w-5 h-5 rounded border flex-shrink-0 flex items-center justify-center transition-colors ${attached
                                                ? 'bg-amber-500 border-amber-500 text-white'
                                                : 'bg-white border-gray-300 group-hover:border-amber-400'
                                            }`}>
                                            {attached && <Check size={14} />}
                                        </div>
                                        <div className="flex-1 min-w-0">
                                            {/* Path breadcrumb (project / dataset) */}
                                            {pathParts.length > 0 && (
                                                <div className="flex flex-wrap items-center gap-1 mb-1.5 text-xs text-gray-500">
                                                    {pathParts.map((part: string, idx: number) => (
                                                        <span key={idx} className="flex items-center gap-1">
                                                            <span className="break-all">{part}</span>
                                                            {idx < pathParts.length - 1 && (
                                                                <ChevronRight size={12} className="text-gray-300 flex-shrink-0" />
                                                            )}
                                                        </span>
                                                    ))}
                                                </div>
                                            )}
                                            {/* Table name - prominent */}
                                            <div className="font-semibold text-gray-900 break-words leading-snug mb-2">
                                                {tableName}
                                            </div>
                                            {/* Metadata row */}
                                            <div className="flex flex-wrap items-center gap-2 text-xs">
                                                <span className="px-2 py-0.5 rounded-full bg-amber-100 text-amber-700 font-medium flex-shrink-0">
                                                    {table.source_id}
                                                </span>
                                                <span className="flex items-center gap-1 text-gray-500 flex-shrink-0">
                                                    <Layout size={12} /> {table.column_count} columns
                                                </span>
                                                {hasPrompt && res.relevance_score > 0 && (
                                                    <span className="text-amber-600 font-medium flex-shrink-0">
                                                        {(res.relevance_score * 100).toFixed(0)}% match
                                                    </span>
                                                )}
                                            </div>
                                            {/* Description - wraps naturally */}
                                            {table.description && (
                                                <p className="mt-2 text-xs text-gray-500 leading-relaxed">
                                                    {table.description}
                                                </p>
                                            )}
                                        </div>
                                    </div>
                                );
                            })
                        )}
                    </div>
                </div>

                <div className="px-6 py-4 bg-gray-50 border-t border-gray-100 flex justify-between items-center">
                    <div className="text-xs text-gray-500">
                        {attachedDatabaseTables.length} table{attachedDatabaseTables.length === 1 ? '' : 's'} selected
                    </div>
                    <button
                        onClick={onClose}
                        className="px-6 py-2 bg-gray-900 text-white rounded-xl font-medium hover:bg-gray-800 transition-colors"
                    >
                        Done
                    </button>
                </div>
            </div>
        </div>
    );
};
