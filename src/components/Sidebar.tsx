import { useChatStore } from '../store/chat-store';
import { useEffect } from 'react';

type SidebarProps = {
    className?: string;
};

export function Sidebar({ className = "" }: SidebarProps) {
    const { history, fetchHistory } = useChatStore();

    useEffect(() => {
        fetchHistory();
    }, []);

    return (
        <div className={`bg-gray-50 text-gray-800 flex flex-col h-full w-full border-r border-gray-200 font-sans text-sm ${className}`}>
            {/* Scrollable Content - History */}
            <div className="flex-1 overflow-y-auto scrollbar-hide px-3 pt-4">
                {/* New Chat Button */}
                <button
                    onClick={() => {
                        useChatStore.setState({ messages: [], input: '' });
                    }}
                    className="w-full flex items-center gap-2 text-gray-700 hover:bg-gray-100 transition-colors px-3 py-2.5 rounded-lg mb-4 group"
                >
                    <span className="text-sm">✎</span>
                    <span className="font-medium text-sm">New Chat</span>
                </button>

                {/* Chat History Section */}
                <div className="mb-8">
                    <div className="text-xs font-semibold text-gray-500 mb-3 px-3 uppercase tracking-wider">Chat History</div>
                    <div className="space-y-1">
                        {history.length === 0 ? (
                            <div className="px-3 text-sm text-gray-400 italic py-2">No history yet</div>
                        ) : (
                            history.map((chat) => (
                                <div key={chat.id} className="text-gray-800 px-3 py-2.5 hover:bg-gray-100 rounded-lg cursor-pointer truncate transition-all text-sm border border-transparent">
                                    {chat.title || "Untitled Chat"}
                                </div>
                            ))
                        )}
                    </div>
                </div>

                {/* Pinned Section */}
                <div className="mb-6">
                    <div className="text-xs font-semibold text-gray-500 mb-3 px-3 uppercase tracking-wider">Pinned</div>
                    <div className="space-y-1">
                        {/* Empty for now - will be populated dynamically */}
                        <div className="px-3 text-sm text-gray-400 italic py-2">No pinned chats</div>
                    </div>
                </div>
            </div>

            {/* Bottom Actions */}
            <div className="p-3 space-y-1">
                <div className="pt-2 mt-2 border-t border-gray-200">
                    {/* Empty for now, user profile removed */}
                </div>
            </div>
        </div>
    )
}

