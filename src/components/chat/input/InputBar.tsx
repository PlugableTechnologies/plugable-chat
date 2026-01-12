import { useState, type RefObject, type KeyboardEvent } from 'react';
import { AttachmentMenu } from '../attachments';

interface InputBarProps {
    className?: string;
    input: string;
    setInput: (s: string) => void;
    handleSend: () => void;
    handleStop: () => void;
    handleKeyDown: (e: KeyboardEvent) => void;
    textareaRef: RefObject<HTMLTextAreaElement | null>;
    isLoading: boolean;
    attachedCount: number;
    onAttachFiles: () => void;
    onAttachFolder: () => void;
    onAttachDatabase: () => void;
    onAttachTool: () => void;
    onClearAttachments: () => void;
    filesDisabled?: boolean;
    dbDisabled?: boolean;
    disabled?: boolean;
}

/**
 * Input Bar Component - chat input with attachment support
 */
export const InputBar = ({
    className = "",
    input,
    setInput,
    handleSend,
    handleStop,
    handleKeyDown,
    textareaRef,
    isLoading,
    attachedCount,
    onAttachFiles,
    onAttachFolder,
    onAttachDatabase,
    onAttachTool,
    onClearAttachments,
    filesDisabled = false,
    dbDisabled = false,
    disabled = false
}: InputBarProps) => {
    const [menuOpen, setMenuOpen] = useState(false);
    const isMultiline = input.includes('\n') || (textareaRef.current && textareaRef.current.scrollHeight > 44);
    const hasAttachments = attachedCount > 0;
    const isDisabled = disabled || isLoading;

    return (
        <div className={`chat-input-shell w-full flex justify-center ${className}`}>
            <div className={`chat-input-surface flex items-center gap-3 w-full max-w-[900px] bg-[#f5f5f5] border border-transparent px-4 py-2.5 shadow-[0px_2px_8px_rgba(15,23,42,0.08)] focus-within:border-gray-300 transition-all ${isMultiline ? 'rounded-2xl' : 'rounded-full'}`}>
                <div className="chat-attachment-trigger relative">
                    <button
                        type="button"
                        onClick={() => setMenuOpen(!menuOpen)}
                        className={`chat-attach-button flex h-9 w-9 items-center justify-center rounded-full text-xl shadow-sm transition shrink-0 relative ${hasAttachments
                                ? 'bg-blue-500 text-white hover:bg-blue-600'
                                : 'bg-white text-gray-600 hover:bg-gray-100'
                            }`}
                        aria-label="Attach files"
                    >
                        +
                        {hasAttachments && (
                            <span className="absolute -top-1 -right-1 bg-blue-700 text-white text-[10px] font-bold rounded-full h-4 w-4 flex items-center justify-center">
                                {attachedCount}
                            </span>
                        )}
                    </button>
                    <AttachmentMenu
                        isOpen={menuOpen}
                        onClose={() => setMenuOpen(false)}
                        onSelectFiles={onAttachFiles}
                        onSelectFolder={onAttachFolder}
                        onSelectDatabase={onAttachDatabase}
                        onSelectTool={onAttachTool}
                        filesDisabled={filesDisabled}
                        dbDisabled={dbDisabled}
                    />
                </div>
                {hasAttachments && (
                    <button
                        onClick={onClearAttachments}
                        className="text-xs text-gray-500 hover:text-gray-700 underline"
                        title="Clear attachments"
                    >
                        Clear
                    </button>
                )}
                <textarea
                    ref={textareaRef}
                    className={`chat-input-textarea flex-1 bg-transparent text-gray-700 resize-none focus:outline-none focus:ring-0 focus:border-none max-h-[200px] overflow-y-auto placeholder:text-gray-400 font-normal text-[15px] leading-6 border-none py-1 ${disabled ? 'opacity-50 cursor-not-allowed' : ''}`}
                    rows={1}
                    value={input}
                    onChange={(e) => !disabled && setInput(e.target.value)}
                    onKeyDown={(e) => !disabled && handleKeyDown(e)}
                    placeholder={disabled ? "Response streaming in another chat..." : hasAttachments ? "Ask about your documents..." : "Ask anything"}
                    style={{ height: 'auto', minHeight: '32px' }}
                    disabled={disabled}
                />
                {isLoading && !disabled ? (
                    <button
                        onClick={handleStop}
                        className="h-9 w-9 flex items-center justify-center rounded-full text-base transition bg-red-500 text-white hover:bg-red-600 shrink-0"
                        aria-label="Stop generation"
                    >
                        ■
                    </button>
                ) : (
                    <button
                        onClick={handleSend}
                        className={`h-9 w-9 flex items-center justify-center rounded-full text-xl transition shrink-0 ${!isDisabled && input.trim() ? 'bg-gray-900 text-white hover:bg-gray-800' : 'bg-gray-300 text-gray-500 cursor-not-allowed'}`}
                        disabled={isDisabled || !input.trim()}
                        aria-label="Send message"
                    >
                        ↩
                    </button>
                )}
            </div>
        </div>
    );
};
