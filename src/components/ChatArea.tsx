import { useChatStore } from '../store/chat-store';
// Icons replaced with unicode characters
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import { invoke } from '../lib/api';
import { useEffect, useRef } from 'react';

// Helper to parse thinking blocks
const parseMessageContent = (content: string) => {
    const parts: { type: 'text' | 'think', content: string }[] = [];
    let current = content;

    while (current.length > 0) {
        const start = current.indexOf('<think>');
        if (start === -1) {
            if (current.trim()) parts.push({ type: 'text', content: current });
            break;
        }

        // Text before <think>
        if (start > 0) {
            parts.push({ type: 'text', content: current.substring(0, start) });
        }

        const rest = current.substring(start + 7); // 7 is length of <think>
        const end = rest.indexOf('</think>');

        if (end === -1) {
            // Unclosed think block (streaming)
            parts.push({ type: 'think', content: rest });
            break;
        }

        parts.push({ type: 'think', content: rest.substring(0, end) });
        current = rest.substring(end + 8); // 8 is length of </think>
    }
    return parts;
};

// Input Bar Component
const InputBar = ({
    className = "",
    input,
    setInput,
    handleSend,
    handleKeyDown,
    textareaRef
}: {
    className?: string,
    input: string,
    setInput: (s: string) => void,
    handleSend: () => void,
    handleKeyDown: (e: React.KeyboardEvent) => void,
    textareaRef: React.RefObject<HTMLTextAreaElement | null>
}) => (
    <div className={`w-full flex items-end gap-3 ${className}`}>
        {/* Input Field */}
        <div className="flex-1 bg-white rounded-3xl p-1 pl-5 pr-2 flex items-end gap-3 border border-gray-300 shadow-sm">
            <textarea
                ref={textareaRef}
                className="flex-1 bg-transparent text-gray-900 resize-none focus:outline-none max-h-[200px] py-3 min-h-[24px] overflow-y-auto scrollbar-hide placeholder:text-gray-400 font-normal text-[15px] leading-relaxed"
                rows={1}
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder={`Message Plugable Chat`}
            />
            <button
                onClick={handleSend}
                className={`px-5 py-2.5 rounded-full font-semibold text-sm transition-all transform duration-200 shrink-0 mb-0.5 ${input.trim() ? 'bg-gray-900 text-white hover:bg-gray-800' : 'bg-gray-200 text-gray-400 cursor-not-allowed'}`}
                disabled={!input.trim()}
            >
                ↑
            </button>
        </div>
    </div>
);


export function ChatArea() {
    const {
        messages, input, setInput, addMessage, isLoading, setIsLoading
    } = useChatStore();
    const textareaRef = useRef<HTMLTextAreaElement>(null);

    // Setup Streaming Listeners
    useEffect(() => {
        // Initialize listeners via the store
        useChatStore.getState().setupListeners();
    }, []);

    // Setup Streaming Listeners
    useEffect(() => {
        // Initialize listeners via the store
        useChatStore.getState().setupListeners();

        // Ideally we keep them alive, but if we want to be strict about cleanup:
        return () => {
            useChatStore.getState().cleanupListeners();
        };
    }, []);

    // Auto-resize textarea
    useEffect(() => {
        if (textareaRef.current) {
            textareaRef.current.style.height = 'auto';
            textareaRef.current.style.height = `${textareaRef.current.scrollHeight}px`;
        }
    }, [input]);

    const handleSend = async () => {
        const text = input;
        if (!text.trim()) return;

        // Add user message
        addMessage({ id: Date.now().toString(), role: 'user', content: text, timestamp: Date.now() });
        setInput('');
        if (textareaRef.current) textareaRef.current.style.height = 'auto';
        setIsLoading(true);

        // Add placeholder for assistant
        addMessage({
            id: (Date.now() + 1).toString(),
            role: 'assistant',
            content: '',
            timestamp: Date.now()
        });

        try {
            const history = messages.map(m => ({ role: m.role, content: m.content }));
            // Call backend - streaming will trigger events
            await invoke('chat', {
                message: text,
                history: history
            });
        } catch (error) {
            console.error('[ChatArea] Failed to send message:', error);
            // Update the last message with error
            useChatStore.setState((state) => {
                const newMessages = [...state.messages];
                const lastIdx = newMessages.length - 1;
                if (lastIdx >= 0) {
                    newMessages[lastIdx] = {
                        ...newMessages[lastIdx],
                        content: `Error: ${error}`
                    };
                }
                return { messages: newMessages };
            });
            setIsLoading(false);
        }
    };

    const handleKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            handleSend();
        }
    };

    return (
        <div className="h-full w-full flex flex-col bg-white text-gray-800 font-sans relative overflow-hidden">
            {/* Scrollable Messages Area - takes all remaining space */}
            <div className="flex-1 min-h-0 w-full overflow-y-auto flex flex-col px-2 sm:px-6 pt-6 pb-6">
                {messages.length === 0 ? (
                    <div className="flex-1 flex flex-col items-center justify-center px-6">
                        <div className="mb-8 text-center">
                            <h1 className="text-2xl font-bold text-gray-900">How can I help you today?</h1>
                        </div>
                    </div>
                ) : (
                    <div className="w-full max-w-none space-y-6 py-0">
                        {messages.map(m => (
                            <div key={m.id} className={`flex w-full ${m.role === 'user' ? 'justify-end' : 'justify-start'}`}>
                                <div
                                    className={`
                                    relative w-full max-w-none rounded-2xl px-5 py-3.5 text-[15px] leading-7
                                    ${m.role === 'user'
                                            ? 'bg-gray-100 text-gray-900'
                                            : 'bg-gray-50 text-gray-900'
                                        }
                                `}
                                >
                                    <div className="prose prose-slate max-w-none break-words text-gray-900">
                                        {m.role === 'assistant' ? (
                                            parseMessageContent(m.content).map((part, idx) => (
                                                part.type === 'think' ? (
                                                    <details key={idx} className="mb-4 group">
                                                        <summary className="cursor-pointer text-xs font-medium text-gray-500 hover:text-gray-700 select-none flex items-center gap-2 mb-2">
                                                            <span className="uppercase tracking-wider text-gray-500">Thought Process</span>
                                                            <span className="h-px flex-1 bg-gray-300 group-open:bg-gray-400 transition-colors"></span>
                                                            <span className="text-sm group-open:rotate-180 transition-transform inline-block">▼</span>
                                                        </summary>
                                                        <div className="pl-3 border-l-2 border-gray-400 text-gray-600 text-sm italic bg-gray-100 p-3 rounded-r-lg">
                                                            {part.content || "Thinking..."}
                                                        </div>
                                                    </details>
                                                ) : (
                                                    <ReactMarkdown
                                                        key={idx}
                                                        remarkPlugins={[remarkGfm, remarkMath]}
                                                        rehypePlugins={[rehypeKatex]}
                                                        components={{
                                                            code({ node, inline, className, children, ...props }: any) {
                                                                const match = /language-(\w+)/.exec(className || '')
                                                                const codeContent = String(children).replace(/\n$/, '');

                                                                return !inline && match ? (
                                                                    <div className="my-4 rounded-xl overflow-hidden border border-gray-200 bg-gray-50 shadow-sm group/code">
                                                                        <div className="flex justify-between items-center bg-gray-100 px-3 py-2 border-b border-gray-200">
                                                                            <span className="text-xs text-gray-600 font-mono font-medium">{match[1]}</span>
                                                                            <button
                                                                                onClick={() => navigator.clipboard.writeText(codeContent)}
                                                                                className="text-xs text-gray-600 hover:text-gray-900 transition-colors px-2 py-1 hover:bg-gray-200 rounded opacity-0 group-hover/code:opacity-100"
                                                                            >
                                                                                📋
                                                                            </button>
                                                                        </div>
                                                                        <div className="bg-white p-4 overflow-x-auto text-sm">
                                                                            <code className={className} {...props}>
                                                                                {children}
                                                                            </code>
                                                                        </div>
                                                                    </div>
                                                                ) : (
                                                                    <code className={`${className} bg-gray-200 px-1.5 py-0.5 rounded text-[13px] text-gray-900 font-mono`} {...props}>
                                                                        {children}
                                                                    </code>
                                                                )
                                                            }
                                                        }}
                                                    >
                                                        {part.content}
                                                    </ReactMarkdown>
                                                )
                                            ))
                                        ) : (
                                            <div className="whitespace-pre-wrap">{m.content}</div>
                                        )}
                                    </div>
                                </div>
                            </div>
                        ))}
                        {isLoading && messages[messages.length - 1]?.role !== 'assistant' && (
                            <div className="flex w-full justify-start">
                                <div className="bg-gray-50 rounded-2xl px-6 py-4">
                                    <div className="flex gap-1.5">
                                        <div className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce" style={{ animationDelay: '0ms' }} />
                                        <div className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce" style={{ animationDelay: '150ms' }} />
                                        <div className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce" style={{ animationDelay: '300ms' }} />
                                    </div>
                                </div>
                            </div>
                        )}
                    </div>
                )}
            </div>

            {/* Fixed Input Area at Bottom */}
            <div className="flex-shrink-0 mt-1 pb-4">
                <div className="px-2 sm:px-6">
                    <InputBar
                        className=""
                        input={input}
                        setInput={setInput}
                        handleSend={handleSend}
                        handleKeyDown={handleKeyDown}
                        textareaRef={textareaRef}
                    />
                    <div className="text-center text-xs text-gray-500 mt-3 font-normal">
                        Plugable Chat can make mistakes. Check important info.
                    </div>
                </div>
            </div>
        </div>
    )
}
