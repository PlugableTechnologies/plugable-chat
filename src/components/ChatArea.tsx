import { useChatStore, ToolCallRecord, CodeExecutionRecord, RagChunk } from '../store/chat-store';
import { StatusBar, StreamingWarningBar } from './StatusBar';
// Icons replaced with unicode characters
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import rehypeRaw from 'rehype-raw';
import 'katex/dist/katex.min.css';
import { invoke } from '../lib/api';
import { useEffect, useRef, useState, useCallback, type JSX } from 'react';
import { parseMessageContent, hasOnlyThinkContent, hasOnlyToolCallContent } from '../lib/response-parser';

// Format elapsed time helper
const formatTime = (seconds: number) => {
    if (seconds < 60) return `${seconds}s`;
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins}m ${secs}s`;
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

// Searching indicator component for RAG retrieval
const SearchingIndicator = ({ startTime, stage }: { startTime: number, stage: 'indexing' | 'searching' }) => {
    const [elapsed, setElapsed] = useState(0);
    
    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    const label = stage === 'indexing' ? 'Indexing documents' : 'Searching documents';
    const color = stage === 'indexing' ? 'bg-blue-400' : 'bg-emerald-400';

    return (
        <div className="flex items-center gap-2 text-xs text-gray-500 mt-2 mb-1">
            <div className="flex gap-1">
                <div className={`w-1.5 h-1.5 ${color} rounded-full animate-pulse`} />
                <div className={`w-1.5 h-1.5 ${color} rounded-full animate-pulse`} style={{ animationDelay: '300ms' }} />
                <div className={`w-1.5 h-1.5 ${color} rounded-full animate-pulse`} style={{ animationDelay: '600ms' }} />
            </div>
            <span className="font-medium text-gray-500">
                {label}{elapsed >= 1 ? ` · ${formatTime(elapsed)}` : '...'}
            </span>
        </div>
    );
};

// Tool execution indicator component (shown in the fixed footer area)
const ToolExecutionIndicator = ({ server, tool }: { server: string; tool: string }) => {
    const [elapsed, setElapsed] = useState(0);
    const startTime = useRef(Date.now());
    
    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime.current) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, []);

    return (
        <div className="flex items-center gap-2 text-xs text-gray-500 mt-2 mb-1">
            <div className="flex gap-1">
                <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" />
                <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" style={{ animationDelay: '300ms' }} />
                <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" style={{ animationDelay: '600ms' }} />
            </div>
            <span className="font-medium text-gray-500">
                Executing tool <code className="bg-purple-100 px-1 py-0.5 rounded text-purple-700">{tool}</code> on {server}
                {elapsed >= 1 ? ` · ${formatTime(elapsed)}` : '...'}
            </span>
        </div>
    );
};

// Parse tool call JSON to extract name, server, and arguments
interface ParsedToolCallInfo {
    server: string;
    tool: string;
    arguments: Record<string, unknown>;
    rawContent: string;
}

function parseToolCallJson(jsonContent: string): ParsedToolCallInfo | null {
    try {
        const parsed = JSON.parse(jsonContent.trim());
        
        // Extract tool name - could be "name" or "tool_name" (GPT-OSS legacy)
        const fullName = parsed.name || parsed.tool_name || 'unknown';
        
        // Check if the name contains server prefix (server___tool format)
        let server = 'unknown';
        let tool = fullName;
        
        if (fullName.includes('___')) {
            const parts = fullName.split('___');
            server = parts[0];
            tool = parts.slice(1).join('___');
        } else if (parsed.server) {
            server = parsed.server;
        }
        
        // Extract arguments - could be "arguments", "parameters" (Llama), or "tool_args" (GPT-OSS)
        const args = parsed.arguments || parsed.parameters || parsed.tool_args || {};
        
        return {
            server,
            tool,
            arguments: args,
            rawContent: jsonContent,
        };
    } catch {
        return null;
    }
}

// Tool processing block (shown inline in message when only tool_call content exists)
// Shows a collapsible block with tool call details and processing status
const ToolProcessingBlock = ({ content, startTime }: { content: string; startTime: number }) => {
    const [elapsed, setElapsed] = useState(0);
    
    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    // Parse tool calls from the message content
    const parts = parseMessageContent(content);
    const toolCallParts = parts.filter(p => p.type === 'tool_call');
    
    // Parse each tool call JSON
    const parsedCalls = toolCallParts
        .map(part => parseToolCallJson(part.content))
        .filter((call): call is ParsedToolCallInfo => call !== null);

    if (parsedCalls.length === 0) {
        // Fallback with expandable raw content if we can't parse the tool calls
        // Extract the raw tool call content to display
        const rawToolContent = toolCallParts.map(p => p.content).join('\n\n');
        
        return (
            <details className="my-2 group/processing border border-purple-300 rounded-xl overflow-hidden bg-purple-50/70">
                <summary className="cursor-pointer px-4 py-3 flex items-center gap-3 hover:bg-purple-100/50 transition-colors select-none">
                    <div className="flex gap-1">
                        <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" />
                        <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" style={{ animationDelay: '200ms' }} />
                        <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" style={{ animationDelay: '400ms' }} />
                    </div>
                    <span className="font-medium text-purple-900 text-sm">
                        Processing tool
                    </span>
                    <span className="text-xs px-2 py-0.5 rounded-full bg-purple-200 text-purple-700 animate-pulse">
                        {elapsed >= 1 ? formatTime(elapsed) : '...'}
                    </span>
                    <span className="ml-auto text-xs text-purple-400 group-open/processing:rotate-180 transition-transform">▼</span>
                </summary>
                <div className="border-t border-purple-200 px-4 py-3 bg-white/80">
                    {rawToolContent ? (
                        <pre className="text-xs bg-gray-50 p-2 rounded overflow-x-auto text-gray-700 whitespace-pre-wrap">
                            {rawToolContent}
                        </pre>
                    ) : (
                        <p className="text-xs text-gray-500 italic">Tool call content is being streamed...</p>
                    )}
                </div>
            </details>
        );
    }

    return (
        <details className="my-2 group/processing border border-purple-300 rounded-xl overflow-hidden bg-purple-50/70" open>
            <summary className="cursor-pointer px-4 py-3 flex items-center gap-3 hover:bg-purple-100/50 transition-colors select-none">
                <div className="flex gap-1">
                    <div className="w-1.5 h-1.5 bg-purple-500 rounded-full animate-pulse" />
                    <div className="w-1.5 h-1.5 bg-purple-500 rounded-full animate-pulse" style={{ animationDelay: '200ms' }} />
                    <div className="w-1.5 h-1.5 bg-purple-500 rounded-full animate-pulse" style={{ animationDelay: '400ms' }} />
                </div>
                <span className="font-medium text-purple-900 text-sm">
                    Processing {parsedCalls.length} tool call{parsedCalls.length !== 1 ? 's' : ''}
                </span>
                <span className="text-xs px-2 py-0.5 rounded-full bg-purple-200 text-purple-700 animate-pulse">
                    Running{elapsed >= 1 ? ` · ${formatTime(elapsed)}` : '...'}
                </span>
                <span className="ml-auto text-xs text-purple-400 group-open/processing:rotate-180 transition-transform">▼</span>
            </summary>
            <div className="border-t border-purple-200 divide-y divide-purple-100">
                {parsedCalls.map((call, idx) => (
                    <div key={idx} className="px-4 py-3 bg-white/80">
                        <div className="flex items-center gap-2 flex-wrap">
                            <code className="text-xs px-2 py-0.5 rounded bg-gray-100 text-gray-600">{call.server}</code>
                            <span className="text-gray-400">›</span>
                            <code className="text-sm px-2 py-1 rounded bg-purple-100 text-purple-800 font-medium">{call.tool}</code>
                            <span className="ml-auto flex items-center gap-1.5">
                                <div className="w-1.5 h-1.5 bg-purple-500 rounded-full animate-pulse" />
                                <span className="text-xs text-purple-600 font-medium">Processing</span>
                            </span>
                        </div>
                        {Object.keys(call.arguments).length > 0 && (
                            <details className="mt-2">
                                <summary className="text-xs text-gray-500 cursor-pointer hover:text-gray-700">
                                    Arguments
                                </summary>
                                <pre className="mt-1 text-xs bg-gray-50 p-2 rounded overflow-x-auto text-gray-700">
                                    {JSON.stringify(call.arguments, null, 2)}
                                </pre>
                            </details>
                        )}
                    </div>
                ))}
            </div>
        </details>
    );
};

// Tool approval dialog component
const ToolApprovalDialog = ({ 
    calls, 
    onApprove, 
    onReject 
}: { 
    calls: { server: string; tool: string; arguments: Record<string, unknown> }[];
    onApprove: () => void;
    onReject: () => void;
}) => {
    return (
        <div className="bg-amber-50 border border-amber-200 rounded-xl p-4 my-4">
            <div className="flex items-start gap-3">
                <span className="text-xl">⚠️</span>
                <div className="flex-1">
                    <h4 className="font-semibold text-amber-900 mb-2">Tool Execution Requires Approval</h4>
                    <div className="space-y-2 mb-4">
                        {calls.map((call, idx) => (
                            <div key={idx} className="bg-white rounded-lg p-3 border border-amber-100">
                                <div className="flex items-center gap-2 text-sm">
                                    <span className="font-medium text-gray-700">Server:</span>
                                    <code className="bg-gray-100 px-1.5 py-0.5 rounded text-gray-800">{call.server}</code>
                                </div>
                                <div className="flex items-center gap-2 text-sm mt-1">
                                    <span className="font-medium text-gray-700">Tool:</span>
                                    <code className="bg-purple-100 px-1.5 py-0.5 rounded text-purple-800">{call.tool}</code>
                                </div>
                                {Object.keys(call.arguments).length > 0 && (
                                    <details className="mt-2">
                                        <summary className="text-xs text-gray-500 cursor-pointer hover:text-gray-700">
                                            View arguments
                                        </summary>
                                        <pre className="mt-1 text-xs bg-gray-50 p-2 rounded overflow-x-auto">
                                            {JSON.stringify(call.arguments, null, 2)}
                                        </pre>
                                    </details>
                                )}
                            </div>
                        ))}
                    </div>
                    <div className="flex gap-3">
                        <button
                            onClick={onApprove}
                            className="px-4 py-2 bg-green-600 text-white rounded-lg text-sm font-medium hover:bg-green-700 transition-colors"
                        >
                            ✓ Approve
                        </button>
                        <button
                            onClick={onReject}
                            className="px-4 py-2 bg-gray-200 text-gray-700 rounded-lg text-sm font-medium hover:bg-gray-300 transition-colors"
                        >
                            ✕ Reject
                        </button>
                    </div>
                </div>
            </div>
        </div>
    );
};

// Format milliseconds to human-readable duration
const formatDurationMs = (ms?: number): string => {
    if (!ms) return '';
    if (ms < 1000) return `${ms}ms`;
    const seconds = Math.floor(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins}m ${secs}s`;
};

// Inline Tool Call Result - shows a single tool call result inline in the message
const InlineToolCallResult = ({ call }: { call: ToolCallRecord }) => {
    return (
        <details className="my-3 group/tool border border-purple-200 rounded-xl overflow-hidden bg-purple-50/50">
            <summary className="cursor-pointer px-4 py-3 flex items-center gap-3 hover:bg-purple-100/50 transition-colors select-none">
                <span className="text-purple-600 text-lg">🔧</span>
                <span className="font-medium text-purple-900 text-sm">
                    1 tool call
                </span>
                {call.isError ? (
                    <span className="text-xs px-1.5 py-0.5 rounded-full bg-red-100 text-red-700">
                        1 ✗
                    </span>
                ) : (
                    <span className="text-xs px-1.5 py-0.5 rounded-full bg-green-100 text-green-700">
                        1 ✓
                    </span>
                )}
                <span className="ml-auto text-xs text-purple-400 group-open/tool:rotate-180 transition-transform">▼</span>
            </summary>
            <div className="border-t border-purple-200">
                <div className="px-4 py-3 bg-white">
                    <div className="flex items-center gap-2 flex-wrap">
                        <code className="text-xs px-2 py-0.5 rounded bg-gray-100 text-gray-600">{call.server}</code>
                        <span className="text-gray-400">›</span>
                        <code className="text-sm px-2 py-1 rounded bg-purple-100 text-purple-800 font-medium">{call.tool}</code>
                        {call.isError ? (
                            <span className="text-xs px-1.5 py-0.5 rounded bg-red-100 text-red-600 ml-auto">Error</span>
                        ) : (
                            <span className="text-xs px-1.5 py-0.5 rounded bg-green-100 text-green-600 ml-auto">Success</span>
                        )}
                        {call.durationMs && (
                            <span className="text-xs text-gray-400">{formatDurationMs(call.durationMs)}</span>
                        )}
                    </div>
                    {Object.keys(call.arguments).length > 0 && (
                        <details className="mt-2">
                            <summary className="text-xs text-gray-500 cursor-pointer hover:text-gray-700">
                                Arguments
                            </summary>
                            <pre className="mt-1 text-xs bg-gray-50 p-2 rounded overflow-x-auto text-gray-700">
                                {JSON.stringify(call.arguments, null, 2)}
                            </pre>
                        </details>
                    )}
                    <details className="mt-2">
                        <summary className={`text-xs cursor-pointer hover:text-gray-700 ${call.isError ? 'text-red-500' : 'text-gray-500'}`}>
                            {call.isError ? 'Error' : 'Result'}
                        </summary>
                        <pre className={`mt-1 text-xs p-2 rounded overflow-x-auto whitespace-pre-wrap ${
                            call.isError ? 'bg-red-50 text-red-700' : 'bg-gray-50 text-gray-700'
                        }`}>
                            {call.result.length > 2000 
                                ? call.result.slice(0, 2000) + '\n... (truncated)'
                                : call.result}
                        </pre>
                    </details>
                </div>
            </div>
        </details>
    );
};

// Collapsible RAG Context Block - shows document chunks used as context
const RagContextBlock = ({ chunks }: { chunks: RagChunk[] }) => {
    if (!chunks || chunks.length === 0) return null;
    
    // Get unique source files
    const uniqueFiles = [...new Set(chunks.map(c => c.source_file))];
    
    // Truncate content preview
    const truncateContent = (content: string, maxLen: number = 60) => {
        const cleaned = content.replace(/\s+/g, ' ').trim();
        if (cleaned.length <= maxLen) return cleaned;
        return cleaned.slice(0, maxLen - 3) + '...';
    };
    
    return (
        <details className="my-4 group/rag border border-emerald-200 rounded-xl overflow-hidden bg-emerald-50/50">
            <summary className="cursor-pointer px-4 py-3 flex items-center gap-3 hover:bg-emerald-100/50 transition-colors select-none">
                <span className="text-emerald-600 text-lg">📚</span>
                <span className="font-medium text-emerald-900 text-sm">
                    {chunks.length} document chunk{chunks.length !== 1 ? 's' : ''} used
                </span>
                <span className="text-xs px-1.5 py-0.5 rounded-full bg-emerald-100 text-emerald-700">
                    {uniqueFiles.length} file{uniqueFiles.length !== 1 ? 's' : ''}
                </span>
                <span className="ml-auto text-xs text-emerald-400 group-open/rag:rotate-180 transition-transform">▼</span>
            </summary>
            <div className="border-t border-emerald-200 divide-y divide-emerald-100">
                {chunks.map((chunk, idx) => (
                    <div key={chunk.id || idx} className="px-4 py-3 bg-white">
                        <div className="flex items-center gap-2 flex-wrap">
                            <span className="text-emerald-500">📄</span>
                            <code className="text-xs px-2 py-0.5 rounded bg-emerald-100 text-emerald-700 font-medium">
                                {chunk.source_file}
                            </code>
                            <span className="text-xs px-1.5 py-0.5 rounded bg-gray-100 text-gray-600 ml-auto">
                                {(chunk.score * 100).toFixed(0)}% match
                            </span>
                        </div>
                        <p className="mt-2 text-xs text-gray-600 italic">
                            "{truncateContent(chunk.content)}"
                        </p>
                    </div>
                ))}
            </div>
        </details>
    );
};

// Collapsible Code Execution Block - shows Python code execution
const CodeExecutionBlock = ({ executions }: { executions: CodeExecutionRecord[] }) => {
    if (!executions || executions.length === 0) return null;
    
    const errorCount = executions.filter(e => !e.success).length;
    const successCount = executions.length - errorCount;
    
    return (
        <details className="my-4 group/code border border-blue-200 rounded-xl overflow-hidden bg-blue-50/50">
            <summary className="cursor-pointer px-4 py-3 flex items-center gap-3 hover:bg-blue-100/50 transition-colors select-none">
                <span className="text-blue-600 text-lg">🐍</span>
                <span className="font-medium text-blue-900 text-sm">
                    {executions.length} code execution{executions.length !== 1 ? 's' : ''}
                </span>
                {successCount > 0 && (
                    <span className="text-xs px-1.5 py-0.5 rounded-full bg-green-100 text-green-700">
                        {successCount} ✓
                    </span>
                )}
                {errorCount > 0 && (
                    <span className="text-xs px-1.5 py-0.5 rounded-full bg-red-100 text-red-700">
                        {errorCount} ✗
                    </span>
                )}
                <span className="ml-auto text-xs text-blue-400 group-open/code:rotate-180 transition-transform">▼</span>
            </summary>
            <div className="border-t border-blue-200 divide-y divide-blue-100">
                {executions.map((exec) => (
                    <div key={exec.id} className="px-4 py-3 bg-white">
                        <div className="flex items-center gap-2 mb-2">
                            {exec.success ? (
                                <span className="text-xs px-1.5 py-0.5 rounded bg-green-100 text-green-600">Success</span>
                            ) : (
                                <span className="text-xs px-1.5 py-0.5 rounded bg-red-100 text-red-600">Error</span>
                            )}
                            <span className="text-xs text-gray-400">{formatDurationMs(exec.durationMs)}</span>
                            {exec.innerToolCalls.length > 0 && (
                                <span className="text-xs px-1.5 py-0.5 rounded bg-purple-100 text-purple-600">
                                    {exec.innerToolCalls.length} inner tool{exec.innerToolCalls.length !== 1 ? 's' : ''}
                                </span>
                            )}
                        </div>
                        <details className="mt-2" open>
                            <summary className="text-xs text-gray-500 cursor-pointer hover:text-gray-700">
                                Code ({exec.code.length} line{exec.code.length !== 1 ? 's' : ''})
                            </summary>
                            <pre className="mt-1 text-xs bg-gray-900 text-gray-100 p-3 rounded overflow-x-auto font-mono">
                                {exec.code.join('\n')}
                            </pre>
                        </details>
                        {exec.stdout && (
                            <details className="mt-2">
                                <summary className="text-xs text-green-600 cursor-pointer hover:text-green-700">
                                    stdout
                                </summary>
                                <pre className="mt-1 text-xs bg-green-50 text-green-800 p-2 rounded overflow-x-auto whitespace-pre-wrap">
                                    {exec.stdout}
                                </pre>
                            </details>
                        )}
                        {exec.stderr && (
                            <details className="mt-2">
                                <summary className="text-xs text-red-500 cursor-pointer hover:text-red-700">
                                    stderr
                                </summary>
                                <pre className="mt-1 text-xs bg-red-50 text-red-700 p-2 rounded overflow-x-auto whitespace-pre-wrap">
                                    {exec.stderr}
                                </pre>
                            </details>
                        )}
                        {exec.innerToolCalls.length > 0 && (
                            <div className="mt-3 pl-3 border-l-2 border-purple-200">
                                <p className="text-xs text-purple-600 mb-2 font-medium">Inner Tool Calls:</p>
                                <div className="space-y-2">
                                    {exec.innerToolCalls.map((call) => (
                                        <div key={call.id} className="bg-purple-50 rounded p-2 text-xs">
                                            <div className="flex items-center gap-2">
                                                <code className="text-purple-700 font-medium">{call.tool}</code>
                                                {call.isError ? (
                                                    <span className="text-red-500">✗</span>
                                                ) : (
                                                    <span className="text-green-500">✓</span>
                                                )}
                                            </div>
                                        </div>
                                    ))}
                                </div>
                            </div>
                        )}
                    </div>
                ))}
            </div>
        </details>
    );
};

// RAG File Pills Component - shows indexed files above input with remove buttons
const RagFilePills = ({ 
    files, 
    onRemove,
    isIndexing
}: { 
    files: string[], 
    onRemove: (file: string) => void,
    isIndexing: boolean
}) => {
    if (files.length === 0 && !isIndexing) return null;
    
    // Truncate filename to first 15 chars
    const truncateFilename = (filename: string) => {
        if (filename.length <= 15) return filename;
        return filename.slice(0, 12) + '...';
    };
    
    return (
        <div className="flex flex-wrap gap-2 px-2 py-2 max-w-[900px] mx-auto">
            {isIndexing && (
                <div className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-blue-100 text-blue-700 rounded-full text-xs font-medium">
                    <div className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse" />
                    <span>Indexing...</span>
                </div>
            )}
            {files.map((file) => (
                <div 
                    key={file}
                    className="inline-flex items-center gap-1.5 px-3 py-1.5 bg-emerald-100 text-emerald-700 rounded-full text-xs font-medium group"
                    title={file}
                >
                    <span>📄</span>
                    <span>{truncateFilename(file)}</span>
                    <button
                        onClick={() => onRemove(file)}
                        className="w-4 h-4 flex items-center justify-center rounded-full hover:bg-emerald-200 text-emerald-600 hover:text-emerald-800 transition-colors"
                        title={`Remove ${file}`}
                    >
                        ×
                    </button>
                </div>
            ))}
        </div>
    );
};

// Attachment Menu Component
const AttachmentMenu = ({ 
    isOpen, 
    onClose, 
    onSelectFiles, 
    onSelectFolder 
}: { 
    isOpen: boolean, 
    onClose: () => void, 
    onSelectFiles: () => void, 
    onSelectFolder: () => void 
}) => {
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
            className="absolute bottom-full left-0 mb-2 bg-white rounded-lg shadow-lg border border-gray-200 py-1 min-w-[160px] z-50"
        >
            <button
                onClick={() => { onSelectFiles(); onClose(); }}
                className="w-full px-4 py-2 text-left text-sm text-gray-700 hover:bg-gray-100 flex items-center gap-2"
            >
                <span>📄</span>
                <span>Attach Files</span>
            </button>
            <button
                onClick={() => { onSelectFolder(); onClose(); }}
                className="w-full px-4 py-2 text-left text-sm text-gray-700 hover:bg-gray-100 flex items-center gap-2"
            >
                <span>📁</span>
                <span>Attach Folder</span>
            </button>
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
    isLoading,
    attachedCount,
    onAttachFiles,
    onAttachFolder,
    onClearAttachments,
    disabled = false
}: {
    className?: string,
    input: string,
    setInput: (s: string) => void,
    handleSend: () => void,
    handleStop: () => void,
    handleKeyDown: (e: React.KeyboardEvent) => void,
    textareaRef: React.RefObject<HTMLTextAreaElement | null>,
    isLoading: boolean,
    attachedCount: number,
    onAttachFiles: () => void,
    onAttachFolder: () => void,
    onClearAttachments: () => void,
    disabled?: boolean
}) => {
    const [menuOpen, setMenuOpen] = useState(false);
    const isMultiline = input.includes('\n') || (textareaRef.current && textareaRef.current.scrollHeight > 44);
    const hasAttachments = attachedCount > 0;
    const isDisabled = disabled || isLoading;
    
    return (
        <div className={`w-full flex justify-center ${className}`}>
            <div className={`flex items-center gap-3 w-full max-w-[900px] bg-[#f5f5f5] border border-transparent px-4 py-2.5 shadow-[0px_2px_8px_rgba(15,23,42,0.08)] focus-within:border-gray-300 transition-all ${isMultiline ? 'rounded-2xl' : 'rounded-full'}`}>
                <div className="relative">
                    <button
                        type="button"
                        onClick={() => setMenuOpen(!menuOpen)}
                        className={`flex h-9 w-9 items-center justify-center rounded-full text-xl shadow-sm transition shrink-0 relative ${
                            hasAttachments 
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
                    className={`flex-1 bg-transparent text-gray-700 resize-none focus:outline-none focus:ring-0 focus:border-none max-h-[200px] overflow-y-auto placeholder:text-gray-400 font-normal text-[15px] leading-6 border-none py-1 ${disabled ? 'opacity-50 cursor-not-allowed' : ''}`}
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

// Strip OpenAI special tokens that may leak through
const stripOpenAITokens = (content: string): string => {
    // Remove common OpenAI special tokens
    // Patterns: <|start|>, <|end|>, <|im_start|>, <|im_end|>, <|endoftext|>
    // Also handles role markers like <|start|>assistant, <|im_start|>user, etc.
    return content
        .replace(/<\|(?:start|end|im_start|im_end|endoftext|eot_id|begin_of_text|end_of_text)\|>(?:assistant|user|system)?/gi, '')
        .replace(/<\|(?:start|end|im_start|im_end|endoftext|eot_id|begin_of_text|end_of_text)\|>/gi, '')
        // Clean up any leftover newlines at the start from removed tokens
        .replace(/^\n+/, '');
};

// Common LaTeX commands that indicate math content
const LATEX_MATH_COMMANDS = [
    'frac', 'sqrt', 'sum', 'prod', 'int', 'oint', 'lim', 'infty',
    'alpha', 'beta', 'gamma', 'delta', 'epsilon', 'zeta', 'eta', 'theta',
    'iota', 'kappa', 'lambda', 'mu', 'nu', 'xi', 'pi', 'rho', 'sigma',
    'tau', 'upsilon', 'phi', 'chi', 'psi', 'omega',
    'Alpha', 'Beta', 'Gamma', 'Delta', 'Epsilon', 'Zeta', 'Eta', 'Theta',
    'Iota', 'Kappa', 'Lambda', 'Mu', 'Nu', 'Xi', 'Pi', 'Rho', 'Sigma',
    'Tau', 'Upsilon', 'Phi', 'Chi', 'Psi', 'Omega',
    'times', 'div', 'pm', 'mp', 'cdot', 'ast', 'star', 'circ',
    'leq', 'geq', 'neq', 'approx', 'equiv', 'sim', 'simeq', 'cong',
    'subset', 'supset', 'subseteq', 'supseteq', 'in', 'notin', 'ni',
    'cup', 'cap', 'setminus', 'emptyset', 'varnothing',
    'forall', 'exists', 'nexists', 'neg', 'land', 'lor', 'implies', 'iff',
    'partial', 'nabla', 'degree',
    'sin', 'cos', 'tan', 'cot', 'sec', 'csc', 'arcsin', 'arccos', 'arctan',
    'sinh', 'cosh', 'tanh', 'coth',
    'log', 'ln', 'exp', 'min', 'max', 'arg', 'det', 'dim', 'ker', 'hom',
    'left', 'right', 'bigl', 'bigr', 'Bigl', 'Bigr',
    'vec', 'hat', 'bar', 'dot', 'ddot', 'tilde', 'overline', 'underline',
    'overbrace', 'underbrace',
    'text', 'textbf', 'textit', 'textrm', 'mathrm', 'mathbf', 'mathit',
    'mathbb', 'mathcal', 'mathscr', 'mathfrak',
    'boxed', 'cancel', 'bcancel', 'xcancel',
    'begin', 'end', 'matrix', 'pmatrix', 'bmatrix', 'vmatrix', 'cases',
    'hspace', 'vspace', 'quad', 'qquad', 'space',
    'displaystyle', 'textstyle', 'scriptstyle',
];

// Build regex pattern for detecting LaTeX commands
const LATEX_COMMAND_PATTERN = new RegExp(
    `\\\\(${LATEX_MATH_COMMANDS.join('|')})(?![a-zA-Z])`,
    'g'
);

// Convert LaTeX bracket/paren delimiters to dollar signs for remark-math
const convertLatexDelimiters = (content: string): string => {
    let result = content;
    
    // Convert \[...\] to $$...$$ (display math)
    // Use a non-greedy match to handle multiple blocks
    result = result.replace(/\\\[([\s\S]*?)\\\]/g, (_match, inner) => {
        return `$$${inner}$$`;
    });
    
    // Convert \(...\) to $...$ (inline math)
    result = result.replace(/\\\(([\s\S]*?)\\\)/g, (_match, inner) => {
        return `$${inner}$`;
    });
    
    // Handle bare brackets [ ... ] that contain LaTeX (has backslash commands)
    // Be careful not to match markdown links or array-like content
    // Only match if the content has LaTeX patterns like \frac, \text, \times, etc.
    result = result.replace(/(?<!\[)\[\s*((?:\\[a-zA-Z]+|[^[\]]*)+)\s*\](?!\()/g, (match, inner) => {
        // Check if content looks like LaTeX (has backslash commands like \frac, \text, etc.)
        const hasLatexCommands = /\\[a-zA-Z]+/.test(inner);
        // Also check for common math patterns
        const hasMathPatterns = /[_^{}]|\\[a-zA-Z]/.test(inner);
        
        if (hasLatexCommands || hasMathPatterns) {
            return `$$${inner.trim()}$$`;
        }
        return match; // Leave as-is if not LaTeX
    });
    
    // Handle bare parentheses ( ... ) that contain LaTeX
    // Be more conservative here since parentheses are common
    // Only convert if content has clear LaTeX commands
    result = result.replace(/\(\s*((?:[^()]*\\[a-zA-Z]+[^()]*)+)\s*\)/g, (match, inner) => {
        // Check for any LaTeX command (backslash followed by letters)
        const hasLatexCommand = /\\[a-zA-Z]{2,}/.test(inner);
        // Also check for subscript/superscript patterns common in math
        const hasMathNotation = /[_^]/.test(inner) && /\\/.test(inner);
        // Scientific notation pattern
        const hasScientificNotation = /\\times\s*10\s*\^/.test(inner);
        
        // Exclude things that look like file paths
        const looksLikePath = /^\/[a-zA-Z]/.test(inner.trim());
        
        if ((hasLatexCommand || hasMathNotation || hasScientificNotation) && !looksLikePath) {
            return `$${inner.trim()}$`;
        }
        return match; // Leave as-is if not clearly LaTeX
    });
    
    // NEW: Wrap undelimited LaTeX expressions in inline math delimiters
    // This catches cases where LaTeX commands appear in plain text without any delimiters
    result = wrapUndelimitedLatex(result);
    
    return result;
};

// Wrap undelimited LaTeX expressions in $ delimiters
// This handles cases where the model outputs LaTeX without proper math delimiters
const wrapUndelimitedLatex = (content: string): string => {
    // Track positions that are already in math mode or code
    const mathRanges: [number, number][] = [];
    const codeRanges: [number, number][] = [];
    
    // Find existing math delimiters ($$...$$ and $...$)
    let match;
    const displayMathRegex = /\$\$[\s\S]*?\$\$/g;
    while ((match = displayMathRegex.exec(content)) !== null) {
        mathRanges.push([match.index, match.index + match[0].length]);
    }
    
    const inlineMathRegex = /\$(?!\$)[^\$\n]+\$(?!\$)/g;
    while ((match = inlineMathRegex.exec(content)) !== null) {
        mathRanges.push([match.index, match.index + match[0].length]);
    }
    
    // Find code blocks and inline code
    const codeBlockRegex = /```[\s\S]*?```/g;
    while ((match = codeBlockRegex.exec(content)) !== null) {
        codeRanges.push([match.index, match.index + match[0].length]);
    }
    
    const inlineCodeRegex = /`[^`\n]+`/g;
    while ((match = inlineCodeRegex.exec(content)) !== null) {
        codeRanges.push([match.index, match.index + match[0].length]);
    }
    
    // Check if a position is inside math or code
    const isProtected = (pos: number): boolean => {
        return mathRanges.some(([start, end]) => pos >= start && pos < end) ||
               codeRanges.some(([start, end]) => pos >= start && pos < end);
    };
    
    // Find and wrap undelimited LaTeX expressions
    // Pattern matches: LaTeX command followed by more math content
    // e.g., \frac{4}{3} \pi r^3 or V = \frac{a}{b}
    const latexExpressionRegex = /(?:^|[^\\$])((\\(?:frac|sqrt|sum|prod|int|lim)\s*\{[^}]*\}\s*(?:\{[^}]*\})?|\\(?:text|textbf|textit|mathrm|mathbf)\s*\{[^}]*\})(?:\s*[+\-*/=^_]?\s*(?:\\[a-zA-Z]+(?:\s*\{[^}]*\})*|[a-zA-Z0-9.]+|\{[^}]*\}|[+\-*/=^_]))*)/g;
    
    const replacements: { start: number; end: number; text: string }[] = [];
    
    while ((match = latexExpressionRegex.exec(content)) !== null) {
        const fullMatch = match[1];
        const startPos = match.index + match[0].indexOf(fullMatch);
        
        // Skip if this position is already in math or code
        if (isProtected(startPos)) continue;
        
        // Only wrap if it contains actual LaTeX commands
        if (LATEX_COMMAND_PATTERN.test(fullMatch)) {
            replacements.push({
                start: startPos,
                end: startPos + fullMatch.length,
                text: `$${fullMatch.trim()}$`
            });
        }
        
        // Reset the regex lastIndex to avoid infinite loops
        LATEX_COMMAND_PATTERN.lastIndex = 0;
    }
    
    // Also catch simpler patterns: standalone LaTeX commands with arguments
    // e.g., \times 10^{27} or \approx 1.41
    const simpleLatexRegex = /(?:^|[\s(=])((\\(?:times|approx|equiv|leq|geq|neq|pm|mp|cdot|div|infty|pi|alpha|beta|gamma|delta|theta|lambda|mu|sigma|omega|phi|psi|partial|nabla|sum|prod|int)\b)(?:\s*[0-9.]+)?(?:\s*\\times\s*[0-9.]+)?(?:\s*\^[\s{]*[-0-9]+\}?)?(?:\s*\\text\{[^}]*\})?)/g;
    
    while ((match = simpleLatexRegex.exec(content)) !== null) {
        const fullMatch = match[1];
        const startPos = match.index + match[0].indexOf(fullMatch);
        
        if (isProtected(startPos)) continue;
        
        // Check it's not already inside our planned replacements
        const overlaps = replacements.some(r => 
            (startPos >= r.start && startPos < r.end) ||
            (startPos + fullMatch.length > r.start && startPos + fullMatch.length <= r.end)
        );
        
        if (!overlaps) {
            replacements.push({
                start: startPos,
                end: startPos + fullMatch.length,
                text: `$${fullMatch.trim()}$`
            });
        }
    }
    
    // Sort replacements by position (descending) to apply from end to start
    replacements.sort((a, b) => b.start - a.start);
    
    // Apply replacements
    let result = content;
    for (const { start, end, text } of replacements) {
        result = result.slice(0, start) + text + result.slice(end);
    }
    
    return result;
};

// Helper to wrap raw \boxed{} in math delimiters to ensure they render
const preprocessLaTeX = (content: string) => {
    // First strip OpenAI tokens
    let processed = stripOpenAITokens(content);
    
    // Then convert LaTeX delimiters
    processed = convertLatexDelimiters(processed);
    
    // Now handle \boxed{} and other special cases
    let result = '';
    let i = 0;

    // States
    let inMath: false | '$' | '$$' = false;
    let inCode: false | '`' | '```' = false;

    while (i < processed.length) {
        // 1. Handle Code Blocks
        if (!inMath && !inCode && processed.startsWith('```', i)) {
            inCode = '```';
            result += '```';
            i += 3;
            continue;
        }
        if (!inMath && inCode === '```' && processed.startsWith('```', i)) {
            inCode = false;
            result += '```';
            i += 3;
            continue;
        }

        // 2. Handle Inline Code
        if (!inMath && !inCode && processed.startsWith('`', i)) {
            inCode = '`';
            result += '`';
            i += 1;
            continue;
        }
        if (!inMath && inCode === '`' && processed.startsWith('`', i)) {
            inCode = false;
            result += '`';
            i += 1;
            continue;
        }

        // If in code, just consume
        if (inCode) {
            result += processed[i];
            i++;
            continue;
        }

        // 3. Handle Math Delimiters
        // Escaped dollar? \$
        if (processed.startsWith('\\$', i)) {
            result += '\\$';
            i += 2;
            continue;
        }

        if (processed.startsWith('$$', i)) {
            if (inMath === '$$') inMath = false;
            else if (!inMath) inMath = '$$';
            result += '$$';
            i += 2;
            continue;
        }
        if (processed.startsWith('$', i)) {
            if (inMath === '$') inMath = false;
            else if (!inMath) inMath = '$';
            result += '$';
            i += 1;
            continue;
        }

        // 4. Handle \boxed{
        if (!inMath && processed.startsWith('\\boxed{', i)) {
            // Look ahead to find matching brace
            let braceCount = 1;
            let ptr = i + 7; // skip \boxed{

            while (ptr < processed.length && braceCount > 0) {
                if (processed[ptr] === '\\') {
                    ptr += 2; // skip escaped char
                    continue;
                }
                if (processed[ptr] === '{') braceCount++;
                if (processed[ptr] === '}') braceCount--;
                ptr++;
            }

            if (braceCount === 0) {
                // Found complete block - extract the inner content
                const innerContent = processed.substring(i + 7, ptr - 1);
                
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

        result += processed[i];
        i++;
    }
    return result;
};

export function ChatArea() {
    const {
        chatMessages,
        chatInputValue,
        setChatInputValue,
        appendChatMessage,
        assistantStreamingActive,
        setAssistantStreamingActive,
        stopActiveChatGeneration,
        currentChatId,
        reasoningEffort,
        triggerRelevanceSearch, clearRelevanceSearch, isConnecting,
        // RAG state
        attachedPaths, ragIndexedFiles, isIndexingRag,
        addAttachment, searchRagContext, clearRagContext, removeRagFile,
        // Tool execution state
        pendingToolApproval, toolExecution, approveCurrentToolCall, rejectCurrentToolCall,
        // Streaming state
        streamingChatId
    } = useChatStore();
    
    // Check if streaming is active in a different chat (input should be blocked)
    const isStreamingInOtherChat = streamingChatId !== null && streamingChatId !== currentChatId;
    const textareaRef = useRef<HTMLTextAreaElement>(null);
    const messagesEndRef = useRef<HTMLDivElement>(null);
    const [thinkingStartTime, setThinkingStartTime] = useState<number | null>(null);
    const [toolProcessingStartTime, setToolProcessingStartTime] = useState<number | null>(null);
    // Local RAG state for UI (controlled directly, not from store)
    const [ragStartTime, setRagStartTime] = useState<number | null>(null);
    const [ragStage, setRagStage] = useState<'indexing' | 'searching'>('indexing');
    const [isRagProcessing, setIsRagProcessing] = useState(false);

    // Follow mode: auto-scroll to bottom when true, stop when user scrolls up
    const scrollContainerRef = useRef<HTMLDivElement>(null);
    const [isFollowMode, setIsFollowMode] = useState(true);

    // Scroll handler to detect when user scrolls away from bottom
    const handleScroll = useCallback(() => {
        const container = scrollContainerRef.current;
        if (!container) return;
        const { scrollTop, scrollHeight, clientHeight } = container;
        const atBottom = scrollHeight - scrollTop - clientHeight < 50;
        setIsFollowMode(atBottom);
    }, []);

    // Track when thinking phase starts
    useEffect(() => {
        const lastMessage = chatMessages[chatMessages.length - 1];
        const isThinkingOnly = lastMessage?.role === 'assistant' &&
                               hasOnlyThinkContent(lastMessage.content) &&
                               assistantStreamingActive;
        
        if (isThinkingOnly && !thinkingStartTime) {
            setThinkingStartTime(Date.now());
        } else if (!assistantStreamingActive || (lastMessage?.role === 'assistant' && !hasOnlyThinkContent(lastMessage.content))) {
            setThinkingStartTime(null);
        }
    }, [chatMessages, assistantStreamingActive, thinkingStartTime]);

    // Track when tool processing phase starts (only tool_call content, no visible text)
    useEffect(() => {
        const lastMessage = chatMessages[chatMessages.length - 1];
        const isToolProcessingOnly = lastMessage?.role === 'assistant' &&
                                      hasOnlyToolCallContent(lastMessage.content) &&
                                      assistantStreamingActive;
        
        if (isToolProcessingOnly && !toolProcessingStartTime) {
            setToolProcessingStartTime(Date.now());
        } else if (!assistantStreamingActive || (lastMessage?.role === 'assistant' && !hasOnlyToolCallContent(lastMessage.content))) {
            setToolProcessingStartTime(null);
        }
    }, [chatMessages, assistantStreamingActive, toolProcessingStartTime]);

    // Reset follow mode when switching chats
    useEffect(() => {
        setIsFollowMode(true);
    }, [currentChatId]);

    // Auto-scroll to bottom (only when in follow mode)
    useEffect(() => {
        if (isFollowMode) {
            messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
        }
    }, [chatMessages, assistantStreamingActive, isFollowMode]);

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
    }, [chatInputValue]);

    // Trigger relevance search as user types (debounced in store)
    useEffect(() => {
        if (chatInputValue.trim().length >= 3) {
            triggerRelevanceSearch(chatInputValue);
        } else {
            clearRelevanceSearch();
        }
    }, [chatInputValue, triggerRelevanceSearch, clearRelevanceSearch]);

    // Handle file selection via Tauri dialog
    const handleAttachFiles = async () => {
        try {
            const { open } = await import('@tauri-apps/plugin-dialog');
            const selected = await open({
                multiple: true,
                filters: [{
                    name: 'Documents',
                    extensions: ['txt', 'csv', 'tsv', 'md', 'json', 'pdf', 'docx']
                }]
            });
            if (selected) {
                const paths = Array.isArray(selected) ? selected : [selected];
                // Process each file sequentially (addAttachment now triggers immediate indexing)
                for (const path of paths) {
                    if (path) await addAttachment(path);
                }
            }
        } catch (e) {
            console.error('[ChatArea] Failed to open file dialog:', e);
        }
    };

    // Handle folder selection via Tauri dialog
    const handleAttachFolder = async () => {
        try {
            const { open } = await import('@tauri-apps/plugin-dialog');
            const selected = await open({
                directory: true,
                multiple: false
            });
            if (selected && typeof selected === 'string') {
                await addAttachment(selected);
            }
        } catch (e) {
            console.error('[ChatArea] Failed to open folder dialog:', e);
        }
    };

    // Handle clearing attachments (also clears RAG context)
    const handleClearAttachments = async () => {
        await clearRagContext();
    };

    const handleSend = async () => {
        const text = chatInputValue;
        if (!text.trim()) return;
        const trimmedText = text.trim();

        const storeState = useChatStore.getState();
        const isNewChat = !currentChatId;
        const chatId = isNewChat ? generateClientChatId() : currentChatId!;
        if (isNewChat) {
            storeState.setCurrentChatId(chatId);
            // Note: RAG context is managed by user via the pills above input
            // They can remove individual files or clear all - no automatic clearing
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

        // Add user message (show original text to user)
        appendChatMessage({
            id: Date.now().toString(),
            role: 'user',
            content: text,
            timestamp: Date.now(),
        });
        setChatInputValue('');
        clearRelevanceSearch(); // Clear relevance results when sending
        if (textareaRef.current) textareaRef.current.style.height = 'auto';
        setAssistantStreamingActive(true);
        
        // Track which chat we're streaming to (for cross-chat switching)
        storeState.setStreamingChatId(chatId);
        
        // Show streaming status in status bar
        storeState.setOperationStatus({
            type: 'streaming',
            message: 'Generating response...',
            startTime: Date.now(),
        });

        // Add placeholder for assistant
        appendChatMessage({
            id: (Date.now() + 1).toString(),
            role: 'assistant',
            content: '',
            timestamp: Date.now()
        });

        try {
            // Check if we have RAG context to search (files are indexed immediately on attach)
            let messageToSend = text;
            const hasRagContext = storeState.ragChunkCount > 0;
            
            if (hasRagContext) {
                console.log('[ChatArea] Searching RAG context with', storeState.ragChunkCount, 'indexed chunks');
                
                // Show RAG indicator
                setIsRagProcessing(true);
                setRagStartTime(Date.now());
                setRagStage('searching');
                
                const relevantChunks = await searchRagContext(trimmedText, 5);
                
                if (relevantChunks.length > 0) {
                    // Store chunks on the assistant message for display
                    useChatStore.setState((state) => {
                        const newMessages = [...state.chatMessages];
                        const lastIdx = newMessages.length - 1;
                        if (lastIdx >= 0 && newMessages[lastIdx].role === 'assistant') {
                            newMessages[lastIdx] = { ...newMessages[lastIdx], ragChunks: relevantChunks };
                        }
                        return { chatMessages: newMessages };
                    });
                    
                    // Build context string for the model
                    const contextParts = relevantChunks.map((chunk, idx) => 
                        `[${idx + 1}] From "${chunk.source_file}" (relevance: ${(chunk.score * 100).toFixed(1)}%):\n${chunk.content}`
                    );
                    const contextString = contextParts.join('\n\n');
                    
                    // Prepend context to the message
                    messageToSend = `Context from attached documents:\n\n${contextString}\n\n---\n\nUser question: ${text}`;
                    console.log('[ChatArea] Added', relevantChunks.length, 'chunks as context');
                }
                
                // Hide RAG indicator
                setIsRagProcessing(false);
                setRagStartTime(null);
            }

            const history = chatMessages.map((m) => ({ role: m.role, content: m.content }));
            // Call backend - streaming will trigger events
            const returnedChatId = await invoke<string>('chat', {
                chatId,
                title: isNewChat ? derivedTitle : undefined,
                message: messageToSend,
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
            // Chat is already in history via upsertHistoryEntry() above - no need to refetch
        } catch (error) {
            console.error('[ChatArea] Failed to send message:', error);
            // Reset RAG state on error
            setIsRagProcessing(false);
            setRagStartTime(null);
            // Update the last message with error
            useChatStore.setState((state) => {
                const newMessages = [...state.chatMessages];
                const lastIdx = newMessages.length - 1;
                if (lastIdx >= 0) {
                    newMessages[lastIdx] = {
                        ...newMessages[lastIdx],
                        content: `Error: ${error}`
                    };
                }
                return { chatMessages: newMessages };
            });
            setAssistantStreamingActive(false);
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
        {/* Status Bar for model operations */}
        <StatusBar />
        
        {/* Warning when streaming in another chat */}
        <StreamingWarningBar />
        
        {/* Scrollable Messages Area - takes all remaining space */}
        <div ref={scrollContainerRef} onScroll={handleScroll} className="flex-1 min-h-0 w-full overflow-y-auto flex flex-col px-4 sm:px-6 pt-6 pb-6">
                {chatMessages.length === 0 ? (
                    <div className="flex-1 flex flex-col items-center justify-center px-6">
                        <div className="mb-8 text-center">
                            <h1 className="text-2xl font-bold text-gray-900">
                                {isConnecting ? "Wait, Loading Local Models ..." : "How can I help you today?"}
                            </h1>
                        </div>
                    </div>
                ) : (
                    <div className="w-full max-w-none space-y-6 py-0">
                        {chatMessages.map(m => (
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
                                            (() => {
                                                // Parse content and track tool call index for inline rendering
                                                const parts = parseMessageContent(m.content);
                                                const toolCalls = m.toolCalls || [];
                                                const pythonToolCalls = toolCalls.filter((call) => call.tool === 'python_execution');
                                                const hasPythonToolCalls = pythonToolCalls.length > 0;
                                                const toolCallPartIndices = parts
                                                    .map((p, idx) => p.type === 'tool_call' ? idx : -1)
                                                    .filter(idx => idx !== -1);
                                                const lastThinkIndex = parts.reduce((last, part, idx) => part.type === 'think' ? idx : last, -1);
                                                const shouldInlineFallbackToolCalls = toolCalls.length > toolCallPartIndices.length;
                                                const fallbackInsertAfter = shouldInlineFallbackToolCalls
                                                    ? (toolCallPartIndices.length > 0
                                                        ? toolCallPartIndices[toolCallPartIndices.length - 1]
                                                        : (lastThinkIndex !== -1 ? lastThinkIndex : (parts.length > 0 ? 0 : -1)))
                                                    : -1;
                                                const isCodeOnlyBlock = (text: string) => {
                                                    const trimmed = text.trim();
                                                    // Treat pure fenced code blocks (with optional language) as non-visible for final answer purposes
                                                    return /^```[\s\S]*```$/.test(trimmed);
                                                };
                                                const textParts = parts.filter((part) => part.type === 'text');
                                                const hasAnyText = textParts.some((part) => part.content.trim().length > 0);
                                                const textAllCodeOnly = textParts.length > 0 && textParts.every((part) => isCodeOnlyBlock(part.content));
                                                const hasVisibleText = hasAnyText && !textAllCodeOnly;
                                                // Once a python tool call is present, hide the assistant-rendered code/text (it will be shown via tool UI/output below).
                                                const shouldHideTextForToolCalls = hasPythonToolCalls;
                                                const latestCodeExecutionStdout = m.codeExecutions
                                                    ?.slice()
                                                    .reverse()
                                                    .find((exec) => exec.stdout && exec.stdout.trim().length > 0)
                                                    ?.stdout.trim();
                                                const latestPythonStdout = pythonToolCalls
                                                    .slice()
                                                    .reverse()
                                                    .find((call) => call.result && call.result.trim().length > 0)
                                                    ?.result.trim();
                                                const latestNonPythonToolResult = toolCalls
                                                    .slice()
                                                    .reverse()
                                                    .find((call) => call.tool !== 'python_execution' && call.result && call.result.trim().length > 0)
                                                    ?.result.trim();
                                                // Always show Python stdout separately so it can't be hidden by text-rendering logic.
                                                const pythonOutputToShow = latestPythonStdout || latestCodeExecutionStdout || '';
                                                // Fallback answer is only for non-Python tool results when there is no visible text.
                                                const fallbackAnswer = !hasVisibleText
                                                    ? (latestNonPythonToolResult || '')
                                                    : '';
                                                let toolCallIndex = 0;
                                                const renderedParts: JSX.Element[] = [];

                                                parts.forEach((part, idx) => {
                                                    if (part.type === 'think') {
                                                        renderedParts.push(
                                                            <details key={`think-${idx}`} className="mb-4 group">
                                                                <summary className="cursor-pointer text-xs font-medium text-gray-400 hover:text-gray-600 select-none flex items-center gap-2 mb-2">
                                                                    <span className="h-px flex-1 bg-gray-200 group-open:bg-gray-300 transition-colors"></span>
                                                                    <span className="text-sm group-open:rotate-180 transition-transform inline-block">▼</span>
                                                                </summary>
                                                                <div className="pl-3 border-l-2 border-gray-300 text-gray-600 text-sm italic bg-gray-50 p-3 rounded-r-lg">
                                                                    {part.content || "Thinking..."}
                                                                </div>
                                                            </details>
                                                        );
                                                    } else if (part.type === 'tool_call') {
                                                        const toolCallRecord = toolCalls[toolCallIndex];
                                                        if (toolCallRecord) {
                                                            renderedParts.push(
                                                                <InlineToolCallResult key={`toolcall-${toolCallRecord.id}`} call={toolCallRecord} />
                                                            );
                                                            toolCallIndex++;
                                                        }
                                                    } else {
                                                        // Skip rendering the raw text if we're hiding it for tool calls,
                                                        // but still continue so fallback tool insertion can happen.
                                                        if (!shouldHideTextForToolCalls) {
                                                            renderedParts.push(
                                                                <ReactMarkdown
                                                                    key={`text-${idx}`}
                                                                    remarkPlugins={[remarkGfm, remarkMath]}
                                                                    rehypePlugins={[
                                                                        rehypeRaw, 
                                                                        [rehypeKatex, { 
                                                                            throwOnError: false, 
                                                                            errorColor: '#666666',
                                                                            strict: false
                                                                        }]
                                                                    ]}
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
                                                            );
                                                        }
                                                    }

                                                    if (
                                                        shouldInlineFallbackToolCalls &&
                                                        idx === fallbackInsertAfter &&
                                                        toolCallIndex < toolCalls.length
                                                    ) {
                                                        for (; toolCallIndex < toolCalls.length; toolCallIndex++) {
                                                            const call = toolCalls[toolCallIndex];
                                                            renderedParts.push(
                                                                <InlineToolCallResult key={`toolcall-fallback-${call.id}`} call={call} />
                                                            );
                                                        }
                                                    }
                                                });

                                                if (shouldInlineFallbackToolCalls && parts.length === 0 && toolCalls.length > 0) {
                                                    toolCalls.forEach((call) => {
                                                        renderedParts.push(
                                                            <InlineToolCallResult key={`toolcall-fallback-${call.id}`} call={call} />
                                                        );
                                                    });
                                                    toolCallIndex = toolCalls.length;
                                                }
                                                
                                                return (
                                                    <>
                                                        {/* RAG context block - shows document chunks used as context (before response) */}
                                                        {m.ragChunks && m.ragChunks.length > 0 && (
                                                            <RagContextBlock chunks={m.ragChunks} />
                                                        )}
                                                        {renderedParts}
                                                        {(pythonOutputToShow || fallbackAnswer) && (
                                                            <div className="mt-3">
                                                                <div className="bg-white border border-gray-200 rounded-xl px-4 py-3 text-gray-900 whitespace-pre-wrap">
                                                                    {pythonOutputToShow || fallbackAnswer}
                                                                </div>
                                                            </div>
                                                        )}
                                                        {/* Show thinking indicator when only think content is visible */}
                                                        {thinkingStartTime && 
                                                         chatMessages[chatMessages.length - 1]?.id === m.id && 
                                                         hasOnlyThinkContent(m.content) && (
                                                            <ThinkingIndicator startTime={thinkingStartTime} />
                                                        )}
                                                        {/* Show tool processing block when only tool_call content is visible */}
                                                        {toolProcessingStartTime && 
                                                         chatMessages[chatMessages.length - 1]?.id === m.id && 
                                                         hasOnlyToolCallContent(m.content) && (
                                                            <ToolProcessingBlock content={m.content} startTime={toolProcessingStartTime} />
                                                        )}
                                                        {/* Collapsible code execution block - shown at end since executions aren't tracked by position */}
                                                        {m.codeExecutions && m.codeExecutions.length > 0 && (
                                                            <CodeExecutionBlock executions={m.codeExecutions} />
                                                        )}
                                                    </>
                                                );
                                            })()
                                        ) : (
                                            <div className="whitespace-pre-wrap">{m.content}</div>
                                        )}
                                    </div>
                                </div>
                            </div>
                        ))}
                        {assistantStreamingActive && chatMessages[chatMessages.length - 1]?.role !== 'assistant' && (
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

            {/* RAG Searching Indicator */}
            {isRagProcessing && ragStartTime && (
                <div className="flex-shrink-0 px-4 sm:px-6">
                    <div className="max-w-[900px] mx-auto">
                        <SearchingIndicator startTime={ragStartTime} stage={ragStage} />
                    </div>
                </div>
            )}

            {/* Tool Execution Indicator */}
            {toolExecution.currentTool && (
                <div className="flex-shrink-0 px-4 sm:px-6">
                    <div className="max-w-[900px] mx-auto">
                        <ToolExecutionIndicator 
                            server={toolExecution.currentTool.server} 
                            tool={toolExecution.currentTool.tool} 
                        />
                    </div>
                </div>
            )}

            {/* Tool Approval Dialog */}
            {pendingToolApproval && (
                <div className="flex-shrink-0 px-4 sm:px-6">
                    <div className="max-w-[900px] mx-auto">
                        <ToolApprovalDialog
                            calls={pendingToolApproval.calls}
                            onApprove={approveCurrentToolCall}
                            onReject={rejectCurrentToolCall}
                        />
                    </div>
                </div>
            )}

            {/* Fixed Input Area at Bottom */}
            <div className="flex-shrink-0 mt-1 pb-4">
                {/* RAG File Pills - show indexed files above input */}
                <div className="px-2 sm:px-6">
                    <RagFilePills 
                        files={ragIndexedFiles} 
                        onRemove={removeRagFile}
                        isIndexing={isIndexingRag}
                    />
                </div>
                <div className="px-2 sm:px-6">
                    <InputBar
                        className=""
                        input={chatInputValue}
                        setInput={setChatInputValue}
                        handleSend={handleSend}
                        handleStop={stopActiveChatGeneration}
                        handleKeyDown={handleKeyDown}
                        textareaRef={textareaRef}
                        isLoading={assistantStreamingActive}
                        attachedCount={attachedPaths.length}
                        onAttachFiles={handleAttachFiles}
                        onAttachFolder={handleAttachFolder}
                        onClearAttachments={handleClearAttachments}
                        disabled={isStreamingInOtherChat}
                    />
                </div>
            </div>
        </div>
    )
}
