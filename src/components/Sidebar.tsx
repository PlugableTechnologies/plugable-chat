import { Plus, Search } from 'lucide-react';
import { useChatStore } from '../store/chat-store';
import { useEffect } from 'react';

export function Sidebar() {
  const { history, fetchHistory } = useChatStore();

  useEffect(() => {
      fetchHistory();
  }, []);
  
  return (
    <div className="w-[260px] bg-[#f9f9f9] text-gray-900 flex flex-col h-full border-r border-gray-200 shrink-0 font-sans text-sm">
        {/* Logo */}
        <div className="px-4 pt-4 pb-2">
            <img src="/plugable-logo.png" alt="Plugable" className="w-full h-auto object-contain" />
        </div>

        {/* New Chat Button */}
        <div className="px-3 pt-2">
            <button className="w-full flex items-center gap-2 p-2 hover:bg-gray-200 rounded-lg transition-colors border border-gray-200 bg-white shadow-sm text-left">
                <Plus size={16} className="text-gray-500" />
                <span className="font-medium text-gray-700">New chat</span>
            </button>
        </div>

        {/* Scrollable Content - History */}
        <div className="flex-1 overflow-y-auto scrollbar-hide px-3 mt-4">
            <div className="mb-6">
                 <div className="text-xs font-medium text-gray-500 mb-2 px-2">History</div>
                 <div className="space-y-1">
                    {history.length === 0 ? (
                        <div className="px-2 text-xs text-gray-400 italic">No history yet</div>
                    ) : (
                        history.map((chat) => (
                            <div key={chat.id} className="text-gray-700 p-2 hover:bg-gray-200 rounded-lg cursor-pointer truncate transition-colors text-xs">
                                {chat.title || "Untitled Chat"}
                            </div>
                        ))
                    )}
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
