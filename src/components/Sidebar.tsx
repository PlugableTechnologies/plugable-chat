import { Plus, FileCode } from 'lucide-react';
import { useChatStore } from '../store/chat-store';

export function Sidebar() {
  const { toggleCodeEditor } = useChatStore();

  return (
    <div className="w-[260px] bg-gray-50 text-gray-900 p-2 flex flex-col h-full border-r border-gray-200 shrink-0">
        <button className="border border-gray-300 rounded-md p-3 w-full mb-4 hover:bg-gray-200 text-left flex items-center gap-2 transition-colors text-sm text-gray-900">
             <Plus size={16} />
             <span>New chat</span>
        </button>
        
        <div className="flex-1 overflow-y-auto scrollbar-hide">
            <div className="px-2 py-2">
                <div className="text-xs font-medium text-gray-500 mb-2 pl-2">Today</div>
                <div className="text-gray-700 text-sm p-2 hover:bg-gray-200 rounded-md cursor-pointer truncate transition-colors">
                    Project Planning
                </div>
                <div className="text-gray-700 text-sm p-2 hover:bg-gray-200 rounded-md cursor-pointer truncate transition-colors">
                    Rust Actor System
                </div>
            </div>
        </div>
        
        <div className="p-2 border-t border-gray-200 space-y-1">
             <button 
                onClick={toggleCodeEditor}
                className="w-full flex items-center gap-2 p-2 hover:bg-gray-200 rounded-md cursor-pointer text-sm text-gray-900 text-left"
             >
                <FileCode size={16} />
                <span>Code Editor</span>
             </button>
             
             <div className="flex items-center gap-2 p-2 hover:bg-gray-200 rounded-md cursor-pointer text-sm text-gray-900">
                 <div className="w-8 h-8 bg-green-600 rounded-sm flex items-center justify-center text-xs">
                     B
                 </div>
                 <div className="font-medium">User</div>
             </div>
        </div>
    </div>
  )
}
