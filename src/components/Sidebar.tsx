import { useChatStore } from '../store/chat-store';
import { useSettingsStore } from '../store/settings-store';
import { useEffect, useState, useRef } from 'react';
import { MoreHorizontal, Pin, Trash, Edit, MessageSquare, Plus, Search, Loader2, Settings } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';

type SidebarProps = {
    className?: string;
};

// ChatItem props type
type ChatItemProps = {
    chat: any;
    isActive: boolean;
    isEditing: boolean;
    isMenuOpen: boolean;
    editTitle: string;
    menuRef: React.RefObject<HTMLDivElement | null>;
    onLoadChat: (id: string) => void;
    onMenuToggle: (id: string | null) => void;
    onEditTitleChange: (title: string) => void;
    onRenameSubmit: (id: string) => void;
    onStartEdit: (id: string, title: string) => void;
    onTogglePin: (id: string) => void;
    onDelete: (id: string) => Promise<void>;
};

// Extracted ChatItem component - stable identity across parent re-renders
function ChatItem({
    chat,
    isActive,
    isEditing,
    isMenuOpen,
    editTitle,
    menuRef,
    onLoadChat,
    onMenuToggle,
    onEditTitleChange,
    onRenameSubmit,
    onStartEdit,
    onTogglePin,
    onDelete,
}: ChatItemProps) {
    return (
        <div
            className={`chat-list-item group relative flex items-center gap-2 px-3 py-2 rounded-lg cursor-pointer transition-all text-sm border border-transparent
                ${isActive ? 'bg-gray-200 text-gray-900 font-medium' : 'text-gray-700 hover:bg-gray-100'}
            `}
            onClick={() => !isEditing && onLoadChat(chat.id)}
        >
            <MessageSquare size={16} className={`shrink-0 ${isActive ? 'text-gray-900' : 'text-gray-500'}`} />

            {isEditing ? (
                <input
                    autoFocus
                    type="text"
                    value={editTitle}
                    onChange={(e) => onEditTitleChange(e.target.value)}
                    onBlur={() => onRenameSubmit(chat.id)}
                    onKeyDown={(e) => e.key === 'Enter' && onRenameSubmit(chat.id)}
                    onClick={(e) => e.stopPropagation()}
                    className="flex-1 bg-white border border-gray-300 rounded px-1 py-0.5 text-sm focus:outline-none focus:border-blue-500"
                />
            ) : (
                <span className="truncate flex-1">{chat.title || "Untitled Chat"}</span>
            )}

            {/* Menu Button and Dropdown - wrapped in ref for click-outside detection */}
            <div 
                ref={isMenuOpen ? menuRef : undefined}
                className={`absolute right-2 opacity-0 group-hover:opacity-100 transition-opacity ${isMenuOpen ? 'opacity-100' : ''}`}
            >
                <button
                    onClick={(e) => {
                        e.stopPropagation();
                        onMenuToggle(isMenuOpen ? null : chat.id);
                    }}
                    onMouseDown={(e) => e.stopPropagation()}
                    className="p-1 hover:bg-gray-300 rounded text-gray-500 hover:text-gray-900"
                >
                    <MoreHorizontal size={14} />
                </button>

                {/* Dropdown Menu */}
                {isMenuOpen && (
                    <div
                        className="absolute right-0 top-full mt-1 w-32 bg-white border border-gray-200 rounded-lg shadow-lg z-50 py-1 flex flex-col"
                        onClick={(e) => e.stopPropagation()}
                        onMouseDown={(e) => e.stopPropagation()}
                    >
                        <button
                            onClick={() => {
                                onStartEdit(chat.id, chat.title);
                                onMenuToggle(null);
                            }}
                            className="flex items-center gap-2 px-3 py-2 text-xs text-gray-700 hover:bg-gray-100 w-full text-left"
                        >
                            <Edit size={12} /> Rename
                        </button>
                        <button
                            onClick={() => {
                                onTogglePin(chat.id);
                                onMenuToggle(null);
                            }}
                            className="flex items-center gap-2 px-3 py-2 text-xs text-gray-700 hover:bg-gray-100 w-full text-left"
                        >
                            <Pin size={12} /> {chat.pinned ? 'Unpin' : 'Pin'}
                        </button>
                        <div className="h-px bg-gray-100 my-1"></div>
                        <button
                            onClick={async (e) => {
                                e.stopPropagation();
                                e.preventDefault();
                                await onDelete(chat.id);
                                onMenuToggle(null);
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
}

export function Sidebar({ className = "" }: SidebarProps) {
    const {
        history, fetchHistory, loadChat, deleteChat, renameChat, togglePin, currentChatId,
        relevanceResults, isSearchingRelevance, chatInputValue
    } = useChatStore();

    const [editingId, setEditingId] = useState<string | null>(null);
    const [editTitle, setEditTitle] = useState("");
    const [menuOpenId, setMenuOpenId] = useState<string | null>(null);
    const menuRef = useRef<HTMLDivElement | null>(null);

    useEffect(() => {
        console.log('[Sidebar] Component mounted, fetching history...');
        fetchHistory();
    }, []);

    // Log when history changes
    useEffect(() => {
        console.log(`[Sidebar] History updated: ${history.length} chats`, 
            history.map(c => ({ id: c.id.slice(0, 8), title: c.title })));
    }, [history]);

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

    const handleDelete = async (id: string) => {
        await invoke('log_to_terminal', { message: `[Sidebar] Delete button clicked for chat: ${id}` });
        
        // Skip confirm dialog - it may not work well in Tauri webview
        await invoke('log_to_terminal', { message: `[Sidebar] Calling deleteChat...` });
        try {
            await deleteChat(id);
            await invoke('log_to_terminal', { message: '[Sidebar] deleteChat completed successfully' });
        } catch (err) {
            await invoke('log_to_terminal', { message: `[Sidebar] deleteChat ERROR: ${err}` });
        }
    };

    const handleStartEdit = (id: string, title: string) => {
        setEditTitle(title);
        setEditingId(id);
    };

    // Use relevance results if available (user is typing), otherwise use normal history
    const isShowingRelevance =
        relevanceResults !== null && chatInputValue.trim().length >= 3;
    const displayChats = isShowingRelevance ? relevanceResults : history;
    
    const pinnedChats = displayChats.filter(c => c.pinned);
    const recentChats = displayChats.filter(c => !c.pinned);

    return (
        <div className={`sidebar-layout text-gray-900 flex flex-col h-full w-full font-sans text-sm ${className}`} style={{ backgroundColor: '#e5e7eb' }}>
            {/* Scrollable Content - History */}
            <div className="sidebar-scroll-region flex-1 overflow-y-auto scrollbar-hide px-3 pt-4">
                {/* New Chat Button */}
                <button
                    onClick={() => {
                        useChatStore.setState({
                            chatMessages: [],
                            chatInputValue: '',
                            currentChatId: null,
                        });
                    }}
                    className="sidebar-new-chat-button inline-flex items-center gap-2 text-gray-600 hover:text-gray-900 transition-colors px-3 py-1.5 rounded-full group text-xs font-semibold uppercase tracking-wide self-start"
                    style={{ marginBottom: '24px' }}
                >
                    <Plus size={16} className="text-gray-500 group-hover:text-gray-700" />
                    <span>New Chat</span>
                </button>

                {/* Pinned Section */}
                {pinnedChats.length > 0 && (
                    <div className="sidebar-pinned-section" style={{ paddingBottom: '24px' }}>
                        <div className="text-xs font-semibold text-gray-500 mb-2 px-3 uppercase tracking-wider flex items-center gap-2">
                            <Pin size={10} /> Pinned
                        </div>
                        <div className="space-y-0.5">
                            {pinnedChats.map((chat) => (
                                <ChatItem
                                    key={chat.id}
                                    chat={chat}
                                    isActive={chat.id === currentChatId}
                                    isEditing={editingId === chat.id}
                                    isMenuOpen={menuOpenId === chat.id}
                                    editTitle={editTitle}
                                    menuRef={menuRef}
                                    onLoadChat={loadChat}
                                    onMenuToggle={setMenuOpenId}
                                    onEditTitleChange={setEditTitle}
                                    onRenameSubmit={handleRenameSubmit}
                                    onStartEdit={handleStartEdit}
                                    onTogglePin={togglePin}
                                    onDelete={handleDelete}
                                />
                            ))}
                        </div>
                    </div>
                )}

                {/* Chat History Section */}
                <div className="sidebar-history-section mb-8">
                    {isShowingRelevance && (
                        <div className="text-xs font-semibold text-gray-500 mb-2 px-3 uppercase tracking-wider flex items-center gap-2">
                            <Search size={10} />
                            <span>Relevant</span>
                            {isSearchingRelevance && <Loader2 size={10} className="animate-spin" />}
                        </div>
                    )}
                    <div className="space-y-0.5">
                        {recentChats.length === 0 ? (
                            <div className="px-3 text-sm text-gray-400 italic py-2">
                                {isShowingRelevance ? "No matching chats" : "No history yet"}
                            </div>
                        ) : (
                            recentChats.map((chat) => (
                                <ChatItem
                                    key={chat.id}
                                    chat={chat}
                                    isActive={chat.id === currentChatId}
                                    isEditing={editingId === chat.id}
                                    isMenuOpen={menuOpenId === chat.id}
                                    editTitle={editTitle}
                                    menuRef={menuRef}
                                    onLoadChat={loadChat}
                                    onMenuToggle={setMenuOpenId}
                                    onEditTitleChange={setEditTitle}
                                    onRenameSubmit={handleRenameSubmit}
                                    onStartEdit={handleStartEdit}
                                    onTogglePin={togglePin}
                                    onDelete={handleDelete}
                                />
                            ))
                        )}
                    </div>
                </div>
            </div>

            {/* Bottom Actions */}
            <div className="sidebar-footer p-3 border-t border-gray-300">
                <button
                    onClick={() => useSettingsStore.getState().openSettings()}
                    className="sidebar-settings-button flex items-center gap-2 w-full px-3 py-2 text-sm text-gray-600 hover:text-gray-900 hover:bg-gray-200 rounded-lg transition-colors"
                >
                    <Settings size={16} />
                    <span>Settings</span>
                </button>
            </div>
        </div>
    )
}
