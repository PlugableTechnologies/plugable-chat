import { useChatStore } from '../store/chat-store';
import { Code, ChevronDown, Check } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
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

// Input Bar Component
const InputBar = ({ 
    className = "",
    input,
    setInput,
    handleSend,
    handleKeyDown,
    textareaRef,
    currentModel
}: { 
    className?: string,
    input: string,
    setInput: (s: string) => void,
    handleSend: () => void,
    handleKeyDown: (e: React.KeyboardEvent) => void,
    textareaRef: React.RefObject<HTMLTextAreaElement | null>,
    currentModel: string
}) => (
    <div className={`max-w-3xl mx-auto bg-[#f0fdf4] rounded-2xl p-2 flex items-center gap-3 transition-all duration-200 ${className}`}>
        <button className="w-10 h-10 flex items-center justify-center text-gray-400 hover:text-gray-600 hover:bg-black/5 rounded-xl transition-all shrink-0 text-xl">
            📎
        </button>
        <textarea 
            ref={textareaRef}
            className="flex-1 bg-transparent text-gray-800 resize-none focus:outline-none max-h-[200px] py-3 min-h-[24px] overflow-y-auto scrollbar-hide placeholder:text-gray-400 font-medium text-base leading-relaxed"
            rows={1}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={`Message ${currentModel}`}
        />
        <button 
            onClick={handleSend}
            className={`w-10 h-10 flex items-center justify-center rounded-xl transition-all transform duration-200 shrink-0 ${input.trim() ? 'bg-black text-white hover:bg-gray-800 scale-100 shadow-md' : 'bg-transparent text-gray-300 scale-95 cursor-not-allowed'}`}
            disabled={!input.trim()}
        >
            <span className="text-sm font-bold mt-0.5">➤</span>
        </button>
    </div>
);

export function ChatArea() {
  const { 
      messages, input, setInput, addMessage, isLoading, setIsLoading, toggleCodeEditor,
      availableModels, currentModel, fetchModels, setModel 
  } = useChatStore();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [isModelMenuOpen, setIsModelMenuOpen] = useState(false);

  // Fetch models on mount
  useEffect(() => {
      fetchModels();
  }, []);

  // Setup Streaming Listeners
  useEffect(() => {
      // Initialize listeners via the store
      useChatStore.getState().setupListeners();
      
      // Ideally we keep them alive, but if we want to be strict about cleanup:
      return () => {
         // useChatStore.getState().cleanupListeners(); 
         // Commenting out cleanup to persist listeners across re-renders if needed, 
         // but standard practice is to clean up. 
         // Given the singleton check in setupListeners, we can safely leave them 
         // or clean them up. Let's clean up to be safe.
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
    <div className="flex-1 flex flex-col bg-[#ffffff] text-gray-900 h-full relative min-w-0 font-sans">
        {/* Top Header */}
        <div className="sticky top-0 z-10 bg-white/80 backdrop-blur-md p-3 flex items-center justify-between px-6 border-b border-gray-50/50">
            <div className="relative">
                <button 
                    onClick={() => setIsModelMenuOpen(!isModelMenuOpen)}
                    className="flex items-center gap-2 text-sm font-semibold text-gray-700 hover:bg-gray-100 px-3 py-1.5 rounded-lg transition-colors"
                >
                    {currentModel} <ChevronDown size={14} className="text-gray-400" />
                </button>
                
                {isModelMenuOpen && (
                    <>
                        <div className="fixed inset-0 z-10" onClick={() => setIsModelMenuOpen(false)} />
                        <div className="absolute top-full left-0 mt-2 w-64 bg-white rounded-xl shadow-xl border border-gray-100 py-1 z-20 overflow-hidden ring-1 ring-black/5">
                            <div className="px-3 py-2 text-[10px] font-bold text-gray-400 uppercase tracking-wider">
                                Model
                            </div>
                            {availableModels.length > 0 ? (
                                availableModels.map((model) => (
                                    <button
                                        key={model}
                                        onClick={() => {
                                            setModel(model);
                                            setIsModelMenuOpen(false);
                                        }}
                                        className="w-full text-left px-3 py-2 hover:bg-gray-50 flex items-center justify-between group"
                                    >
                                        <span className={`text-sm ${currentModel === model ? 'text-gray-900 font-medium' : 'text-gray-600'}`}>
                                            {model}
                                        </span>
                                        {currentModel === model && <Check size={14} className="text-gray-900" />}
                                    </button>
                                ))
                            ) : (
                                <div className="px-3 py-2 text-sm text-gray-500">No models found</div>
                            )}
                        </div>
                    </>
                )}
            </div>
        </div>

        {/* Content Area */}
        <div className="flex-1 flex flex-col min-h-0 relative overflow-hidden">
            {/* Scrollable Messages */}
            <div className="flex-1 overflow-y-auto p-4 scroll-smooth">
                {messages.length === 0 ? (
                    <div className="h-full flex flex-col items-center justify-center pb-20">
                        <div className="mb-8 text-center">
                            <div className="w-16 h-16 bg-gradient-to-tr from-gray-900 to-gray-700 rounded-2xl flex items-center justify-center mb-6 mx-auto shadow-xl shadow-gray-200">
                                <div className="w-8 h-8 bg-white rounded-full opacity-90" />
                            </div>
                            <h1 className="text-2xl font-medium text-gray-900">How can I help you today?</h1>
                        </div>
                    </div>
                ) : (
                    <div className="max-w-3xl mx-auto w-full space-y-6 py-4 pb-8">
                        {messages.map(m => (
                            <div key={m.id} className={`flex w-full ${m.role === 'user' ? 'justify-end' : 'justify-start'}`}>
                                <div 
                                    className={`
                                        relative max-w-[85%] rounded-2xl px-6 py-4 text-[15px] leading-7 shadow-sm
                                        ${m.role === 'user' 
                                            ? 'bg-[#f0f2f5] text-gray-900' 
                                            : 'bg-white text-gray-900'
                                        }
                                    `}
                                >
                                    {/* Removed User/Assistant Avatars for cleaner look */}
                                    
                                    <div className="prose prose-slate max-w-none break-words">
                                        {m.role === 'assistant' ? (
                                            parseMessageContent(m.content).map((part, idx) => (
                                                part.type === 'think' ? (
                                                    <details key={idx} className="mb-4 group">
                                                        <summary className="cursor-pointer text-xs font-medium text-gray-400 hover:text-gray-600 select-none flex items-center gap-2 mb-2">
                                                            <span className="uppercase tracking-wider">Thought Process</span>
                                                            <span className="h-px flex-1 bg-gray-100 group-open:bg-gray-200 transition-colors"></span>
                                                            <ChevronDown size={12} className="group-open:rotate-180 transition-transform" />
                                                        </summary>
                                                        <div className="pl-3 border-l-2 border-gray-100 text-gray-500 text-sm italic bg-gray-50/50 p-3 rounded-r-lg">
                                                            {part.content || "Thinking..."}
                                                        </div>
                                                    </details>
                                                ) : (
                                                    <ReactMarkdown 
                                                        key={idx}
                                                        remarkPlugins={[remarkGfm]} 
                                                        rehypePlugins={[rehypeKatex]}
                                                        components={{
                                                            code({node, inline, className, children, ...props}: any) {
                                                                const match = /language-(\w+)/.exec(className || '')
                                                                return !inline && match ? (
                                                                    <div className="my-4 rounded-xl overflow-hidden border border-gray-200 bg-gray-50 shadow-sm">
                                                                        <div className="flex justify-between items-center bg-gray-50/80 px-3 py-2 border-b border-gray-200/50 backdrop-blur-sm">
                                                                            <span className="text-xs text-gray-500 font-mono font-medium">{match[1]}</span>
                                                                        </div>
                                                                        <div className="bg-[#1e1e1e] p-4 overflow-x-auto text-sm">
                                                                            <code className={className} {...props}>
                                                                                {children}
                                                                            </code>
                                                                        </div>
                                                                    </div>
                                                                ) : (
                                                                    <code className={`${className} bg-gray-100 px-1.5 py-0.5 rounded text-[13px] text-gray-800 font-mono border border-gray-200/50`} {...props}>
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
                                 <div className="bg-white rounded-2xl px-6 py-4 shadow-sm border border-gray-50">
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
            
            {/* Input Area */}
            <div className="w-full bg-white/80 backdrop-blur-sm p-4 pb-6 z-20 border-t border-gray-50">
                <InputBar 
                    className="" 
                    input={input}
                    setInput={setInput}
                    handleSend={handleSend}
                    handleKeyDown={handleKeyDown}
                    textareaRef={textareaRef}
                    currentModel={currentModel}
                />
                <div className="text-center text-[10px] text-gray-400 mt-3 font-medium">
                    Plugable Chat can make mistakes. Check important info.
                </div>
            </div>
        </div>
    </div>
  )
}
