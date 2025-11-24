import { useChatStore } from '../store/chat-store';
import { Send, Code } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeKatex from 'rehype-katex';
import 'katex/dist/katex.min.css';
import { invoke } from '@tauri-apps/api/core';

export function ChatArea() {
  const { messages, input, setInput, addMessage, isLoading, setIsLoading, toggleCodeEditor } = useChatStore();

  const handleSend = async () => {
      const text = input;
      if (!text.trim()) return;
      
      addMessage({ id: Date.now().toString(), role: 'user', content: text, timestamp: Date.now() });
      setInput('');
      setIsLoading(true);
      
      try {
          // Prepare history (excluding the new message as the backend appends it, 
          // OR check if backend expects full history including current?
          // lib.rs: chat(message, history) -> history.push(message).
          // So we send previous history.)
          const history = messages.map(m => ({ role: m.role, content: m.content }));
          
          const response = await invoke<string>('chat', { 
              message: text, 
              history: history 
          });

          addMessage({ 
              id: (Date.now() + 1).toString(), 
              role: 'assistant', 
              content: response, 
              timestamp: Date.now() 
          });
      } catch (error) {
          console.error('Failed to send message:', error);
          addMessage({ 
              id: (Date.now() + 1).toString(), 
              role: 'assistant', 
              content: `Error: ${error}`, 
              timestamp: Date.now() 
          });
      } finally {
          setIsLoading(false);
      }
  };

  return (
    <div className="flex-1 flex flex-col bg-white text-gray-900 h-full relative min-w-0">
        {/* Chat History */}
        <div className="flex-1 overflow-y-auto">
            {messages.length === 0 ? (
                <div className="h-full flex flex-col items-center justify-center text-gray-900 p-8">
                    <div className="text-4xl font-semibold mb-8">Plugable Chat</div>
                    <div className="grid grid-cols-1 md:grid-cols-3 gap-4 max-w-3xl text-center">
                         <div className="flex flex-col gap-2">
                             <div className="text-lg mb-2">Examples</div>
                             <button className="bg-gray-100 p-3 rounded-md hover:bg-gray-200 text-sm">"Explain quantum computing"</button>
                         </div>
                         <div className="flex flex-col gap-2">
                             <div className="text-lg mb-2">Capabilities</div>
                             <div className="bg-gray-100 p-3 rounded-md text-sm">Remembers context</div>
                         </div>
                         <div className="flex flex-col gap-2">
                             <div className="text-lg mb-2">Limitations</div>
                             <div className="bg-gray-100 p-3 rounded-md text-sm">May generate incorrect info</div>
                         </div>
                    </div>
                </div>
            ) : (
                <div className="flex flex-col pb-32">
                    {messages.map(m => (
                        <div key={m.id} className={`w-full border-b border-black/5 ${m.role === 'assistant' ? 'bg-gray-50' : 'bg-white'}`}>
                            <div className="max-w-3xl mx-auto p-4 flex gap-4 m-auto md:max-w-2xl lg:max-w-[38rem] xl:max-w-3xl">
                                <div className={`w-8 h-8 min-w-[2rem] rounded-sm flex items-center justify-center shrink-0 text-white ${m.role === 'assistant' ? 'bg-green-500' : 'bg-indigo-500'}`}>
                                    {m.role === 'assistant' ? 'AI' : 'U'}
                                </div>
                                <div className="prose w-full max-w-none break-words text-gray-900">
                                    <ReactMarkdown 
                                        remarkPlugins={[remarkGfm]} 
                                        rehypePlugins={[rehypeKatex]}
                                        components={{
                                            code({node, inline, className, children, ...props}: any) {
                                                const match = /language-(\w+)/.exec(className || '')
                                                return !inline && match ? (
                                                    <div className="my-4 rounded-md overflow-hidden border border-gray-700">
                                                        <div className="flex justify-between items-center bg-gray-800 px-4 py-1 border-b border-gray-700">
                                                            <span className="text-xs text-gray-400 font-mono">{match[1]}</span>
                                                            <button 
                                                                onClick={toggleCodeEditor}
                                                                className="text-xs text-gray-400 hover:text-white flex items-center gap-1"
                                                            >
                                                                <Code size={12} /> Open in Editor
                                                            </button>
                                                        </div>
                                                        <div className="bg-black p-4 overflow-x-auto">
                                                            <code className={className} {...props}>
                                                                {children}
                                                            </code>
                                                        </div>
                                                    </div>
                                                ) : (
                                                    <code className={`${className} bg-black/30 px-1 py-0.5 rounded`} {...props}>
                                                        {children}
                                                    </code>
                                                )
                                            }
                                        }}
                                    >
                                        {m.content}
                                    </ReactMarkdown>
                                </div>
                            </div>
                        </div>
                    ))}
                    {isLoading && (
                         <div className="w-full bg-gray-50 border-b border-black/5">
                             <div className="max-w-3xl mx-auto p-4 flex gap-4 m-auto md:max-w-2xl lg:max-w-[38rem] xl:max-w-3xl">
                                <div className="w-8 h-8 bg-green-500 rounded-sm flex items-center justify-center text-white">AI</div>
                                <div className="flex items-center gap-1">
                                    <span className="w-2 h-2 bg-gray-400 rounded-full animate-pulse"></span>
                                    <span className="w-2 h-2 bg-gray-400 rounded-full animate-pulse delay-75"></span>
                                    <span className="w-2 h-2 bg-gray-400 rounded-full animate-pulse delay-150"></span>
                                </div>
                             </div>
                         </div>
                    )}
                </div>
            )}
        </div>
        
        {/* Input Area */}
        <div className="absolute bottom-0 left-0 w-full bg-gradient-to-t from-white via-white to-transparent pt-10 pb-6">
            <div className="max-w-3xl mx-auto px-4 md:max-w-2xl lg:max-w-[38rem] xl:max-w-3xl">
                <div className="relative flex flex-col w-full p-3 bg-white border border-gray-200 rounded-xl shadow-lg">
                    <textarea 
                        className="w-full bg-transparent text-gray-900 resize-none focus:outline-none max-h-[200px] overflow-y-auto pr-10 scrollbar-hide"
                        rows={1}
                        value={input}
                        onChange={(e) => setInput(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === 'Enter' && !e.shiftKey) {
                                e.preventDefault();
                                handleSend();
                            }
                        }}
                        placeholder="Send a message..."
                        style={{ height: '24px', maxHeight: '200px' }}
                    />
                    <button 
                        onClick={handleSend}
                        className="absolute right-3 bottom-3 text-gray-400 hover:text-gray-900 p-1 rounded-md hover:bg-gray-100 disabled:hover:bg-transparent disabled:text-gray-300"
                        disabled={!input.trim()}
                    >
                        <Send size={16} />
                    </button>
                </div>
                <div className="text-center text-xs text-gray-400 mt-2">
                    Plugable Chat can make mistakes. Consider checking important information.
                </div>
            </div>
        </div>
    </div>
  )
}
