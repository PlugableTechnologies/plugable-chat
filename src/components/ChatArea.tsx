import { useChatStore } from '../store/chat-store';
import { ChevronDown, Plus } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
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
    textareaRef,
    onNewChat
}: {
    className?: string,
    input: string,
    setInput: (s: string) => void,
    handleSend: () => void,
    handleKeyDown: (e: React.KeyboardEvent) => void,
    textareaRef: React.RefObject<HTMLTextAreaElement | null>,
    onNewChat: () => void
}) => (
    <div className={`w-full flex items-end gap-3 ${className}`}>
        {/* New Chat Button */}
        <button
            onClick={onNewChat}
            className="flex items-center gap-2 text-slate-400 hover:text-white transition-colors pb-3 shrink-0 group"
        >
            <div className="w-9 h-9 rounded-full border-2 border-slate-700 flex items-center justify-center group-hover:border-slate-400 transition-colors">
                <Plus size={18} />
            </div>
            <span className="font-medium text-sm">New Chat</span>
        </button>

        {/* Input Field */}
        <div className="flex-1 bg-[#1a1f26] rounded-3xl p-1 pl-5 pr-2 flex items-end gap-3 border border-transparent shadow-[0_15px_40px_rgba(1,9,22,0.45)]">
            <textarea
                ref={textareaRef}
                className="flex-1 bg-transparent text-slate-100 resize-none focus:outline-none max-h-[200px] py-3 min-h-[24px] overflow-y-auto scrollbar-hide placeholder:text-slate-500 font-normal text-[15px] leading-relaxed"
                rows={1}
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder={`Message Plugable Chat`}
            />
            <button
                onClick={handleSend}
                className={`px-5 py-2.5 rounded-full font-semibold text-sm transition-all transform duration-200 shrink-0 mb-0.5 ${input.trim() ? 'bg-cyan-400 text-black hover:bg-cyan-300 shadow-lg shadow-cyan-400/20' : 'bg-slate-700 text-slate-500 cursor-not-allowed'}`}
                disabled={!input.trim()}
            >
                Send
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
        <div className="h-full flex-1 w-full flex flex-col bg-[#0f1419] text-slate-200 font-sans relative overflow-hidden">
            {/* Scrollable Messages Area - takes all remaining space */}
            <div className="flex-1 min-h-0 flex flex-col">
                <div className="flex-1 min-h-0 w-full overflow-y-auto flex flex-col px-2 sm:px-6 pt-6 pb-6">
                    {messages.length === 0 ? (
                        <div className="flex-1 flex flex-col items-center justify-center px-6">
                            <div className="mb-8 text-center">
                                <div className="w-16 h-16 bg-gradient-to-tr from-cyan-500 to-blue-500 rounded-2xl flex items-center justify-center mb-6 mx-auto shadow-xl shadow-cyan-500/20">
                                    <div className="w-8 h-8 bg-white rounded-full opacity-90" />
                                </div>
                                <h1 className="text-2xl font-bold text-white">How can I help you today?</h1>
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
                                                ? 'bg-[#1a1f26] text-slate-200 border border-transparent shadow-[0_10px_30px_rgba(2,6,23,0.45)]'
                                                : 'bg-transparent text-slate-200'
                                            }
                                    `}
                                    >
                                        <div className="prose prose-slate max-w-none break-words text-slate-200">
                                            {m.role === 'assistant' ? (
                                                parseMessageContent(m.content).map((part, idx) => (
                                                    part.type === 'think' ? (
                                                        <details key={idx} className="mb-4 group">
                                                            <summary className="cursor-pointer text-xs font-medium text-slate-400 hover:text-slate-200 select-none flex items-center gap-2 mb-2">
                                                                <span className="uppercase tracking-wider text-slate-400">Thought Process</span>
                                                                <span className="h-px flex-1 bg-white/10 group-open:bg-white/20 transition-colors"></span>
                                                                <ChevronDown size={12} className="group-open:rotate-180 transition-transform" />
                                                            </summary>
                                                            <div className="pl-3 border-l-2 border-cyan-500/60 text-slate-400 text-sm italic bg-white/5 p-3 rounded-r-lg">
                                                                {part.content || "Thinking..."}
                                                            </div>
                                                        </details>
                                                    ) : (
                                                        <ReactMarkdown
                                                            key={idx}
                                                            remarkPlugins={[remarkGfm]}
                                                            rehypePlugins={[rehypeKatex]}
                                                            components={{
                                                                code({ node, inline, className, children, ...props }: any) {
                                                                    const match = /language-(\w+)/.exec(className || '')
                                                                    const codeContent = String(children).replace(/\n$/, '');

                                                                    return !inline && match ? (
                                                                        <div className="my-4 rounded-xl overflow-hidden border border-white/10 bg-[#0d1117] shadow-sm group/code">
                                                                            <div className="flex justify-between items-center bg-[#161b22] px-3 py-2 border-b border-white/10 backdrop-blur-sm">
                                                                                <span className="text-xs text-slate-400 font-mono font-medium">{match[1]}</span>
                                                                                <button
                                                                                    onClick={() => navigator.clipboard.writeText(codeContent)}
                                                                                    className="text-xs text-slate-400 hover:text-white transition-colors px-2 py-1 hover:bg-white/5 rounded opacity-0 group-hover/code:opacity-100"
                                                                                >
                                                                                    Copy
                                                                                </button>
                                                                            </div>
                                                                            <div className="bg-[#0d1117] p-4 overflow-x-auto text-sm">
                                                                                <code className={className} {...props}>
                                                                                    {children}
                                                                                </code>
                                                                            </div>
                                                                        </div>
                                                                    ) : (
                                                                        <code className={`${className} bg-white/10 px-1.5 py-0.5 rounded text-[13px] text-cyan-300 font-mono border border-white/10`} {...props}>
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
                                    <div className="bg-[#171d24] rounded-2xl px-6 py-4 border border-transparent">
                                        <div className="flex gap-1.5">
                                            <div className="w-1.5 h-1.5 bg-cyan-400 rounded-full animate-bounce" style={{ animationDelay: '0ms' }} />
                                            <div className="w-1.5 h-1.5 bg-cyan-400 rounded-full animate-bounce" style={{ animationDelay: '150ms' }} />
                                            <div className="w-1.5 h-1.5 bg-cyan-400 rounded-full animate-bounce" style={{ animationDelay: '300ms' }} />
                                        </div>
                                    </div>
                                </div>
                            )}
                        </div>
                    )}
                </div>
            </div>

            {/* Fixed Input Area at Bottom */}
            <div className="flex-shrink-0 mt-1">
                <div className="px-2 sm:px-6">
                    <InputBar
                        className=""
                        input={input}
                        setInput={setInput}
                        handleSend={handleSend}
                        handleKeyDown={handleKeyDown}
                        textareaRef={textareaRef}
                        onNewChat={() => {
                            useChatStore.setState({ messages: [] });
                            setInput('');
                        }}
                    />
                    <div className="text-center text-xs text-slate-400 mt-3 font-normal">
                        Plugable Chat can make mistakes. Check important info.
                    </div>
                </div>
            </div>
        </div>
    )
}
