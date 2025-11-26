import { useChatStore } from '../store/chat-store';
// Icons replaced with unicode characters
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import rehypeRaw from 'rehype-raw';
import 'katex/dist/katex.min.css';
import { invoke } from '../lib/api';
import { useEffect, useRef, useState } from 'react';

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

// Check if message has only think content (no visible text)
const hasOnlyThinkContent = (content: string): boolean => {
    const parts = parseMessageContent(content);
    const textParts = parts.filter(p => p.type === 'text');
    const thinkParts = parts.filter(p => p.type === 'think');
    // Has think content but no meaningful visible text
    return thinkParts.length > 0 && textParts.every(p => !p.content.trim());
};

// Thinking indicator component with elapsed time
const ThinkingIndicator = ({ startTime }: { startTime: number }) => {
    const [elapsed, setElapsed] = useState(0);
    
    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    // Format elapsed time
    const formatTime = (seconds: number) => {
        if (seconds < 60) return `${seconds}s`;
        const mins = Math.floor(seconds / 60);
        const secs = seconds % 60;
        return `${mins}m ${secs}s`;
    };

    return (
        <div className="flex items-center gap-2 text-xs text-gray-500 mt-2 mb-1">
            <div className="flex gap-1">
                <div className="w-1.5 h-1.5 bg-amber-400 rounded-full animate-pulse" />
                <div className="w-1.5 h-1.5 bg-amber-400 rounded-full animate-pulse" style={{ animationDelay: '300ms' }} />
                <div className="w-1.5 h-1.5 bg-amber-400 rounded-full animate-pulse" style={{ animationDelay: '600ms' }} />
            </div>
            <span className="font-medium text-gray-500">
                Reasoning{elapsed >= 1 ? ` · ${formatTime(elapsed)}` : '...'}
            </span>
        </div>
    );
};

// Input Bar Component
const InputBar = ({
    className = "",
    input,
    setInput,
    handleSend,
    handleStop,
    handleKeyDown,
    textareaRef,
    isLoading
}: {
    className?: string,
    input: string,
    setInput: (s: string) => void,
    handleSend: () => void,
    handleStop: () => void,
    handleKeyDown: (e: React.KeyboardEvent) => void,
    textareaRef: React.RefObject<HTMLTextAreaElement | null>,
    isLoading: boolean
}) => {
    const isMultiline = input.includes('\n') || (textareaRef.current && textareaRef.current.scrollHeight > 44);
    
    return (
        <div className={`w-full flex justify-center ${className}`}>
            <div className={`flex items-center gap-3 w-full max-w-[900px] bg-[#f5f5f5] border border-transparent px-4 py-2.5 shadow-[0px_2px_8px_rgba(15,23,42,0.08)] focus-within:border-gray-300 transition-all ${isMultiline ? 'rounded-2xl' : 'rounded-full'}`}>
                <button
                    type="button"
                    className="flex h-9 w-9 items-center justify-center rounded-full bg-white text-gray-600 text-xl shadow-sm hover:bg-gray-100 transition shrink-0"
                    aria-label="Start new request"
                >
                    +
                </button>
                <textarea
                    ref={textareaRef}
                    className="flex-1 bg-transparent text-gray-700 resize-none focus:outline-none focus:ring-0 focus:border-none max-h-[200px] overflow-y-auto placeholder:text-gray-400 font-normal text-[15px] leading-6 border-none py-1"
                    rows={1}
                    value={input}
                    onChange={(e) => setInput(e.target.value)}
                    onKeyDown={handleKeyDown}
                    placeholder="Ask anything"
                    style={{ height: 'auto', minHeight: '32px' }}
                />
                {isLoading ? (
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
                        className={`h-9 w-9 flex items-center justify-center rounded-full text-xl transition shrink-0 ${input.trim() ? 'bg-gray-900 text-white hover:bg-gray-800' : 'bg-gray-300 text-gray-500 cursor-not-allowed'}`}
                        disabled={!input.trim()}
                        aria-label="Send message"
                    >
                        ↩
                    </button>
                )}
            </div>
        </div>
    );
};


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
                // Found complete block - extract the inner content
                const innerContent = content.substring(i + 7, ptr - 1);
                
                // Check if content contains LaTeX commands like \text{}, \mathbf{}, etc.
                const hasLatexCommands = /\\[a-zA-Z]+\{/.test(innerContent);
                
                // Check if content looks like plain prose text (has spaces, no math operators, no LaTeX commands)
                const looksLikePlainText = !hasLatexCommands && 
                    innerContent.includes(' ') && 
                    !/[+\-*/=^_{}\\]/.test(innerContent);
                
                if (looksLikePlainText) {
                    // For plain text content (no LaTeX commands), use HTML box
                    result += '<div style="border: 2px solid #2e2e2e; padding: 0.5em 0.75em; border-radius: 6px; margin: 0.5em 0; display: inline-block; max-width: 100%; word-wrap: break-word;">' + innerContent + '</div>';
                } else {
                    // Content has LaTeX commands or math - let KaTeX handle it
                    result += '$\\boxed{' + innerContent + '}$';
                }
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
        messages, input, setInput, addMessage, isLoading, setIsLoading, stopGeneration, currentChatId, reasoningEffort,
        triggerRelevanceSearch, clearRelevanceSearch, isConnecting
    } = useChatStore();
    const textareaRef = useRef<HTMLTextAreaElement>(null);
    const messagesEndRef = useRef<HTMLDivElement>(null);
    const [thinkingStartTime, setThinkingStartTime] = useState<number | null>(null);

    // Track when thinking phase starts
    useEffect(() => {
        const lastMessage = messages[messages.length - 1];
        const isThinkingOnly = lastMessage?.role === 'assistant' && 
                               hasOnlyThinkContent(lastMessage.content) && 
                               isLoading;
        
        if (isThinkingOnly && !thinkingStartTime) {
            setThinkingStartTime(Date.now());
        } else if (!isLoading || (lastMessage?.role === 'assistant' && !hasOnlyThinkContent(lastMessage.content))) {
            setThinkingStartTime(null);
        }
    }, [messages, isLoading, thinkingStartTime]);

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

    // Auto-resize textarea (ChatGPT-style: starts compact, grows as you type)
    useEffect(() => {
        if (textareaRef.current) {
            // Reset height to auto to get accurate scrollHeight
            textareaRef.current.style.height = 'auto';
            // Set height to scrollHeight, capped by CSS max-height
            const newHeight = Math.max(32, Math.min(textareaRef.current.scrollHeight, 200));
            textareaRef.current.style.height = `${newHeight}px`;
        }
    }, [input]);

    // Trigger relevance search as user types (debounced in store)
    useEffect(() => {
        if (input.trim().length >= 3) {
            triggerRelevanceSearch(input);
        } else {
            clearRelevanceSearch();
        }
    }, [input, triggerRelevanceSearch, clearRelevanceSearch]);

    const handleSend = async () => {
        const text = input;
        if (!text.trim()) return;
        const trimmedText = text.trim();

        const storeState = useChatStore.getState();
        const isNewChat = !currentChatId;
        const chatId = isNewChat ? generateClientChatId() : currentChatId!;
        if (isNewChat) {
            storeState.setCurrentChatId(chatId);
            if (storeState.currentModel === 'Loading...') {
                try {
                    await storeState.fetchModels();
                } catch (error) {
                    console.error('[ChatArea] Failed to refresh models before sending new chat:', error);
                }
            }
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
        clearRelevanceSearch(); // Clear relevance results when sending
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
    <div className="h-full w-full flex flex-col text-gray-800 font-sans relative overflow-hidden">
        {/* Scrollable Messages Area - takes all remaining space */}
        <div className="flex-1 min-h-0 w-full overflow-y-auto flex flex-col px-4 sm:px-6 pt-6 pb-6">
                {messages.length === 0 ? (
                    <div className="flex-1 flex flex-col items-center justify-center px-6">
                        <div className="mb-8 text-center">
                            <h1 className="text-2xl font-bold text-gray-900">
                                {isConnecting ? "Wait, Loading Local Models ..." : "How can I help you today?"}
                            </h1>
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
                                            <>
                                                {parseMessageContent(m.content).map((part, idx) => (
                                                    part.type === 'think' ? (
                                                        <details key={idx} className="mb-4 group">
                                                            <summary className="cursor-pointer text-xs font-medium text-gray-400 hover:text-gray-600 select-none flex items-center gap-2 mb-2">
                                                                <span className="h-px flex-1 bg-gray-200 group-open:bg-gray-300 transition-colors"></span>
                                                                <span className="text-sm group-open:rotate-180 transition-transform inline-block">▼</span>
                                                            </summary>
                                                            <div className="pl-3 border-l-2 border-gray-300 text-gray-600 text-sm italic bg-gray-50 p-3 rounded-r-lg">
                                                                {part.content || "Thinking..."}
                                                            </div>
                                                        </details>
                                                    ) : (
                                                        <ReactMarkdown
                                                            key={idx}
                                                            remarkPlugins={[remarkGfm, remarkMath]}
                                                            rehypePlugins={[rehypeRaw, rehypeKatex]}
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
                                                ))}
                                                {/* Show thinking indicator when only think content is visible */}
                                                {thinkingStartTime && 
                                                 messages[messages.length - 1]?.id === m.id && 
                                                 hasOnlyThinkContent(m.content) && (
                                                    <ThinkingIndicator startTime={thinkingStartTime} />
                                                )}
                                            </>
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
                        handleStop={stopGeneration}
                        handleKeyDown={handleKeyDown}
                        textareaRef={textareaRef}
                        isLoading={isLoading}
                    />
                </div>
            </div>
        </div>
    )
}
