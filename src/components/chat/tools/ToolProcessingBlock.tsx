import { useEffect, useState, useMemo } from 'react';
import { parseMessageContent } from '../../../lib/response-parser';
import { parseToolCallJsonFromContent, type ParsedToolCallInfo } from '../utils';
import { formatSecondsAsTime } from '../utils';

interface ToolProcessingBlockProps {
    content: string;
    startTime: number;
}

/**
 * Tool processing block (shown inline in message when only tool_call content exists)
 * Shows a collapsible block with tool call details and processing status
 */
export const ToolProcessingBlock = ({ content, startTime }: ToolProcessingBlockProps) => {
    const [elapsed, setElapsed] = useState(0);
    const [showRaw, setShowRaw] = useState(false);

    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    const parsedData = useMemo(() => {
        // Avoid expensive parsing for very large payloads; show raw toggle instead.
        if (content.length > 80000) {
            return { parsedCalls: [] as ParsedToolCallInfo[], rawContent: content, oversized: true };
        }
        const parts = parseMessageContent(content);
        const toolCallParts = parts.filter(p => p.type === 'tool_call');
        const parsedCalls = toolCallParts
            .map(part => parseToolCallJsonFromContent(part.content))
            .filter((call): call is ParsedToolCallInfo => call !== null);
        const rawToolContent = toolCallParts.map(p => p.content).join('\n\n');
        return { parsedCalls, rawContent: rawToolContent, oversized: false };
    }, [content]);

    const { parsedCalls, rawContent, oversized } = parsedData;

    if (parsedCalls.length === 0) {
        // Fallback with expandable raw content if we can't parse the tool calls
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
                        {elapsed >= 1 ? formatSecondsAsTime(elapsed) : '...'}
                    </span>
                    <span className="ml-auto text-xs text-purple-400 group-open/processing:rotate-180 transition-transform">▼</span>
                </summary>
                <div className="border-t border-purple-200 px-4 py-3 bg-white/80">
                    {oversized && !showRaw ? (
                        <div className="text-xs text-gray-600">
                            Large tool payload omitted for performance.{" "}
                            <button
                                className="underline text-purple-700"
                                onClick={() => setShowRaw(true)}
                            >
                                Show anyway
                            </button>
                        </div>
                    ) : rawContent ? (
                        <pre className="text-xs bg-gray-50 p-2 rounded overflow-x-auto text-gray-700 whitespace-pre-wrap">
                            {rawContent}
                        </pre>
                    ) : (
                        <p className="text-xs text-gray-500 italic">Tool call content is being streamed...</p>
                    )}
                </div>
            </details>
        );
    }

    return (
        <details className="my-2 group/processing border border-purple-300 rounded-xl overflow-hidden bg-purple-50/70">
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
                    Running{elapsed >= 1 ? ` · ${formatSecondsAsTime(elapsed)}` : '...'}
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
