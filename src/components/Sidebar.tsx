import { useChatStore } from '../store/chat-store';
import { useEffect, useState, useRef } from 'react';
import { MoreHorizontal, Pin, Trash, Edit, MessageSquare, Plus } from 'lucide-react';

type SidebarProps = {
    className?: string;
};

export function Sidebar({ className = "" }: SidebarProps) {
    const {
        history, fetchHistory, loadChat, deleteChat, renameChat, togglePin, currentChatId
    } = useChatStore();

    const [editingId, setEditingId] = useState<string | null>(null);
    const [editTitle, setEditTitle] = useState("");
    const [menuOpenId, setMenuOpenId] = useState<string | null>(null);
    const menuRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        fetchHistory();
    }, []);

    // Close menu when clicking outside
    useEffect(() => {
        const handleClickOutside = (event: MouseEvent) => {
            if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
                setMenuOpenId(null);
            }
        };
        document.addEventListener('mousedown', handleClickOutside);
        return () => document.removeEventListener('mousedown', handleClickOutside);
    }, []);

    const handleRenameSubmit = async (id: string) => {
        if (editTitle.trim()) {
            await renameChat(id, editTitle);
        }
        setEditingId(null);
    };

    const pinnedChats = history.filter(c => c.pinned);
    const recentChats = history.filter(c => !c.pinned);

    const ChatItem = ({ chat }: { chat: any }) => {
        const isActive = chat.id === currentChatId;
        const isEditing = editingId === chat.id;

        return (
            <div
                className={`group relative flex items-center gap-2 px-3 py-2 rounded-lg cursor-pointer transition-all text-sm border border-transparent
                    ${isActive ? 'bg-gray-200 text-gray-900 font-medium' : 'text-gray-700 hover:bg-gray-100'}
                `}
                onClick={() => !isEditing && loadChat(chat.id)}
            >
                <MessageSquare size={16} className={`shrink-0 ${isActive ? 'text-gray-900' : 'text-gray-500'}`} />

                {isEditing ? (
                    <input
                        autoFocus
                        type="text"
                        value={editTitle}
                        onChange={(e) => setEditTitle(e.target.value)}
                        onBlur={() => handleRenameSubmit(chat.id)}
                        onKeyDown={(e) => e.key === 'Enter' && handleRenameSubmit(chat.id)}
                        onClick={(e) => e.stopPropagation()}
                        className="flex-1 bg-white border border-gray-300 rounded px-1 py-0.5 text-sm focus:outline-none focus:border-blue-500"
                    />
                ) : (
                    <span className="truncate flex-1">{chat.title || "Untitled Chat"}</span>
                )}

                {/* Menu Button (visible on hover or if menu open) */}
                <div className={`absolute right-2 opacity-0 group-hover:opacity-100 transition-opacity ${menuOpenId === chat.id ? 'opacity-100' : ''}`}>
                    <button
                        onClick={(e) => {
                            e.stopPropagation();
                            setMenuOpenId(menuOpenId === chat.id ? null : chat.id);
                        }}
                        className="p-1 hover:bg-gray-300 rounded text-gray-500 hover:text-gray-900"
                    >
                        <MoreHorizontal size={14} />
                    </button>

                    {/* Dropdown Menu */}
                    {menuOpenId === chat.id && (
                        <div
                            ref={menuRef}
                            className="absolute right-0 top-full mt-1 w-32 bg-white border border-gray-200 rounded-lg shadow-lg z-50 py-1 flex flex-col"
                            onClick={(e) => e.stopPropagation()}
                        >
                            <button
                                onClick={() => {
                                    setEditTitle(chat.title);
                                    setEditingId(chat.id);
                                    setMenuOpenId(null);
                                }}
                                className="flex items-center gap-2 px-3 py-2 text-xs text-gray-700 hover:bg-gray-100 w-full text-left"
                            >
                                <Edit size={12} /> Rename
                            </button>
                            <button
                                onClick={() => {
                                    togglePin(chat.id);
                                    setMenuOpenId(null);
                                }}
                                className="flex items-center gap-2 px-3 py-2 text-xs text-gray-700 hover:bg-gray-100 w-full text-left"
                            >
                                <Pin size={12} /> {chat.pinned ? 'Unpin' : 'Pin'}
                            </button>
                            <div className="h-px bg-gray-100 my-1"></div>
                            <button
                                onClick={() => {
                                    if (confirm('Are you sure you want to delete this chat?')) {
                                        deleteChat(chat.id);
                                    }
                                    setMenuOpenId(null);
                                }}
                                className="flex items-center gap-2 px-3 py-2 text-xs text-red-600 hover:bg-red-50 w-full text-left"
                            >
                                <Trash size={12} /> Delete
                            </button>
                        </div>
                    )}
                </div>
            </div>
        );
    };

    return (
        <div className={`bg-gray-50 text-gray-800 flex flex-col h-full w-full border-r border-gray-200 font-sans text-sm ${className}`}>
            {/* Scrollable Content - History */}
            <div className="flex-1 overflow-y-auto scrollbar-hide px-3 pt-4">
                {/* New Chat Button */}
                <button
                    onClick={() => {
                        useChatStore.setState({ messages: [], input: '', currentChatId: null });
                    }}
                    className="w-full flex items-center gap-2 text-gray-700 hover:bg-gray-100 transition-colors px-3 py-2.5 rounded-lg mb-4 group border border-gray-200 hover:border-gray-300 bg-white shadow-sm"
                >
                    <Plus size={16} className="text-gray-500 group-hover:text-gray-900" />
                    <span className="font-medium text-sm">New Chat</span>
                </button>

                {/* Pinned Section */}
                {pinnedChats.length > 0 && (
                    <div className="mb-6">
                        <div className="text-xs font-semibold text-gray-500 mb-2 px-3 uppercase tracking-wider flex items-center gap-2">
                            <Pin size={10} /> Pinned
                        </div>
                        <div className="space-y-0.5">
                            {pinnedChats.map((chat) => (
                                <ChatItem key={chat.id} chat={chat} />
                            ))}
                        </div>
                    </div>
                )}

                {/* Chat History Section */}
                <div className="mb-8">
                    <div className="text-xs font-semibold text-gray-500 mb-2 px-3 uppercase tracking-wider">Recent</div>
                    <div className="space-y-0.5">
                        {recentChats.length === 0 ? (
                            <div className="px-3 text-sm text-gray-400 italic py-2">No history yet</div>
                        ) : (
                            recentChats.map((chat) => (
                                <ChatItem key={chat.id} chat={chat} />
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

