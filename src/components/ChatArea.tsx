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
    <div className={`w-full flex justify-center ${className}`}>
        <div className="flex items-center gap-3 w-full max-w-[900px] bg-[#f5f5f5] border border-transparent rounded-full px-3 py-2 shadow-[0px_2px_8px_rgba(15,23,42,0.08)] focus-within:border-gray-300 transition-all">
            <button
                type="button"
                className="flex h-9 w-9 items-center justify-center rounded-full bg-white text-gray-600 shadow-sm hover:bg-gray-100 transition"
                aria-label="Start new request"
            >
                +
            </button>
            <textarea
                ref={textareaRef}
                className="flex-1 bg-transparent text-gray-700 resize-none focus:outline-none focus:ring-0 focus:border-none max-h-[280px] min-h-[72px] overflow-y-auto scrollbar-hide placeholder:text-gray-400 font-normal text-[15px] leading-relaxed border-none"
                rows={3}
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder="Ask anything"
            />
            <button
                onClick={handleSend}
                className={`h-9 w-9 flex items-center justify-center rounded-full text-lg transition ${input.trim() ? 'bg-gray-900 text-white hover:bg-gray-800' : 'bg-gray-300 text-gray-500 cursor-not-allowed'}`}
                disabled={!input.trim()}
                aria-label="Send message"
            >
                ↩
            </button>
        </div>
    </div>
);


const generateClientChatId = () => {
    const cryptoObj = typeof globalThis !== 'undefined' ? globalThis.crypto : undefined;
    if (cryptoObj && typeof cryptoObj.randomUUID === 'function') {
        return cryptoObj.randomUUID();
    }
    return `chat-${Date.now()}-${Math.floor(Math.random() * 1000)}`;
};

const createChatTitleFromPrompt = (prompt: string) => {
    const cleaned = prompt.trim().replace(/\s+/g, ' ');
    if (!cleaned) {
        return "Untitled Chat";
    }
    const sentenceEnd = cleaned.search(/[.!?]/);
    const base = sentenceEnd > 0 ? cleaned.substring(0, sentenceEnd).trim() : cleaned;
    if (base.length <= 40) {
        return base;
    }
    return `${base.substring(0, 37).trim()}...`;
};

const createChatPreviewFromMessage = (message: string) => {
    const cleaned = message.trim().replace(/\s+/g, ' ');
    if (!cleaned) return "";
    if (cleaned.length <= 80) {
        return cleaned;
    }
    return `${cleaned.substring(0, 77)}...`;
};

// Helper to wrap raw \boxed{} in math delimiters to ensure they render
const preprocessLaTeX = (content: string) => {
    let result = '';
    let i = 0;

    // States
    let inMath: false | '$' | '$$' = false;
    let inCode: false | '`' | '```' = false;

    while (i < content.length) {
        // 1. Handle Code Blocks
        if (!inMath && !inCode && content.startsWith('```', i)) {
            inCode = '```';
            result += '```';
            i += 3;
            continue;
        }
        if (!inMath && inCode === '```' && content.startsWith('```', i)) {
            inCode = false;
            result += '```';
            i += 3;
            continue;
        }

        // 2. Handle Inline Code
        if (!inMath && !inCode && content.startsWith('`', i)) {
            inCode = '`';
            result += '`';
            i += 1;
            continue;
        }
        if (!inMath && inCode === '`' && content.startsWith('`', i)) {
            inCode = false;
            result += '`';
            i += 1;
            continue;
        }

        // If in code, just consume
        if (inCode) {
            result += content[i];
            i++;
            continue;
        }

        // 3. Handle Math Delimiters
        // Escaped dollar? \$
        if (content.startsWith('\\$', i)) {
            result += '\\$';
            i += 2;
            continue;
        }

        if (content.startsWith('$$', i)) {
            if (inMath === '$$') inMath = false;
            else if (!inMath) inMath = '$$';
            result += '$$';
            i += 2;
            continue;
        }
        if (content.startsWith('$', i)) {
            if (inMath === '$') inMath = false;
            else if (!inMath) inMath = '$';
            result += '$';
            i += 1;
            continue;
        }

        // 4. Handle \boxed{
        if (!inMath && content.startsWith('\\boxed{', i)) {
            // Look ahead to find matching brace
            let braceCount = 1;
            let ptr = i + 7; // skip \boxed{

            while (ptr < content.length && braceCount > 0) {
                if (content[ptr] === '\\') {
                    ptr += 2; // skip escaped char
                    continue;
                }
                if (content[ptr] === '{') braceCount++;
                if (content[ptr] === '}') braceCount--;
                ptr++;
            }

            if (braceCount === 0) {
                // Found complete block
                const original = content.substring(i, ptr);
                result += '$' + original + '$';
                i = ptr;
                continue;
            }
            // If not found (unclosed), fall through to default char handling
        }

        result += content[i];
        i++;
    }
    return result;
};

export function ChatArea() {
    const {
        messages, input, setInput, addMessage, isLoading, setIsLoading, currentChatId, reasoningEffort
    } = useChatStore();
    const textareaRef = useRef<HTMLTextAreaElement>(null);
    const messagesEndRef = useRef<HTMLDivElement>(null);

    // Auto-scroll to bottom
    useEffect(() => {
        messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
    }, [messages, isLoading]);

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
        const trimmedText = text.trim();

        const storeState = useChatStore.getState();
        let chatId = currentChatId;
        const isNewChat = !chatId;
        if (isNewChat) {
            chatId = generateClientChatId();
            storeState.setCurrentChatId(chatId);
        }

        const existingSummary = storeState.history.find((chat) => chat.id === chatId);
        const derivedTitle = existingSummary?.title ?? createChatTitleFromPrompt(trimmedText);
        const preview = createChatPreviewFromMessage(trimmedText);
        const summaryScore = existingSummary?.score ?? 0;
        const summaryPinned = existingSummary?.pinned ?? false;

        storeState.upsertHistoryEntry({
            id: chatId,
            title: derivedTitle,
            preview,
            score: summaryScore,
            pinned: summaryPinned
        });

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
            const returnedChatId = await invoke<string>('chat', {
                chatId,
                title: isNewChat ? derivedTitle : undefined,
                message: text,
                history: history,
                reasoningEffort
            });

            if (returnedChatId && returnedChatId !== chatId) {
                storeState.setCurrentChatId(returnedChatId);
                storeState.upsertHistoryEntry({
                    id: returnedChatId,
                    title: derivedTitle,
                    preview,
                    score: summaryScore,
                    pinned: summaryPinned
                });
            }

            storeState.fetchHistory();
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
        <div className="flex-1 min-h-0 w-full overflow-y-auto flex flex-col px-4 sm:px-6 pt-6 pb-6">
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
                                                        {preprocessLaTeX(part.content)}
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
                        <div ref={messagesEndRef} />
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
                </div>
            </div>
        </div>
    )
}
