import { useState } from 'react';
import { Search, Wrench, X, Check } from 'lucide-react';
import { useChatStore, type AttachedTool } from '../../../store/chat-store';
import { useSettingsStore } from '../../../store/settings-store';

interface ToolAttachmentModalProps {
    isOpen: boolean;
    onClose: () => void;
}

// Built-in tool definitions
const BUILTIN_TOOLS = [
    { name: 'python_execution', desc: 'Execute Python code in a secure sandbox' },
    { name: 'tool_search', desc: 'Discover MCP tools by semantic search' },
    { name: 'schema_search', desc: 'Find database tables and structures' },
    { name: 'sql_select', desc: 'Execute SQL SELECT queries on databases' }
];

/**
 * Tool Attachment Modal - allows selecting tools to attach to the chat
 */
export const ToolAttachmentModal = ({
    isOpen,
    onClose
}: ToolAttachmentModalProps) => {
    const { attachedTools, addAttachedTool, removeAttachedTool } = useChatStore();
    const { serverStatuses } = useSettingsStore();
    const [query, setQuery] = useState("");

    if (!isOpen) return null;

    // Build list of all available tools
    const availableTools: AttachedTool[] = [];

    // 1. Built-in tools
    BUILTIN_TOOLS.forEach(b => {
        availableTools.push({
            key: `builtin::${b.name}`,
            name: b.name,
            server: 'builtin',
            isBuiltin: true
        });
    });

    // 2. MCP tools from connected servers
    Object.entries(serverStatuses).forEach(([serverId, status]) => {
        if (status.connected && status.tools) {
            status.tools.forEach(tool => {
                availableTools.push({
                    key: `${serverId}::${tool.name}`,
                    name: tool.name,
                    server: serverId,
                    isBuiltin: false
                });
            });
        }
    });

    const filteredTools = availableTools.filter(t =>
        t.name.toLowerCase().includes(query.toLowerCase()) ||
        t.server.toLowerCase().includes(query.toLowerCase())
    );

    const isAttached = (key: string) => attachedTools.some(t => t.key === key);

    return (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/50 backdrop-blur-sm p-4">
            <div className="bg-white rounded-2xl shadow-xl w-full max-w-2xl max-h-[80vh] flex flex-col overflow-hidden">
                <div className="px-6 py-4 border-b border-gray-100 flex items-center justify-between">
                    <div className="flex items-center gap-2">
                        <Wrench className="text-purple-500" size={20} />
                        <h2 className="text-lg font-semibold text-gray-900">Attach Tools</h2>
                    </div>
                    <button onClick={onClose} className="text-gray-400 hover:text-gray-600">
                        <X size={20} />
                    </button>
                </div>

                <div className="p-6 space-y-4 flex-1 overflow-hidden flex flex-col">
                    <div className="relative">
                        <Search className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" size={18} />
                        <input
                            type="text"
                            autoFocus
                            placeholder="Search tools by name or server..."
                            className="w-full pl-10 pr-4 py-2 bg-gray-50 border border-gray-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-purple-500/20 focus:border-purple-500 transition-all"
                            value={query}
                            onChange={(e) => setQuery(e.target.value)}
                        />
                    </div>

                    <div className="flex-1 overflow-y-auto space-y-2 pr-2 custom-scrollbar">
                        {filteredTools.length === 0 ? (
                            <div className="text-center py-12 text-gray-500 italic">
                                No tools found.
                            </div>
                        ) : (
                            filteredTools.map((tool) => {
                                const attached = isAttached(tool.key);
                                return (
                                    <div
                                        key={tool.key}
                                        onClick={() => {
                                            if (attached) {
                                                removeAttachedTool(tool.key);
                                            } else {
                                                addAttachedTool(tool);
                                            }
                                        }}
                                        className={`group cursor-pointer p-4 rounded-xl border transition-all flex items-start gap-4 ${attached
                                                ? 'bg-purple-50 border-purple-200 ring-1 ring-purple-200'
                                                : 'bg-white border-gray-100 hover:border-purple-200 hover:bg-gray-50'
                                            }`}
                                    >
                                        <div className={`mt-0.5 w-5 h-5 rounded border flex items-center justify-center transition-colors ${attached
                                                ? 'bg-purple-500 border-purple-500 text-white'
                                                : 'bg-white border-gray-300 group-hover:border-purple-400'
                                            }`}>
                                            {attached && <Check size={14} />}
                                        </div>
                                        <div className="flex-1 min-w-0">
                                            <div className="flex items-center gap-2 mb-1">
                                                <span className="font-medium text-gray-900 truncate">
                                                    {tool.name}
                                                </span>
                                                <span className={`px-1.5 py-0.5 rounded text-[10px] font-semibold uppercase ${tool.isBuiltin
                                                        ? 'bg-blue-100 text-blue-600'
                                                        : 'bg-gray-100 text-gray-500'
                                                    }`}>
                                                    {tool.server}
                                                </span>
                                            </div>
                                            {tool.isBuiltin && (
                                                <p className="text-xs text-gray-500">
                                                    {BUILTIN_TOOLS.find(b => b.name === tool.name)?.desc}
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
                        {attachedTools.length} tool{attachedTools.length === 1 ? '' : 's'} selected
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
