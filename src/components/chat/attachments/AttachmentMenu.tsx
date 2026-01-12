import { useEffect, useRef } from 'react';

interface AttachmentMenuProps {
    isOpen: boolean;
    onClose: () => void;
    onSelectFiles: () => void;
    onSelectFolder: () => void;
    onSelectDatabase: () => void;
    onSelectTool: () => void;
    filesDisabled: boolean;
    dbDisabled: boolean;
}

/**
 * Attachment Menu Component - dropdown for selecting attachment types
 */
export const AttachmentMenu = ({
    isOpen,
    onClose,
    onSelectFiles,
    onSelectFolder,
    onSelectDatabase,
    onSelectTool,
    filesDisabled,
    dbDisabled
}: AttachmentMenuProps) => {
    const menuRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        const handleClickOutside = (e: MouseEvent) => {
            if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
                onClose();
            }
        };
        if (isOpen) {
            document.addEventListener('mousedown', handleClickOutside);
        }
        return () => document.removeEventListener('mousedown', handleClickOutside);
    }, [isOpen, onClose]);

    if (!isOpen) return null;

    return (
        <div
            ref={menuRef}
            className="absolute bottom-full left-0 mb-2 bg-white rounded-lg shadow-lg border border-gray-200 py-1 min-w-[180px] z-50"
        >
            <button
                onClick={() => { onSelectFiles(); onClose(); }}
                disabled={filesDisabled}
                className={`w-full px-4 py-2 text-left text-sm flex items-center gap-2 ${filesDisabled ? 'text-gray-300 cursor-not-allowed' : 'text-gray-700 hover:bg-gray-100'
                    }`}
            >
                <span>📄</span>
                <span>Attach Files</span>
            </button>
            <button
                onClick={() => { onSelectFolder(); onClose(); }}
                disabled={filesDisabled}
                className={`w-full px-4 py-2 text-left text-sm flex items-center gap-2 ${filesDisabled ? 'text-gray-300 cursor-not-allowed' : 'text-gray-700 hover:bg-gray-100'
                    }`}
            >
                <span>📁</span>
                <span>Attach Folder</span>
            </button>
            <div className="border-t border-gray-100 my-1" />
            <button
                onClick={() => { onSelectDatabase(); onClose(); }}
                disabled={dbDisabled}
                className={`w-full px-4 py-2 text-left text-sm flex items-center gap-2 ${dbDisabled ? 'text-gray-300 cursor-not-allowed' : 'text-gray-700 hover:bg-gray-100'
                    }`}
            >
                <span>🗄️</span>
                <span>Attach Database</span>
            </button>
            <button
                onClick={() => { onSelectTool(); onClose(); }}
                className="w-full px-4 py-2 text-left text-sm text-gray-700 hover:bg-gray-100 flex items-center gap-2"
            >
                <span>🔧</span>
                <span>Attach Tool</span>
            </button>
        </div>
    );
};
